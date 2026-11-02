//! データレイヤー。地図の上に重ねて描く、マスごとの値の面を作る。
//!
//! レイヤーは [`DataLayer`] の 1 つの型で表す。値の面を作る関数と、その値が変わったかを見分ける版の関数を
//! 差し替えれば、交通量や幸福度のレイヤーも同じ形で足せる。値の意味（色）は画面の側の
//! `web/src/map/layers.ts` が決める。

use std::collections::BTreeMap;

use chizu_core::service::{Balance, supply_balance};
use chizu_core::zone::tiles_of;
use chizu_core::{
    CHUNK_AREA, CHUNK_SIDE, ChunkCoord, Congestion, SegmentId, SegmentKind, ServiceKind,
    StateHasher, TilePos, World,
};
use wasm_bindgen::prelude::*;

use crate::Simulation;
use crate::sim::Sim;

// 電気と水道のレイヤーの値。0 は何も塗らない
/// 届いている建物。
pub const SUPPLY_BUILDING_ON: u8 = 1;
/// 届いていない建物。
pub const SUPPLY_BUILDING_OFF: u8 = 2;
/// 供給量が需要に足りている網の道路。
pub const SUPPLY_ROAD_ENOUGH: u8 = 3;
/// 施設はあるが、供給量が需要に足りない網の道路。
pub const SUPPLY_ROAD_SHORT: u8 = 4;
/// 施設のない網の道路。
pub const SUPPLY_ROAD_NONE: u8 = 5;
/// そのレイヤーの施設（電気なら発電所）。
pub const SUPPLY_FACILITY: u8 = 6;

/// データレイヤー 1 枚。
pub struct DataLayer {
    /// 画面とやりとりする番号。
    pub code: u8,
    /// 値の面が変わりうるときに変わる版。
    pub version: fn(&Sim) -> u64,
    /// 値の面を作る。版が変わったときだけ呼ぶ。
    pub values: fn(&World) -> LayerGrid,
}

pub const LAYERS: &[DataLayer] = &[
    DataLayer {
        code: 1,
        version: supply_version,
        values: |w| supply_values(w, ServiceKind::PowerPlant),
    },
    DataLayer {
        code: 2,
        version: supply_version,
        values: |w| supply_values(w, ServiceKind::WaterPump),
    },
    DataLayer {
        code: 3,
        version: traffic_version,
        values: traffic_values,
    },
];

// 交通のレイヤーの値。0 は何も塗らない。中核の `Congestion` の段階に 1 を足した値である
/// 空いている道路。
pub const TRAFFIC_FREE: u8 = 1;
/// やや混んでいる道路。
pub const TRAFFIC_BUSY: u8 = 2;
/// 混んでいる道路。
pub const TRAFFIC_HEAVY: u8 = 3;
/// 詰まっている道路。
pub const TRAFFIC_JAMMED: u8 = 4;

/// 交通のレイヤーを作り直す間隔（tick）。毎 tick は数えず、この間隔ごとに車の位置から区間の混み具合を数え直す。
pub const TRAFFIC_REFRESH_TICKS: u64 = 10;

/// マスごとの 1 バイトの値を、値のあるチャンクにだけ持つ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayerGrid {
    chunks: BTreeMap<ChunkCoord, Vec<u8>>,
}

impl LayerGrid {
    pub fn set(&mut self, t: TilePos, value: u8) {
        let Some(c) = t.chunk() else {
            return;
        };
        let side = CHUNK_SIDE as i64;
        let i = (t.y - i64::from(c.cy) * side) * side + (t.x - i64::from(c.cx) * side);
        let tiles = self.chunks.entry(c).or_insert_with(|| vec![0; CHUNK_AREA]);
        tiles[i as usize] = value;
    }

    /// チャンクの 64×64 の値を行ごとに並べて返す。値のないチャンクはすべて 0 にする。
    pub fn chunk(&self, c: ChunkCoord) -> Vec<u8> {
        self.chunks
            .get(&c)
            .cloned()
            .unwrap_or_else(|| vec![0; CHUNK_AREA])
    }
}

/// 建物の表（供給の有無を含む）か道路網が変わると変わる。HUD の街の数字も、この版が変わったときに送り直す。
pub fn supply_version(sim: &Sim) -> u64 {
    let mut h = StateHasher::new();
    h.write(&sim.buildings_version().to_bits());
    h.write(&sim.network_version());
    h.finish()
}

