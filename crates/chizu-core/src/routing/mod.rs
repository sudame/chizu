//! 経路探索。
//!
//! 網から車と歩行者のグラフを作り、A* か、根を共有する依頼を束ねた 1 対多の探索で解く。
//! 網が変わったら [`Router::rebuild`] でグラフを作り直し、覚えた結果も捨てる。探索の結果は状態に入れない。同じ網と同じ依頼からは、
//! 覚えた結果の有無やスレッドの数に関わらず同じ経路を返す。
//!
//! 碁盤の目の街には、費用の同じ経路がたくさんある。全員が同じ 1 本を選ぶと、その道だけが混む。そこで車の経路は、
//! 1 回の探索から費用の同じ経路を [`ROUTE_VARIANTS`] 本の候補として取り出し、依頼の `variant` で選ぶ。
//! 費用は変えないので、ヒューリスティックは下限のままで、覚える結果の数も探す回数も変わらない。

mod astar;
mod delay;
mod graph;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use astar::{Marks, extract, search_many};
pub use astar::{Route, Scratch, Toward, astar, astar_variants, one_to_many};
pub use delay::Delays;
pub use graph::{
    Connector, Graph, GraphError, WALK_SPEED, connectors, speed_limit, try_connectors,
};

use crate::network::{LaneMode, LinkId, Network};
use crate::units::LaneOffset;

/// 1 日を分ける時間帯の数。重みは時間帯ごとに写す。
pub const TIME_BANDS: u8 = 96;

/// 1 つの時間帯の tick 数。
pub const BAND_TICKS: u64 = crate::clock::TICKS_PER_DAY / TIME_BANDS as u64;

/// 1 対多の探索に束ねる依頼の数の既定の下限。これより少ない組は A* で解く。
pub const DEFAULT_BUNDLE_MIN: usize = 4;

/// 1 つの組の経路を、スレッドに分けて取り出し始める依頼の数。
#[cfg(feature = "parallel")]
const PARALLEL_EXTRACT_MIN: usize = 256;

/// 覚えておく結果の大きさの既定の上限。経路のリンクの数の和で数え、鍵ごとに 1 を足す。共有した経路は 1 回だけ数える。
/// 超えそうになったら、新しい結果を入れる前に全部捨てる。鍵の数で数えないのは、車の鍵が最大で
/// [`ROUTE_VARIANTS`] 本の経路を持ち、鍵あたりの大きさが鍵によって大きく違うためである。
pub const DEFAULT_CACHE_LIMIT: usize = 1 << 22;

/// 経路があるかを覚える数の上限。超えそうになったら、先に全部捨てる。
const REACH_LIMIT: usize = 1 << 18;

/// 車の経路の、費用の同じ経路から選ぶ候補の数。
pub const ROUTE_VARIANTS: u8 = 8;

/// 手段 `mode` の経路の候補の数。歩く経路は散らさず、1 本にする。
pub fn variants_of(mode: LaneMode) -> u8 {
    match mode {
        LaneMode::Car => ROUTE_VARIANTS,
        LaneMode::Sidewalk => 1,
    }
}

/// 探索の依頼。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteRequest {
    pub from: LinkId,
    pub to: LinkId,
    /// 車で探すなら `Car`、歩いて探すなら `Sidewalk`。
    pub mode: LaneMode,
    /// 費用の同じ経路のうち、どの候補を選ぶか。手段の候補の数（[`variants_of`]）で割った余りを使う。
    pub variant: u8,
}

impl RouteRequest {
    fn key(&self) -> Key {
        Key {
            from: self.from,
            to: self.to,
            mode: self.mode,
        }
    }
}

/// 探して覚える単位。候補は 1 回の探索からまとめて取り出すので、鍵に入れない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    from: LinkId,
    to: LinkId,
    mode: LaneMode,
}

/// 1 つの鍵の、候補の順に並べた経路。同じ経路は 1 つを共有する。
type Variants = Arc<[Arc<Route>]>;

/// 覚えた 1 つの結果の大きさ。鍵ごとの 1 に、持っている経路のリンクの数を足す。共有した経路は 1 回だけ数える。
fn size_of_entry(v: &Option<Variants>) -> usize {
    let routes = v.as_deref().unwrap_or_default();
    let links: usize = routes
        .iter()
        .enumerate()
        .filter(|(i, r)| !routes[..*i].iter().any(|o| Arc::ptr_eq(o, r)))
        .map(|(_, r)| r.links.len())
        .sum();
    1 + links
}

