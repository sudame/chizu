//! 建物の駐車場と、建物から建物への車の移動（歩く → 車 → 歩く）。
//!
//! 車は出発の建物の前の道路から走り出し、行き先の建物の駐車場へ向かう。満車なら、行き先から
//! [`PARK_RADIUS`] マスの中で空きのある、いちばん近い建物の駐車場へ向かい、そこから行き先まで歩く。
//! どこも満車なら、行き先の前の道路まで走って、そこで車は消える（路上に停めたとみなし、台数は数えない）。
//! 駐車場の空きは出発のときに押さえる。出発の建物に停めてある車があれば、1 台ぶん空く。

use std::collections::BTreeMap;

use super::{SidewalkPos, Trips, pos};
use crate::building::{Building, Buildings};
use crate::id::{BuildingId, TripId};
use crate::network::{LaneMode, Network};
use crate::routing::AstarRouter;
use crate::state_hash::{StateHash, StateHasher};
use crate::units::{LaneOffset, Tick};

/// 満車のときに、ほかの駐車場を探す範囲（マス）。
pub const PARK_RADIUS: u64 = 16;

/// 建物ごとの、停めてある車の数。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Parking {
    used: BTreeMap<BuildingId, u32>,
}

impl Parking {
    pub(super) fn used(&self, b: BuildingId) -> u32 {
        self.used.get(&b).copied().unwrap_or(0)
    }

    fn take(&mut self, b: BuildingId) {
        let n = self.used.entry(b).or_insert(0);
        *n = n.checked_add(1).expect("停めた台数があふれた");
    }

    fn leave(&mut self, b: BuildingId) {
        if let Some(n) = self.used.get_mut(&b) {
            *n -= 1;
            if *n == 0 {
                self.used.remove(&b);
            }
        }
    }

    /// 行き先 `to` の近くで空きのある駐車場。行き先そのものを先に見る。
    fn find(&self, buildings: &Buildings, to: BuildingId, dest: &Building) -> Option<BuildingId> {
        let free = |id: BuildingId, b: &Building| self.used(id) < b.parking_spaces();
        if free(to, dest) {
            return Some(to);
        }
        buildings
            .iter()
            .filter(|(id, b)| free(*id, b))
            .map(|(id, b)| {
                let d = b.front.x.abs_diff(dest.front.x).max(b.front.y.abs_diff(dest.front.y));
                (d, id)
            })
            .filter(|(d, _)| *d <= PARK_RADIUS)
            .min()
            .map(|(_, id)| id)
    }
}

impl StateHash for Parking {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.used);
    }
}

/// 建物の前の、出入口の歩道の上の位置。車の位置も同じ表し方で、同じ側の車のリンクの上になる。
pub fn building_spot(net: &Network, b: &Building) -> Option<SidewalkPos> {
    let (start, step, _) = pos::link_geometry(net, b.entrance)?;
    let road = crate::tile::TilePos::new(
        b.front.x.checked_add(b.toward_road.0)?,
        b.front.y.checked_add(b.toward_road.1)?,
    );
    let k = road.x.abs_diff(start.x).max(road.y.abs_diff(start.y));
    let per = pos::step_mm(step);
    let offset = u32::try_from(k).ok()?.checked_mul(per)?;
    SidewalkPos::on_any_link(net, b.entrance, LaneOffset(offset))
}

impl Trips {
    /// 停めてある車の数。
    pub fn parked(&self, b: BuildingId) -> u32 {
        self.parking.used(b)
    }

    /// 建物 `from` から建物 `to` へ車で向かう移動を作る。駐車場の空きはこのときに押さえる。
    /// 建物がないか、前の道路がつながっていなければ `None`。
    pub fn spawn_drive_between(
        &mut self,
        net: &Network,
        router: &mut AstarRouter,
        buildings: &Buildings,
        from: BuildingId,
        to: BuildingId,
        now: Tick,
    ) -> Option<TripId> {
        let origin = building_spot(net, buildings.get(from)?)?;
        let dest_b = buildings.get(to)?;
        let dest = building_spot(net, dest_b)?;
        let park = self.parking.find(buildings, to, dest_b);
        let park_at = match park {
            Some(p) => building_spot(net, buildings.get(p)?)?,
            None => dest,
        };
        let id = self.spawn_mode(net, router, LaneMode::Car, &[(origin, park_at)], now)[0]?;
        if let Some(p) = park {
            self.parking.take(p);
            if p != to {
                self.rows[id.slot()].as_mut().expect("作った主体がない").then = Some(dest);
            }
        }
        self.parking.leave(from);
        Some(id)
    }
}