/// 建物は中核が判定して持っている供給の有無で、道路は網ごとの供給量と需要の釣り合いで塗る。
// 建物が建った tick は、中核が次の tick の頭まで供給を判定し直さないので、道路は足りていても新しい建物は赤になる
fn supply_values(world: &World, kind: ServiceKind) -> LayerGrid {
    let slot = match kind {
        ServiceKind::PowerPlant => 0,
        ServiceKind::WaterPump => 1,
    };
    let mut grid = LayerGrid::default();
    let network = world.network();
    let balance = supply_balance(network, world.facilities(), world.buildings());
    for (id, comp) in &balance.segments {
        // 高速道路は買った範囲の外へ長く続くので塗らない
        if network
            .segment(*id)
            .is_none_or(|s| s.kind == SegmentKind::Highway)
        {
            continue;
        }
        let Some(line) = network.segment_line(*id) else {
            continue;
        };
        let value = road_value(balance.components[*comp][slot]);
        for t in line.tiles() {
            grid.set(t, value);
        }
    }
    for f in world.facilities().iter().filter(|f| f.kind == kind) {
        for t in tiles_of(f.area) {
            grid.set(t, SUPPLY_FACILITY);
        }
    }
    for (_, b) in world.buildings().iter() {
        let on = match kind {
            ServiceKind::PowerPlant => b.supply.power,
            ServiceKind::WaterPump => b.supply.water,
        };
        let value = if on {
            SUPPLY_BUILDING_ON
        } else {
            SUPPLY_BUILDING_OFF
        };
        for t in tiles_of(b.area) {
            grid.set(t, value);
        }
    }
    grid
}

/// 道路網が変わるか、`TRAFFIC_REFRESH_TICKS` が過ぎると変わる。
fn traffic_version(sim: &Sim) -> u64 {
    let mut h = StateHasher::new();
    h.write(&sim.network_version());
    h.write(&(sim.world().tick().0 / TRAFFIC_REFRESH_TICKS));
    h.finish()
}

/// 高速道路を除くすべての区間を、混み具合の段階で塗る。車のいない区間は「空いている」になる。
/// 区間が交わるマスは、混んでいるほうの段階にする。
fn traffic_values(world: &World) -> LayerGrid {
    let network = world.network();
    let congestion = world.road_congestion();
    let mut segments: Vec<(u8, SegmentId)> = network
        .segments()
        .filter(|(_, s)| s.kind != SegmentKind::Highway)
        .map(|(id, _)| (traffic_value(congestion.get(&id).copied()), id))
        .collect();
    // 段階の低いものから塗り、あとから塗る高い段階が上書きする
    segments.sort();
    let mut grid = LayerGrid::default();
    for (value, id) in segments {
        let Some(line) = network.segment_line(id) else {
            continue;
        };
        for t in line.tiles() {
            grid.set(t, value);
        }
    }
    grid
}

/// 区間ごとの交通の値。並びは `Sim::road_segments` と同じ（`network.segments()` の順）で、
/// 高速道路は混み具合を数えないので 0 にする。マスの値と違い、交わるマスで隣の区間の値に上書きされない。
pub fn traffic_segment_values(world: &World) -> Vec<u8> {
    let congestion = world.road_congestion();
    world
        .network()
        .segments()
        .map(|(id, s)| {
            if s.kind == SegmentKind::Highway {
                0
            } else {
                traffic_value(congestion.get(&id).copied())
            }
        })
        .collect()
}

fn traffic_value(level: Option<Congestion>) -> u8 {
    match level.unwrap_or_default() {
        Congestion::Free => TRAFFIC_FREE,
        Congestion::Busy => TRAFFIC_BUSY,
        Congestion::Heavy => TRAFFIC_HEAVY,
        Congestion::Jammed => TRAFFIC_JAMMED,
    }
}

fn road_value(b: Balance) -> u8 {
    if b.capacity == 0 {
        SUPPLY_ROAD_NONE
    } else if b.enough() {
        SUPPLY_ROAD_ENOUGH
    } else {
        SUPPLY_ROAD_SHORT
    }
}

/// 電気と水道の、道路網につながった施設の供給量の合計と、需要に足りない網の数。
/// `[電気の供給量, 水道の供給量, 電気の足りない網, 水道の足りない網]`。
pub fn utility_totals(world: &World) -> [u64; 4] {
    let balance = supply_balance(world.network(), world.facilities(), world.buildings());
    let mut out = [0; 4];
    for c in &balance.components {
        for slot in 0..2 {
            out[slot] += c[slot].capacity;
            out[2 + slot] += u64::from(c[slot].demand > 0 && !c[slot].enough());
        }
    }
    out
}

