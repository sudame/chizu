//! 押された建物や施設の様子。画面の「見る」道具のパネルに出す。
//!
//! 押された 1 つについてだけ集計し、毎 tick 全体を集計することはしない。

use chizu_core::clock::GAME_SECONDS_PER_TICK;
use chizu_core::happiness::{commute_time, total};
use chizu_core::{BuildingId, CarPlace, StationId, TilePos, TileRect, World, Zone, ZoneKind};

/// 住宅の住民の様子。住民が 1 人もいなければ `None` にする項目が多い。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Residents {
    /// 住民の数。
    pub count: u64,
    /// 住民の幸福度の平均。
    pub happiness: u8,
    /// 幸福度の要素のうち、通勤、生活サービス、暮らし向きの平均。
    pub parts: [u8; 3],
    /// 幸福度の要素のうち、駐車場の平均。
    pub parking: u8,
    /// 職場のある住民の通勤の所要時間の平均を、時計の秒にしたもの。幸福度の通勤の点と同じく、測った通勤の時間を
    /// 均した値で、まだ測っていない住民は直線距離からの見積もりを使う（[`commute_time`]）。移動の 1 秒（1 tick）は
    /// 時計の 12 秒にあたるので、[`GAME_SECONDS_PER_TICK`] を掛ける。observe の `commute.time` と同じ物差しである。
    /// 職場のある住民がいなければ `None`。
    pub commute_seconds: Option<u64>,
}

/// 押された建物の様子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildingInfo {
    /// 区画の種類。低密度の住宅は高密度の住宅と別の値になる。
    pub zone: Zone,
    pub level: u8,
    pub power: bool,
    pub water: bool,
    /// 住宅なら世帯の上限、職場なら職場の上限。
    pub capacity: u64,
    /// 住宅なら住んでいる世帯の数、職場なら働いている人の数。
    pub occupied: u64,
    /// 住宅だけ。住民がいなければ `None`。
    pub residents: Option<Residents>,
    /// 住宅だけ。車を置く枠の様子。
    pub car_places: Option<CarPlaceInfo>,
}

/// 住宅の車を置く枠の様子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarPlaceInfo {
    /// 建物の駐車場の台数。自宅の枠の数である。
    pub home_spaces: u64,
    /// 自宅の枠を持つ世帯の数。
    pub home: u64,
    /// 共同の駐車場の枠を借りている世帯の数。
    pub shared: u64,
    /// 枠を持たない世帯の数。車を持てない世帯である。
    pub none: u64,
}

/// 押された施設の様子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacilityInfo {
    pub kind: chizu_core::ServiceKind,
    /// 電気か水道の供給量。共同の駐車場は 0 である。
    pub capacity: u64,
    pub upkeep_per_day: i64,
    /// 共同の駐車場だけ。
    pub parking: Option<ParkingInfo>,
}

/// 共同の駐車場の埋まり具合。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParkingInfo {
    /// 台数。
    pub spaces: u64,
    /// 世帯に貸した枠の数。
    pub rented: u64,
    /// 停まっている車の台数。借りた世帯の車と、借りていない車（外来）の和である。
    pub parked: u64,
    /// 外来の車が停まっているか、向かっていて押さえている台数。
    pub visitors: u64,
}

/// 押された駅の様子。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationInfo {
    /// 駅で待っている人。
    pub waiting: u64,
    /// 始まりからの乗った人と降りた人。1 日の数は画面が日の変わり目の差で求める。
    pub boarded: u64,
    pub alighted: u64,
    /// 出入口の徒歩圏の世帯の数と、そのうち車を持てない世帯の数。出入口がなければ `None`。
    pub catchment: Option<(u32, u32)>,
    /// 直前に乗りきれず人が残ったか。
    pub left_behind: bool,
    /// 駅に止まる路線の添え字。
    pub lines: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Inspected {
    Building(BuildingInfo),
    Facility(FacilityInfo),
    Station(StationInfo),
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
    /// 駅。世代つきの ID で、置き直された駅とは区別できる。
    Station(StationId),
}

/// 見つけたものと、その識別子。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub target: Target,
    pub info: Inspected,
}

