//! データレイヤー。地図の上に重ねて描く、マスごとの値の面を作る。
//!
//! レイヤーは [`DataLayer`] の 1 つの型で表す。値の面を作る関数と、その値が変わったかを見分ける版の関数を
//! 差し替えれば、交通量や幸福度のレイヤーも同じ形で足せる。値の意味（色）は画面の側の
//! `web/src/map/layers.ts` が決める。

use std::collections::BTreeMap;

use chizu_core::service::{Balance, supply_balance};
use chizu_core::zone::tiles_of;
use chizu_core::{
    CHUNK_AREA, CHUNK_SIDE, ChunkCoord, SegmentKind, ServiceKind, StateHasher, TilePos, World,
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
];

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

/// 建物の表（供給の有無を含む）か道路網が変わると変わる。
fn supply_version(sim: &Sim) -> u64 {
    let mut h = StateHasher::new();
    h.write(&sim.buildings_version().to_bits());
    h.write(&sim.network_version());
    h.finish()
}

/// 建物は中核が判定して持っている供給の有無で、道路は網ごとの供給量と需要の釣り合いで塗る。
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
    /// データレイヤーの版。レイヤーの番号も混ぜ、JS の数で誤差なく表せる下位 52 ビットにする。
    pub fn layer_version(&self, code: u8) -> Result<f64, JsError> {
        let layer = layer(code)?;
        let mut h = StateHasher::new();
        h.write(&code);
        h.write(&(layer.version)(&self.0));
        Ok((h.finish() & ((1 << 52) - 1)) as f64)
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
        assert_eq!(utility_totals(sim.world()), [400, 0, 0, 1]);
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
