//! 建物の駐車場と、建物から建物への「歩く → 車 → 歩く」の移動。
//!
//! 移動は 3 つの区切りに分ける。出発の建物の出入口から、その建物の駐車場の出入りの位置まで歩く。
//! 車で行き先の駐車場の出入りの位置まで走る。車を停めて、行き先の出入口まで歩く。出入りの位置
//! （[`lot_spot`]）と出入口は、どちらも建物の前の道路の上の位置で、同じ間口の中にある。
//!
//! 着く車は、出入りの位置へ入る速さ（`car::PARK_ENTRY_SPEED`）まで落として近づき、届いたら車線から
//! 外れて駐車場に入る。車線の上で止まらないので、後ろの車は減速に付き合うだけで済む。
//! 出る車は、車線に空きができるまで駐車場の中で待つ（`Trip::waiting`）。待つ車は車線の外にいて、
//! 流れを止めない。車線に入るときの決まりは `car::place` にある。
//!
//! 行き先の駐車場は出発のときに決めて、空きを 1 台ぶん押さえる。満車なら、行き先から
//! [`PARK_RADIUS`] マスの中で空きのある、いちばん近い建物の駐車場にする。どこも満車なら車では行けない。
//! 停めてある台数と押さえた台数は別に数える。建物から車が出るときは停めてある台数だけを減らすので、
//! まだ着いていない車のために押さえた空きを、出る車が使ってしまうことはない。
//! 建物が消えると、その駐車場に停めてあった車も消える。そこへ向かっていた車は、着いたら停めずに消える。
//!
//! 遊ぶ人が置く共同の駐車場（[`LotId::Shared`] と [`LotId::Rented`]）もある。世帯が借りた枠は、車がいない
//! あいだも空かない（月極）。借りられていない枠は、近くへ来た車（外来）が建物の駐車場と同じく使う。
//! 出入りの位置は施設の正面の道路の上にとる（[`facility_spot`]）。

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::collections::{BTreeMap, BTreeSet};

use super::{
    Ends, SidewalkPos, Trip, Trips, candidates, car, junction, lane_shift, make_trip, plan, pos,
};
use crate::building::{Building, Buildings, LANE_WIDTH_MM, ParkingCut, SLOT_WIDTH_MM};
use crate::clock::TICKS_PER_DAY;
use crate::command::ZoneKind;
use crate::id::{BuildingId, TripId};
use crate::network::{Dir, LaneMode, LinkId, Network, SegmentId, TILE_MM};
use crate::routing::AstarRouter;
use crate::service::Facility;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::WorldPos;
use crate::units::{LaneOffset, Tick};

/// 満車のときに、ほかの駐車場を探す範囲（マス）。
pub const PARK_RADIUS: u64 = 16;

/// いまの区切りが終わったあとにすること。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Then {
    /// 移動を終える。
    Arrive,
    /// 車に乗って `park_at` まで走り、駐車場 `lot` に停めて `to` まで歩く。
    /// `lot` が `None` なのは、向かっていた共同の駐車場が取り壊されたときで、着いても停めずに消える。
    Drive {
        park_at: SidewalkPos,
        lot: Option<LotId>,
        to: SidewalkPos,
    },
    /// 車を駐車場 `lot` に停めて、`to` まで歩く。`lot` が `None` のときは [`Then::Drive`] と同じ。
    Walk { lot: Option<LotId>, to: SidewalkPos },
}

impl Then {
    /// 押さえている駐車場。
    pub(super) fn lot(&self) -> Option<LotId> {
        match self {
            Then::Arrive => None,
            Then::Drive { lot, .. } | Then::Walk { lot, .. } => *lot,
        }
    }

    /// 共同の駐車場 `corner` が取り壊されたので、そこへ向かう車は着いても停めないものにする。
    /// 同じ角に置き直した駐車場は別の駐車場なので、前に押さえた空きの持ち主と取り違えない。
    fn forget_shared_lot(&mut self, corner: TilePos) {
        let gone = |lot: &mut Option<LotId>| {
            if lot.and_then(LotId::shared_corner) == Some(corner) {
                *lot = None;
            }
        };
        match self {
            Then::Arrive => {}
            Then::Drive { lot, .. } | Then::Walk { lot, .. } => gone(lot),
        }
    }
}

impl StateHash for Then {
    fn write_state(&self, h: &mut StateHasher) {
        match self {
            Then::Arrive => h.write(&0_u8),
            Then::Drive {
                park_at,
                lot: Some(lot),
                to,
            } => {
                h.write(&1_u8);
                h.write(park_at);
                h.write(lot);
                h.write(to);
            }
            Then::Walk { lot: Some(lot), to } => {
                h.write(&2_u8);
                h.write(lot);
                h.write(to);
            }
            // 向かっていた駐車場がなくなった移動。駐車場のある移動と印を分ける。
            Then::Drive {
                park_at,
                lot: None,
                to,
            } => {
                h.write(&3_u8);
                h.write(park_at);
                h.write(to);
            }
            Then::Walk { lot: None, to } => {
                h.write(&4_u8);
                h.write(to);
            }
        }
    }
}

/// 車で向かおうとした移動の、駐車場の決まり方の数。読み取り用で、状態ハッシュには入れない。
/// 始まりからの積み上げで、日ごとの数は 2 つの時点の差で求める。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParkingTally {
    /// 車で走り始めた移動の数。
    pub drives: u64,
    /// そのうち、行き先の建物の駐車場が満車か、そこへ車か歩きで届かず、近くのほかの建物の駐車場に
    /// 停めに向かった数。
    pub parked_elsewhere: u64,
    /// 行き先の近くの駐車場がどこも満車で、車で向かうのをやめた数。やめた移動は、歩けるなら歩く。
    pub lots_full: u64,
    /// 世帯の車をほかの住民が使っているか、離れた所に停めてあって、車で向かえなかった通勤の数。
    /// 向かえなかった移動は、歩けるなら歩く。
    pub no_car: u64,
    /// 世帯が車を置く枠を持たず、車で向かわなかった通勤の数。車で向かうほど遠い移動だけを数える。
    /// `no_car` とは別に数え、`no_car` には含めない。
    pub no_place: u64,
    /// `no_car` のうち、世帯の車を家族が乗っていた数。
    pub no_car_in_use: u64,
    /// `no_car` のうち、世帯の車を家族が家でない所（職場など）に停めていた数。
    pub no_car_parked_away: u64,
    /// `no_car` のうち、車は家に停めてあるが、出発の建物が家でなく、本人は家にいなかった数。
    pub no_car_at_home: u64,
    /// `no_car` のうち、上のどれにも当てはまらない数（家のない住民の通勤で、車が停めてあった場合）。
    pub no_car_other: u64,
}

/// ある建物を行き先にした車が、駐車場で困った数。ゲーム内の 1 日ぶん。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LotMisses {
    /// 近くの駐車場がどこも満車で、車で向かうのをやめた数。
    pub lots_full: u32,
    /// 行き先の建物の駐車場に停められず、近くのほかの建物の駐車場に回った数。
    pub parked_elsewhere: u32,
}

