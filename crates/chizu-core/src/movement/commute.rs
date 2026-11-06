//! 住民の通勤の移動。
//!
//! 出発の時刻が来た住民ごとに、出発の建物の出入口から行き先の建物の出入口へ向かう移動を作る。
//! 手段は、2 つの建物の正面のマスの直線の距離で決める。車を持つ世帯の住民は [`OWNER_DRIVE_MIN_MM`]、
//! 持たない世帯の住民は [`DRIVE_MIN_MM`] 以上なら歩く → 車 → 歩く で向かい、
//! それより近いか、車で行けなければ歩く。車を置く枠（[`CarPlace`]）のない世帯は、遠くても歩く。
//! 移動の主体ごとに、誰がどこへ向かっているかを覚えておき、移動が終わったら返す。
//!
//! 車は世帯ごとに 1 台で、世帯の中で同時に車を使えるのは 1 人だけである。世帯の車のいる所
//! （[`HouseholdCar`]）を覚え、次に車で出るときはそこから車を出す。車を使えるのは、車が家に停めてあって
//! 家から出るときか、自分で停めた車が出発の建物から [`super::PARK_RADIUS`] マスの中にあるとき（満車で近くの
//! 別の建物に停めたときも含む）だけである。出発の建物の駐車場でない所から出るときは、そこまで歩いて乗る。
//! 家に帰る住民は、自分が家でない所に停めた車が近くにあれば、近くても車で帰る。近くになければ歩いて帰り、
//! 翌朝は歩いて職場へ向かって、夕方に職場から車で帰る。
//! 職場が変わるかなくなった住民が家でない所に停めた車は、どこにもないものに戻す（[`Trips::return_car`]）。
//! 世帯の枠に停めた車は、家に帰る住民が家でない所から取りに行かない。
//! ほかの住民が乗っているときや、離れた所に停めてあるときは、車がないものとして歩く。
//! 車を持つ世帯の車は、枠を得たときから枠に停まっている（[`Trips::place_car`]）。走っているあいだに車をなくした世帯と、
//! 車を持たない世帯の車はどこにもなく、次に車で出るときに出発の建物から現れる。停めてある台数は世帯の数を超えて増えない。
//! 共同の駐車場の枠を借りた世帯（[`CarPlace::Shared`]）の車は、家にいるあいだ借りた枠に停まり、
//! 家と借りた枠のあいだは歩く。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::parking::{lot_exists, lot_near};
use super::{LotId, Trips, building_spot};
use crate::building::Buildings;
use crate::car_place::CarPlace;
use crate::citizens::Activity;
use crate::id::{BuildingId, CitizenId, HouseholdId, TripId};
use crate::network::{Network, TILE_MM};
use crate::rail::RidePlan;
use crate::rail::choice::{
    RideChoice, RideOffer, WalkEnd, car_seconds, tile_distance_mm, walk_seconds,
};
use crate::routing::AstarRouter;
use crate::service::Facility;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::TilePos;
use crate::units::Tick;

/// 車を持たない世帯の住民が車で向かう、建物どうしの直線の距離の下限（mm）。500m である。
pub const DRIVE_MIN_MM: i64 = 500_000;

/// 車を持つ世帯の住民が車で向かう、建物どうしの直線の距離の下限（mm）。150m である。
/// これより近ければ歩いて行ける。
pub const OWNER_DRIVE_MIN_MM: i64 = 150_000;

/// 移動している住民と、その世帯と行き先。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commuter {
    pub citizen: CitizenId,
    pub household: HouseholdId,
    pub to: Activity,
}

impl StateHash for Commuter {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.citizen);
        h.write(&self.household);
        h.write(&self.to);
    }
}

/// 世帯の車のいる所。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HouseholdCar {
    /// 駐車場 `lot` に停めてある。`by` は停めた住民である。
    Parked { lot: LotId, by: CitizenId },
    /// 移動 `0` が使っている。
    InUse(TripId),
}

impl StateHash for HouseholdCar {
    fn write_state(&self, h: &mut StateHasher) {
        match self {
            HouseholdCar::Parked { lot, by } => {
                h.write(&0_u8);
                h.write(lot);
                h.write(by);
            }
            HouseholdCar::InUse(trip) => {
                h.write(&1_u8);
                h.write(trip);
            }
        }
    }
}

/// 移動の主体ごとの、移動している住民と、世帯ごとの車のいる所。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Commuters {
    by_trip: BTreeMap<TripId, Commuter>,
    /// 世帯の車のいる所。どこにもない世帯は持たない。
    cars: BTreeMap<HouseholdId, HouseholdCar>,
}

