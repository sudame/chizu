//! 街全体の状態。

use std::collections::BTreeMap;

use crate::chunk::{Chunk, Layer, TileUse};
use crate::command::{Command, RoadKind, SimSpeed, ZoneKind};
use crate::network::{Line, Network, road_cost};
use crate::rng::{RngPurpose, mix_words};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{CHUNK_SIDE, ChunkCoord, TilePos, TileRect, local_index};
use crate::units::{Money, Tick};

/// 始めたときの所持金。
pub const INITIAL_MONEY: Money = Money(500_000);

/// 始めたときの税率（‰）。
pub const INITIAL_TAX_PERMILLE: u16 = 100;

/// 始めたときに買ってあるチャンクの範囲。原点を中心とする 3×3 である。
pub const START_CHUNK_RADIUS: i32 = 1;

/// 最初に買うチャンクの値段。
pub const CHUNK_BASE_PRICE: Money = Money(50_000);

/// 1 つ買うごとに、次のチャンクの値段に足す額。
pub const CHUNK_PRICE_STEP: Money = Money(25_000);

/// 外部接続の高速道路が、始めたときに買ってある土地の端から外へ延びる長さ（マス）。
pub const HIGHWAY_OUTSIDE_TILES: u64 = 1_024;

/// コマンドを適用した結果。どの結果でも状態ハッシュには影響しない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// 状態に反映した。
    Applied,
    /// 中核にまだ中身がない種類なので、何もしなかった。
    NotImplemented,
    /// 値が受け付ける範囲の外なので、何もしなかった。記録を通したコマンドは検証済みなので、
    /// 記録を通さずに直接渡された不正なコマンドだけがここに来る。
    Rejected,
    /// 値は正しいが、いまの状態では受け付けられない（買えない土地、所持金の不足など）ので、何もしなかった。
    Denied,
}

/// 街全体の状態の入れ物。
///
/// 状態を変えるのは [`World::apply`] と [`World::step`] だけである。
/// 1 tick の進め方は、その tick に記録されたコマンドを番号順に `apply` してから `step` を呼ぶ。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct World {
    seed: u64,
    /// 次に進める tick。tick 0 から始め、`step` のたびに 1 増える。
    tick: Tick,
    speed: SimSpeed,
    money: Money,
    tax_permille: [u16; 4],
    /// メモリに置いているチャンク。いまは買った土地だけを置く。
    chunks: BTreeMap<ChunkCoord, Chunk>,
    /// 始めてから買ったチャンクの数。最初から買ってあるチャンクは数えない。
    chunks_bought: u32,
    network: Network,
}

/// 外部接続の高速道路の通り道。始めたときに買ってある土地の端のマスから外へ向かう。
/// どの辺から出るかと辺のどこから出るかは、種から決める。地形もこの通り道を見て水辺を抑える。
pub fn highway_line(seed: u64) -> Line {
    let side = CHUNK_SIDE as i64;
    let r = i64::from(START_CHUNK_RADIUS);
    let (min, max) = (-r * side, (r + 1) * side - 1);
    let h = mix_words(seed, RngPurpose::TrunkLine, &[0]);
    // 原点のチャンクの幅の中から出す。
    let along = (h >> 8) as i64 & (side - 1);
    let (start, step) = match h % 4 {
        0 => (TilePos::new(max, along), (1, 0)),
        1 => (TilePos::new(min, along), (-1, 0)),
        2 => (TilePos::new(along, max), (0, 1)),
        _ => (TilePos::new(along, min), (0, -1)),
    };
    Line {
        start,
        step,
        len: HIGHWAY_OUTSIDE_TILES,
    }
}

impl World {
    pub fn new(seed: u64) -> World {
        let r = START_CHUNK_RADIUS;
        let chunks = (-r..=r)
            .flat_map(|cy| (-r..=r).map(move |cx| ChunkCoord::new(cx, cy)))
            .map(|coord| {
                let mut chunk = Chunk::generate(seed, coord);
                chunk.set_owned();
                (coord, chunk)
            })
            .collect();
        let mut network = Network::new();
        let highway = highway_line(seed);
        network.add_highway(highway.start, highway.end());
        let mut world = World {
            seed,
            tick: Tick::ZERO,
            speed: SimSpeed::NORMAL,
            money: INITIAL_MONEY,
            tax_permille: [INITIAL_TAX_PERMILLE; 4],
            chunks,
            chunks_bought: 0,
            network,
        };
        let coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
        for coord in coords {
            world.sync_chunk_roads(coord);
        }
        world
    }

