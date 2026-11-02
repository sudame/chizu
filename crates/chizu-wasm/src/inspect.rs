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

/// 押したものの識別子。押したあとはマスではなくこれで追う。同じマスに別のものが建っても取り違えない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// 世代つきの ID で、建て直された建物とは区別できる。
    Building(BuildingId),
    /// 施設には ID がないので、占めるマスの min で見分ける。
    Facility(TilePos),
}

/// 見つけたものと、その識別子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    pub target: Target,
    pub info: Inspected,
}

fn facility_info(f: &chizu_core::service::Facility) -> Found {
    Found {
        target: Target::Facility(f.area.min),
        info: Inspected::Facility(FacilityInfo {
            kind: f.kind,
            capacity: f.kind.capacity(),
            upkeep_per_day: f.kind.upkeep_per_day().0,
        }),
    }
}

fn found_building(world: &World, id: BuildingId) -> Option<Found> {
    building_info(world, id).map(|b| Found {
        target: Target::Building(id),
        info: Inspected::Building(b),
    })
}

/// マス `at` にある建物か施設を調べる。何もなければ `None`。
pub fn inspect(world: &World, at: TilePos) -> Option<Found> {
    if let Some((id, _)) = world.buildings().iter().find(|(_, b)| contains(b.area, at)) {
        return found_building(world, id);
    }
    world
        .facilities()
        .iter()
        .find(|f| contains(f.area, at))
        .map(facility_info)
}

/// 識別子で調べる。消えていれば `None`。
pub fn inspect_target(world: &World, target: Target) -> Option<Found> {
    match target {
        Target::Building(id) => found_building(world, id),
        Target::Facility(min) => world
            .facilities()
            .iter()
            .find(|f| f.area.min == min)
            .map(facility_info),
    }
}

