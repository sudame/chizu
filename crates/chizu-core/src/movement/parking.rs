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

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::collections::BTreeMap;

use super::{
    Ends, SidewalkPos, Trip, Trips, candidates, car, junction, lane_shift, make_trip, plan, pos,
};
use crate::building::{Building, Buildings, ParkingCut, SLOT_WIDTH_MM};
use crate::id::{BuildingId, TripId};
use crate::network::{Dir, LaneMode, LinkId, Network, TILE_MM};
use crate::routing::AstarRouter;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::{LaneOffset, Tick};

/// 満車のときに、ほかの駐車場を探す範囲（マス）。
pub const PARK_RADIUS: u64 = 16;

/// いまの区切りが終わったあとにすること。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Then {
    /// 移動を終える。
    Arrive,
    /// 車に乗って `park_at` まで走り、駐車場 `lot` に停めて `to` まで歩く。
    Drive {
        park_at: SidewalkPos,
        lot: BuildingId,
        to: SidewalkPos,
    },
    /// 車を駐車場 `lot` に停めて、`to` まで歩く。
    Walk { lot: BuildingId, to: SidewalkPos },
}

impl Then {
    /// 押さえている駐車場。
    pub(super) fn lot(&self) -> Option<BuildingId> {
        match self {
            Then::Arrive => None,
            Then::Drive { lot, .. } | Then::Walk { lot, .. } => Some(*lot),
        }
    }
}