    /// マスの使い道を網に合わせて書き直し、変えたチャンクの版を上げる。メモリにないチャンクのマスは飛ばす。
    fn sync_road_tiles(&mut self, tiles: impl IntoIterator<Item = TilePos>) {
        let mut edits = BTreeMap::<ChunkCoord, Vec<(usize, TileUse)>>::new();
        for t in tiles {
            let Some(coord) = t.chunk().filter(|c| self.chunks.contains_key(c)) else {
                continue;
            };
            let (lx, ly) = t.local();
            let u = if self.network.is_road_tile(t) {
                TileUse::Road
            } else {
                TileUse::Empty
            };
            edits
                .entry(coord)
                .or_default()
                .push((local_index(lx, ly), u));
        }
        for (coord, list) in edits {
            let chunk = self.chunks.get_mut(&coord).expect("チャンクがない");
            chunk.edit_layer(Layer::Ground, |tiles| {
                for (i, u) in list {
                    tiles.occupancy[i] = u;
                }
            });
        }
    }

    /// メモリに置いたばかりのチャンクに、そこを通る道路を書く。
    fn sync_chunk_roads(&mut self, coord: ChunkCoord) {
        let tiles: Vec<TilePos> = self
            .network
            .segments_in_chunk(coord)
            .iter()
            .filter_map(|s| self.network.segment_line(*s))
            .flat_map(|l| l.tiles().collect::<Vec<_>>())
            .filter(|t| t.chunk() == Some(coord))
            .collect();
        self.sync_road_tiles(tiles);
    }

    pub fn network(&self) -> &Network {
        &self.network
    }

    /// 道路を引けるマスか。買った土地で、水辺でないマスである。
    pub fn is_buildable(&self, t: TilePos) -> bool {
        t.chunk()
            .and_then(|c| self.chunks.get(&c))
            .is_some_and(|c| {
                let (lx, ly) = t.local();
                c.owned() && !c.terrain().is_water(lx, ly)
            })
    }

    fn build_road(&mut self, from: TilePos, to: TilePos, kind: RoadKind) -> ApplyOutcome {
        let Ok(line) = Line::between(from, to) else {
            return ApplyOutcome::Rejected;
        };
        if !line.tiles().all(|t| self.is_buildable(t)) {
            return ApplyOutcome::Denied;
        }
        let cost = road_cost(kind, &line);
        if self.money < cost || self.network.build_road(&line, kind).is_err() {
            return ApplyOutcome::Denied;
        }
        self.money = self.money - cost;
        self.sync_road_tiles(line.tiles().collect::<Vec<_>>());
        ApplyOutcome::Applied
    }