/// 電気と水道の、主の網（需要がいちばん大きい網）の供給量と需要、主の網以外で施設か建物のある網の数。
/// `[電気の供給量, 水道の供給量, 需要, 電気のほかの網の数, 水道のほかの網の数]`。
/// どの網にも需要がなければ、主の網はなく、供給量と需要は 0 で、ほかの網は施設のある網すべてである。
pub fn utility_main(world: &World) -> [u64; 5] {
    let balance = supply_balance(world.network(), world.facilities(), world.buildings());
    let mut out = [0; 5];
    let main = balance.main_network(0);
    if let Some(i) = main {
        out[0] = balance.components[i][0].capacity;
        out[1] = balance.components[i][1].capacity;
        out[2] = balance.components[i][0].demand;
    }
    for (i, c) in balance.components.iter().enumerate() {
        for slot in 0..2 {
            out[3 + slot] +=
                u64::from(Some(i) != main && (c[slot].capacity > 0 || c[slot].demand > 0));
        }
    }
    out
}

/// 電気と水道の不足の中身。`[電気の施設のない網, 水道の施設のない網, 電気の不足量, 水道の不足量]`。
/// 施設のない網は、建物があるのに供給量が 0 の網の数である。
/// 不足量は、施設のある網で需要が供給量を上回った分の合計である。
pub fn utility_gaps(world: &World) -> [u64; 4] {
    let balance = supply_balance(world.network(), world.facilities(), world.buildings());
    gaps_of(&balance.components)
}

fn gaps_of(components: &[[Balance; 2]]) -> [u64; 4] {
    let mut out = [0; 4];
    for c in components {
        for slot in 0..2 {
            let b = c[slot];
            if b.demand > 0 && b.capacity == 0 {
                out[slot] += 1;
            } else {
                out[2 + slot] += b.demand.saturating_sub(b.capacity);
            }
        }
    }
    out
}

/// レイヤーごとに、最後に作った値の面を版と一緒に持つ。
#[derive(Default)]
pub struct LayerCache {
    built: BTreeMap<u8, (u64, LayerGrid)>,
}

fn layer(code: u8) -> Result<&'static DataLayer, JsError> {
    LAYERS
        .iter()
        .find(|l| l.code == code)
        .ok_or_else(|| JsError::new("知らないデータレイヤー"))
}

impl LayerCache {
    /// いまの版の値の面。版が変わっていれば作り直す。
    pub fn grid(&mut self, sim: &Sim, layer: &DataLayer) -> &LayerGrid {
        let version = (layer.version)(sim);
        if self
            .built
            .get(&layer.code)
            .is_none_or(|(v, _)| *v != version)
        {
            let grid = (layer.values)(sim.world());
            self.built.insert(layer.code, (version, grid));
        }
        &self.built[&layer.code].1
    }
}

#[wasm_bindgen]
impl Simulation {
    /// 建物と施設の表と、道路網の版。街の数字（`city_stats`）はこの版が変わったときだけ送り直す。
    pub fn supply_version(&self) -> f64 {
        (supply_version(&self.0) & ((1 << 52) - 1)) as f64
    }

    /// データレイヤーの版。レイヤーの番号も混ぜ、JS の数で誤差なく表せる下位 52 ビットにする。
    pub fn layer_version(&self, code: u8) -> Result<f64, JsError> {
        let layer = layer(code)?;
        let mut h = StateHasher::new();
        h.write(&code);
        h.write(&(layer.version)(&self.0));
        Ok((h.finish() & ((1 << 52) - 1)) as f64)
    }

    /// 交通の、区間ごとの値。並びは `road_segments` と同じで、版は `layer_version(3)` で見分ける。
    pub fn traffic_segments(&self) -> Vec<u8> {
        traffic_segment_values(self.0.world())
    }

