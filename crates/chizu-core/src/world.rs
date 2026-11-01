//! 街全体の状態。

use std::collections::BTreeMap;

use crate::chunk::Chunk;
use crate::citizens::{CitizenChange, CitizenRow, Population};
use crate::command::{Command, SimSpeed, ZoneKind};
use crate::id::CitizenId;
use crate::schedule::{bucket_at, update_citizen};
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::ChunkCoord;
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
    population: Population,
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
        World {
            seed,
            tick: Tick::ZERO,
            speed: SimSpeed::NORMAL,
            money: INITIAL_MONEY,
            tax_permille: [INITIAL_TAX_PERMILLE; 4],
            chunks,
            chunks_bought: 0,
            population: Population::default(),
        }
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
        let seed = self.seed;
        self.chunks
            .entry(coord)
            .or_insert_with(|| Chunk::generate(seed, coord))
            .set_owned();
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
            Command::BuildRoad { .. }
            | Command::Bulldoze { .. }
            | Command::PaintZone { .. }
            | Command::PlaceBuilding { .. }
            | Command::SetSignal { .. } => ApplyOutcome::NotImplemented,
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
        let (seed, tick) = (self.seed, self.tick);
        self.population
            .update_bucket(bucket_at(tick), |id, row| update(seed, tick, id, row));
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
        h.write(&self.population);
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