impl LotMisses {
    fn is_empty(self) -> bool {
        self == LotMisses::default()
    }
}

/// 建物の今日と前の日で、満車で車をやめた数の和がこれ以上なら、駐車場が足りないとみなす（画面の知らせの目安）。
pub const SHORTAGE_LOTS_FULL: u32 = 3;

impl DailyLotMisses {
    /// 満車で車をやめた数の、今日と前の日の和。
    pub fn lots_full(self) -> u32 {
        self.today
            .lots_full
            .saturating_add(self.yesterday.lots_full)
    }
}

/// 建物ごとの [`LotMisses`]。数えた日と、その前の日だけを持つ。読み取り用で、状態ハッシュには入れない。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct DayMisses {
    /// `today` を数えた日（0 から数える）。
    day: u64,
    today: LotMisses,
    /// `day` の前の日。
    yesterday: LotMisses,
}

impl DayMisses {
    /// `day` の欄に送る。日が 1 つ進んだなら今日を前の日にし、2 日以上空いたら両方を空にする。
    fn rolled(self, day: u64) -> DayMisses {
        if self.day == day {
            self
        } else {
            DayMisses {
                day,
                today: LotMisses::default(),
                yesterday: if self.day + 1 == day {
                    self.today
                } else {
                    LotMisses::default()
                },
            }
        }
    }
}

/// 建物の今日と前の日の [`LotMisses`]。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DailyLotMisses {
    pub today: LotMisses,
    pub yesterday: LotMisses,
}

/// 建物ごとの、駐車場で困った数。困ったことのない建物と、2 日以上前を最後に困った建物は持ち続けるが、
/// 読み出しでは空として出す。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct MissTable {
    by_building: BTreeMap<BuildingId, DayMisses>,
}

impl MissTable {
    pub(super) fn add(&mut self, b: BuildingId, day: u64, f: impl FnOnce(&mut LotMisses)) {
        let e = self.by_building.entry(b).or_default();
        *e = e.rolled(day);
        f(&mut e.today);
    }

    /// `day` の時点で、困った数のある建物を番号の順に返す。
    pub(super) fn at(&self, day: u64) -> impl Iterator<Item = (BuildingId, DailyLotMisses)> + '_ {
        self.by_building.iter().filter_map(move |(id, m)| {
            let m = m.rolled(day);
            (!(m.today.is_empty() && m.yesterday.is_empty())).then_some((
                *id,
                DailyLotMisses {
                    today: m.today,
                    yesterday: m.yesterday,
                },
            ))
        })
    }

    pub(super) fn retain(&mut self, f: impl FnMut(&BuildingId, &mut DayMisses) -> bool) {
        self.by_building.retain(f);
    }
}

/// 1 つの駐車場の、停めてある台数と、向かっている車のために押さえた台数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lot {
    pub parked: u32,
    pub reserved: u32,
}

impl StateHash for Lot {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.parked);
        h.write(&self.reserved);
    }
}

/// 車を停める駐車場。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LotId {
    /// 建物の駐車場。
    Building(BuildingId),
    /// 共同の駐車場の、借りられていない枠（外来の車が使う）。駐車場の占める範囲の最小の角で指す。
    Shared(TilePos),
    /// 共同の駐車場の、世帯が借りた枠。借りた世帯の車だけが使う。
    Rented(TilePos),
}

impl LotId {
    /// 建物の駐車場なら、その建物。
    pub fn building(self) -> Option<BuildingId> {
        match self {
            LotId::Building(b) => Some(b),
            LotId::Shared(_) | LotId::Rented(_) => None,
        }
    }

    /// 共同の駐車場なら、その角のマス。
    pub fn shared_corner(self) -> Option<TilePos> {
        match self {
            LotId::Building(_) => None,
            LotId::Shared(c) | LotId::Rented(c) => Some(c),
        }
    }
}

impl StateHash for LotId {
    fn write_state(&self, h: &mut StateHasher) {
        match self {
            // 建物の駐車場は建物の ID だけを書く。共同の駐車場のない街では、ハッシュが前と変わらない。
            LotId::Building(b) => h.write(b),
            LotId::Shared(c) | LotId::Rented(c) => {
                // 建物の ID の添え字に使わない値を印にして、建物の駐車場と分ける。
                h.write(&u32::MAX);
                h.write(&u8::from(matches!(self, LotId::Rented(_))));
                h.write(&c.x);
                h.write(&c.y);
            }
        }
    }
}

/// 共同の駐車場 1 つの台数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedLot {
    /// 世帯に貸した枠の数。車がいないあいだも空かない。
    pub rented: u32,
    /// 借りた世帯の車が停まっている台数。
    pub rented_parked: u32,
    /// 借りていない車（外来）の、停めた台数と押さえた台数。
    pub visitors: Lot,
}

impl SharedLot {
    /// 停まっている車の台数。借りた世帯の車と、借りていない車（外来）の合計である。
    pub fn parked(self) -> u64 {
        u64::from(self.rented_parked) + u64::from(self.visitors.parked)
    }

    /// 台数 `spaces` の駐車場に、外来の車か新しく借りる世帯のための空きがあるか。
    pub fn has_room(self, spaces: u32) -> bool {
        u64::from(self.rented) + u64::from(self.visitors.parked) + u64::from(self.visitors.reserved)
            < u64::from(spaces)
    }
}

impl StateHash for SharedLot {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.rented);
        h.write(&self.rented_parked);
        h.write(&self.visitors);
    }
}

/// 角のマスが `corner` の共同の駐車場。
pub fn shared_facility(facilities: &[Facility], corner: TilePos) -> Option<&Facility> {
    facilities
        .iter()
        .find(|f| f.kind.is_parking() && f.area.min == corner)
}

/// 駐車場 `lot` がいまもあるか。
pub(super) fn lot_exists(lot: LotId, buildings: &Buildings, facilities: &[Facility]) -> bool {
    match lot {
        LotId::Building(b) => buildings.get(b).is_some(),
        LotId::Shared(c) | LotId::Rented(c) => shared_facility(facilities, c).is_some(),
    }
}

/// 建物ごとの駐車場と、共同の駐車場ごとの台数。どの台数も 0 の駐車場は持たない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Parking {
    lots: BTreeMap<BuildingId, Lot>,
    /// 共同の駐車場の台数。角のマスで引く。
    shared: BTreeMap<TilePos, SharedLot>,
}

impl Parking {
    fn get(&self, b: BuildingId) -> Lot {
        self.lots.get(&b).copied().unwrap_or_default()
    }

    /// 建物 `b` の駐車場を `f` で書き換える。どちらの台数も 0 になったら表から外す。
    fn update(&mut self, b: BuildingId, f: impl FnOnce(&mut Lot)) {
        let lot = self.lots.entry(b).or_default();
        f(lot);
        if *lot == Lot::default() {
            self.lots.remove(&b);
        }
    }

    fn shared_lot(&self, c: TilePos) -> SharedLot {
        self.shared.get(&c).copied().unwrap_or_default()
    }