/// 画面から届いた識別子 `[種類, a, b]` を読む。種類 1 は建物（a が添え字、b が世代）、2 は施設（a, b が min のマス）。
pub fn decode_target(kind: f64, a: f64, b: f64) -> Option<Target> {
    let int = |v: f64| (v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0).then_some(v as i64);
    match kind as i64 {
        1 => Some(Target::Building(BuildingId {
            index: u32::try_from(int(a)?).ok()?,
            generation: u16::try_from(int(b)?).ok()?,
        })),
        2 => Some(Target::Facility(TilePos::new(int(a)?, int(b)?))),
        _ => None,
    }
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
        info.capacity = b.workplaces_at(world.tax_permille(b.zone));
        // 税率を上げた直後は、職の割り当て直しまで勤め人が枠を超えている。枠で頭打ちにして出す。
        info.occupied = (pop
            .citizen_ids()
            .filter(|c| pop.citizen(*c).and_then(|r| r.workplace) == Some(id))
            .count() as u64)
            .min(info.capacity);
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
pub const INSPECT_LENGTH: usize = 15;

/// 数に直す。要素がないものは -1 にする。なければ空の配列。
///
/// 建物：`[1, 区画の種類, 段階, 電気, 水道, 上限, 埋まっている数, 住民の数, 幸福度, 通勤の点数,
/// 生活サービスの点数, 暮らし向きの点数, 通勤の所要時間（秒）]`。
/// 施設：`[2, 施設の種類, 供給量, 1 日の維持費, -1, ...]`。
/// どちらも最後の 2 つは識別子の a と b で、建物なら ID の添え字と世代、施設なら min のマスの x と y である。
/// 先頭の値が識別子の種類を兼ねる。
pub fn encode(found: Option<Found>) -> Vec<f64> {
    let mut out = vec![-1.0; INSPECT_LENGTH];
    let Some(found) = found else {
        return Vec::new();
    };
    let (a, b) = match found.target {
        Target::Building(id) => (f64::from(id.index), f64::from(id.generation)),
        Target::Facility(min) => (min.x as f64, min.y as f64),
    };
    out[INSPECT_LENGTH - 2] = a;
    out[INSPECT_LENGTH - 1] = b;
    match found.info {
        Inspected::Facility(f) => {
            out[0] = 2.0;
            out[1] = f64::from(f.kind as u8);
            out[2] = f.capacity as f64;
            out[3] = f.upkeep_per_day as f64;
        }
        Inspected::Building(b) => {
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
    use chizu_core::world::growth::GROWTH_INTERVAL;
    use chizu_core::{CitizenRow, Command, RoadKind, ServiceKind, TICKS_PER_DAY};

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

    /// 外の世界とつながった道路の南側に住宅を塗り、発電所と浄水場を置いた街。商業を塗るかは選べる。
    fn new_town(commercial: bool) -> Sim {
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
        if commercial {
            paint(&mut sim, (21, 30), ZoneKind::Commercial);
        }
        sim
    }

    fn town(commercial: bool) -> Sim {
        let mut sim = new_town(commercial);
        sim.advance((TICKS_PER_DAY * 3) as u32);
        sim
    }

    fn building(world: &World, at: TilePos) -> (BuildingId, BuildingInfo) {
        let found = inspect(world, at).expect("建物が見つからない");
        let (Target::Building(id), Inspected::Building(info)) = (found.target, found.info) else {
            panic!("建物でない");
        };
        (id, info)
    }

    fn area_min(world: &World, id: BuildingId) -> TilePos {
        world.buildings().get(id).unwrap().area.min
    }

    /// 家の住民の行。
    fn rows_of(world: &World, id: BuildingId) -> Vec<CitizenRow> {
        let pop = world.population();
        pop.household_ids()
            .filter(|h| pop.home(*h).flatten() == Some(id))
            .flat_map(|h| pop.members(h).unwrap().to_vec())
            .map(|m| pop.citizen(m).unwrap())
            .collect()
    }

    fn homes(world: &World) -> Vec<BuildingId> {
        world
            .buildings()
            .iter()
            .filter(|(_, b)| b.zone == ZoneKind::Residential)
            .map(|(id, _)| id)
            .collect()
    }

    fn households_in(world: &World, id: BuildingId) -> u64 {
        let pop = world.population();
        pop.household_ids()
            .filter(|h| pop.home(*h).flatten() == Some(id))
            .count() as u64
    }

    #[test]
    fn nothing_there_gives_nothing() {
        let sim = town(true);
        assert!(sim.inspect(TilePos::new(0, 0)).is_empty());
    }

    #[test]
    fn a_facility_shows_its_kind_supply_and_upkeep() {
        let sim = town(true);
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
        // 末尾の識別子で引き直すと同じ答えが返り、ずれた識別子では空になる
        let (a, b) = (found[INSPECT_LENGTH - 2], found[INSPECT_LENGTH - 1]);
        assert_eq!(sim.inspect_target(2.0, a, b), found);
        assert!(sim.inspect_target(2.0, a + 1.0, b).is_empty());
    }

    #[test]
    fn a_home_sums_only_its_own_residents() {
        let sim = town(true);
        let world = sim.world();
        // 職のある住民とない住民が混ざる家があれば、それを選ぶ
        let mixed = |id: &BuildingId| {
            let rows = rows_of(world, *id);
            let jobs = rows.iter().filter(|r| r.workplace.is_some()).count();
            (jobs > 0 && jobs < rows.len(), rows.len())
        };
        let id = homes(world)
            .into_iter()
            .max_by_key(mixed)
            .expect("住宅が建たない");
        let (_, info) = building(world, area_min(world, id));
        let rows = rows_of(world, id);
        assert!(!rows.is_empty(), "住民が越してこない");
        assert_eq!(info.occupied, households_in(world, id));
        assert!(info.occupied <= info.capacity);
        let r = info.residents.expect("住民がいる");
        let n = rows.len() as u64;
        assert_eq!(r.count, n);
        let mean = |f: &dyn Fn(&CitizenRow) -> u64| rows.iter().map(f).sum::<u64>() / n;
        assert_eq!(
            u64::from(r.happiness),
            mean(&|c| u64::from(total(c.happiness)))
        );
        assert_eq!(
            u64::from(r.parts[0]),
            mean(&|c| u64::from(c.happiness.commute))
        );
        assert_eq!(
            u64::from(r.parts[1]),
            mean(&|c| u64::from(c.happiness.services))
        );
        assert_eq!(
            u64::from(r.parts[2]),
            mean(&|c| u64::from(c.happiness.wealth))
        );
        // 通勤の所要時間は、職のある住民だけで割る
        let home = world.buildings().get(id).unwrap();
        let times: Vec<u64> = rows
            .iter()
            .filter_map(|c| c.workplace)
            .map(|w| commute_seconds(home, world.buildings().get(w).unwrap()))
            .collect();
        assert!(!times.is_empty(), "職に就いた住民がいない");
        assert_eq!(
            r.commute_seconds,
            Some(times.iter().sum::<u64>() / times.len() as u64)
        );
        let encoded = encode(inspect(world, area_min(world, id)));
        assert_eq!(encoded[0], 1.0);
        assert_eq!(encoded[7], r.count as f64);
        assert_eq!(encoded[8], f64::from(r.happiness));
        assert_eq!(encoded[9], f64::from(r.parts[0]));
        assert_eq!(encoded[10], f64::from(r.parts[1]));
        assert_eq!(encoded[12], r.commute_seconds.unwrap() as f64);
    }

    #[test]
    fn without_jobs_the_commute_time_is_none() {
        let sim = town(false);
        let world = sim.world();
        let mut checked = 0;
        for id in homes(world) {
            let (_, info) = building(world, area_min(world, id));
            if let Some(r) = info.residents {
                assert_eq!(r.commute_seconds, None);
                assert_eq!(encode(inspect(world, area_min(world, id)))[12], -1.0);
                checked += 1;
            }
        }
        assert!(checked > 0, "住民のいる住宅がない");
    }

    #[test]
    fn a_home_nobody_lives_in_has_no_residents() {
        let mut sim = new_town(true);
        // 住宅が建った直後で、まだ誰も越してきていない tick を探す
        let empty = (0..GROWTH_INTERVAL * 3).find_map(|_| {
            sim.advance(1);
            let world = sim.world();
            homes(world)
                .into_iter()
                .find(|id| households_in(world, *id) == 0)
        });
        let id = empty.expect("空き家が見つからない");
        let world = sim.world();
        let found = inspect(world, area_min(world, id));
        let (_, info) = building(world, area_min(world, id));
        assert_eq!(info.residents, None);
        assert_eq!(info.occupied, 0);
        let out = encode(found);
        assert_eq!(out[7], 0.0);
        assert_eq!(out[8..13], [-1.0; 5]);
    }

    #[test]
    fn a_workplace_counts_the_people_working_there() {
        let sim = town(true);
        let world = sim.world();
        let pop = world.population();
        let (id, _) = world
            .buildings()
            .iter()
            .find(|(_, b)| b.zone == ZoneKind::Commercial)
            .expect("商業が建たない");
        let (_, info) = building(world, area_min(world, id));
        let workers = pop
            .citizen_ids()
            .filter(|c| pop.citizen(*c).unwrap().workplace == Some(id))
            .count() as u64;
        assert_eq!(info.occupied, workers);
        assert!(workers <= info.capacity);
        assert_eq!(info.residents, None);
        let encoded = encode(inspect(world, area_min(world, id)));
        assert_eq!(encoded[7..13], [-1.0; 6]);
    }

    #[test]
    fn a_target_follows_the_building_and_not_the_tile() {
        let mut sim = town(true);
        let world = sim.world();
        let id = homes(world)[0];
        let at = area_min(world, id);
        let pressed = encode(inspect(world, at));
        let (a, b) = (pressed[INSPECT_LENGTH - 2], pressed[INSPECT_LENGTH - 1]);
        assert_eq!(sim.inspect_target(1.0, a, b), pressed);
        // 世代の違う ID は、同じ添え字でも別のものとして断る
        assert!(sim.inspect_target(1.0, a, b + 1.0).is_empty());
        // 建物が消えたら、同じ ID では何も返らない
        sim.submit(Command::Bulldoze {
            area: TileRect::from_corners(TilePos::new(10, 10), TilePos::new(30, 10)),
        })
        .unwrap();
        assert!(sim.world().buildings().get(id).is_none());
        assert!(sim.inspect_target(1.0, a, b).is_empty());
    }

    #[test]
    fn unreadable_targets_are_refused() {
        assert_eq!(decode_target(0.0, 1.0, 1.0), None);
        assert_eq!(decode_target(1.0, -1.0, 0.0), None);
        assert_eq!(decode_target(1.0, 0.0, 70_000.0), None);
        assert_eq!(decode_target(1.0, 0.5, 0.0), None);
        assert_eq!(decode_target(f64::NAN, 0.0, 0.0), None);
        assert_eq!(
            decode_target(2.0, -3.0, 4.0),
            Some(Target::Facility(TilePos::new(-3, 4)))
        );
    }
}