impl Commuters {
    /// 移動 `trip` の車を駐車場 `lot` に停めた。通勤でないか、世帯の車でなければ何もせず `false`。
    pub(super) fn parked(&mut self, trip: TripId, lot: LotId) -> bool {
        let Some(c) = self.by_trip.get(&trip) else {
            return false;
        };
        let Some(car) = self.cars.get_mut(&c.household) else {
            return false;
        };
        if *car != HouseholdCar::InUse(trip) {
            return false;
        }
        *car = HouseholdCar::Parked { lot, by: c.citizen };
        true
    }

    /// 移動 `trip` が世帯の車で向かう通勤で、走っているあいだにその車をなくしたか（[`Trips::forget_car`]）。
    /// なくした車は、着いても駐車場に停めない。
    pub(super) fn dropped(&self, trip: TripId) -> bool {
        self.by_trip
            .get(&trip)
            .is_some_and(|c| self.cars.get(&c.household) != Some(&HouseholdCar::InUse(trip)))
    }

    /// 共同の駐車場 `corner` に停めてある世帯の車を、どこにもないものにする。
    pub(super) fn forget_cars_at(&mut self, corner: TilePos) {
        self.cars.retain(|_, car| match car {
            HouseholdCar::Parked { lot, .. } => lot.shared_corner() != Some(corner),
            HouseholdCar::InUse(_) => true,
        });
    }

    /// 移動 `trip` を覚えていた分を消す。世帯の車を使ったまま停めずに終わったなら、車はどこにもないものとする。
    fn forget_trip(&mut self, trip: TripId) -> Option<Commuter> {
        let c = self.by_trip.remove(&trip)?;
        if self.cars.get(&c.household) == Some(&HouseholdCar::InUse(trip)) {
            self.cars.remove(&c.household);
        }
        Some(c)
    }
}

impl StateHash for Commuters {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.by_trip);
        h.write(&self.cars);
    }
}

/// 通勤の 1 件の依頼。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommuteRequest {
    pub who: Commuter,
    pub from: BuildingId,
    pub to: BuildingId,
    /// 住民の家。
    pub home: Option<BuildingId>,
    /// 住民の世帯の、車を置く枠。枠のない世帯は車を持たず、どの移動も歩く。
    pub place: Option<CarPlace>,
    /// 住民の世帯が車を持つか。
    pub owns_car: bool,
}

impl CommuteRequest {
    /// 世帯の枠のある駐車場。住宅の駐車場は、その建物の自宅の枠を持つ世帯の車だけが使える。
    fn own_lot(&self) -> Option<LotId> {
        CarPlace::lot(self.place, self.home)
    }
}

/// 建物 `a` と `b` の正面のマスが、車で向かうほど離れているか。
fn far_enough(buildings: &Buildings, a: BuildingId, b: BuildingId, owns_car: bool) -> bool {
    let (Some(a), Some(b)) = (buildings.get(a), buildings.get(b)) else {
        return false;
    };
    let sq = |p: i64, q: i64| {
        let d = i128::from(p.checked_sub(q)?).checked_mul(i128::from(TILE_MM))?;
        d.checked_mul(d)
    };
    let min = i128::from(if owns_car {
        OWNER_DRIVE_MIN_MM
    } else {
        DRIVE_MIN_MM
    });
    let dist = sq(a.front.x, b.front.x)
        .zip(sq(a.front.y, b.front.y))
        .and_then(|(x, y)| x.checked_add(y))
        .expect("距離があふれた");
    dist >= min.checked_mul(min).expect("距離があふれた")
}

/// 建物 `a` と `b` の正面のマスの直線の距離（mm）。
fn front_distance(
    buildings: &Buildings,
    a: BuildingId,
    b: BuildingId,
) -> Option<(TilePos, TilePos, u64)> {
    let (a, b) = (buildings.get(a)?.front, buildings.get(b)?.front);
    Some((a, b, tile_distance_mm(a, b)))
}

/// 依頼ごとに選んだ手段。
enum Pick {
    /// 車で走り始めた。
    Drive(TripId),
    /// 歩く。
    Walk,
    /// 駅まで歩いて鉄道に乗る。
    Ride(RideOffer),
}

impl Trips {
    /// 通勤の移動をまとめて作り、依頼の順に ID を返す。車で向かう依頼から 1 件ずつ作り、
    /// 車で行けなかった依頼と近い依頼は、歩く移動としてまとめて作る。どちらでも行けなければ `None`。
    /// 同じ世帯の依頼が並んだら、先の依頼が世帯の車を使う。
    pub fn start_commutes(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        sites: (&Buildings, &[Facility]),
        requests: &[CommuteRequest],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        self.start_commutes_with_rides(net, router, sites, requests, now, None)
    }