    /// 共同の駐車場 `c` を `f` で書き換える。どの台数も 0 になったら表から外す。
    fn update_shared(&mut self, c: TilePos, f: impl FnOnce(&mut SharedLot)) {
        let lot = self.shared.entry(c).or_default();
        f(lot);
        if *lot == SharedLot::default() {
            self.shared.remove(&c);
        }
    }

    fn reserve(&mut self, lot: LotId) {
        match lot {
            LotId::Building(b) => self.update(b, |l| {
                l.reserved = l.reserved.checked_add(1).expect("押さえた台数があふれた");
            }),
            LotId::Shared(c) => self.update_shared(c, |s| {
                s.visitors.reserved = s
                    .visitors
                    .reserved
                    .checked_add(1)
                    .expect("押さえた台数があふれた");
            }),
            // 借りた枠はいつも空けてあるので、押さえない。
            LotId::Rented(_) => {}
        }
    }

    /// 押さえた空きを返す。駐車場がなくなっていれば何もしない。
    fn release(&mut self, lot: LotId) {
        match lot {
            LotId::Building(b) if self.lots.contains_key(&b) => self.update(b, |l| {
                l.reserved = l
                    .reserved
                    .checked_sub(1)
                    .expect("押さえていない空きを返した");
            }),
            // 取り壊された駐車場へ向かう移動は `Then` の `lot` を `None` にしてあるので、ここへは来ない。念のため残っていない分は返さない。
            LotId::Shared(c) if self.shared_lot(c).visitors.reserved > 0 => {
                self.update_shared(c, |s| s.visitors.reserved -= 1);
            }
            LotId::Building(_) | LotId::Shared(_) | LotId::Rented(_) => {}
        }
    }

    /// 押さえた空きに車を停め、停めたら `true`。駐車場がなくなっていれば、車は停めずに消える。
    fn park(&mut self, lot: LotId) -> bool {
        match lot {
            LotId::Building(b) => {
                let there = self.lots.contains_key(&b);
                if there {
                    self.update(b, |l| {
                        l.reserved = l
                            .reserved
                            .checked_sub(1)
                            .expect("押さえていない空きに停めた");
                        l.parked = l.parked.checked_add(1).expect("停めた台数があふれた");
                    });
                }
                there
            }
            LotId::Shared(c) => {
                // 取り壊された駐車場へ向かう移動は `lot` が `None` なので、ここへは来ない。念のため押さえた空きがなければ停めない。
                let there = self.shared_lot(c).visitors.reserved > 0;
                if there {
                    self.update_shared(c, |s| {
                        s.visitors.reserved -= 1;
                        s.visitors.parked = s
                            .visitors
                            .parked
                            .checked_add(1)
                            .expect("停めた台数があふれた");
                    });
                }
                there
            }
            LotId::Rented(c) => {
                // 借りた枠の数は、借りている世帯がいるあいだ 0 にならない。
                let there = self.shared_lot(c).rented > 0;
                if there {
                    self.update_shared(c, |s| {
                        s.rented_parked = s
                            .rented_parked
                            .checked_add(1)
                            .expect("停めた台数があふれた");
                    });
                }
                there
            }
        }
    }

    /// 停めてある車を 1 台出す。停めてある車がなければ `false`。
    pub(super) fn unpark(&mut self, lot: LotId) -> bool {
        match lot {
            LotId::Building(b) => {
                let had = self.get(b).parked > 0;
                if had {
                    self.update(b, |l| l.parked -= 1);
                }
                had
            }
            LotId::Shared(c) => {
                let had = self.shared_lot(c).visitors.parked > 0;
                if had {
                    self.update_shared(c, |s| s.visitors.parked -= 1);
                }
                had
            }
            LotId::Rented(c) => {
                let had = self.shared_lot(c).rented_parked > 0;
                if had {
                    self.update_shared(c, |s| s.rented_parked -= 1);
                }
                had
            }
        }
    }

    /// 空きを押さえずに、車を 1 台停める。出した車を、車で出られなかったので駐車場に戻すときと、
    /// 枠を得た世帯の車をその枠に置くとき（[`Trips::place_car`]）に使う。
    pub(super) fn put_back(&mut self, lot: LotId) {
        let add = |n: u32| n.checked_add(1).expect("停めた台数があふれた");
        match lot {
            LotId::Building(b) => self.update(b, |l| l.parked = add(l.parked)),
            LotId::Shared(c) => {
                self.update_shared(c, |s| s.visitors.parked = add(s.visitors.parked));
            }
            LotId::Rented(c) => self.update_shared(c, |s| s.rented_parked = add(s.rented_parked)),
        }
    }

    /// 共同の駐車場ごとの、貸した枠の数を `counts` にする。載っていない駐車場は 0 にする。
    fn set_rented(&mut self, counts: &BTreeMap<TilePos, u32>) {
        let corners: BTreeSet<TilePos> = self.shared.keys().chain(counts.keys()).copied().collect();
        for c in corners {
            let n = counts.get(&c).copied().unwrap_or(0);
            self.update_shared(c, |s| s.rented = n);
        }
    }

    /// 世帯の枠が駐車場 `own` にある運転する人が、建物 `id` の駐車場に停められるか。空きがあり、
    /// 住宅なら `own` であるときに限る。住宅の駐車場は、その建物の自宅の枠を持つ世帯の車だけが使う。
    fn is_free(
        &self,
        (id, b): (BuildingId, &Building),
        seed: u64,
        own: Option<BuildingId>,
    ) -> bool {
        let l = self.get(id);
        (b.zone.kind() != ZoneKind::Residential || own == Some(id))
            && u64::from(l.parked) + u64::from(l.reserved) < b.parking_spaces(seed)
    }

    /// 世帯の枠が駐車場 `own` にある運転する人が停められる、行き先 `to` の近くの駐車場を、試す順に返す。行き先そのものを
    /// 先に、そのあとは [`PARK_RADIUS`] マスの中の建物を近い順に並べる。同じ距離なら番号の小さい建物を先にする。
    ///
    /// 共同の駐車場は、外来の空きがあれば同じ範囲の候補に入る。距離が同じなら建物を先にする。
    /// 枠を借りた世帯の車は、借りた枠をいつも使える。家（`home`）に帰るときは、遠くても借りた枠を先に試す。
    fn candidates(
        &self,
        (buildings, facilities): (&Buildings, &[Facility]),
        (to, dest): (BuildingId, &Building),
        own: Option<LotId>,
        home: Option<BuildingId>,
    ) -> Vec<LotId> {
        let r = i64::try_from(PARK_RADIUS).expect("範囲が i64 に収まらない");
        let at = |dx: i64, dy: i64| {
            let c = |v: i64, d: i64| v.checked_add(d).expect("マスの座標があふれた");
            TilePos::new(c(dest.front.x, dx), c(dest.front.y, dy))
        };
        let area = TileRect::from_corners(at(-r, -r), at(r, r));
        let dist = |a: TilePos| a.x.abs_diff(dest.front.x).max(a.y.abs_diff(dest.front.y));
        let own_building = own.and_then(LotId::building);
        let mut near: Vec<(u64, LotId)> = buildings
            .fronts_in(area)
            .filter(|id| *id != to)
            .filter_map(|id| Some((id, buildings.get(id)?)))
            .filter(|(id, b)| self.is_free((*id, b), buildings.seed(), own_building))
            .map(|(id, b)| (dist(b.front), LotId::Building(id)))
            .collect();
        let first = match own {
            Some(LotId::Rented(c))
                if home == Some(to) && shared_facility(facilities, c).is_some() =>
            {
                Some(LotId::Rented(c))
            }
            _ => self
                .is_free((to, dest), buildings.seed(), own_building)
                .then_some(LotId::Building(to)),
        };
        for f in facilities.iter().filter(|f| f.kind.is_parking()) {
            let (c, d) = (f.area.min, dist(f.front));
            let lot = if own == Some(LotId::Rented(c)) {
                LotId::Rented(c)
            } else if self.shared_lot(c).has_room(f.kind.parking_spaces()) {
                LotId::Shared(c)
            } else {
                continue;
            };
            if d <= PARK_RADIUS && first != Some(lot) {
                near.push((d, lot));
            }
        }
        // `LotId` は建物を先に並べるので、距離が同じなら建物を先に試す。
        near.sort_unstable();
        first
            .into_iter()
            .chain(near.into_iter().map(|(_, lot)| lot))
            .collect()
    }
}