fn facility_info(world: &World, f: &chizu_core::service::Facility) -> Found {
    let parking = f.kind.is_parking().then(|| {
        let lot = world.trips().shared_lot(f.area.min);
        ParkingInfo {
            spaces: u64::from(f.kind.parking_spaces()),
            rented: u64::from(lot.rented),
            parked: lot.parked(),
            visitors: u64::from(lot.visitors.parked) + u64::from(lot.visitors.reserved),
        }
    });
    Found {
        target: Target::Facility(f.area.min),
        info: Inspected::Facility(FacilityInfo {
            kind: f.kind,
            capacity: f.kind.capacity(),
            upkeep_per_day: f.kind.upkeep_per_day().0,
            parking,
        }),
    }
}

fn station_info(world: &World, id: StationId) -> Option<Found> {
    let rail = world.rail();
    rail.station(id)?;
    let (boarded, alighted) = rail
        .ride_tally()
        .station_counts
        .get(&id)
        .copied()
        .unwrap_or((0, 0));
    let catchment = world
        .station_door(id)
        .and_then(|door| world.catchments(&[door]).pop())
        .map(|c| (c.households, c.carless));
    Some(Found {
        target: Target::Station(id),
        info: Inspected::Station(StationInfo {
            waiting: rail.waiting_at(id) as u64,
            boarded,
            alighted,
            catchment,
            left_behind: rail.left_behind().iter().any(|(s, _, _)| *s == id),
            lines: rail
                .lines()
                .filter(|(_, l)| l.stations.contains(&id))
                .map(|(lid, _)| lid.index)
                .collect(),
        }),
    })
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
    if let Some(f) = world.facilities().iter().find(|f| contains(f.area, at)) {
        return Some(facility_info(world, f));
    }
    world
        .rail()
        .station_at(at)
        .and_then(|id| station_info(world, id))
}

/// 識別子で調べる。消えていれば `None`。
pub fn inspect_target(world: &World, target: Target) -> Option<Found> {
    match target {
        Target::Building(id) => found_building(world, id),
        Target::Facility(min) => world
            .facilities()
            .iter()
            .find(|f| f.area.min == min)
            .map(|f| facility_info(world, f)),
        Target::Station(id) => station_info(world, id),
    }
}

/// 画面から届いた識別子 `[種類, a, b]` を読む。種類 1 は建物（a が添え字、b が世代）、2 は施設（a, b が min のマス）、3 は駅（a が添え字、b が世代）。
pub fn decode_target(kind: f64, a: f64, b: f64) -> Option<Target> {
    let int = |v: f64| (v.fract() == 0.0 && v.abs() <= 9_007_199_254_740_991.0).then_some(v as i64);
    match kind as i64 {
        1 => Some(Target::Building(BuildingId {
            index: u32::try_from(int(a)?).ok()?,
            generation: u16::try_from(int(b)?).ok()?,
        })),
        2 => Some(Target::Facility(TilePos::new(int(a)?, int(b)?))),
        3 => Some(Target::Station(StationId {
            index: u32::try_from(int(a)?).ok()?,
            generation: u16::try_from(int(b)?).ok()?,
        })),
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
        car_places: None,
    };
    if b.zone.kind() != ZoneKind::Residential {
        info.capacity = b.workplaces_at(world.tax_permille(b.zone.kind()));
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
    let mut parking = 0u64;
    let mut happiness = 0u64;
    let (mut commute_total, mut commuters) = (0u64, 0u64);
    let mut places = CarPlaceInfo {
        home_spaces: b.parking_spaces(world.seed()),
        home: 0,
        shared: 0,
        none: 0,
    };
    for household in pop.household_ids() {
        if pop.home(household).flatten() != Some(id) {
            continue;
        }
        info.occupied += 1;
        match world.car_places().get(household) {
            Some(CarPlace::Home) => places.home += 1,
            Some(CarPlace::Shared(_)) => places.shared += 1,
            None => places.none += 1,
        }
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
            parking += u64::from(h.parking);
            if let Some(work) = row.workplace.and_then(|w| buildings.get(w)) {
                commute_total += commute_time(&row, b, work);
                commuters += 1;
            }
        }
    }
    info.car_places = Some(places);
    if count > 0 {
        info.residents = Some(Residents {
            count,
            happiness: (happiness / count) as u8,
            parts: sums.map(|s| (s / count) as u8),
            parking: (parking / count) as u8,
            commute_seconds: (commuters > 0)
                .then(|| commute_total / commuters * u64::from(GAME_SECONDS_PER_TICK)),
        });
    }
    Some(info)
}