/// 候補の順に並べた経路から [`Variants`] を作る。
fn share(routes: Vec<Route>) -> Variants {
    let mut out: Vec<Arc<Route>> = Vec::with_capacity(routes.len());
    for r in routes {
        let same = out.iter().find(|o| ***o == r).cloned();
        out.push(same.unwrap_or_else(|| Arc::new(r)));
    }
    out.into()
}

/// 探索の方式。A* と結果の再利用のほか、出発地の束ねや CCH に差し替えられるようにする。
pub trait Router {
    /// 網からグラフを作り直す。覚えた結果も捨てる。
    fn rebuild(&mut self, net: &Network);

    /// 重みを写す時間帯を変える。時間帯が変わったら覚えた結果を捨てる。
    fn set_band(&mut self, band: u8);

    /// 依頼をまとめて解き、依頼の順に結果を返す。
    fn route_batch(&mut self, requests: &[RouteRequest]) -> Vec<Option<Arc<Route>>>;
}

/// 手段ごとの作業領域。車と歩行者でグラフの頂点の数が違うので、手段が替わるたびに作り直さないよう 1 組ずつ持つ。
#[derive(Clone, Debug, Default)]
struct Scratches {
    car: Scratch,
    walk: Scratch,
}

impl Scratches {
    fn get(&mut self, mode: LaneMode) -> &mut Scratch {
        match mode {
            LaneMode::Car => &mut self.car,
            LaneMode::Sidewalk => &mut self.walk,
        }
    }
}

/// A* か 1 対多の探索で解き、`(出発のリンク, 着くリンク, 手段)` ごとに結果を覚える方式。
/// 時間帯は覚える範囲を区切る。
#[derive(Clone, Debug)]
pub struct AstarRouter {
    /// グラフを作ったときの網の版。
    version: u64,
    car: Graph,
    walk: Graph,
    connectors: Vec<Connector>,
    band: u8,
    bundle_min: usize,
    cache_limit: usize,
    cache: BTreeMap<Key, Option<Variants>>,
    /// `cache` の大きさ（[`DEFAULT_CACHE_LIMIT`] と同じ数え方）。
    cache_size: usize,
    /// 車のグラフの費用に足している遅れ。グラフを作り直しても引き継ぐ。
    delays: Delays,
    /// 2 点のあいだに経路があるか。費用によらないので、遅れが変わっても捨てず、網を作り直すまで覚える。
    reach: BTreeMap<ReachKey, bool>,
}

/// 経路があるかを覚える鍵。手段と、出発と行き先のリンクの上の位置。
pub type ReachKey = (LaneMode, (LinkId, LaneOffset), (LinkId, LaneOffset));

/// 一括の探索で、1 本のスレッドが解く単位。
enum Job {
    Single(Key),
    Bundle {
        mode: LaneMode,
        root: LinkId,
        toward: Toward,
        others: Vec<LinkId>,
    },
}

type Solved = Vec<(Key, Option<Variants>)>;

impl AstarRouter {
    /// 網からグラフを作る。網の中身が食い違っていたら panic する。
    pub fn new(net: &Network) -> AstarRouter {
        AstarRouter::try_new(net).expect("網の中身が食い違っている")
    }

    /// 網からグラフを作る。網の中身が食い違っていたら `Err` を返す。読み戻した街に使う。
    pub fn try_new(net: &Network) -> Result<AstarRouter, GraphError> {
        Ok(AstarRouter {
            version: net.version(),
            car: Graph::try_build(net, LaneMode::Car)?,
            walk: Graph::try_build(net, LaneMode::Sidewalk)?,
            connectors: try_connectors(net)?,
            band: 0,
            bundle_min: DEFAULT_BUNDLE_MIN,
            cache_limit: DEFAULT_CACHE_LIMIT,
            cache: BTreeMap::new(),
            cache_size: 0,
            delays: Delays::default(),
            reach: BTreeMap::new(),
        })
    }

    /// グラフを作ったときの網の版。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 網の版が変わっていたら作り直す。作り直したら `true`。
    pub fn sync(&mut self, net: &Network) -> bool {
        if self.version == net.version() {
            return false;
        }
        self.rebuild(net);
        true
    }

    /// グラフを作ったときの網の版を `version` にする。読み戻した街で、作り直しの要否を書き出す前と同じにするのに使う。
    pub(crate) fn with_version(mut self, version: u64) -> AstarRouter {
        self.version = version;
        self
    }