impl StateHash for Parking {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.lots);
        // 共同の駐車場のない街では、ハッシュが前と変わらないようにする。
        if !self.shared.is_empty() {
            h.write(&self.shared);
        }
    }
}

/// 車線に出るのを待つ車を、待っている点ごとに数えた 1 行。読み取り用で、状態ハッシュには入れない。
/// 道路をはさんで向かい合う建物は、出入りの位置が道路の中心線の上の同じ点になり、どちらの建物の前で
/// 待つ車か見分けられない。そこで、点ごとにまとめ、その点を出入口か出入りの位置にする建物を全部並べる。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CurbWait {
    /// 車が待っている点。
    pub at: WorldPos,
    /// この点を出入口か出入りの位置にする建物。番号の順。
    pub buildings: Vec<BuildingId>,
    /// 駐車場の中で、車線に出る順番を待っている車の台数。
    pub waiting: u32,
    /// そのうち、待ち始めてから指定の tick 数以上たった車の台数。
    pub long: u32,
}

/// 建物の出入口の位置。出入口の歩道の上でもあり、同じ側の車のリンクの上でもある。
/// 出入口のリンクがなければ `None`。
///
/// 私道の敷地（`Building::lane_plan`）の住民は、私道を通って通りに出るので、私道の入口（[`lot_spot`] と同じ点）を
/// 出入口にする。私道は経路の網に入れず、私道の中の道のりも足さない（`docs/design.md` の 3.8）。
pub fn building_spot(net: &Network, b: &Building) -> Option<SidewalkPos> {
    if let Some(plan) = b.lane_plan() {
        let cut = ParkingCut::Lane {
            houses: plan.houses,
        };
        return lot_choice(net, b, cut).map(|(p, _)| p);
    }
    door_spot(net, b)
}

/// `Building::entrance` の位置。間口の `front` のマスに最も近い歩道の上の点である。
fn door_spot(net: &Network, b: &Building) -> Option<SidewalkPos> {
    SidewalkPos::on_any_link(net, b.entrance, b.entrance_at)
}

/// 建物の駐車場の出入りの位置。着く車はここで車線から外れて駐車場に入り、出る車はここから車線に入る。
/// 出入口と同じく、歩道の上でもあり、同じ側の車のリンクの上でもある。
///
/// 切り欠いた平面の駐車場は、間口のどちらかの端に寄せる。脇の駐車場なら端の列の、道路に面した辺の
/// 真ん中、前面の帯なら端から並べた枠の真ん中が出入りの位置になる。端は、面した道路の近い交差点から
/// 遠い側にし、同じならワールド座標の大きい側にする。出入りの位置を、交差点の手前の停止線と、
/// 交差点を抜けた直後の合流から離すためである。
/// 平面の駐車場のない建物（斜めの道路に面した建物など）は、建物の中の駐車場に出入口から入る。
/// 出入りの位置が道路の上に取れなければ（行き止まりの端など）、出入口の位置にする。
/// `seed` は街の種で、駐車場の形（`Building::parking_layout`）を決めるのに使う。
pub fn lot_spot(net: &Network, b: &Building, seed: u64) -> Option<SidewalkPos> {
    lot_choice(net, b, b.parking_layout(seed).cut).map(|(p, _)| p)
}

/// 共同の駐車場の出入りの位置。施設の正面の道路の上で、建物の脇の駐車場と同じく、間口の両端のうち
/// 近い交差点から遠い側の端のマスの真ん中にとる（[`lot_spot`]）。歩道の上でもあり、同じ側の車のリンクの上でもある。
/// 端が道路の上に取れなければ、正面のマス（`Facility::front`）の前にする。面した道路がなければ `None`。
pub fn facility_spot(net: &Network, f: &Facility) -> Option<SidewalkPos> {
    facility_choice(net, f).map(|(p, _)| p)
}

/// 共同の駐車場の出入りの位置と、それが間口のどちらの端にあるか。端が道路の上に取れず、
/// 正面のマスの前にしたときの端は `None`。[`facility_spot`] と [`facility_end`] が共用する。
fn facility_choice(net: &Network, f: &Facility) -> Option<(SidewalkPos, Option<LotEnd>)> {
    let link = f.entrance?;
    let (_, step, _) = pos::link_geometry(net, link)?;
    let site = (f.front, f.toward_road, f.area);
    if let Some((p, end)) = end_spot(net, link, site, step, (TILE_MM / 2, false)) {
        return Some((p, Some(end)));
    }
    let line = net.segment_line(link.segment)?;
    let road = TilePos::new(
        f.front.x.checked_add(f.toward_road.0)?,
        f.front.y.checked_add(f.toward_road.1)?,
    );
    let k = line.position_of(road)?;
    let steps = match link.dir {
        Dir::Forward => k,
        Dir::Backward => line.len.checked_sub(k)?,
    };
    let at = steps.checked_mul(u64::try_from(TILE_MM).ok()?)?;
    SidewalkPos::on_any_link(net, link, LaneOffset(u32::try_from(at).ok()?)).map(|p| (p, None))
}

/// 切り欠いた駐車場が寄る、間口の端。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LotEnd {
    /// `Building::front` のある端。
    Front,
    /// 間口を `front` から進んだ先の、もう一方の端。
    Far,
}

/// 共同の駐車場の出入りの位置がある、間口の端（[`facility_spot`] が選ぶ端と同じ）。出入りの位置が
/// 端に取れないときは、間口に沿う向きがワールド座標の大きい側へ向かうほうの端を返す。描画が枠の向きを決めるのに使う。
pub fn facility_end(net: &Network, f: &Facility) -> LotEnd {
    facility_choice(net, f)
        .and_then(|(_, end)| end)
        .unwrap_or_else(|| {
            let (tx, ty) = f.toward_road;
            if tx - ty > 0 {
                LotEnd::Far
            } else {
                LotEnd::Front
            }
        })
}

