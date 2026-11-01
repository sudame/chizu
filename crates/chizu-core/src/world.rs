//! 街全体の状態。

use std::collections::BTreeMap;

use crate::building::Buildings;

use crate::chunk::{Chunk, Layer, TileUse};
use crate::citizens::{CitizenChange, CitizenRow, Population};
use crate::command::{Command, RoadKind, SimSpeed, ZoneKind};
use crate::id::{CitizenId, TripId};
use crate::movement::{Mover, SidewalkPos, Trips};
use crate::network::{Line, Network, TILE_MM, road_cost};
use crate::rng::{RngPurpose, mix_words};
use crate::routing::AstarRouter;
use crate::schedule::{bucket_at, update_citizen};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{CHUNK_SIDE, ChunkCoord, TilePos, TileRect, WORLD_LIMIT, local_index};
use crate::units::{Mm, Money, Tick, WorldPos};
use crate::zone::{around, near_road, tiles_of};

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
    /// 値は正しいが、いまの状態では受け付けられないので、何もしなかった。
    Denied(DenyReason),
}

/// コマンドを受け付けなかった理由。値は数えるときの表の添え字である。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DenyReason {
    /// 買っていない土地のマスを通る。
    NotOwned = 0,
    /// 水辺のマスを通る。
    Water = 1,
    /// 所持金が足りない。
    NotEnoughMoney = 2,
    /// 既存の区間と同じ 1 歩を通る。
    Overlap = 3,
    /// 斜めの区間とマスの角で交わる。
    CornerCrossing = 4,
    /// 高速道路の、内側の端のノード以外のマスを通る。
    HighwayMidway = 5,
    /// 消す区間がない。
    NothingToBulldoze = 6,
    /// 買う土地が、買った土地と辺を接していない。
    NotAdjacent = 7,
    /// 買う土地をもう買ってある。
    AlreadyOwned = 8,
    /// 区画を塗る長方形に、塗れるマス（道路から奥行きの範囲にある、買った陸の空きマス）がない。
    NoZonableTile = 9,
    /// 区画を塗っても消しても、変わるマスがない。
    ZoneUnchanged = 10,
}

impl DenyReason {
    pub const ALL: [DenyReason; 11] = [
        DenyReason::NotOwned,
        DenyReason::Water,
        DenyReason::NotEnoughMoney,
        DenyReason::Overlap,
        DenyReason::CornerCrossing,
        DenyReason::HighwayMidway,
        DenyReason::NothingToBulldoze,
        DenyReason::NotAdjacent,
        DenyReason::AlreadyOwned,
        DenyReason::NoZonableTile,
        DenyReason::ZoneUnchanged,
    ];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// 街全体の状態の入れ物。
///
/// 状態を変えるのは [`World::apply`] と [`World::step`] だけである。
/// tick T の状態は、tick が T 以下のコマンドを番号順に適用し、`step` を T 回済ませたものである。
/// 記録したコマンドは [`World::apply_recorded`] で適用し、どの番号まで適用したかを状態に残す。
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
    population: Population,
    buildings: Buildings,
    lots: growth::LotFronts,
    /// 適用済みの記録の次の番号。記録から続きを回すときは、この番号から適用する。
    next_seq: u64,
    trips: Trips,
    router: RouterSlot,
}

pub mod growth;

/// 網から作り直せる経路探索。状態に入れず、`World` どうしを比べるときも見ない。
#[derive(Clone, Debug)]
struct RouterSlot {
    router: AstarRouter,
}

impl PartialEq for RouterSlot {
    fn eq(&self, _: &RouterSlot) -> bool {
        true
    }
}