    /// [`Trips::start_commutes`] と同じだが、`rides` があれば鉄道の候補と費用で比べる（設計 3.10 の 4）。
    ///
    /// 歩きと車は今の距離の決まりで選び、そのうえで鉄道の費用が小さければ鉄道にする。車を選んだ移動は車の費用と、
    /// 歩きを選んだ移動は歩きの費用と比べる。費用が同じなら今の手段を残す。[`OWNER_DRIVE_MIN_MM`] より近い移動は、
    /// 車を持つかによらず鉄道と比べない。家の枠でない駐車場に停めた車で出る移動（停めた車を取りに帰る移動を含む）も、
    /// 車を残さないよう鉄道と比べない。車を使えず鉄道を選んだ移動は、車がない移動（`no_car`）に数えない。
    /// 鉄道を選んだ移動は、乗る駅の出入口まで歩く移動として作り、着いたら駅の待ち行列に入る。
    pub fn start_commutes_with_rides(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        (buildings, facilities): (&Buildings, &[Facility]),
        requests: &[CommuteRequest],
        now: Tick,
        rides: Option<&RideChoice>,
    ) -> Vec<Option<TripId>> {
        let rides = rides.filter(|c| !c.is_empty());
        let picks: Vec<Pick> = requests
            .iter()
            .map(|r| {
                // 鉄道の候補。費用 `base` より小さいときだけ返す。
                let ride = |base: fn(u64) -> u64| -> Option<RideOffer> {
                    let c = rides?;
                    let (a, b, d) = front_distance(buildings, r.from, r.to)?;
                    if d < OWNER_DRIVE_MIN_MM.unsigned_abs() {
                        return None;
                    }
                    // 駅まで歩く時間は、建物の出入口から歩道を歩く時間で見積もる。歩く移動の出発と行き先と同じ位置である。
                    // 出入口が建物の入口なら、入口のリンクから直接求め、マスから歩道を探し直さない。
                    let end = |id: BuildingId, tile: TilePos| -> Option<WalkEnd> {
                        let b = buildings.get(id)?;
                        Some(if b.lane_plan().is_some() {
                            WalkEnd::new(net, tile, building_spot(net, b)?)
                        } else {
                            WalkEnd::on_link(net, tile, b.entrance, b.entrance_at)
                        })
                    };
                    c.offer(&end(r.from, a)?, &end(r.to, b)?)
                        .filter(|o| o.cost < base(d))
                };
                let walk = || ride(walk_seconds).map_or(Pick::Walk, Pick::Ride);
                // 家に帰る住民は、自分が家でない所に停めた車が近くにあれば、近くても車で帰る。歩いて帰ると、
                // 車がその駐車場に残り、翌朝からも使えないためである。
                let fetch = r.who.to == Activity::Home
                    && matches!(
                        self.commuters.cars.get(&r.who.household),
                        Some(HouseholdCar::Parked { lot, by })
                            if *by == r.who.citizen
                                && Some(*lot) != r.own_lot()
                                && lot_exists(*lot, buildings, facilities)
                                && lot_near(*lot, r.from, buildings, facilities)
                    );
                if !fetch && !far_enough(buildings, r.from, r.to, r.owns_car) {
                    return walk();
                }
                if r.place.is_none() {
                    let tally = &mut self.parking_tally;
                    tally.no_place = tally.no_place.saturating_add(1);
                    // 枠をなくす前に乗っていて、そのあと停めた車が残っていれば外す。
                    self.forget_car(r.who.household);
                    return walk();
                }
                // 車が停めてあれば、その駐車場から出す。まだどこにもなければ、車は出発の建物から現れる。
                let take = match self.commuters.cars.get(&r.who.household).copied() {
                    None => None,
                    Some(HouseholdCar::Parked { lot, .. })
                        if !lot_exists(lot, buildings, facilities) =>
                    {
                        None
                    }
                    // 世帯の枠に停めた車は、家に帰る住民が取りに行かない。枠は家の近くにあり、出発の建物の
                    // 近くに見えても、そこから乗って 0m だけ走ることになる。
                    Some(HouseholdCar::Parked { lot, by })
                        if Some(lot) == r.own_lot() && r.home == Some(r.from)
                            || by == r.who.citizen
                                && !(Some(lot) == r.own_lot() && r.who.to == Activity::Home)
                                && lot_near(lot, r.from, buildings, facilities) =>
                    {
                        Some(lot)
                    }
                    Some(car) => {
                        let pick = walk();
                        // 鉄道を選んだ移動は、車がなくて困った移動に数えない。鉄道で通う人の車は家の枠に残るので、
                        // 帰りはいつもここに来る。
                        if matches!(pick, Pick::Ride(_)) {
                            return pick;
                        }
                        let tally = &mut self.parking_tally;
                        tally.no_car = tally.no_car.saturating_add(1);
                        // `no_car` の内訳。どれか 1 つだけを増やし、和が `no_car` に等しくなるようにする。
                        let reason = match car {
                            HouseholdCar::InUse(_) => &mut tally.no_car_in_use,
                            HouseholdCar::Parked { .. } if r.home.is_none() => {
                                &mut tally.no_car_other
                            }
                            HouseholdCar::Parked { lot, .. } if Some(lot) == r.own_lot() => {
                                &mut tally.no_car_at_home
                            }
                            HouseholdCar::Parked { .. } => &mut tally.no_car_parked_away,
                        };
                        *reason = reason.saturating_add(1);
                        return pick;
                    }
                };
                // 家の枠でない駐車場に停めた車で出る移動は、車をそこに残さないよう鉄道と比べない。停めた車を
                // 取りに帰る移動（`fetch`）も、職場から店へ向かう移動もここに入る。
                let leaves_car = take.is_some_and(|lot| Some(lot) != r.own_lot());
                if !leaves_car && let Some(o) = ride(car_seconds) {
                    return Pick::Ride(o);
                }
                let Some(id) = self.spawn_drive_taking(
                    net,
                    router,
                    (buildings, facilities),
                    (r.from, r.to),
                    take,
                    (r.own_lot(), r.home),
                    now,
                ) else {
                    return walk();
                };
                self.commuters
                    .cars
                    .insert(r.who.household, HouseholdCar::InUse(id));
                Pick::Drive(id)
            })
            .collect();
        let mut out: Vec<Option<TripId>> = picks
            .iter()
            .map(|p| match p {
                Pick::Drive(id) => Some(*id),
                Pick::Walk | Pick::Ride(_) => None,
            })
            .collect();
        let spot = |b: BuildingId| building_spot(net, buildings.get(b)?);
        // 歩く移動と、駅まで歩く移動を、依頼の順にまとめて作る。
        let walks: Vec<(usize, _)> = picks
            .iter()
            .zip(requests)
            .enumerate()
            .filter_map(|(i, (p, r))| {
                let to = match p {
                    Pick::Drive(_) => return None,
                    Pick::Walk => spot(r.to)?,
                    Pick::Ride(o) => {
                        spot(r.to)?;
                        o.door
                    }
                };
                Some((i, (spot(r.from)?, to)))
            })
            .collect();
        let ends: Vec<_> = walks.iter().map(|(_, e)| *e).collect();
        let mut retry = Vec::new();
        for ((i, _), id) in walks.iter().zip(self.spawn_many(net, router, &ends, now)) {
            let r = &requests[*i];
            match (&picks[*i], id) {
                (Pick::Ride(o), Some(id)) => {
                    let to = spot(r.to).expect("行き先の出入口がない");
                    self.rows[id.slot()]
                        .as_mut()
                        .expect("作った主体がない")
                        .then = super::parking::Then::Ride(RidePlan {
                        board: o.board,
                        line: o.line,
                        dir: o.dir,
                        alight: o.alight,
                        transfer: o.transfer,
                        to,
                        walk_estimate: u32::try_from(o.walk_to_board).unwrap_or(u32::MAX),
                    });
                    out[*i] = Some(id);
                }
                // 駅まで歩けなければ、行き先へ歩く。
                (Pick::Ride(_), None) => retry.push(*i),
                (_, id) => out[*i] = id,
            }
        }
        if !retry.is_empty() {
            let ends: Vec<_> = retry
                .iter()
                .filter_map(|i| Some((*i, (spot(requests[*i].from)?, spot(requests[*i].to)?))))
                .collect();
            let pairs: Vec<_> = ends.iter().map(|(_, e)| *e).collect();
            for ((i, _), id) in ends.iter().zip(self.spawn_many(net, router, &pairs, now)) {
                out[*i] = id;
            }
        }
        for (id, r) in out.iter().zip(requests) {
            if let Some(id) = id {
                self.commuters.by_trip.insert(*id, r.who);
            }
        }
        out
    }