/// 切り欠いた駐車場が寄る端。[`lot_spot`] が選ぶ端と同じである。出入りの位置が道路の上に取れず、
/// 出入口にしたときは、同じ決まりの最後の「座標の大きい側」を返す。描画が切り欠きの位置を決めるのに使う。
pub fn lot_end(net: &Network, b: &Building, seed: u64) -> LotEnd {
    cut_end(net, b, b.parking_layout(seed).cut)
}

/// 切り欠く形が `cut` の建物 `b` の、切り欠いた駐車場が寄る端（[`lot_end`]）。
fn cut_end(net: &Network, b: &Building, cut: ParkingCut) -> LotEnd {
    lot_choice(net, b, cut)
        .and_then(|(_, end)| end)
        .unwrap_or_else(|| {
            let (tx, ty) = b.toward_road;
            // 間口に沿う向き `(-ty, tx)` が、ワールド座標の大きい側へ向かうほうの端
            if tx - ty > 0 {
                LotEnd::Far
            } else {
                LotEnd::Front
            }
        })
}

/// 切り欠いた駐車場にかかるマス。1 マスの一部だけを切り欠く前面の帯のマスも含む。切り欠かない建物は空。
/// 地図に出すためのもので、並びは決まっていない。形は [`lot_end`] と `Building::parking_layout` で決まる。
pub fn lot_tiles(net: &Network, b: &Building, seed: u64) -> Vec<TilePos> {
    let cut = b.parking_layout(seed).cut;
    let (tx, ty) = b.toward_road;
    let (ax, ay) = (-ty, tx);
    let w = b.area.max.x.abs_diff(b.area.min.x) + 1;
    let h = b.area.max.y.abs_diff(b.area.min.y) + 1;
    let (frontage, depth) = if tx == 0 { (w, h) } else { (h, w) };
    // 間口に沿って数えるマスの数 `along` と、道路の側からの行の数 `rows` の長方形を、端に寄せて返す
    let rect = |cols: u64, rows: u64| -> Vec<TilePos> {
        let far = cut_end(net, b, cut) == LotEnd::Far;
        let mut out = Vec::new();
        for k in 0..cols {
            let kk =
                i64::try_from(if far { frontage - 1 - k } else { k }).expect("間口が大きすぎる");
            for j in 0..rows {
                let j = i64::try_from(j).expect("奥行きが大きすぎる");
                out.push(TilePos::new(
                    b.front.x + ax * kk - tx * j,
                    b.front.y + ay * kk - ty * j,
                ));
            }
        }
        out
    };
    match cut {
        // 私道の脇の枠は、私道のマス（[`lane_tiles`]）に含める。
        ParkingCut::None | ParkingCut::Lane { .. } => Vec::new(),
        ParkingCut::FrontStrip { slots } => {
            rect((slots * SLOT_WIDTH_MM).div_ceil(TILE_MM as u64), 1)
        }
        ParkingCut::Side { columns, tiles } => {
            let last = tiles - (columns - 1) * depth;
            let mut out = rect(columns - 1, depth);
            // 最後の列は、道路の側の `last` マスだけを切り欠く。列の位置は `rect` と同じ向きで続ける
            let far = cut_end(net, b, cut) == LotEnd::Far;
            let k = columns - 1;
            let kk =
                i64::try_from(if far { frontage - 1 - k } else { k }).expect("間口が大きすぎる");
            for j in 0..last {
                let j = i64::try_from(j).expect("奥行きが大きすぎる");
                out.push(TilePos::new(
                    b.front.x + ax * kk - tx * j,
                    b.front.y + ay * kk - ty * j,
                ));
            }
            out
        }
    }
}

/// 私道の通るマス。私道は [`lot_end`] の端の列を、道路の側から奥へ延びる。私道の脇の枠もこの列にある。
/// 私道の敷地でない建物は空。地図に出すためのもので、道路の側から順に並べる。
pub fn lane_tiles(net: &Network, b: &Building) -> Vec<TilePos> {
    let Some(plan) = b.lane_plan() else {
        return Vec::new();
    };
    let (tx, ty) = b.toward_road;
    let (ax, ay) = (-ty, tx);
    let span = if tx == 0 {
        b.area.max.x.abs_diff(b.area.min.x)
    } else {
        b.area.max.y.abs_diff(b.area.min.y)
    };
    let cut = ParkingCut::Lane {
        houses: plan.houses,
    };
    let k = if cut_end(net, b, cut) == LotEnd::Far {
        i64::try_from(span).expect("間口が大きすぎる")
    } else {
        0
    };
    (0..plan.length.div_ceil(TILE_MM as u64))
        .map(|j| {
            let j = i64::try_from(j).expect("奥行きが大きすぎる");
            TilePos::new(b.front.x + ax * k - tx * j, b.front.y + ay * k - ty * j)
        })
        .collect()
}

/// [`lot_spot`] の本体。切り欠く形が `cut` の建物 `b` の、出入りの位置と、選んだ端を返す。出入口にしたときの端は `None`。
fn lot_choice(
    net: &Network,
    b: &Building,
    cut: ParkingCut,
) -> Option<(SidewalkPos, Option<LotEnd>)> {
    let door = door_spot(net, b)?;
    let half = TILE_MM / 2;
    // 端の外側の辺から、出入りの位置まで内へ入る距離（mm）。
    let inward = match cut {
        ParkingCut::None => return Some((door, None)),
        ParkingCut::FrontStrip { slots } => slots
            .checked_mul(SLOT_WIDTH_MM)
            .and_then(|w| i64::try_from(w / 2).ok())
            .expect("帯の長さがあふれた"),
        ParkingCut::Side { .. } => half,
        // 私道の真ん中の線が、私道の入口になる。
        ParkingCut::Lane { .. } => {
            i64::try_from(LANE_WIDTH_MM / 2).expect("私道の幅が i64 に収まらない")
        }
    };
    let (_, step, _) = pos::link_geometry(net, b.entrance)?;
    let site = (b.front, b.toward_road, b.area);
    let lane = matches!(cut, ParkingCut::Lane { .. });
    Some(
        end_spot(net, b.entrance, site, step, (inward, lane))
            .map_or((door, None), |(p, end)| (p, Some(end))),
    )
}

