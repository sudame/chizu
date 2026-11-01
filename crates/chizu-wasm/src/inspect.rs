//! 押された建物や施設の様子。画面の「見る」道具のパネルに出す。
//!
//! 押された 1 つについてだけ集計し、毎 tick 全体を集計することはしない。

use chizu_core::happiness::{commute_seconds, total};
use chizu_core::{BuildingId, TilePos, TileRect, World, ZoneKind};

/// 住宅の住民の様子。住民が 1 人もいなければ `None` にする項目が多い。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Residents {
    /// 住民の数。
    pub count: u64,
    /// 住民の幸福度の平均。
    pub happiness: u8,
    /// 幸福度の 3 つの要素（通勤、生活サービス、暮らし向き）の平均。
    pub parts: [u8; 3],
    /// 職場のある住民の通勤の所要時間（秒）の平均。職場のある住民がいなければ `None`。
    pub commute_seconds: Option<u64>,
}

/// 押された建物の様子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildingInfo {
    pub zone: ZoneKind,
    pub level: u8,
    pub power: bool,
    pub water: bool,
    /// 住宅なら世帯の上限、職場なら職場の上限。
    pub capacity: u64,
    /// 住宅なら住んでいる世帯の数、職場なら働いている人の数。
    pub occupied: u64,
    /// 住宅だけ。住民がいなければ `None`。
    pub residents: Option<Residents>,
}

/// 押された施設の様子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacilityInfo {
    pub kind: chizu_core::ServiceKind,
    pub capacity: u64,
    pub upkeep_per_day: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inspected {
    Building(BuildingInfo),
    Facility(FacilityInfo),
}

fn contains(r: TileRect, t: TilePos) -> bool {
    (r.min.x..=r.max.x).contains(&t.x) && (r.min.y..=r.max.y).contains(&t.y)
}

/// マス `at` にある建物か施設を調べる。何もなければ `None`。
pub fn inspect(world: &World, at: TilePos) -> Option<Inspected> {
    if let Some((id, _)) = world.buildings().iter().find(|(_, b)| contains(b.area, at)) {
        return building_info(world, id).map(Inspected::Building);
    }
    world
        .facilities()
        .iter()
        .find(|f| contains(f.area, at))
        .map(|f| {
            Inspected::Facility(FacilityInfo {
                kind: f.kind,
                capacity: f.kind.capacity(),
                upkeep_per_day: f.kind.upkeep_per_day().0,
            })
        })
}

fn building_info(world: &World, id: BuildingId) -> Option<BuildingInfo> {
    let buildings = world.buildings();
    let pop = world.population();
    let b = buildings.get(id)?;
    let mut info = BuildingInfo {
        zone: b.zone,
        level: b.level,
        power: b.supply.power,
        water: b.supply.water,
        capacity: 0,
        occupied: 0,
        residents: None,
    };
    if b.zone != ZoneKind::Residential {
        info.capacity = b.workplaces();
        info.occupied = pop
            .citizen_ids()
            .filter(|c| pop.citizen(*c).and_then(|r| r.workplace) == Some(id))
            .count() as u64;
        return Some(info);
    }
    info.capacity = b.households();
    let mut count = 0u64;
    let mut sums = [0u64; 3];
    let mut happiness = 0u64;
    let (mut commute_total, mut commuters) = (0u64, 0u64);
    for household in pop.household_ids() {
        if pop.home(household).flatten() != Some(id) {
            continue;
        }
        info.occupied += 1;
        for member in pop.members(household).unwrap_or(&[]) {
            let Some(row) = pop.citizen(*member) else {
                continue;
            };
            count += 1;
            let h = row.happiness;
            happiness += u64::from(total(h));
            for (sum, v) in sums.iter_mut().zip([h.commute, h.services, h.wealth]) {
                *sum += u64::from(v);
            }
            if let Some(work) = row.workplace.and_then(|w| buildings.get(w)) {
                commute_total += commute_seconds(b, work);
                commuters += 1;
            }
        }
    }
    if count > 0 {
        info.residents = Some(Residents {
            count,
            happiness: (happiness / count) as u8,
            parts: sums.map(|s| (s / count) as u8),
            commute_seconds: (commuters > 0).then(|| commute_total / commuters),
        });
    }
    Some(info)
}

/// 画面に渡す数の個数。
pub const INSPECT_LENGTH: usize = 13;