    /// 移動が終わった主体 `id` の住民を返し、覚えていた分を消す。通勤でなければ `None`。
    pub fn take_commuter(&mut self, id: TripId) -> Option<Commuter> {
        self.commuters.forget_trip(id)
    }

    /// 移動している住民。
    pub fn commuter(&self, id: TripId) -> Option<Commuter> {
        self.commuters.by_trip.get(&id).copied()
    }

    /// 移動 `trip` が世帯の車で向かう通勤で、走っているあいだにその車をなくしたか（[`Trips::forget_car`]）。
    /// なくした車は、着いても駐車場に停めず、押さえた空きを返す。
    pub fn car_dropped(&self, trip: TripId) -> bool {
        self.commuters.dropped(trip)
    }

    /// 世帯の車をなくしたまま、車で走っている移動のある世帯。その車は着いても停めずに消えるので、
    /// 走っているあいだに新しい車を置くと、同じ世帯の車が 2 台に見える。
    pub fn households_with_dropped_drive(&self) -> BTreeSet<HouseholdId> {
        self.iter()
            .filter(|(id, t)| t.parking_lot().is_some() && self.commuters.dropped(*id))
            .filter_map(|(id, _)| Some(self.commuters.by_trip.get(&id)?.household))
            .collect()
    }

    /// 世帯の車のいる所。
    pub fn household_car(&self, household: HouseholdId) -> Option<HouseholdCar> {
        self.commuters.cars.get(&household).copied()
    }