/// 間口の両端のうち、面した道路の近い交差点から遠い側（同じならワールド座標の大きい側）の端の、
/// 道路の上の出入りの位置と、その端。`(front, toward_road, area)` は敷地の正面のマス、道路へ向かう向き、
/// 占めるマスで、出入りの位置は端の外側の辺から `inward` mm 内へ入った所にとる。`step` は出入口のリンク
/// `entrance` の 1 歩の向きである。`lane` なら私道の入口で、マスの真ん中には寄せない。
/// どちらの端も道路の上に取れなければ `None`。
fn end_spot(
    net: &Network,
    entrance: LinkId,
    (front, toward_road, area): (TilePos, (i64, i64), TileRect),
    step: (i8, i8),
    (inward, lane): (i64, bool),
) -> Option<(SidewalkPos, LotEnd)> {
    let half = TILE_MM / 2;
    let (tx, ty) = toward_road;
    let along = (-ty, tx);
    // `front` から間口のもう一方の端のマスまでのマスの数。
    let span = if tx == 0 {
        area.max.x.abs_diff(area.min.x)
    } else {
        area.max.y.abs_diff(area.min.y)
    };
    let far = i64::try_from(span).expect("間口が i64 に収まらない");
    let at = |t: TilePos, (dx, dy): (i64, i64), k: i64| {
        let c = |v: i64, d: i64| {
            d.checked_mul(k)
                .and_then(|o| v.checked_add(o))
                .expect("マスの座標があふれた")
        };
        TilePos::new(c(t.x, dx), c(t.y, dy))
    };
    // 間口の両端のマスと、そこから外へ向かう向き。1 マスの間口なら、同じマスの両側になる。
    let ends = [
        (LotEnd::Front, front, (-along.0, -along.1)),
        (LotEnd::Far, at(front, along, far), along),
    ];
    // 出入りの位置の、端のマスの真ん中からのずれ（mm）。帯の真ん中は、マスの真ん中より内に入りうる。
    let shift = half - inward;
    ends.into_iter()
        .filter_map(|(end, tile, out)| {
            let road = at(tile, (tx, ty), 1);
            let sign = out.0 * i64::from(step.0) + out.1 * i64::from(step.1);
            // 私道の入口は私道の幅の中に取らなければならないので、マスの真ん中には寄せない。
            // 取れない端（行き止まりの端など）は選ばず、反対の端に私道を通す。
            let offsets: &[i64] = if lane {
                &[sign * shift]
            } else {
                &[sign * shift, 0]
            };
            let p = offsets
                .iter()
                .flat_map(|d| spots_near(road, step, *d))
                .find(|p| {
                    p.resolve_on(net, LaneMode::Sidewalk)
                        .is_some_and(|(l, _)| joined(net, l.segment, entrance.segment))
                        && p.resolve_on(net, LaneMode::Car).is_some()
                })?;
            let (link, off) = p.resolve_on(net, LaneMode::Car)?;
            let w = p.world();
            Some(((junction_clearance(net, link, off), w.x, w.y), p, end))
        })
        .max_by_key(|(k, _, _)| *k)
        .map(|(_, p, end)| (p, end))
}

/// 区間 `a` と `b` が同じ区間か、端のノードでつながっているか。建物の間口は、隣り合うだけで
/// つながっていない 2 本の道路にまたがりうるので、出入りの位置は出入口の区間とつながる区間に取る。
fn joined(net: &Network, a: SegmentId, b: SegmentId) -> bool {
    a == b
        || net
            .segment(a)
            .zip(net.segment(b))
            .is_some_and(|(x, y)| [x.a, x.b].iter().any(|n| *n == y.a || *n == y.b))
}

/// [`lot_spot`] のうち、出入口から歩道で届くもの。届かなければ出入口にする。出入口の区間と
/// つながる区間に取っても、片側の歩道が途切れているなどで届かないことがあるためである。
fn reachable_lot_spot(
    net: &Network,
    router: &mut AstarRouter,
    b: &Building,
    seed: u64,
) -> Option<SidewalkPos> {
    let door = building_spot(net, b)?;
    let at = lot_spot(net, b, seed)?;
    let ok = at == door || reachable(net, router, LaneMode::Sidewalk, door, at);
    Some(if ok { at } else { door })
}

/// 道路のマス `road` の真ん中から、1 歩の向き `step` に `d` mm ずれた位置の表し方。真ん中ちょうどは、
/// 前の 1 歩の終わりと、次の 1 歩の始まりの 2 つで表せる。区間の端のマスでは片方しか道路の上にないので、
/// 両方を試す順に返す。
fn spots_near(road: TilePos, step: (i8, i8), d: i64) -> SmallVec<[SidewalkPos; 2]> {
    let per = TILE_MM;
    let back = TilePos::new(
        road.x
            .checked_sub(i64::from(step.0))
            .expect("マスの座標があふれた"),
        road.y
            .checked_sub(i64::from(step.1))
            .expect("マスの座標があふれた"),
    );
    let along = |v: i64| LaneOffset(u32::try_from(v).expect("1 歩の中の距離が u32 に収まらない"));
    let mut out = SmallVec::new();
    if d <= 0 {
        out.push(SidewalkPos {
            tile: back,
            step,
            along: along(per + d),
        });
    }
    if d >= 0 {
        out.push(SidewalkPos {
            tile: road,
            step,
            along: along(d),
        });
    }
    out
}

/// 車のリンク `link` の、始まりから `at` の位置から、リンクの両端の交差点のうち近いほうまでの距離（mm）。
/// どちらの端も交差点でなければ `u32::MAX`。
fn junction_clearance(net: &Network, link: LinkId, at: LaneOffset) -> u32 {
    let Some((seg, (_, _, len))) = net.segment(link.segment).zip(pos::link_geometry(net, link))
    else {
        return u32::MAX;
    };
    let (start, end) = match link.dir {
        Dir::Forward => (seg.a, seg.b),
        Dir::Backward => (seg.b, seg.a),
    };
    let to_start = junction::is_junction(net, start).then_some(at.0);
    let to_end = junction::is_junction(net, end).then(|| len.0.saturating_sub(at.0));
    to_start.into_iter().chain(to_end).min().unwrap_or(u32::MAX)
}

/// 手段 `mode` で `from` から `to` へ行けるか。
fn reachable(
    net: &Network,
    router: &mut AstarRouter,
    mode: LaneMode,
    from: SidewalkPos,
    to: SidewalkPos,
) -> bool {
    let Some(e) = from.resolve_on(net, mode).zip(to.resolve_on(net, mode)) else {
        return false;
    };
    // 経路があるかは費用によらない。混み具合の遅れが変わるたびに探し直さないよう、まず網の形から確かめる。
    if candidates(net, e, false)
        .iter()
        .any(|((a, _), (b, _))| router.surely_connected(mode, *a, *b))
    {
        return true;
    }
    // 網の形から言えなければ探す。探した答えも費用によらないので、網が変わるまで覚えておく。
    let key = (mode, e.0, e.1);
    if let Some(found) = router.known_reach(&key) {
        return found;
    }
    let found = plan(router, net, mode, &[e], false)[0].is_some();
    router.remember_reach(key, found);
    found
}

impl Trip {
    /// 空きを押さえている駐車場。車を停めるまでのあいだだけ持つ。
    pub fn parking_lot(&self) -> Option<LotId> {
        self.then.lot()
    }
}

impl Trips {
    /// 車で向かおうとした移動の、駐車場の決まり方の数。
    pub fn parking_tally(&self) -> ParkingTally {
        self.parking_tally
    }