impl Eq for RouterSlot {}

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
    /// 所持金を `money` にして始める。テストとベンチマークで、広い土地を買い進めた街を作るのに使う。
    pub fn new_with_money(seed: u64, money: Money) -> World {
        World {
            money,
            ..World::new(seed)
        }
    }

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
        let router = RouterSlot {
            router: AstarRouter::new(&network),
        };
        let mut world = World {
            seed,
            tick: Tick::ZERO,
            speed: SimSpeed::NORMAL,
            money: INITIAL_MONEY,
            tax_permille: [INITIAL_TAX_PERMILLE; 4],
            chunks,
            chunks_bought: 0,
            network,
            population: Population::default(),
            buildings: Buildings::default(),
            lots: Default::default(),
            next_seq: 0,
            trips: Trips::default(),
            router,
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
        let touched = World::edited_tiles(&edits);
        for (coord, list) in edits {
            let chunk = self.chunks.get_mut(&coord).expect("チャンクがない");
            chunk.edit_layer(Layer::Ground, |tiles| {
                for (i, u) in list {
                    tiles.occupancy[i] = u;
                    // 道路を引いたマスの区画は消える。
                    if u == TileUse::Road {
                        tiles.zone[i] = None;
                    }
                }
            });
        }
        self.refresh_lot_fronts(touched);
    }

    /// マスの区画。メモリにないチャンクのマスは `None` である。
    pub fn zone_at(&self, t: TilePos) -> Option<ZoneKind> {
        let chunk = self.chunks.get(&t.chunk()?)?;
        let (lx, ly) = t.local();
        chunk.layer(Layer::Ground)?.zone[local_index(lx, ly)]
    }

    /// 区画の表をまとめて書き換え、変えたチャンクの版を上げる。
    fn write_zones(&mut self, edits: BTreeMap<ChunkCoord, Vec<(usize, Option<ZoneKind>)>>) {
        let touched = World::edited_tiles(&edits);
        for (coord, list) in edits {
            let chunk = self.chunks.get_mut(&coord).expect("チャンクがない");
            chunk.edit_layer(Layer::Ground, |tiles| {
                for (i, z) in list {
                    tiles.zone[i] = z;
                }
            });
        }
        self.refresh_lot_fronts(touched);
    }

    /// 長方形の中の、区画を塗れるマスに `zone` を塗る。`None` なら区画を消す。
    fn paint_zone(&mut self, area: TileRect, zone: Option<ZoneKind>) -> ApplyOutcome {
        let near = zone.map(|_| near_road(&self.network, area));
        let mut edits = BTreeMap::<ChunkCoord, Vec<(usize, Option<ZoneKind>)>>::new();
        let mut zonable = false;
        for (i, t) in tiles_of(area).enumerate() {
            let Some((coord, chunk)) = t.chunk().and_then(|c| self.chunks.get_key_value(&c)) else {
                continue;
            };
            let tiles = chunk.layer(Layer::Ground).expect("地上の層がない");
            let (lx, ly) = t.local();
            let idx = local_index(lx, ly);
            if let Some(near) = &near {
                let can = near[i]
                    && tiles.occupancy[idx] == TileUse::Empty
                    && self.check_buildable(t).is_ok();
                if !can {
                    continue;
                }
                zonable = true;
            }
            if tiles.zone[idx] != zone {
                edits.entry(*coord).or_default().push((idx, zone));
            }
        }
        if edits.is_empty() {
            let reason = if zone.is_some() && !zonable {
                DenyReason::NoZonableTile
            } else {
                DenyReason::ZoneUnchanged
            };
            return ApplyOutcome::Denied(reason);
        }
        self.write_zones(edits);
        self.refresh_buildings();
        ApplyOutcome::Applied
    }

    /// 長方形の中で、道路から奥行きの範囲を外れた区画を消す。
    fn prune_zones(&mut self, area: TileRect) {
        let near = near_road(&self.network, area);
        let mut edits = BTreeMap::<ChunkCoord, Vec<(usize, Option<ZoneKind>)>>::new();
        for (i, t) in tiles_of(area).enumerate() {
            if near[i] || self.zone_at(t).is_none() {
                continue;
            }
            let (lx, ly) = t.local();
            let coord = t.chunk().expect("区画のあるマスのチャンクがない");
            edits
                .entry(coord)
                .or_default()
                .push((local_index(lx, ly), None));
        }
        self.write_zones(edits);
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
        self.check_buildable(t).is_ok()
    }

    /// 道路を引けるマスか確かめ、引けないなら理由を返す。
    pub fn check_buildable(&self, t: TilePos) -> Result<(), DenyReason> {
        let chunk = t
            .chunk()
            .and_then(|c| self.chunks.get(&c))
            .filter(|c| c.owned())
            .ok_or(DenyReason::NotOwned)?;
        let (lx, ly) = t.local();
        if chunk.terrain().is_water(lx, ly) {
            return Err(DenyReason::Water);
        }
        Ok(())
    }

    fn build_road(&mut self, from: TilePos, to: TilePos, kind: RoadKind) -> ApplyOutcome {
        let Ok(line) = Line::between(from, to) else {
            return ApplyOutcome::Rejected;
        };
        if let Err(reason) = line.tiles().try_for_each(|t| self.check_buildable(t)) {
            return ApplyOutcome::Denied(reason);
        }
        let cost = road_cost(kind, &line);
        if self.money < cost {
            return ApplyOutcome::Denied(DenyReason::NotEnoughMoney);
        }
        if let Err(reason) = self.network.build_road(&line, kind) {
            return ApplyOutcome::Denied(reason);
        }
        self.money = self.money - cost;
        self.sync_road_tiles(line.tiles().collect::<Vec<_>>());
        self.refresh_buildings();
        ApplyOutcome::Applied
    }

    // 消した道路の建設費は払い戻さない。
    fn bulldoze(&mut self, area: TileRect) -> ApplyOutcome {
        let removed = match self.network.bulldoze(area) {
            Ok(removed) => removed,
            Err(reason) => return ApplyOutcome::Denied(reason),
        };
        self.sync_road_tiles(
            removed
                .iter()
                .flat_map(|l| l.tiles().collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        );
        // 消した道路にだけ面していた区画も消える。
        for line in &removed {
            self.prune_zones(around(line.start, line.end()));
        }
        self.refresh_buildings();
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

    pub fn trips(&self) -> &Trips {
        &self.trips
    }

    /// 網が変わっていたら経路探索を作り直し、通れなくなった移動を探し直す。
    fn sync_router(&mut self) {
        if self.router.router.sync(&self.network) {
            self.trips.reroute(&self.network, &mut self.router.router);
        }
    }

    /// `from` から `to` へ歩く移動を作る。住民の予定とつなぐまで、テストとシナリオから移動を作る入口にする。
    /// 出入口が歩道の上にないか、つながっていなければ `None`。
    pub fn spawn_walk(&mut self, from: SidewalkPos, to: SidewalkPos) -> Option<TripId> {
        self.sync_router();
        let now = self.tick;
        self.trips
            .spawn(&self.network, &mut self.router.router, from, to, now)
    }

    /// マスの範囲 `rect` にいる移動体を ID の順に返す。状態は変えない。
    pub fn movers_in(&self, rect: TileRect) -> Vec<Mover> {
        // 範囲はコマンドで指せる広さに切り詰める。その外にマスはないので、結果は変わらない。
        let edge = |t: i64| {
            let t = t.clamp(-WORLD_LIMIT, WORLD_LIMIT + 1);
            Mm(t.checked_mul(TILE_MM).expect("座標があふれた"))
        };
        let min = WorldPos::new(edge(rect.min.x), edge(rect.min.y));
        let max = WorldPos::new(
            edge(rect.max.x.saturating_add(1)),
            edge(rect.max.y.saturating_add(1)),
        );
        self.trips.movers_in(min, max)
    }

    /// 区画に建った建物。
    pub fn buildings(&self) -> &Buildings {
        &self.buildings
    }

    pub fn population(&self) -> &Population {
        &self.population
    }

    #[cfg(test)]
    pub(crate) fn population_mut(&mut self) -> &mut Population {
        &mut self.population
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
        self.check_buy_chunk(coord).is_ok()
    }

    /// チャンクを買えるか確かめ、買えないなら理由を返す。所持金は見ない。
    pub fn check_buy_chunk(&self, coord: ChunkCoord) -> Result<(), DenyReason> {
        if self.is_owned(coord) {
            Err(DenyReason::AlreadyOwned)
        } else if !coord.edge_neighbors().any(|n| self.is_owned(n)) {
            Err(DenyReason::NotAdjacent)
        } else {
            Ok(())
        }
    }

    // 人口のマイルストーンで買える枠が増える仕組みは、マイルストーンを入れるときに足す。
    fn buy_chunk(&mut self, coord: ChunkCoord) -> ApplyOutcome {
        let price = self.next_chunk_price();
        if let Err(reason) = self.check_buy_chunk(coord) {
            return ApplyOutcome::Denied(reason);
        }
        if self.money < price {
            return ApplyOutcome::Denied(DenyReason::NotEnoughMoney);
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

    /// 適用済みの記録の次の番号。
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// 記録の通し番号 `seq` のコマンドを適用し、適用済みの番号を進める。
    pub fn apply_recorded(&mut self, seq: u64, command: &Command) -> ApplyOutcome {
        assert!(seq >= self.next_seq, "記録 {seq} はもう適用してある");
        self.next_seq = seq.checked_add(1).expect("記録の番号があふれた");
        self.apply(command)
    }

    /// コマンドを 1 つ適用する。記録を通さないので、適用済みの番号は進めない。
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
            Command::PaintZone { area, zone } => self.paint_zone(area, zone),
            Command::PlaceBuilding { .. } | Command::SetSignal { .. } => {
                ApplyOutcome::NotImplemented
            }
        }
    }

    /// 1 tick 進める。
    pub fn step(&mut self) {
        self.step_with(update_citizen);
    }

    /// 住民の 1 人ぶんの処理を `update` にして 1 tick 進める。
    pub(crate) fn step_with<F>(&mut self, update: F)
    where
        F: Fn(u64, Tick, CitizenId, CitizenRow) -> Option<CitizenChange> + Sync,
    {
        self.sync_router();
        let (seed, tick) = (self.seed, self.tick);
        self.population
            .update_bucket(bucket_at(tick), |id, row| update(seed, tick, id, row));
        self.trips.step(&self.network);
        self.grow_buildings();
        self.tick = tick.next();
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
        h.write(&self.population);
        h.write(&self.trips);
        h.write(&self.buildings);
        h.write(&self.next_seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_reason_all_matches_the_index_and_every_variant() {
        for (i, reason) in DenyReason::ALL.iter().enumerate() {
            assert_eq!(reason.index(), i);
        }
        // 種類を足したら、ここの match がコンパイルできなくなり、ALL にも足すことを思い出させる。
        let listed = |r: DenyReason| match r {
            DenyReason::NotOwned
            | DenyReason::Water
            | DenyReason::NotEnoughMoney
            | DenyReason::Overlap
            | DenyReason::CornerCrossing
            | DenyReason::HighwayMidway
            | DenyReason::NothingToBulldoze
            | DenyReason::NotAdjacent
            | DenyReason::AlreadyOwned
            | DenyReason::NoZonableTile
            | DenyReason::ZoneUnchanged => DenyReason::ALL.contains(&r),
        };
        assert!(DenyReason::ALL.into_iter().all(listed));
        assert_eq!(DenyReason::ALL.len(), 11);
    }
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
        let version = w.network().version();
        assert_eq!(w.apply(&Command::Bulldoze { area }), ApplyOutcome::Applied);
        assert_eq!(w.network().version(), version + 1);
        assert_eq!(
            w.apply(&Command::Bulldoze { area }),
            ApplyOutcome::Denied(DenyReason::NothingToBulldoze)
        );
        assert_eq!(w.network().version(), version + 1);
        assert_eq!(w.money(), INITIAL_MONEY - Money(2_500));
    }

    fn tile_use(w: &World, x: i64, y: i64) -> TileUse {
        let t = TilePos::new(x, y);
        let (lx, ly) = t.local();
        let tiles = w.chunk(t.chunk().unwrap()).unwrap().layer(Layer::Ground);
        tiles.unwrap().occupancy[local_index(lx, ly)]
    }

    fn paint(w: &mut World, a: (i64, i64), b: (i64, i64), zone: Option<ZoneKind>) -> ApplyOutcome {
        let area = TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1));
        w.apply(&Command::PaintZone { area, zone })
    }

    #[test]
    fn zones_reach_six_tiles_from_roads() {
        let mut w = World::new(0);
        let res = Some(ZoneKind::Residential);
        assert_eq!(
            paint(&mut w, (0, 0), (10, 10), res),
            ApplyOutcome::Denied(DenyReason::NoZonableTile)
        );
        road(&mut w, (0, 0), (10, 0), RoadKind::Small);
        let origin = ChunkCoord::new(0, 0);
        let v = w.chunk(origin).unwrap().version();
        assert_eq!(
            paint(&mut w, (0, -10), (10, 10), res),
            ApplyOutcome::Applied
        );
        assert_eq!(w.chunk(origin).unwrap().version(), v + 1);
        assert_eq!(w.zone_at(TilePos::new(5, 6)), res);
        assert_eq!(w.zone_at(TilePos::new(10, 6)), res);
        assert_eq!(w.zone_at(TilePos::new(5, 7)), None);
        assert_eq!(
            w.zone_at(TilePos::new(5, 0)),
            None,
            "道路のマスには塗れない"
        );
        assert_eq!(
            paint(&mut w, (0, -10), (10, 10), res),
            ApplyOutcome::Denied(DenyReason::ZoneUnchanged)
        );
        // 区画のあるマスに道路を引くと、そのマスの区画は消える。
        road(&mut w, (3, 0), (3, 4), RoadKind::Small);
        assert_eq!(w.zone_at(TilePos::new(3, 2)), None);
        // 消す。
        assert_eq!(paint(&mut w, (0, 1), (1, 6), None), ApplyOutcome::Applied);
        assert_eq!(w.zone_at(TilePos::new(0, 1)), None);
        assert_eq!(w.zone_at(TilePos::new(2, 1)), res);
        assert_eq!(
            paint(&mut w, (0, 1), (1, 6), None),
            ApplyOutcome::Denied(DenyReason::ZoneUnchanged)
        );
    }

    #[test]
    fn zones_are_not_painted_off_the_owned_land_or_on_water() {
        let mut w = World::new(42);
        road(&mut w, (120, 0), (127, 0), RoadKind::Small);
        assert_eq!(
            paint(&mut w, (120, 1), (133, 1), Some(ZoneKind::Office)),
            ApplyOutcome::Applied
        );
        assert_eq!(w.zone_at(TilePos::new(127, 1)), Some(ZoneKind::Office));
        assert_eq!(w.zone_at(TilePos::new(128, 1)), None);
        // 水辺は is_buildable と同じ判定で弾く。
        assert!(w.check_buildable(TilePos::new(128, 1)).is_err());
    }

    #[test]
    fn bulldozing_a_road_removes_zones_that_only_faced_it() {
        let mut w = World::new(0);
        let res = Some(ZoneKind::Residential);
        road(&mut w, (0, 0), (20, 0), RoadKind::Small);
        road(&mut w, (0, 10), (20, 10), RoadKind::Small);
        assert_eq!(paint(&mut w, (0, -6), (20, 16), res), ApplyOutcome::Applied);
        let area = TileRect::from_corners(TilePos::new(5, 0), TilePos::new(5, 0));
        assert_eq!(w.apply(&Command::Bulldoze { area }), ApplyOutcome::Applied);
        // 消した道路にだけ面していたマスは消え、残った道路にも面するマスは残る。
        assert_eq!(w.zone_at(TilePos::new(5, -3)), None);
        assert_eq!(w.zone_at(TilePos::new(5, 3)), None);
        assert_eq!(w.zone_at(TilePos::new(5, 4)), res);
        assert_eq!(w.zone_at(TilePos::new(5, 14)), res);
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
                (0_u8..6, -8_i64..8, -8_i64..8, -1_i64..=1, -1_i64..=1, 1_i64..10),
                1..30,
            )
        ) {
            let mut w = World::new(0);
            for (op, x, y, sx, sy, len) in edits {
                if op == 0 {
                    let area = TileRect::from_corners(TilePos::new(x, y), TilePos::new(x + len / 2, y));
                    w.apply(&Command::Bulldoze { area });
                } else if op == 1 {
                    // 区画を塗るか消す。sx で種類を選ぶ。
                    let zone = [None, Some(ZoneKind::Residential), Some(ZoneKind::Office)][(sx + 1) as usize];
                    paint(&mut w, (x, y), (x + len, y + len), zone);
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
            // 区画は、道路から奥行きの範囲にある道路でないマスにだけある。
            let area = TileRect::from_corners(TilePos::new(-20, -20), TilePos::new(19, 19));
            let near = near_road(w.network(), area);
            for (i, t) in tiles_of(area).enumerate() {
                if w.zone_at(t).is_some() {
                    proptest::prop_assert!(near[i] && !w.network().is_road_tile(t), "{:?}", t);
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
            ApplyOutcome::Denied(DenyReason::NotOwned)
        );
        // 1 本で所持金を超える。
        w.money = Money(2_999);
        assert_eq!(
            road(&mut w, (-60, 0), (-56, 0), RoadKind::Large),
            ApplyOutcome::Denied(DenyReason::NotEnoughMoney)
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
            ApplyOutcome::Denied(DenyReason::Water)
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
        let owned = DenyReason::AlreadyOwned;
        let detached = DenyReason::NotAdjacent;
        for (cx, cy, reason) in [
            (0, 0, owned),
            (1, 1, owned),
            (3, 0, detached),
            (2, 2, detached),
            (-2, -2, detached),
        ] {
            let outcome = buy(&mut w, cx, cy);
            assert_eq!(outcome, ApplyOutcome::Denied(reason), "({cx}, {cy})");
        }
        // 世界の座標の範囲の外は、検証で弾く。
        assert_eq!(buy(&mut w, i32::MIN, 0), ApplyOutcome::Rejected);
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
                assert_eq!(
                    buy(&mut w, cx, 0),
                    ApplyOutcome::Denied(DenyReason::NotEnoughMoney)
                );
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