    /// 街を出る世帯や、車を置く枠をなくした世帯の車をなくす。停めてある車は駐車場から外す。
    /// 乗っている車は、その移動が着いても駐車場に停めずに消える（[`Commuters::dropped`]）。
    pub fn forget_car(&mut self, household: HouseholdId) {
        if let Some(HouseholdCar::Parked { lot, .. }) = self.commuters.cars.remove(&household) {
            self.parking.unpark(lot);
        }
    }

    /// 車がまだどこにもない世帯 `household` の車を、世帯の枠のある駐車場 `lot` に停めた状態にする。
    /// 停めた住民は `by` とする。枠を得た世帯の車が、初めて車で出る前から枠に停まっているようにする。
    pub fn place_car(&mut self, household: HouseholdId, by: CitizenId, lot: LotId) {
        assert!(
            !self.commuters.cars.contains_key(&household),
            "車のある世帯に車を置いた"
        );
        self.parking.put_back(lot);
        self.commuters
            .cars
            .insert(household, HouseholdCar::Parked { lot, by });
    }

    /// 住民 `citizen` が世帯の枠（`own`。自宅か借りた枠）でない所に停めた世帯の車を、どこにもないものに戻す。
    /// 職場が変わったりなくなったりした住民の車が、前の職場に停めたまま残らないようにする。次に車で出る住民の
    /// 出発の建物から車が現れる。
    pub fn return_car(&mut self, household: HouseholdId, citizen: CitizenId, own: Option<LotId>) {
        if let Some(HouseholdCar::Parked { lot, by }) = self.commuters.cars.get(&household).copied()
            && by == citizen
            && Some(lot) != own
        {
            self.commuters.cars.remove(&household);
            self.parking.unpark(lot);
        }
    }

    /// 住民 `citizen` が家に着いたとき、自分が家でない所に停めた世帯の車が、家からも職場からも
    /// [`super::PARK_RADIUS`] マスより離れて置き去りになっていれば、車をどこにもないものにして `true` を返す。
    /// `own` は世帯の枠のある駐車場、`bases` は家と職場である。近くに残る車は、翌朝か夕方に使えるので外さない。
    pub fn return_stranded_car(
        &mut self,
        household: HouseholdId,
        citizen: CitizenId,
        own: Option<LotId>,
        (buildings, facilities): (&Buildings, &[Facility]),
        bases: [Option<BuildingId>; 2],
    ) -> bool {
        let Some(HouseholdCar::Parked { lot, by }) = self.commuters.cars.get(&household).copied()
        else {
            return false;
        };
        let near = bases
            .into_iter()
            .flatten()
            .any(|b| lot_near(lot, b, buildings, facilities));
        if by != citizen || Some(lot) == own || near || !lot_exists(lot, buildings, facilities) {
            return false;
        }
        self.commuters.cars.remove(&household);
        self.parking.unpark(lot);
        true
    }

    /// 移動を途中でやめさせる。押さえていた駐車場の空きは返す。
    pub fn cancel(&mut self, id: TripId) {
        self.commuters.forget_trip(id);
        self.remove(id);
    }
}
