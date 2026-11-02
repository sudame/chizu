//! 住民の通勤の移動。
//!
//! 出発の時刻が来た住民ごとに、出発の建物の出入口から行き先の建物の出入口へ向かう移動を作る。
//! 手段は、2 つの建物の正面のマスの直線の距離で決める。車を持つ世帯の住民は [`OWNER_DRIVE_MIN_MM`]、
//! 持たない世帯の住民は [`DRIVE_MIN_MM`] 以上なら歩く → 車 → 歩く で向かい、
//! それより近いか、車で行けなければ歩く。
//! 移動の主体ごとに、誰がどこへ向かっているかを覚えておき、移動が終わったら返す。
//! 住民ごとに自分の車を停めた駐車場も覚え、次に車で出るときはそこから車を出す。満車で別の建物に
//! 停めた車も、持ち主が乗って出るので、停めてある台数は住民の数を超えて増えない。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{Trips, building_spot};
use crate::building::Buildings;
use crate::citizens::Activity;
use crate::id::{BuildingId, CitizenId, TripId};
use crate::network::{Network, TILE_MM};
use crate::routing::AstarRouter;
use crate::state_hash::{StateHash, StateHasher};
use crate::units::Tick;

/// 車を持たない世帯の住民が車で向かう、建物どうしの直線の距離の下限（mm）。500m である。
pub const DRIVE_MIN_MM: i64 = 500_000;

/// 車を持つ世帯の住民が車で向かう、建物どうしの直線の距離の下限（mm）。150m である。
/// これより近ければ歩いて行ける。
pub const OWNER_DRIVE_MIN_MM: i64 = 150_000;

/// 移動している住民と、その行き先。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commuter {
    pub citizen: CitizenId,
    pub to: Activity,
}

impl StateHash for Commuter {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.citizen);
        h.write(&self.to);
    }
}

/// 移動の主体ごとの、移動している住民と、住民ごとの車を停めた駐車場。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Commuters {
    by_trip: BTreeMap<TripId, Commuter>,
    /// 住民が自分の車を停めた駐車場。次に車で出るときは、ここから車を出す。
    cars: BTreeMap<CitizenId, BuildingId>,
}

impl Commuters {
    /// 移動 `trip` の車を駐車場 `lot` に停めた。通勤でなければ何もしない。
    pub(super) fn parked(&mut self, trip: TripId, lot: BuildingId) {
        if let Some(c) = self.by_trip.get(&trip) {
            self.cars.insert(c.citizen, lot);
        }
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
    /// 住民の世帯が車を持つか。
    pub owns_car: bool,
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

impl Trips {
    /// 通勤の移動をまとめて作り、依頼の順に ID を返す。車で向かう依頼から 1 件ずつ作り、
    /// 車で行けなかった依頼と近い依頼は、歩く移動としてまとめて作る。どちらでも行けなければ `None`。
    pub fn start_commutes(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        buildings: &Buildings,
        requests: &[CommuteRequest],
        now: Tick,
    ) -> Vec<Option<TripId>> {
        let mut out: Vec<Option<TripId>> = requests
            .iter()
            .map(|r| {
                if !far_enough(buildings, r.from, r.to, r.owns_car) {
                    return None;
                }
                // 住民の車が停めてあれば、その駐車場から出す。なければ車は出発の建物から現れる。
                let take = self.commuters.cars.get(&r.who.citizen).copied();
                let id =
                    self.spawn_drive_taking(net, router, buildings, (r.from, r.to), take, now)?;
                self.commuters.cars.remove(&r.who.citizen);
                Some(id)
            })
            .collect();
        let spot = |b: BuildingId| building_spot(net, buildings.get(b)?);
        let walks: Vec<(usize, _)> = out
            .iter()
            .zip(requests)
            .enumerate()
            .filter(|(_, (t, _))| t.is_none())
            .filter_map(|(i, (_, r))| Some((i, (spot(r.from)?, spot(r.to)?))))
            .collect();
        let ends: Vec<_> = walks.iter().map(|(_, e)| *e).collect();
        for ((i, _), id) in walks.iter().zip(self.spawn_many(net, router, &ends, now)) {
            out[*i] = id;
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
        self.commuters.by_trip.remove(&id)
    }

    /// 移動している住民。
    pub fn commuter(&self, id: TripId) -> Option<Commuter> {
        self.commuters.by_trip.get(&id).copied()
    }

    /// 住民が車を停めた駐車場。
    pub fn parked_car(&self, citizen: CitizenId) -> Option<BuildingId> {
        self.commuters.cars.get(&citizen).copied()
    }

    /// 街を出る住民の、停めてある車を駐車場から外す。
    pub fn forget_car(&mut self, citizen: CitizenId) {
        if let Some(lot) = self.commuters.cars.remove(&citizen) {
            self.parking.unpark(lot);
        }
    }

    /// 移動を途中でやめさせる。押さえていた駐車場の空きは返す。
    pub fn cancel(&mut self, id: TripId) {
        self.commuters.by_trip.remove(&id);
        self.remove(id);
    }
}