    // 消した道路の建設費は払い戻さない。
    fn bulldoze(&mut self, area: TileRect) -> ApplyOutcome {
        let removed = self.network.bulldoze(area);
        if removed.is_empty() {
            return ApplyOutcome::Denied;
        }
        self.sync_road_tiles(
            removed
                .iter()
                .flat_map(|l| l.tiles().collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        );
        ApplyOutcome::Applied
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// 次に進める tick。これまでに進めた tick 数でもある。
    pub fn tick(&self) -> Tick {
        self.tick
    }

    pub fn speed(&self) -> SimSpeed {
        self.speed
    }

    pub fn money(&self) -> Money {
        self.money
    }

    pub fn tax_permille(&self, zone: ZoneKind) -> u16 {
        self.tax_permille[zone.index()]
    }

    pub fn chunk(&self, coord: ChunkCoord) -> Option<&Chunk> {
        self.chunks.get(&coord)
    }

    /// メモリに置いているチャンクを座標の順に返す。
    pub fn chunks(&self) -> impl Iterator<Item = &Chunk> {
        self.chunks.values()
    }

    pub fn is_owned(&self, coord: ChunkCoord) -> bool {
        self.chunk(coord).is_some_and(|c| c.owned())
    }

    /// 次に買うチャンクの値段。買うたびに一定額ずつ上がる。
    pub fn next_chunk_price(&self) -> Money {
        CHUNK_BASE_PRICE + Money(CHUNK_PRICE_STEP.0 * i64::from(self.chunks_bought))
    }

    /// まだ買っておらず、買った土地と辺を接するチャンクか。
    pub fn can_buy_chunk(&self, coord: ChunkCoord) -> bool {
        !self.is_owned(coord) && coord.edge_neighbors().any(|n| self.is_owned(n))
    }

    // 人口のマイルストーンで買える枠が増える仕組みは、マイルストーンを入れるときに足す。
    fn buy_chunk(&mut self, coord: ChunkCoord) -> ApplyOutcome {
        let price = self.next_chunk_price();
        if !self.can_buy_chunk(coord) || self.money < price {
            return ApplyOutcome::Denied;
        }
        self.money = self.money - price;
        self.chunks_bought += 1;
        let fresh = !self.chunks.contains_key(&coord);
        let seed = self.seed;
        self.chunks
            .entry(coord)
            .or_insert_with(|| Chunk::generate(seed, coord))
            .set_owned();
        if fresh {
            self.sync_chunk_roads(coord);
        }
        ApplyOutcome::Applied
    }

    /// コマンドを 1 つ適用する。いまの tick の頭で呼ぶ。
    pub fn apply(&mut self, command: &Command) -> ApplyOutcome {
        if command.validate().is_err() {
            return ApplyOutcome::Rejected;
        }
        match *command {
            Command::SetSimSpeed { speed } => {
                self.speed = speed;
                ApplyOutcome::Applied
            }
            Command::SetTaxRate { zone, permille } => {
                self.tax_permille[zone.index()] = permille;
                ApplyOutcome::Applied
            }
            Command::BuyChunk { coord } => self.buy_chunk(coord),
            Command::BuildRoad { from, to, kind } => self.build_road(from, to, kind),
            Command::Bulldoze { area } => self.bulldoze(area),
            Command::PaintZone { .. }
            | Command::PlaceBuilding { .. }
            | Command::SetSignal { .. } => ApplyOutcome::NotImplemented,
        }
    }

    /// 1 tick 進める。
    pub fn step(&mut self) {
        self.tick = self.tick.next();
    }

    /// 状態ハッシュ。表は決まった順番で書き出す。
    pub fn state_hash(&self) -> u64 {
        let mut h = StateHasher::new();
        self.write_state(&mut h);
        h.finish()
    }
}

impl StateHash for World {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.seed);
        h.write(&self.tick);
        h.write(&self.speed.multiplier());
        h.write(&self.money);
        h.write(&self.tax_permille);
        h.write(&self.chunks);
        h.write(&self.chunks_bought);
        h.write(&self.network);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{MAX_TAX_PERMILLE, NodeId};

    #[test]
    fn step_advances_tick() {
        let mut w = World::new(1);
        assert_eq!(w.tick(), Tick::ZERO);
        w.step();
        w.step();
        assert_eq!(w.tick(), Tick(2));
    }

    #[test]
    fn hash_depends_on_seed_and_tick() {
        let a = World::new(1);
        let b = World::new(2);
        assert_ne!(a.state_hash(), b.state_hash());
        let mut c = a.clone();
        c.step();
        assert_ne!(a.state_hash(), c.state_hash());
    }

    #[test]
    fn set_tax_rate() {
        let mut w = World::new(0);
        let before = w.state_hash();
        let cmd = Command::SetTaxRate {
            zone: ZoneKind::Office,
            permille: 150,
        };
        assert_eq!(w.apply(&cmd), ApplyOutcome::Applied);
        assert_eq!(w.tax_permille(ZoneKind::Office), 150);
        assert_eq!(w.tax_permille(ZoneKind::Residential), INITIAL_TAX_PERMILLE);
        assert_ne!(w.state_hash(), before);
    }

    #[test]
    fn out_of_range_tax_rate_is_rejected() {
        let mut w = World::new(0);
        let before = w.clone();
        let cmd = Command::SetTaxRate {
            zone: ZoneKind::Commercial,
            permille: MAX_TAX_PERMILLE + 1,
        };
        assert_eq!(w.apply(&cmd), ApplyOutcome::Rejected);
        assert_eq!(w, before);
    }

    #[test]
    fn set_sim_speed() {
        let mut w = World::new(0);
        let before = w.state_hash();
        let speed = SimSpeed::new(8).unwrap();
        assert_eq!(
            w.apply(&Command::SetSimSpeed { speed }),
            ApplyOutcome::Applied
        );
        assert_eq!(w.speed(), speed);
        assert_ne!(w.state_hash(), before);
    }

    #[test]
    fn not_implemented_commands_leave_state_unchanged() {
        let mut w = World::new(0);
        let before = w.clone();
        let cmd = Command::SetSignal {
            node: NodeId {
                index: 0,
                generation: 0,
            },
            on: true,
        };
        assert_eq!(w.apply(&cmd), ApplyOutcome::NotImplemented);
        assert_eq!(w, before);
    }

    fn road(w: &mut World, from: (i64, i64), to: (i64, i64), kind: RoadKind) -> ApplyOutcome {
        w.apply(&Command::BuildRoad {
            from: TilePos::new(from.0, from.1),
            to: TilePos::new(to.0, to.1),
            kind,
        })
    }

    #[test]
    fn starts_with_a_highway_from_the_edge_of_the_owned_land() {
        let mut sides = std::collections::BTreeSet::new();
        for seed in 0..32 {
            let w = World::new(seed);
            let (_, seg) = w.network().segments().next().unwrap();
            let inner = w.network().node(seg.a).unwrap();
            let outer = w.network().node(seg.b).unwrap();
            assert!(!inner.outside && outer.outside);
            assert!(w.is_buildable(inner.tile()));
            let line = w
                .network()
                .segment_line(w.network().segments().next().unwrap().0);
            let line = line.unwrap();
            assert!(!w.is_buildable(line.tile(1)));
            assert_eq!(line, highway_line(seed));
            sides.insert(line.step);
        }
        assert_eq!(sides.len(), 4, "種によって出る辺が変わる");
    }

    #[test]
    fn building_a_road_charges_its_cost() {
        let mut w = World::new(0);
        let before = w.state_hash();
        assert_eq!(
            road(&mut w, (0, 0), (10, 0), RoadKind::Small),
            ApplyOutcome::Applied
        );
        assert_eq!(w.money(), INITIAL_MONEY - Money(2_500));
        assert_ne!(w.state_hash(), before);
        assert_eq!(w.network().segments().count(), 2);
        let area = TileRect::from_corners(TilePos::new(3, 0), TilePos::new(3, 0));
        assert_eq!(w.apply(&Command::Bulldoze { area }), ApplyOutcome::Applied);
        assert_eq!(w.apply(&Command::Bulldoze { area }), ApplyOutcome::Denied);
        assert_eq!(w.money(), INITIAL_MONEY - Money(2_500));
    }

    fn tile_use(w: &World, x: i64, y: i64) -> TileUse {
        let t = TilePos::new(x, y);
        let (lx, ly) = t.local();
        let tiles = w.chunk(t.chunk().unwrap()).unwrap().layer(Layer::Ground);
        tiles.unwrap().occupancy[local_index(lx, ly)]
    }

    #[test]
    fn road_tiles_follow_the_network_and_bump_the_chunk_version() {
        let mut w = World::new(0);
        let origin = ChunkCoord::new(0, 0);
        let v0 = w.chunk(origin).unwrap().version();
        road(&mut w, (0, 5), (10, 5), RoadKind::Small);
        road(&mut w, (5, 5), (5, 10), RoadKind::Small);
        assert_eq!(w.chunk(origin).unwrap().version(), v0 + 2);
        for (x, y) in [(0, 5), (7, 5), (5, 5), (5, 10)] {
            assert_eq!(tile_use(&w, x, y), TileUse::Road, "({x}, {y})");
        }
        assert_eq!(tile_use(&w, 6, 6), TileUse::Empty);
        let area = TileRect::from_corners(TilePos::new(5, 8), TilePos::new(5, 8));
        assert_eq!(w.apply(&Command::Bulldoze { area }), ApplyOutcome::Applied);
        assert_eq!(w.chunk(origin).unwrap().version(), v0 + 3);
        assert_eq!(tile_use(&w, 5, 8), TileUse::Empty);
        // 残った区間の端のマスは道路のままである。
        assert_eq!(tile_use(&w, 5, 5), TileUse::Road);
    }

    #[test]
    fn highway_tiles_are_written_when_their_chunk_is_bought() {
        let w = World::new(0);
        let (id, _) = w.network().segments().next().unwrap();
        let line = w.network().segment_line(id).unwrap();
        let inner = line.start;
        assert_eq!(tile_use(&w, inner.x, inner.y), TileUse::Road);
        // 高速道路が延びる先の隣のチャンクを買う。
        let next = line.tile(1);
        let coord = next.chunk().unwrap();
        let mut w = w;
        assert_eq!(buy(&mut w, coord.cx, coord.cy), ApplyOutcome::Applied);
        assert_eq!(tile_use(&w, next.x, next.y), TileUse::Road);
        assert_eq!(
            tile_use(&w, line.tile(64).x, line.tile(64).y),
            TileUse::Road
        );
    }

    proptest::proptest! {
        /// 引く・消すを任意に並べても、チャンクの境目のまわりのマスの使い道が網と一致する。
        #[test]
        fn road_tiles_match_the_network(
            edits in proptest::collection::vec(
                (0_u8..4, -8_i64..8, -8_i64..8, -1_i64..=1, -1_i64..=1, 1_i64..10),
                1..30,
            )
        ) {
            let mut w = World::new(0);
            for (op, x, y, sx, sy, len) in edits {
                if op == 0 {
                    let area = TileRect::from_corners(TilePos::new(x, y), TilePos::new(x + len / 2, y));
                    w.apply(&Command::Bulldoze { area });
                } else if (sx, sy) != (0, 0) {
                    road(&mut w, (x, y), (x + sx * len, y + sy * len), RoadKind::Small);
                }
            }
            for x in -20..20 {
                for y in -20..20 {
                    let expected = if w.network().is_road_tile(TilePos::new(x, y)) {
                        TileUse::Road
                    } else {
                        TileUse::Empty
                    };
                    proptest::prop_assert_eq!(tile_use(&w, x, y), expected, "({}, {})", x, y);
                }
            }
        }

        /// 高速道路の内側のノードのまわりで、引く・消す・高速道路の先のチャンクを買うを任意に並べても、
        /// メモリにあるマスの使い道が網と一致する。
        #[test]
        fn road_tiles_match_the_network_around_the_highway(
            seed in 0_u64..8,
            edits in proptest::collection::vec(
                (0_u8..6, -8_i64..8, -8_i64..8, -1_i64..=1, -1_i64..=1, 1_i64..10),
                1..30,
            )
        ) {
            let mut w = World::new(seed);
            w.money = Money(1_000_000_000);
            let highway = highway_line(seed);
            let (inner, (hx, hy)) = (highway.start, highway.step);
            for (op, dx, dy, sx, sy, len) in edits {
                let (x, y) = (inner.x + dx, inner.y + dy);
                match op {
                    0 => {
                        let area = TileRect::from_corners(TilePos::new(x, y), TilePos::new(x + len / 2, y + len / 2));
                        w.apply(&Command::Bulldoze { area });
                    }
                    1 => {
                        // 高速道路が通る、外側の k 番めのチャンクを買う。
                        let k = u64::try_from(dx.rem_euclid(3)).unwrap();
                        let c = highway.tile(1 + 64 * k).chunk().unwrap();
                        buy(&mut w, c.cx, c.cy);
                    }
                    2 => {
                        // 内側のノードから、土地の内へ引く。
                        let to = (inner.x - hx * len, inner.y - hy * len);
                        road(&mut w, (inner.x, inner.y), to, RoadKind::Medium);
                    }
                    _ if (sx, sy) != (0, 0) => {
                        road(&mut w, (x, y), (x + sx * len, y + sy * len), RoadKind::Small);
                    }
                    _ => {}
                }
            }
            let near = (-20..20).flat_map(|dx| (-20..20).map(move |dy| TilePos::new(inner.x + dx, inner.y + dy)));
            let along = (0..200).map(|k| highway.tile(k));
            for t in near.chain(along) {
                if t.chunk().and_then(|c| w.chunk(c)).is_none() {
                    continue;
                }
                let expected = if w.network().is_road_tile(t) {
                    TileUse::Road
                } else {
                    TileUse::Empty
                };
                proptest::prop_assert_eq!(tile_use(&w, t.x, t.y), expected, "{:?}", t);
            }
            let (id, _) = w.network().segments().next().unwrap();
            proptest::prop_assert_eq!(w.network().segment_line(id), Some(highway));
        }
    }

    #[test]
    fn roads_are_denied_off_the_owned_land_on_water_and_without_money() {
        let mut w = World::new(42);
        let before = w.clone();
        // 買った土地の端から 1 マスはみ出す。
        assert_eq!(
            road(&mut w, (100, 0), (128, 0), RoadKind::Small),
            ApplyOutcome::Denied
        );
        // 1 本で所持金を超える。
        w.money = Money(2_999);
        assert_eq!(
            road(&mut w, (-60, 0), (-56, 0), RoadKind::Large),
            ApplyOutcome::Denied
        );
        w.money = Money(3_000);
        assert_eq!(
            road(&mut w, (-60, 0), (-56, 0), RoadKind::Large),
            ApplyOutcome::Applied
        );
        assert_eq!(w.money(), Money::ZERO);
        w = before;

        // 水辺のある土地を買い、水辺のマスを通る道路を引こうとする。
        w.money = Money(1_000_000_000);
        let coord = (2..40)
            .map(|cx| ChunkCoord::new(cx, 0))
            .find(|c| {
                assert_eq!(buy(&mut w, c.cx, c.cy), ApplyOutcome::Applied);
                w.chunk(*c).unwrap().terrain().water_count() > 0
            })
            .expect("水辺のある土地が見つからない");
        let o = coord.origin();
        let water = (0..CHUNK_SIDE * CHUNK_SIDE)
            .map(|i| (i % CHUNK_SIDE, i / CHUNK_SIDE))
            .find(|(lx, ly)| w.chunk(coord).unwrap().terrain().is_water(*lx, *ly))
            .unwrap();
        let t = (o.x + water.0 as i64, o.y + water.1 as i64);
        let before = w.clone();
        assert_eq!(
            road(&mut w, (t.0 - 1, t.1), (t.0, t.1), RoadKind::Small),
            ApplyOutcome::Denied
        );
        assert_eq!(w, before);
    }

    fn buy(w: &mut World, cx: i32, cy: i32) -> ApplyOutcome {
        w.apply(&Command::BuyChunk {
            coord: ChunkCoord::new(cx, cy),
        })
    }

    #[test]
    fn starts_with_the_chunks_around_the_origin() {
        let w = World::new(5);
        let owned: Vec<_> = w.chunks().filter(|c| c.owned()).collect();
        assert_eq!(owned.len(), 9);
        for cx in -1..=1 {
            for cy in -1..=1 {
                assert!(w.is_owned(ChunkCoord::new(cx, cy)));
            }
        }
        assert!(!w.is_owned(ChunkCoord::new(2, 0)));
        assert_eq!(w.next_chunk_price(), CHUNK_BASE_PRICE);
    }

    #[test]
    fn buying_an_adjacent_chunk_pays_and_generates_its_terrain() {
        let mut w = World::new(5);
        let before = w.state_hash();
        assert_eq!(buy(&mut w, 2, 0), ApplyOutcome::Applied);
        assert_eq!(w.money(), INITIAL_MONEY - CHUNK_BASE_PRICE);
        let coord = ChunkCoord::new(2, 0);
        let c = w.chunk(coord).unwrap();
        assert!(c.owned());
        assert_eq!(c.version(), 1);
        assert_eq!(*c.terrain(), crate::terrain::terrain(5, coord));
        assert_ne!(w.state_hash(), before);
        // 買った土地の隣は、さらに買える。
        assert_eq!(w.next_chunk_price(), CHUNK_BASE_PRICE + CHUNK_PRICE_STEP);
        assert_eq!(buy(&mut w, 3, 0), ApplyOutcome::Applied);
    }

    #[test]
    fn buying_is_denied_for_owned_detached_or_diagonal_chunks() {
        let mut w = World::new(5);
        let before = w.clone();
        for (cx, cy) in [(0, 0), (1, 1), (3, 0), (2, 2), (-2, -2), (i32::MIN, 0)] {
            assert_eq!(buy(&mut w, cx, cy), ApplyOutcome::Denied, "({cx}, {cy})");
        }
        assert_eq!(w, before);
    }

    #[test]
    fn buying_is_denied_without_enough_money() {
        let mut w = World::new(5);
        let mut spent = Money::ZERO;
        let mut bought = 0;
        // 原点から右へ一列に買い進め、所持金が尽きたところで止まる。
        for cx in 2..100 {
            let price = w.next_chunk_price();
            if w.money() < price {
                let before = w.clone();
                assert_eq!(buy(&mut w, cx, 0), ApplyOutcome::Denied);
                assert_eq!(w, before);
                break;
            }
            assert_eq!(buy(&mut w, cx, 0), ApplyOutcome::Applied);
            spent = spent + price;
            bought += 1;
        }
        assert!(bought > 0);
        assert_eq!(w.money(), INITIAL_MONEY - spent);
        assert!(w.money() < w.next_chunk_price());
    }
}