    /// 1 対多の探索に束ねる依頼の数の下限を変える。`usize::MAX` なら束ねずにすべて A* で解く。
    pub fn with_bundle_min(mut self, n: usize) -> AstarRouter {
        self.bundle_min = n.max(1);
        self
    }

    /// 覚えておく結果の大きさの上限を変える。数え方は [`DEFAULT_CACHE_LIMIT`] と同じである。
    pub fn with_cache_limit(mut self, n: usize) -> AstarRouter {
        self.cache_limit = n;
        self
    }

    pub fn graph(&self, mode: LaneMode) -> &Graph {
        match mode {
            LaneMode::Car => &self.car,
            LaneMode::Sidewalk => &self.walk,
        }
    }

    /// ノードでの車線のつながり。ノード、元の車線、先の車線の順に並ぶ。
    pub fn connectors(&self) -> &[Connector] {
        &self.connectors
    }

    pub fn band(&self) -> u8 {
        self.band
    }

    /// 車のリンクの費用に足す遅れを `delays` にする。変わったら、覚えた車の結果を捨てる。
    pub fn set_delays(&mut self, delays: &Delays) {
        if *delays == self.delays {
            return;
        }
        self.delays.clone_from(delays);
        self.car.set_delays(delays);
        self.cache.retain(|r, _| r.mode != LaneMode::Car);
        self.cache_size = self.cache.values().map(size_of_entry).sum();
    }

    /// 手段 `mode` で、リンク `from` の終わりからリンク `to` へ必ず経路があると言えるか。
    /// 費用によらず網の形だけで決まるので、探さずに答えられる。`false` なら探して確かめる。
    pub fn surely_connected(&self, mode: LaneMode, from: LinkId, to: LinkId) -> bool {
        self.graph(mode).surely_connected(from, to)
    }

    /// 覚えている、`key` の 2 点のあいだに経路があるか。
    pub fn known_reach(&self, key: &ReachKey) -> Option<bool> {
        self.reach.get(key).copied()
    }

    /// `key` の 2 点のあいだに経路があるかを覚える。上限を超えそうなら、先に全部捨てる。
    pub fn remember_reach(&mut self, key: ReachKey, found: bool) {
        if self.reach.len() >= REACH_LIMIT {
            self.reach.clear();
        }
        self.reach.insert(key, found);
    }

    /// 車のリンクの費用に足している遅れ。
    pub fn delays(&self) -> &Delays {
        &self.delays
    }

    /// 覚えている結果の数。
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// 覚えている結果の大きさ。数え方は [`DEFAULT_CACHE_LIMIT`] と同じである。
    pub fn cached_size(&self) -> usize {
        self.cache_size
    }

    /// 依頼を解く単位に分ける。手段ごとに、着くリンクと出発のリンクのどちらで束ねるかを決め、
    /// 下限に満たない組は 1 件ずつ A* で解く。
    fn plan(&self, misses: &[Key]) -> Vec<Job> {
        let mut jobs = Vec::new();
        for mode in [LaneMode::Car, LaneMode::Sidewalk] {
            let mut by_to = BTreeMap::<LinkId, Vec<LinkId>>::new();
            let mut by_from = BTreeMap::<LinkId, Vec<LinkId>>::new();
            for r in misses.iter().filter(|r| r.mode == mode) {
                by_to.entry(r.to).or_default().push(r.from);
                by_from.entry(r.from).or_default().push(r.to);
            }
            // 下限以上の組に入る依頼が多いほうの向きで束ねる。同じなら着くリンクで束ねる。
            let covered = |groups: &BTreeMap<LinkId, Vec<LinkId>>| -> usize {
                groups
                    .values()
                    .map(Vec::len)
                    .filter(|n| *n >= self.bundle_min)
                    .sum()
            };
            let (groups, toward) = if covered(&by_to) >= covered(&by_from) {
                (by_to, Toward::Backward)
            } else {
                (by_from, Toward::Forward)
            };
            for (root, others) in groups {
                if others.len() >= self.bundle_min {
                    jobs.push(Job::Bundle {
                        mode,
                        root,
                        toward,
                        others,
                    });
                    continue;
                }
                jobs.extend(others.into_iter().map(|o| {
                    let (from, to) = match toward {
                        Toward::Backward => (o, root),
                        Toward::Forward => (root, o),
                    };
                    Job::Single(Key { from, to, mode })
                }));
            }
        }
        jobs
    }