impl StateHash for Then {
    fn write_state(&self, h: &mut StateHasher) {
        match self {
            Then::Arrive => h.write(&0_u8),
            Then::Drive { park_at, lot, to } => {
                h.write(&1_u8);
                h.write(park_at);
                h.write(lot);
                h.write(to);
            }
            Then::Walk { lot, to } => {
                h.write(&2_u8);
                h.write(lot);
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

/// 建物ごとの駐車場。どちらの台数も 0 の建物は持たない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Parking {
    lots: BTreeMap<BuildingId, Lot>,
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

    fn reserve(&mut self, b: BuildingId) {
        self.update(b, |l| {
            l.reserved = l.reserved.checked_add(1).expect("押さえた台数があふれた");
        });
    }

    /// 押さえた空きを返す。建物が消えて駐車場がなくなっていれば何もしない。
    fn release(&mut self, b: BuildingId) {
        if self.lots.contains_key(&b) {
            self.update(b, |l| {
                l.reserved = l
                    .reserved
                    .checked_sub(1)
                    .expect("押さえていない空きを返した");
            });
        }
    }

    /// 押さえた空きに車を停め、停めたら `true`。駐車場がなくなっていれば、車は停めずに消える。
    fn park(&mut self, b: BuildingId) -> bool {
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

    /// 停めてある車を 1 台出す。停めてある車がなければ `false`。
    pub(super) fn unpark(&mut self, b: BuildingId) -> bool {
        let had = self.get(b).parked > 0;
        if had {
            self.update(b, |l| l.parked -= 1);
        }
        had
    }

    fn is_free(&self, id: BuildingId, b: &Building) -> bool {
        let l = self.get(id);
        u64::from(l.parked) + u64::from(l.reserved) < b.parking_spaces()
    }

    /// 行き先 `to` の近くで空きのある駐車場を、試す順に返す。行き先そのものを先に、そのあとは
    /// [`PARK_RADIUS`] マスの中の建物を近い順に並べる。同じ距離なら番号の小さい建物を先にする。
    fn candidates(
        &self,
        buildings: &Buildings,
        to: BuildingId,
        dest: &Building,
    ) -> Vec<BuildingId> {
        let r = i64::try_from(PARK_RADIUS).expect("範囲が i64 に収まらない");
        let at = |dx: i64, dy: i64| {
            let c = |v: i64, d: i64| v.checked_add(d).expect("マスの座標があふれた");
            TilePos::new(c(dest.front.x, dx), c(dest.front.y, dy))
        };
        let area = TileRect::from_corners(at(-r, -r), at(r, r));
        let dist = |a: TilePos| a.x.abs_diff(dest.front.x).max(a.y.abs_diff(dest.front.y));
        let mut near: Vec<(u64, BuildingId)> = buildings
            .fronts_in(area)
            .filter(|id| *id != to)
            .filter_map(|id| Some((id, buildings.get(id)?)))
            .filter(|(id, b)| self.is_free(*id, b))
            .map(|(id, b)| (dist(b.front), id))
            .collect();
        near.sort_unstable();
        let first = self.is_free(to, dest).then_some(to);
        first
            .into_iter()
            .chain(near.into_iter().map(|(_, id)| id))
            .collect()
    }
}

impl StateHash for Parking {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.lots);
    }
}

/// 建物の出入口の位置。出入口の歩道の上でもあり、同じ側の車のリンクの上でもある。
/// 出入口のリンクがなければ `None`。
pub fn building_spot(net: &Network, b: &Building) -> Option<SidewalkPos> {
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
pub fn lot_spot(net: &Network, b: &Building) -> Option<SidewalkPos> {
    let door = building_spot(net, b)?;
    let half = TILE_MM / 2;
    // 端の外側の辺から、出入りの位置まで内へ入る距離（mm）。
    let inward = match b.parking_layout().cut {
        ParkingCut::None => return Some(door),
        ParkingCut::FrontStrip { slots } => slots
            .checked_mul(SLOT_WIDTH_MM)
            .and_then(|w| i64::try_from(w / 2).ok())
            .expect("帯の長さがあふれた"),
        ParkingCut::Side { .. } => half,
    };
    let (_, step, _) = pos::link_geometry(net, b.entrance)?;
    let (tx, ty) = b.toward_road;
    let along = (-ty, tx);
    // `front` から間口のもう一方の端のマスまでのマスの数。
    let span = if tx == 0 {
        b.area.max.x.abs_diff(b.area.min.x)
    } else {
        b.area.max.y.abs_diff(b.area.min.y)
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
        (b.front, (-along.0, -along.1)),
        (at(b.front, along, far), along),
    ];
    // 出入りの位置の、端のマスの真ん中からのずれ（mm）。帯の真ん中は、マスの真ん中より内に入りうる。
    let shift = half - inward;
    ends.into_iter()
        .filter_map(|(tile, out)| {
            let road = at(tile, (tx, ty), 1);
            let sign = out.0 * i64::from(step.0) + out.1 * i64::from(step.1);
            let p = [sign * shift, 0]
                .into_iter()
                .flat_map(|d| spots_near(road, step, d))
                .find(|p| {
                    p.resolve_on(net, LaneMode::Sidewalk).is_some()
                        && p.resolve_on(net, LaneMode::Car).is_some()
                })?;
            let (link, off) = p.resolve_on(net, LaneMode::Car)?;
            let w = p.world();
            Some(((junction_clearance(net, link, off), w.x, w.y), p))
        })
        .max_by_key(|(k, _)| *k)
        .map_or(Some(door), |(_, p)| Some(p))
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
    let Some((seg, (_, _, len))) = net
        .segment(link.segment)
        .zip(pos::link_geometry(net, link))
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
    pub fn parking_lot(&self) -> Option<BuildingId> {
        self.then.lot()
    }
}

impl Trips {
    /// 車で向かおうとした移動の、駐車場の決まり方の数。
    pub fn parking_tally(&self) -> ParkingTally {
        self.parking_tally
    }

    /// 建物 `b` の駐車場の台数。
    pub fn lot(&self, b: BuildingId) -> Lot {
        self.parking.get(b)
    }

    /// 駐車場を建物の番号の順に返す。
    pub fn lots(&self) -> impl Iterator<Item = (BuildingId, Lot)> + '_ {
        self.parking.lots.iter().map(|(id, l)| (*id, *l))
    }

    /// 建物 `from` から建物 `to` へ、歩く → 車 → 歩く で向かう移動を作る。行き先の駐車場の空きは
    /// このときに押さえ、`from` に停めてある車があれば 1 台出す。建物がないか、行き先の近くがどこも満車か、
    /// 3 つの区切りのどれかの経路がなければ `None` で、駐車場も変えない。
    pub fn spawn_drive(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        buildings: &Buildings,
        (from, to): (BuildingId, BuildingId),
        now: Tick,
    ) -> Option<TripId> {
        self.spawn_drive_taking(net, router, buildings, (from, to), Some(from), now)
    }

    /// [`Trips::spawn_drive`] と同じだが、停めてある車を出す駐車場を `take` で決める。`None` なら
    /// どの駐車場からも出さない。
    pub(super) fn spawn_drive_taking(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        buildings: &Buildings,
        (from, to): (BuildingId, BuildingId),
        take: Option<BuildingId>,
        now: Tick,
    ) -> Option<TripId> {
        let from_b = buildings.get(from)?;
        let door = building_spot(net, from_b)?;
        let origin = lot_spot(net, from_b)?;
        let dest = building_spot(net, buildings.get(to)?)?;
        let took = take.filter(|b| self.parking.unpark(*b));
        let id = self.start_drive(net, router, buildings, (door, origin), (to, dest), now);
        if let (None, Some(b)) = (id, took) {
            self.parking.update(b, |l| {
                l.parked = l.parked.checked_add(1).expect("停めた台数があふれた");
            });
        }
        id
    }

    /// 出入口 `door` から駐車場の出入りの位置 `origin` まで歩き、そこから建物 `to`（出入口 `dest`）へ
    /// 車で向かう移動を作る。
    fn start_drive(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        buildings: &Buildings,
        (door, origin): (SidewalkPos, SidewalkPos),
        (to, dest): (BuildingId, SidewalkPos),
        now: Tick,
    ) -> Option<TripId> {
        router.sync(net);
        // 近い順に試し、車でも歩いても行ける最初の駐車場にする。
        let candidates = self.parking.candidates(buildings, to, buildings.get(to)?);
        if candidates.is_empty() {
            self.parking_tally.lots_full = self.parking_tally.lots_full.saturating_add(1);
            return None;
        }
        let (lot, park_at) = candidates
            .into_iter()
            .filter_map(|lot| Some((lot, lot_spot(net, buildings.get(lot)?)?)))
            .find(|(_, at)| {
                reachable(net, router, LaneMode::Car, origin, *at)
                    && reachable(net, router, LaneMode::Sidewalk, *at, dest)
            })?;
        let id = self.spawn_many(net, router, &[(door, origin)], now)[0]?;
        self.rows[id.slot()]
            .as_mut()
            .expect("作った主体がない")
            .then = Then::Drive {
            park_at,
            lot,
            to: dest,
        };
        self.parking.reserve(lot);
        let tally = &mut self.parking_tally;
        tally.drives = tally.drives.saturating_add(1);
        if lot != to {
            tally.parked_elsewhere = tally.parked_elsewhere.saturating_add(1);
        }
        Some(id)
    }

    /// 建物が消えたあとに呼ぶ。表にない建物の駐車場を、停めてあった車ごと捨てる。
    pub fn forget_lots(&mut self, buildings: &Buildings) {
        self.parking
            .lots
            .retain(|id, _| buildings.get(*id).is_some());
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
            self.parking.release(lot);
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
                    if self.parking.park(lot) {
                        let (index, generation) = self.slots.current(i).expect("着いた主体がない");
                        self.commuters.parked(TripId { index, generation }, lot);
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
            let mut ix = (mode == LaneMode::Car).then(|| car::LaneIndex::of(&self.rows, None));
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