/// 画面に渡す数の個数。
pub const INSPECT_LENGTH: usize = 20;

/// 駅の欄に並べる、止まる路線の数の上限。
pub const STATION_LINES: usize = 10;

/// 数に直す。要素がないものは -1 にする。なければ空の配列。
///
/// 建物：`[1, 区画の種類, 段階, 電気, 水道, 上限, 埋まっている数, 住民の数, 幸福度, 通勤の点数,
/// 生活サービスの点数, 暮らし向きの点数, 通勤の所要時間（時計の秒）, 自宅の枠の数, 自宅の枠を持つ世帯,
/// 共同の駐車場の枠を借りている世帯, 枠を持たない世帯, 駐車場の点数]`。自宅の枠の数から枠を持たない世帯までの 4 つは
/// 住宅だけで、ほかは -1 である。駐車場の点数は、住民のいる住宅だけで、ほかは -1 である。
/// 施設：`[2, 施設の種類, 供給量, 1 日の維持費, 台数, 貸した枠, 停まっている台数, 外来の車, -1, ...]`。
/// 台数からあとの 4 つは共同の駐車場だけで、ほかの施設は -1 である。
/// 駅：`[3, 待っている人, 乗った人（積み上げ）, 降りた人（積み上げ）, 徒歩圏の世帯（出入口がなければ -1）, そのうち車を持てない世帯,
/// 乗りきれず人が残ったか（0 か 1）, 止まる路線の数 n（最大 [`STATION_LINES`]）, 路線の添え字 × n, -1, ...]`。
/// どれも最後の 2 つは識別子の a と b で、建物なら ID の添え字と世代、施設なら min のマスの x と y、駅なら ID の添え字と世代である。
/// 先頭の値が識別子の種類を兼ねる。
pub fn encode(found: Option<Found>) -> Vec<f64> {
    let mut out = vec![-1.0; INSPECT_LENGTH];
    let Some(found) = found else {
        return Vec::new();
    };
    let (a, b) = match found.target {
        Target::Building(id) => (f64::from(id.index), f64::from(id.generation)),
        Target::Facility(min) => (min.x as f64, min.y as f64),
        Target::Station(id) => (f64::from(id.index), f64::from(id.generation)),
    };
    out[INSPECT_LENGTH - 2] = a;
    out[INSPECT_LENGTH - 1] = b;
    match found.info {
        Inspected::Station(st) => {
            out[0] = 3.0;
            out[1] = st.waiting as f64;
            out[2] = st.boarded as f64;
            out[3] = st.alighted as f64;
            if let Some((households, carless)) = st.catchment {
                out[4] = f64::from(households);
                out[5] = f64::from(carless);
            }
            out[6] = f64::from(u8::from(st.left_behind));
            let lines = &st.lines[..st.lines.len().min(STATION_LINES)];
            out[7] = lines.len() as f64;
            for (slot, index) in out[8..].iter_mut().zip(lines) {
                *slot = f64::from(*index);
            }
        }
        Inspected::Facility(f) => {
            out[0] = 2.0;
            out[1] = f64::from(f.kind as u8);
            out[2] = f.capacity as f64;
            out[3] = f.upkeep_per_day as f64;
            if let Some(p) = f.parking {
                out[4..8].copy_from_slice(&[
                    p.spaces as f64,
                    p.rented as f64,
                    p.parked as f64,
                    p.visitors as f64,
                ]);
            }
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
                out[17] = f64::from(r.parking);
            } else if b.zone.kind() == ZoneKind::Residential {
                out[7] = 0.0;
            }
            if let Some(c) = b.car_places {
                out[13..17].copy_from_slice(&[
                    c.home_spaces as f64,
                    c.home as f64,
                    c.shared as f64,
                    c.none as f64,
                ]);
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
    use chizu_core::{CitizenRow, Command, RoadKind, ServiceKind, TICKS_PER_DAY, Zone};

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
            zone: Some(Zone::from(zone)),
        })
        .unwrap();
    }

    /// 外の世界とつながった道路の南側に住宅を塗り、発電所と浄水場を置いた街。商業を塗るかは選べる。
    fn new_town(commercial: bool) -> Sim {
        new_town_with(Zone::Residential, commercial)
    }

    /// [`new_town`] の、住宅の区画を選べる版。
    fn new_town_with(home: Zone, commercial: bool) -> Sim {
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
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(20, 15)),
            zone: Some(home),
        })
        .unwrap();
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
            .filter(|(_, b)| b.zone == Zone::Residential)
            .map(|(id, _)| id)
            .collect()
    }

    fn households_in(world: &World, id: BuildingId) -> u64 {
        let pop = world.population();
        pop.household_ids()
            .filter(|h| pop.home(*h).flatten() == Some(id))
            .count() as u64
    }

    /// 検査の答えの区画は `Zone` の値で送り、低密度の住宅（5）を高密度の住宅（1）と見分けさせる。
    #[test]
    fn a_home_reports_its_density_in_the_zone_code() {
        for (zone, code) in [(Zone::ResidentialLow, 5.0), (Zone::Residential, 1.0)] {
            let mut sim = new_town_with(zone, false);
            sim.advance((TICKS_PER_DAY * 3) as u32);
            let world = sim.world();
            let (id, _) = world
                .buildings()
                .iter()
                .find(|(_, b)| b.zone == zone)
                .expect("住宅が建たない");
            let (_, info) = building(world, area_min(world, id));
            assert_eq!(info.zone, zone);
            let encoded = encode(inspect(world, area_min(world, id)));
            assert_eq!(encoded[1], code);
            // 低密度でも住宅なので、世帯の欄が出る
            assert!(info.capacity > 0);
        }
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
        // 通勤の所要時間は、職のある住民だけで割り、時計の秒（移動の秒の 12 倍）で出す
        let home = world.buildings().get(id).unwrap();
        let times: Vec<u64> = rows
            .iter()
            .filter_map(|c| c.workplace.map(|w| (c, w)))
            .map(|(c, w)| commute_time(c, home, world.buildings().get(w).unwrap()))
            .collect();
        assert!(!times.is_empty(), "職に就いた住民がいない");
        assert_eq!(
            r.commute_seconds,
            Some(times.iter().sum::<u64>() / times.len() as u64 * 12)
        );
        let encoded = encode(inspect(world, area_min(world, id)));
        assert_eq!(encoded[0], 1.0);
        assert_eq!(encoded[7], r.count as f64);
        assert_eq!(encoded[8], f64::from(r.happiness));
        assert_eq!(encoded[9], f64::from(r.parts[0]));
        assert_eq!(encoded[10], f64::from(r.parts[1]));
        assert_eq!(
            u64::from(r.parking),
            mean(&|c| u64::from(c.happiness.parking))
        );
        assert_eq!(encoded[17], f64::from(r.parking));
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
            .find(|(_, b)| b.zone == Zone::Commercial)
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

    /// 住宅は、自宅の枠の数と、枠を持つ世帯、共同の駐車場を借りた世帯、枠のない世帯の数を出す。
    /// 数はどれも、世帯ごとの枠から数えた値と合う。
    #[test]
    fn a_home_shows_how_its_households_keep_cars() {
        let (mut sim, _guard) = crate::test_town::carless_town(2);
        let world = sim.world();
        let id = homes(world)
            .into_iter()
            .max_by_key(|id| households_in(world, *id))
            .expect("住宅が建たない");
        let at = area_min(world, id);
        let (_, info) = building(world, at);
        let places = info.car_places.expect("住宅の枠の様子がない");
        let spaces = world.buildings().parking_spaces(id);
        assert!(info.occupied > 0 && spaces > 0);
        assert_eq!(
            places.home_spaces, spaces,
            "自宅の枠の数は建物の駐車場の台数"
        );
        // 自宅の枠を 0 にしてあるので、枠のない世帯が車を持てない
        assert_eq!(
            (places.home, places.shared, places.none),
            (0, 0, info.occupied)
        );
        let out = encode(inspect(world, at));
        assert_eq!(out[13..17], [spaces as f64, 0.0, 0.0, info.occupied as f64]);

        // 駐車場を置いて次の決め直しを過ぎると、借りた世帯は枠を持つ側に移る
        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingLot, 24);
        sim.advance(chizu_core::world::people::MIGRATION_INTERVAL as u32);
        let world = sim.world();
        let (mut shared, mut none, mut housed) = (0, 0, 0);
        for id in homes(world) {
            let (_, info) = building(world, area_min(world, id));
            let p = info.car_places.unwrap();
            assert_eq!(p.home + p.shared + p.none, info.occupied);
            shared += p.shared;
            none += p.none;
            housed += info.occupied;
        }
        assert_eq!(shared, 12);
        assert_eq!(none + shared, housed);
    }

    /// 共同の駐車場は、供給量ではなく、台数と貸した枠、停まっている台数を出す。
    #[test]
    fn a_shared_lot_shows_spaces_rented_and_parked() {
        let (mut sim, _guard) = crate::test_town::carless_town(1);
        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingLot, 24);
        sim.advance(TICKS_PER_DAY as u32);
        let found = sim.inspect(TilePos::new(24, 9));
        let kind = ServiceKind::ParkingLot;
        assert_eq!(found.len(), INSPECT_LENGTH);
        let lot = sim.world().trips().shared_lot(TilePos::new(23, 8));
        assert_eq!(lot.rented, 12, "枠がすべて借りられていない");
        let parked = f64::from(lot.rented_parked + lot.visitors.parked);
        let visitors = f64::from(lot.visitors.parked + lot.visitors.reserved);
        assert_eq!(
            found[..8],
            [
                2.0,
                f64::from(kind as u8),
                0.0,
                kind.upkeep_per_day().0 as f64,
                12.0,
                12.0,
                parked,
                visitors
            ]
        );
        // 発電所は、供給量を出し、台数の欄は空
        let plant = sim.inspect(TilePos::new(16, 8));
        assert_eq!(plant[2], ServiceKind::PowerPlant.capacity() as f64);
        assert_eq!(plant[4..8], [-1.0; 4]);
    }
    /// 駅は、待っている人、乗り降りの数、徒歩圏の世帯、止まる路線を出す。識別子で引き直せる。
    #[test]
    fn a_station_shows_its_waiting_riders_catchment_and_lines() {
        let mut sim = Sim::new(0);
        sim.submit(road((0, 10), (70, 10))).unwrap();
        for x in [12, 62] {
            sim.submit(Command::PlaceBuilding {
                kind: ServiceKind::Station,
                at: TilePos::new(x, 9),
                rotation: 2,
            })
            .unwrap();
        }
        sim.submit(Command::BuildRail {
            from: TilePos::new(12, 8),
            to: TilePos::new(53, 8),
        })
        .unwrap();
        let ids: Vec<_> = sim.world().rail().stations().map(|(id, _)| id).collect();
        sim.submit(Command::CreateLine {
            from: ids[0],
            to: ids[1],
        })
        .unwrap();
        let line = sim.world().rail().lines().next().unwrap().0;
        let found = sim.inspect(TilePos::new(12, 8));
        assert_eq!(found.len(), INSPECT_LENGTH);
        assert_eq!(found[0], 3.0);
        // 待つ人も乗り降りもなく、徒歩圏に世帯もなく、路線は 1 本
        assert_eq!(found[1..7], [0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(found[7], 1.0);
        assert_eq!(found[8], f64::from(line.index));
        assert_eq!(found[9], -1.0);
        // 識別子（最後の 2 つ）で引き直すと同じ答えになる
        let again = sim.inspect_target(found[0], found[18], found[19]);
        assert_eq!(again, found);
        // 駅のない所は空
        assert!(sim.inspect(TilePos::new(40, 30)).is_empty());
    }
}