/// 数に直す。要素がないものは -1 にする。なければ空の配列。
///
/// 建物：`[1, 区画の種類, 段階, 電気, 水道, 上限, 埋まっている数, 住民の数, 幸福度, 通勤の点数,
/// 生活サービスの点数, 暮らし向きの点数, 通勤の所要時間（秒）]`。
/// 施設：`[2, 施設の種類, 供給量, 1 日の維持費, -1, ...]`。
pub fn encode(found: Option<Inspected>) -> Vec<f64> {
    let mut out = vec![-1.0; INSPECT_LENGTH];
    match found {
        None => return Vec::new(),
        Some(Inspected::Facility(f)) => {
            out[0] = 2.0;
            out[1] = f64::from(f.kind as u8);
            out[2] = f.capacity as f64;
            out[3] = f.upkeep_per_day as f64;
        }
        Some(Inspected::Building(b)) => {
            out[..7].copy_from_slice(&[
                1.0,
                f64::from(b.zone as u8),
                f64::from(b.level),
                f64::from(u8::from(b.power)),
                f64::from(u8::from(b.water)),
                b.capacity as f64,
                b.occupied as f64,
            ]);
            if let Some(r) = b.residents {
                out[7] = r.count as f64;
                out[8] = f64::from(r.happiness);
                for (slot, v) in out[9..12].iter_mut().zip(r.parts) {
                    *slot = f64::from(v);
                }
                if let Some(s) = r.commute_seconds {
                    out[12] = s as f64;
                }
            } else if b.zone == ZoneKind::Residential {
                out[7] = 0.0;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Sim;
    use chizu_core::{Command, RoadKind, ServiceKind, TICKS_PER_DAY};

    fn road(from: (i64, i64), to: (i64, i64)) -> Command {
        Command::BuildRoad {
            from: TilePos::new(from.0, from.1),
            to: TilePos::new(to.0, to.1),
            kind: RoadKind::Small,
        }
    }

    fn paint(sim: &mut Sim, x: (i64, i64), zone: ZoneKind) {
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(x.0, 11), TilePos::new(x.1, 15)),
            zone: Some(zone),
        })
        .unwrap();
    }

    /// 外の世界とつながった道路の両側に住宅と商業を塗り、発電所と浄水場を置いて 3 日進めた街。
    fn town() -> Sim {
        let mut sim = Sim::new(0);
        sim.submit(road((10, 10), (30, 10))).unwrap();
        let inner = chizu_core::world::highway_line(0).start;
        let corner = (inner.x, 10);
        for (from, to) in [
            ((inner.x, inner.y), corner),
            (corner, (inner.x.clamp(10, 30), 10)),
        ] {
            if from != to {
                sim.submit(road(from, to)).unwrap();
            }
        }
        for (kind, x) in [(ServiceKind::PowerPlant, 16), (ServiceKind::WaterPump, 20)] {
            sim.submit(Command::PlaceBuilding {
                kind,
                at: TilePos::new(x, 9),
                rotation: 2,
            })
            .unwrap();
        }
        paint(&mut sim, (10, 20), ZoneKind::Residential);
        paint(&mut sim, (21, 30), ZoneKind::Commercial);
        sim.advance((TICKS_PER_DAY * 3) as u32);
        sim
    }

    fn tile_of(sim: &Sim, id: BuildingId) -> TilePos {
        sim.world().buildings().get(id).unwrap().area.min
    }

    #[test]
    fn nothing_there_gives_nothing() {
        let sim = town();
        assert!(sim.inspect(TilePos::new(0, 0)).is_empty());
    }

    #[test]
    fn a_facility_shows_its_kind_supply_and_upkeep() {
        let sim = town();
        let found = sim.inspect(TilePos::new(16, 8));
        let kind = ServiceKind::PowerPlant;
        assert_eq!(found.len(), INSPECT_LENGTH);
        assert_eq!(
            found[..4],
            [
                2.0,
                f64::from(kind as u8),
                kind.capacity() as f64,
                kind.upkeep_per_day().0 as f64
            ]
        );
    }

    #[test]
    fn a_home_sums_only_its_own_residents() {
        let sim = town();
        let world = sim.world();
        let pop = world.population();
        let (id, _) = world
            .buildings()
            .iter()
            .filter(|(_, b)| b.zone == ZoneKind::Residential)
            .max_by_key(|(id, _)| {
                pop.household_ids()
                    .filter(|h| pop.home(*h).flatten() == Some(*id))
                    .count()
            })
            .expect("住宅が建たない");
        let Some(Inspected::Building(info)) = inspect(world, tile_of(&sim, id)) else {
            panic!("建物が見つからない");
        };
        let homes: Vec<_> = pop
            .household_ids()
            .filter(|h| pop.home(*h).flatten() == Some(id))
            .collect();
        let members: Vec<_> = homes
            .iter()
            .flat_map(|h| pop.members(*h).unwrap().iter().copied())
            .collect();
        assert!(!homes.is_empty(), "住民が越してこない");
        assert_eq!(info.occupied, homes.len() as u64);
        assert!(info.occupied <= info.capacity);
        let r = info.residents.expect("住民がいる");
        assert_eq!(r.count, members.len() as u64);
        let mean = |f: &dyn Fn(chizu_core::CitizenRow) -> u64| {
            members
                .iter()
                .map(|m| f(pop.citizen(*m).unwrap()))
                .sum::<u64>()
                / r.count
        };
        assert_eq!(
            u64::from(r.happiness),
            mean(&|c| u64::from(total(c.happiness)))
        );
        assert_eq!(
            u64::from(r.parts[2]),
            mean(&|c| u64::from(c.happiness.wealth))
        );
        let encoded = encode(Some(Inspected::Building(info)));
        assert_eq!(encoded[0], 1.0);
        assert_eq!(encoded[7], r.count as f64);
        assert_eq!(encoded[8], f64::from(r.happiness));
    }

    #[test]
    fn a_workplace_counts_the_people_working_there() {
        let sim = town();
        let world = sim.world();
        let pop = world.population();
        let (id, _) = world
            .buildings()
            .iter()
            .find(|(_, b)| b.zone == ZoneKind::Commercial)
            .expect("商業が建たない");
        let Some(Inspected::Building(info)) = inspect(world, tile_of(&sim, id)) else {
            panic!("建物が見つからない");
        };
        let workers = pop
            .citizen_ids()
            .filter(|c| pop.citizen(*c).unwrap().workplace == Some(id))
            .count() as u64;
        assert_eq!(info.occupied, workers);
        assert!(workers <= info.capacity);
        assert_eq!(info.residents, None);
        let encoded = encode(Some(Inspected::Building(info)));
        assert_eq!(encoded[7..], [-1.0; 6]);
    }
}