    /// データレイヤーの、チャンクの 64×64 の値。
    pub fn layer_chunk(&mut self, code: u8, cx: i32, cy: i32) -> Result<Vec<u8>, JsError> {
        let layer = layer(code)?;
        Ok(self.1.grid(&self.0, layer).chunk(ChunkCoord::new(cx, cy)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::world::growth::GROWTH_INTERVAL;
    use chizu_core::{ApplyOutcome, Command, RoadKind, TileRect, ZoneKind};

    fn value_at(cache: &mut LayerCache, sim: &Sim, layer: &DataLayer, t: TilePos) -> u8 {
        let c = t.chunk().unwrap();
        let side = CHUNK_SIDE as i64;
        let i = (t.y - i64::from(c.cy) * side) * side + (t.x - i64::from(c.cx) * side);
        cache.grid(sim, layer).chunk(c)[i as usize]
    }

    #[test]
    fn power_layer_follows_the_core_supply() {
        let mut sim = Sim::new(0);
        sim.submit(Command::BuildRoad {
            from: TilePos::new(10, 10),
            to: TilePos::new(30, 10),
            kind: RoadKind::Small,
        })
        .unwrap();
        sim.submit(Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(10, 11), TilePos::new(30, 16)),
            zone: Some(ZoneKind::Residential),
        })
        .unwrap();
        sim.advance(GROWTH_INTERVAL as u32 + 1);
        let (_, house) = sim
            .world()
            .buildings()
            .iter()
            .next()
            .expect("住宅が建たない");
        let house = house.area.min;
        let power = &LAYERS[0];
        let mut cache = LayerCache::default();
        let road = TilePos::new(20, 10);
        assert_eq!(
            value_at(&mut cache, &sim, power, house),
            SUPPLY_BUILDING_OFF
        );
        assert_eq!(value_at(&mut cache, &sim, power, road), SUPPLY_ROAD_NONE);
        assert_eq!(value_at(&mut cache, &sim, power, TilePos::new(20, 5)), 0);
        assert_eq!(utility_totals(sim.world()), [0, 0, 1, 1]);
        assert_eq!(utility_gaps(sim.world()), [1, 1, 0, 0]);
        let before = sim.buildings_version();
        sim.submit(Command::PlaceBuilding {
            kind: ServiceKind::PowerPlant,
            at: TilePos::new(16, 9),
            rotation: 2,
        })
        .unwrap();
        assert_eq!(sim.take_reports()[2].outcome, ApplyOutcome::Applied);
        assert_ne!(sim.buildings_version(), before);
        assert_eq!(value_at(&mut cache, &sim, power, house), SUPPLY_BUILDING_ON);
        assert_eq!(value_at(&mut cache, &sim, power, road), SUPPLY_ROAD_ENOUGH);
        assert_eq!(
            value_at(&mut cache, &sim, power, TilePos::new(16, 9)),
            SUPPLY_FACILITY
        );
        // 水道のレイヤーでは、浄水場がないので届いていない
        let water = &LAYERS[1];
        assert_eq!(
            value_at(&mut cache, &sim, water, house),
            SUPPLY_BUILDING_OFF
        );
        assert_eq!(value_at(&mut cache, &sim, water, TilePos::new(16, 9)), 0);
        assert_eq!(
            utility_totals(sim.world()),
            [ServiceKind::PowerPlant.capacity(), 0, 0, 1]
        );
        // 発電所は足り、浄水場のない網だけが残る
        assert_eq!(utility_gaps(sim.world()), [0, 1, 0, 0]);
        assert_eq!(sim.city_stats()[12..16], [0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn traffic_segment_values_follow_the_road_segments_order() {
        let mut sim = Sim::new(0);
        sim.submit(Command::BuildRoad {
            from: TilePos::new(10, 10),
            to: TilePos::new(30, 10),
            kind: RoadKind::Small,
        })
        .unwrap();
        sim.submit(Command::BuildRoad {
            from: TilePos::new(20, 10),
            to: TilePos::new(20, 20),
            kind: RoadKind::Medium,
        })
        .unwrap();
        let values = traffic_segment_values(sim.world());
        let segments = sim.road_segments();
        assert_eq!(values.len() * 5, segments.len());
        let highways = segments
            .chunks(5)
            .filter(|s| s[4] == f64::from(crate::sim::segment_kind_code(SegmentKind::Highway)))
            .count();
        // 高速道路は 0、ほかの区間は車がいなければ「空いている」
        assert_eq!(values.iter().filter(|v| **v == 0).count(), highways);
        assert_eq!(
            values.iter().filter(|v| **v == TRAFFIC_FREE).count(),
            values.len() - highways
        );
    }

    #[test]
    fn traffic_layer_paints_roads_and_refreshes_on_an_interval() {
        let mut sim = Sim::new(0);
        sim.submit(Command::BuildRoad {
            from: TilePos::new(10, 10),
            to: TilePos::new(30, 10),
            kind: RoadKind::Small,
        })
        .unwrap();
        let traffic = &LAYERS[2];
        let mut cache = LayerCache::default();
        assert_eq!(
            value_at(&mut cache, &sim, traffic, TilePos::new(20, 10)),
            TRAFFIC_FREE
        );
        assert_eq!(value_at(&mut cache, &sim, traffic, TilePos::new(20, 9)), 0);
        // 同じ間隔の中では版が変わらず、間隔をまたぐと変わる
        let to_boundary = TRAFFIC_REFRESH_TICKS - sim.world().tick().0 % TRAFFIC_REFRESH_TICKS;
        sim.advance(to_boundary as u32);
        let v = (traffic.version)(&sim);
        sim.advance(TRAFFIC_REFRESH_TICKS as u32 - 1);
        assert_eq!((traffic.version)(&sim), v);
        sim.advance(1);
        assert_ne!((traffic.version)(&sim), v);
    }

    #[test]
    fn congestion_levels_map_to_values() {
        assert_eq!(traffic_value(None), TRAFFIC_FREE);
        assert_eq!(traffic_value(Some(Congestion::Busy)), TRAFFIC_BUSY);
        assert_eq!(traffic_value(Some(Congestion::Heavy)), TRAFFIC_HEAVY);
        assert_eq!(traffic_value(Some(Congestion::Jammed)), TRAFFIC_JAMMED);
    }

    #[test]
    fn joining_and_splitting_networks_changes_the_stats_version() {
        let mut sim = Sim::new(0);
        let road = |from: (i64, i64), to: (i64, i64)| Command::BuildRoad {
            from: TilePos::new(from.0, from.1),
            to: TilePos::new(to.0, to.1),
            kind: RoadKind::Small,
        };
        for (y, zone) in [(10, ZoneKind::Residential), (20, ZoneKind::Industrial)] {
            sim.submit(road((10, y), (30, y))).unwrap();
            sim.submit(Command::PaintZone {
                area: TileRect::from_corners(TilePos::new(10, y + 1), TilePos::new(25, y + 4)),
                zone: Some(zone),
            })
            .unwrap();
        }
        // 2 本の道路の両方に建物が建つまで進める
        for _ in 0..20 {
            sim.advance(GROWTH_INTERVAL as u32);
            let ys: Vec<i64> = sim
                .world()
                .buildings()
                .iter()
                .map(|(_, b)| b.area.min.y)
                .collect();
            if ys.iter().any(|y| *y < 20) && ys.iter().any(|y| *y > 20) {
                break;
            }
        }
        sim.advance(1);
        let short = |sim: &Sim| sim.city_stats()[10..12].to_vec();
        assert_eq!(short(&sim), [2.0, 2.0], "施設のない網が 2 つある");
        let (buildings, version) = (sim.buildings_version(), supply_version(&sim));
        // 2 本の端をつなぐと網は 1 つになる。建物の表は変わらないが、版は変わる
        sim.submit(road((30, 10), (30, 20))).unwrap();
        assert_eq!(short(&sim), [1.0, 1.0]);
        assert_eq!(sim.buildings_version(), buildings);
        assert_ne!(supply_version(&sim), version);
        // つなぎの区間を消すと、また 2 つに分かれ、版も変わる
        let joined = supply_version(&sim);
        sim.submit(Command::Bulldoze {
            area: TileRect::from_corners(TilePos::new(30, 12), TilePos::new(30, 18)),
        })
        .unwrap();
        assert_eq!(short(&sim), [2.0, 2.0]);
        assert_ne!(supply_version(&sim), joined);
    }

    #[test]
    fn gaps_count_isolated_networks_and_short_amounts() {
        let b = |capacity, demand| Balance { capacity, demand };
        let nets = [
            // 施設のない網（電気）と、水道は需要なし
            [b(0, 5), b(0, 0)],
            // 施設はあるが足りない網：電気は 30 - 10、水道は 7 - 4
            [b(10, 30), b(4, 7)],
            // 足りている網
            [b(10, 4), b(9, 9)],
            // 別の足りない網：電気は 8 - 5
            [b(5, 8), b(1, 0)],
        ];
        assert_eq!(gaps_of(&nets), [1, 0, 23, 3]);
        assert_eq!(gaps_of(&[]), [0; 4]);
    }

    #[test]
    fn short_network_counts_as_short() {
        let b = Balance {
            capacity: 10,
            demand: 11,
        };
        assert_eq!(road_value(b), SUPPLY_ROAD_SHORT);
        assert_eq!(road_value(Balance::default()), SUPPLY_ROAD_NONE);
    }
}