    fn solve(&self, job: &Job, s: &mut Scratches) -> Solved {
        match job {
            Job::Single(r) => vec![(
                *r,
                astar_variants(
                    self.graph(r.mode),
                    r.from,
                    r.to,
                    s.get(r.mode),
                    variants_of(r.mode),
                )
                .map(share),
            )],
            Job::Bundle {
                mode,
                root,
                toward,
                others,
            } => {
                let g = self.graph(*mode);
                let sc = s.get(*mode);
                let n = variants_of(*mode);
                let routes: Vec<Option<Vec<Route>>> =
                    match search_many(g, *root, others, *toward, sc) {
                        None => vec![None; others.len()],
                        Some(r) => extract_all(g, sc, r, others, *toward, n),
                    };
                others
                    .iter()
                    .zip(routes)
                    .map(|(o, route)| {
                        let (from, to) = match toward {
                            Toward::Backward => (*o, *root),
                            Toward::Forward => (*root, *o),
                        };
                        let r = Key {
                            from,
                            to,
                            mode: *mode,
                        };
                        (r, route.map(share))
                    })
                    .collect()
            }
        }
    }
}

/// 1 つの探索から、組の経路を候補ごとにすべて取り出す。組が大きければスレッドに分ける。
/// 取り出しは探索の結果を読むだけなので、分け方で結果は変わらない。
fn extract_all(
    g: &Graph,
    s: &mut Scratch,
    root: u32,
    others: &[LinkId],
    toward: Toward,
    variants: u8,
) -> Vec<Option<Vec<Route>>> {
    let mut m = s.take_marks();
    let sc = &*s;
    let one = |m: &mut Marks, o: &LinkId| extract(g, sc, m, root, *o, toward, variants);
    #[cfg(feature = "parallel")]
    let out = if others.len() >= PARALLEL_EXTRACT_MIN {
        others.par_iter().map_init(Marks::default, one).collect()
    } else {
        others.iter().map(|o| one(&mut m, o)).collect()
    };
    #[cfg(not(feature = "parallel"))]
    let out = others.iter().map(|o| one(&mut m, o)).collect();
    s.put_marks(m);
    out
}

impl Router for AstarRouter {
    fn rebuild(&mut self, net: &Network) {
        let delays = core::mem::take(&mut self.delays);
        *self = AstarRouter {
            band: self.band,
            bundle_min: self.bundle_min,
            cache_limit: self.cache_limit,
            ..AstarRouter::new(net)
        };
        self.set_delays(&delays);
    }

    fn set_band(&mut self, band: u8) {
        assert!(band < TIME_BANDS, "時間帯 {band} が範囲の外");
        if band != self.band {
            self.band = band;
            self.cache.clear();
            self.cache_size = 0;
        }
    }

    fn route_batch(&mut self, requests: &[RouteRequest]) -> Vec<Option<Arc<Route>>> {
        // 覚えていない依頼を、重複を除いて依頼の鍵の順に並べてから解く。
        let misses: Vec<Key> = requests
            .iter()
            .map(RouteRequest::key)
            .filter(|k| !self.cache.contains_key(k))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let jobs = self.plan(&misses);
        let this = &*self;
        #[cfg(feature = "parallel")]
        let solved: Vec<Solved> = jobs
            .par_iter()
            .map_init(Scratches::default, |s, j| this.solve(j, s))
            .collect();
        #[cfg(not(feature = "parallel"))]
        let solved: Vec<Solved> = {
            let mut s = Scratches::default();
            jobs.iter().map(|j| this.solve(j, &mut s)).collect()
        };
        let fresh: BTreeMap<_, _> = solved.into_iter().flatten().collect();
        let out = requests
            .iter()
            .map(|r| {
                let k = r.key();
                let v = self.cache.get(&k).unwrap_or_else(|| &fresh[&k]).as_ref()?;
                Some(v[usize::from(r.variant) % v.len()].clone())
            })
            .collect();
        let fresh_size: usize = fresh.values().map(size_of_entry).sum();
        if self.cache_size + fresh_size > self.cache_limit {
            self.cache.clear();
            self.cache_size = 0;
        }
        // 1 回の一括だけで上限を超えるときは、鍵の順で上限に収まるところまでを覚え、残りは捨てる。
        for (k, v) in fresh {
            let n = size_of_entry(&v);
            if self.cache_size + n > self.cache_limit {
                break;
            }
            self.cache_size += n;
            self.cache.insert(k, v);
        }
        out
    }
}