    /// ゲーム内の日 `day` の時点で、行き先にした車が駐車場で困った建物を、番号の順に返す。
    /// 困った数は今日と前の日で、どちらも 0 の建物は出さない。
    pub fn lot_misses(&self, day: u64) -> impl Iterator<Item = (BuildingId, DailyLotMisses)> + '_ {
        self.lot_misses.at(day)
    }

    /// 建物 `b` の駐車場の台数。
    pub fn lot(&self, b: BuildingId) -> Lot {
        self.parking.get(b)
    }

    /// 駐車場を建物の番号の順に返す。
    pub fn lots(&self) -> impl Iterator<Item = (BuildingId, Lot)> + '_ {
        self.parking.lots.iter().map(|(id, l)| (*id, *l))
    }

    /// 角のマスが `corner` の共同の駐車場の台数。
    pub fn shared_lot(&self, corner: TilePos) -> SharedLot {
        self.parking.shared_lot(corner)
    }

    /// 共同の駐車場ごとの、貸した枠の数を `counts` にする。載っていない駐車場は 0 にする。
    /// 世帯の枠（`CarPlaces`）を変えたあとに呼ぶ。
    pub fn set_rented(&mut self, counts: &BTreeMap<TilePos, u32>) {
        self.parking.set_rented(counts);
    }

    /// 取り壊した共同の駐車場 `corner` の台数を捨てる。そこに停めてあった世帯の車は、どこにもないものにする。
    /// 向かっている車は、着いても停めずに消える。
    pub fn forget_shared_lot(&mut self, corner: TilePos) {
        self.parking.shared.remove(&corner);
        self.commuters.forget_cars_at(corner);
        for t in self.rows.iter_mut().flatten() {
            t.then.forget_shared_lot(corner);
        }
    }

    /// 建物 `from` から建物 `to` へ、歩く → 車 → 歩く で向かう移動を作る。行き先の駐車場の空きは
    /// このときに押さえ、`from` に停めてある車があれば 1 台出す。建物がないか、行き先の近くがどこも満車か、
    /// 3 つの区切りのどれかの経路がなければ `None` で、駐車場も変えない。運転する人は世帯の枠を持たない
    /// ものとし、住宅の駐車場には停めない。`facilities` は共同の駐車場を探すのに使う。
    pub fn spawn_drive(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        (buildings, facilities): (&Buildings, &[Facility]),
        (from, to): (BuildingId, BuildingId),
        now: Tick,
    ) -> Option<TripId> {
        let take = Some(LotId::Building(from));
        let sites = (buildings, facilities);
        self.spawn_drive_taking(net, router, sites, (from, to), take, (None, None), now)
    }

    /// [`Trips::spawn_drive`] と同じだが、停めてある車を出す駐車場を `take` で決め、運転する人の世帯の枠のある
    /// 駐車場と家を `(own, home)` で渡す。`take` が `None` ならどの駐車場からも出さない。
    ///
    /// 車が共同の駐車場から出るときは、出発の建物の出入口からその駐車場の出入りの位置まで歩いて乗る。
    /// 共同の駐車場の枠を借りた世帯の車がまだどこにもなく、家から出るときは、借りた枠から車が出る。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn spawn_drive_taking(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        (buildings, facilities): (&Buildings, &[Facility]),
        (from, to): (BuildingId, BuildingId),
        take: Option<LotId>,
        (own, home): (Option<LotId>, Option<BuildingId>),
        now: Tick,
    ) -> Option<TripId> {
        let from_b = buildings.get(from)?;
        let door = building_spot(net, from_b)?;
        router.sync(net);
        let start = take.or(match own {
            Some(LotId::Rented(c)) if home == Some(from) => Some(LotId::Rented(c)),
            _ => None,
        });
        let origin = match start.and_then(LotId::shared_corner) {
            Some(c) => facility_spot(net, shared_facility(facilities, c)?)?,
            None => reachable_lot_spot(net, router, from_b, buildings.seed())?,
        };
        let dest = building_spot(net, buildings.get(to)?)?;
        let took = take.filter(|lot| self.parking.unpark(*lot));
        let id = self.start_drive(
            net,
            router,
            (buildings, facilities),
            (door, origin),
            (to, dest),
            (own, home),
            now,
        );
        if let (None, Some(lot)) = (id, took) {
            self.parking.put_back(lot);
        }
        id
    }

    /// 出入口 `door` から駐車場の出入りの位置 `origin` まで歩き、そこから建物 `to`（出入口 `dest`）へ
    /// 車で向かう移動を作る。運転する人の世帯の枠のある駐車場と家は `(own, home)` である。
    #[allow(clippy::too_many_arguments)]
    fn start_drive(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        (buildings, facilities): (&Buildings, &[Facility]),
        (door, origin): (SidewalkPos, SidewalkPos),
        (to, dest): (BuildingId, SidewalkPos),
        (own, home): (Option<LotId>, Option<BuildingId>),
        now: Tick,
    ) -> Option<TripId> {
        router.sync(net);
        // 近い順に試し、車でも歩いても行ける最初の駐車場にする。
        let candidates =
            self.parking
                .candidates((buildings, facilities), (to, buildings.get(to)?), own, home);
        if candidates.is_empty() {
            self.parking_tally.lots_full = self.parking_tally.lots_full.saturating_add(1);
            self.lot_misses.add(to, now.0 / TICKS_PER_DAY, |m| {
                m.lots_full = m.lots_full.saturating_add(1);
            });
            return None;
        }
        let (lot, park_at) = candidates.into_iter().find_map(|lot| {
            let at = match lot {
                LotId::Building(b) => {
                    reachable_lot_spot(net, router, buildings.get(b)?, buildings.seed())?
                }
                LotId::Shared(c) | LotId::Rented(c) => {
                    facility_spot(net, shared_facility(facilities, c)?)?
                }
            };
            (reachable(net, router, LaneMode::Car, origin, at)
                && reachable(net, router, LaneMode::Sidewalk, at, dest))
            .then_some((lot, at))
        })?;
        let id = self.spawn_many(net, router, &[(door, origin)], now)[0]?;
        self.rows[id.slot()]
            .as_mut()
            .expect("作った主体がない")
            .then = Then::Drive {
            park_at,
            lot: Some(lot),
            to: dest,
        };
        self.parking.reserve(lot);
        let tally = &mut self.parking_tally;
        tally.drives = tally.drives.saturating_add(1);
        // 家に帰って借りた枠に停めるのは、行き先の駐車場に停められなかったのではない。
        let rented_home = matches!(lot, LotId::Rented(_)) && home == Some(to);
        if lot != LotId::Building(to) && !rented_home {
            tally.parked_elsewhere = tally.parked_elsewhere.saturating_add(1);
            self.lot_misses.add(to, now.0 / TICKS_PER_DAY, |m| {
                m.parked_elsewhere = m.parked_elsewhere.saturating_add(1);
            });
        }
        Some(id)
    }

    /// 車線に出る順番を待っている車を、待っている点ごとに数える。点の位置の順に返す。
    /// `long_ticks` 以上待っている車は `long` にも数える。
    /// 点を出入口か出入りの位置にする建物がない車は載せないので、台数の合計は待っている車の数以下になる。
    /// 読み取り用で、状態は変えず、状態ハッシュにも入れない。
    pub fn curb_waits(
        &self,
        net: &Network,
        buildings: &Buildings,
        long_ticks: u32,
    ) -> Vec<CurbWait> {
        let waiting: Vec<&Trip> = self
            .iter()
            .map(|(_, t)| t)
            .filter(|t| t.waiting_to_depart())
            .collect();
        if waiting.is_empty() {
            return Vec::new();
        }
        let mut spots: BTreeMap<WorldPos, BTreeSet<BuildingId>> = BTreeMap::new();
        for (id, b) in buildings.iter() {
            for p in [lot_spot(net, b, buildings.seed()), building_spot(net, b)]
                .into_iter()
                .flatten()
            {
                spots.entry(p.world()).or_default().insert(id);
            }
        }
        let mut by_point: BTreeMap<WorldPos, CurbWait> = BTreeMap::new();
        for t in waiting {
            let at = t.position().world();
            let Some(near) = spots.get(&at) else {
                continue;
            };
            let row = by_point.entry(at).or_insert_with(|| CurbWait {
                at,
                buildings: near.iter().copied().collect(),
                waiting: 0,
                long: 0,
            });
            row.waiting += 1;
            row.long += u32::from(t.curb_wait_ticks() >= long_ticks);
        }
        by_point.into_values().collect()
    }

    /// 建物が消えたあとに呼ぶ。表にない建物の駐車場を、停めてあった車ごと捨てる。
    pub fn forget_lots(&mut self, buildings: &Buildings) {
        self.parking
            .lots
            .retain(|id, _| buildings.get(*id).is_some());
        self.lot_misses.retain(|id, _| buildings.get(*id).is_some());
    }

    /// 主体を消す。押さえていた駐車場の空きは返す。
    pub(super) fn remove(&mut self, id: TripId) {
        if self.slots.release(id.index, id.generation) {
            let t = self.rows[id.slot()].take().expect("消す主体がない");
            if let Some(lot) = t.then.lot() {
                self.parking.release(lot);
            }
        }
    }

    /// 車線に出られないまま待ち続けた車 `rows` の車での移動をやめ、出発の建物から行き先まで歩く移動に
    /// 切り替える。押さえていた駐車場の空きは返す。歩く経路がなければ `ended` に足す。
    pub(super) fn walk_instead(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        rows: Vec<usize>,
        ended: &mut Vec<usize>,
    ) {
        for i in rows {
            let t = self.rows[i].as_mut().expect("待つ主体がない");
            let Then::Walk { lot, to } = t.then else {
                ended.push(i);
                continue;
            };
            t.then = Then::Arrive;
            // 車の位置は車の経路の最初のリンクの向きで表してある。建物と反対の向きの車線から出る車なら、
            // 建物の側の歩道のリンクで表し直す。
            let spot = t.position().resolve(net);
            let from = if t.walk.far {
                spot.and_then(|s| super::flip(net, s))
            } else {
                spot
            }
            .and_then(|(l, o)| SidewalkPos::on_link(net, l, o))
            .unwrap_or_else(|| t.position());
            let departed = t.departed;
            if let Some(lot) = lot {
                self.parking.release(lot);
            }
            let ends = from
                .resolve_on(net, LaneMode::Sidewalk)
                .zip(to.resolve_on(net, LaneMode::Sidewalk));
            let found =
                ends.and_then(|e| plan(router, net, LaneMode::Sidewalk, &[e], false).remove(0));
            match found {
                Some((route, e, jay)) => {
                    self.rows[i] = Some(make_trip(net, route, e, to, departed, jay));
                    self.rescues.walked_instead = self.rescues.walked_instead.saturating_add(1);
                }
                None => ended.push(i),
            }
        }
    }

    /// 区切りの終わりに着いた行 `done` を、次の区切りへ進める。移動を終える行と、次の区切りの経路が
    /// なくなっていた行を返す。
    pub(super) fn next_legs(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        done: Vec<usize>,
    ) -> Vec<usize> {
        let mut ended = Vec::new();
        let mut legs: [Vec<(usize, SidewalkPos, SidewalkPos, Then)>; 2] = Default::default();
        for i in done {
            let t = self.rows[i].as_mut().expect("着いた主体がない");
            match t.then {
                Then::Arrive => ended.push(i),
                Then::Drive { park_at, lot, to } => {
                    legs[0].push((i, t.dest, park_at, Then::Walk { lot, to }));
                }
                Then::Walk { lot, to } => {
                    t.then = Then::Arrive;
                    let dest = t.dest;
                    let (index, generation) = self.slots.current(i).expect("着いた主体がない");
                    let trip = TripId { index, generation };
                    match lot {
                        // 向かっていた駐車場が取り壊された。押さえた空きも一緒になくなっている。
                        None => {}
                        Some(lot) if self.commuters.dropped(trip) => {
                            // 走っているあいだに世帯が車をなくした。停めずに消し、押さえた空きを返す。
                            self.parking.release(lot);
                        }
                        Some(lot) => {
                            if self.parking.park(lot) {
                                self.commuters.parked(trip, lot);
                            }
                        }
                    }
                    legs[1].push((i, dest, to, Then::Arrive));
                }
            }
        }
        // 歩く区切りを先に作り、そのあと車を車線に置く。降りた車は、もう車線の上にいない。
        let [cars, walks] = legs;
        for (mode, legs) in [(LaneMode::Sidewalk, walks), (LaneMode::Car, cars)] {
            let resolved: Vec<Option<Ends>> = legs
                .iter()
                .map(|(_, a, b, _)| a.resolve_on(net, mode).zip(b.resolve_on(net, mode)))
                .collect();
            let ends: Vec<Ends> = resolved.iter().flatten().copied().collect();
            let mut routes = plan(router, net, mode, &ends, false).into_iter();
            // 置き直す行はまだ歩いているので、車線の上の車には入らない。
            let mut ix = (mode == LaneMode::Car).then(|| car::LaneIndex::of(&self.rows, net, None));
            let (zones, entering) = if mode == LaneMode::Car && !legs.is_empty() {
                (
                    super::crossing::jay_zones(net, &self.rows),
                    car::Entering::of(&self.rows, net, router.connectors()),
                )
            } else {
                Default::default()
            };
            for ((i, _, to, then), e) in legs.into_iter().zip(resolved) {
                let found = e.and_then(|_| routes.next().expect("探した経路の数が合わない"));
                let Some((route, e, jay)) = found else {
                    ended.push(i);
                    continue;
                };
                let old = self.rows[i].as_ref().expect("着いた主体がない");
                let departed = old.departed;
                // 歩道から車線へ、車線から歩道へ移るときも、それまで描いていたずれから少しずつ移す。
                // 車は道路のどちらの側の車線からも出るので、移る幅は道路の幅ほどになりうる。
                let shown = old.shift_on_leg(net, old.leg as usize);
                let mut t: Trip = make_trip(net, route, e, to, departed, jay);
                t.then = then;
                self.rows[i] = Some(t);
                // 出発と同じく、空いた車線に置き、前の車と重なるなら下げ、空きがなければ待たせる。
                if let Some(ix) = ix.as_mut() {
                    car::place(&mut self.rows, net, ix, i, &zones, &entering);
                }
                let t = self.rows[i].as_mut().expect("置いた主体がない");
                t.blend = Some(lane_shift::Blend {
                    from: shown,
                    start: t.offset.0,
                });
            }
        }
        ended
    }
}
