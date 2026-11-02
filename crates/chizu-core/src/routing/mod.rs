//! 経路探索。
//!
//! 網から車と歩行者のグラフを作り、A* か、根を共有する依頼を束ねた 1 対多の探索で解く。
//! 網が変わったら [`Router::rebuild`] でグラフを作り直し、覚えた結果も捨てる。探索の結果は状態に入れない。同じ網と同じ依頼からは、
//! 覚えた結果の有無やスレッドの数に関わらず同じ経路を返す。

mod astar;
mod graph;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use astar::{Marks, extract, search_many};
pub use astar::{Route, Scratch, Toward, astar, one_to_many};
pub use graph::{Connector, Graph, WALK_SPEED, connectors, speed_limit};

use crate::network::{LaneMode, LinkId, Network};

/// 1 日を分ける時間帯の数。重みは時間帯ごとに写す。
pub const TIME_BANDS: u8 = 96;

/// 1 対多の探索に束ねる依頼の数の既定の下限。これより少ない組は A* で解く。
pub const DEFAULT_BUNDLE_MIN: usize = 4;

/// 1 つの組の経路を、スレッドに分けて取り出し始める依頼の数。
#[cfg(feature = "parallel")]
const PARALLEL_EXTRACT_MIN: usize = 256;

/// 覚えておく結果の数の既定の上限。超えそうになったら、新しい結果を入れる前に全部捨てる。
pub const DEFAULT_CACHE_LIMIT: usize = 1 << 18;

/// 探索の依頼。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RouteRequest {
    pub from: LinkId,
    pub to: LinkId,
    /// 車で探すなら `Car`、歩いて探すなら `Sidewalk`。
    pub mode: LaneMode,
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
    cache: BTreeMap<RouteRequest, Option<Arc<Route>>>,
}

/// 一括の探索で、1 本のスレッドが解く単位。
enum Job {
    Single(RouteRequest),
    Bundle {
        mode: LaneMode,
        root: LinkId,
        toward: Toward,
        others: Vec<LinkId>,
    },
}

type Solved = Vec<(RouteRequest, Option<Arc<Route>>)>;

impl AstarRouter {
    pub fn new(net: &Network) -> AstarRouter {
        AstarRouter {
            version: net.version(),
            car: Graph::build(net, LaneMode::Car),
            walk: Graph::build(net, LaneMode::Sidewalk),
            connectors: connectors(net),
            band: 0,
            bundle_min: DEFAULT_BUNDLE_MIN,
            cache_limit: DEFAULT_CACHE_LIMIT,
            cache: BTreeMap::new(),
        }
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

    /// 覚えておく結果の数の上限を変える。
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

    /// 覚えている結果の数。
    pub fn cached(&self) -> usize {
        self.cache.len()
    }

    /// 依頼を解く単位に分ける。手段ごとに、着くリンクと出発のリンクのどちらで束ねるかを決め、
    /// 下限に満たない組は 1 件ずつ A* で解く。
    fn plan(&self, misses: &[RouteRequest]) -> Vec<Job> {
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
                    Job::Single(RouteRequest { from, to, mode })
                }));
            }
        }
        jobs
    }

    fn solve(&self, job: &Job, s: &mut Scratches) -> Solved {
        match job {
            Job::Single(r) => vec![(
                *r,
                astar(self.graph(r.mode), r.from, r.to, s.get(r.mode)).map(Arc::new),
            )],
            Job::Bundle {
                mode,
                root,
                toward,
                others,
            } => {
                let g = self.graph(*mode);
                let sc = s.get(*mode);
                let routes: Vec<Option<Route>> = match search_many(g, *root, others, *toward, sc) {
                    None => vec![None; others.len()],
                    Some(r) => extract_all(g, sc, r, others, *toward),
                };
                others
                    .iter()
                    .zip(routes)
                    .map(|(o, route)| {
                        let (from, to) = match toward {
                            Toward::Backward => (*o, *root),
                            Toward::Forward => (*root, *o),
                        };
                        let r = RouteRequest {
                            from,
                            to,
                            mode: *mode,
                        };
                        (r, route.map(Arc::new))
                    })
                    .collect()
            }
        }
    }
}

/// 1 つの探索から、組の経路をすべて取り出す。組が大きければスレッドに分ける。
/// 取り出しは探索の結果を読むだけなので、分け方で結果は変わらない。
fn extract_all(
    g: &Graph,
    s: &mut Scratch,
    root: u32,
    others: &[LinkId],
    toward: Toward,
) -> Vec<Option<Route>> {
    let mut m = s.take_marks();
    let sc = &*s;
    let one = |m: &mut Marks, o: &LinkId| extract(g, sc, m, root, *o, toward);
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
        *self = AstarRouter {
            band: self.band,
            bundle_min: self.bundle_min,
            cache_limit: self.cache_limit,
            ..AstarRouter::new(net)
        };
    }

    fn set_band(&mut self, band: u8) {
        assert!(band < TIME_BANDS, "時間帯 {band} が範囲の外");
        if band != self.band {
            self.band = band;
            self.cache.clear();
        }
    }

    fn route_batch(&mut self, requests: &[RouteRequest]) -> Vec<Option<Arc<Route>>> {
        // 覚えていない依頼を、重複を除いて依頼の鍵の順に並べてから解く。
        let misses: Vec<RouteRequest> = requests
            .iter()
            .filter(|r| !self.cache.contains_key(r))
            .copied()
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
            .map(|r| self.cache.get(r).unwrap_or_else(|| &fresh[r]).clone())
            .collect();
        if self.cache.len() + fresh.len() > self.cache_limit {
            self.cache.clear();
        }
        // 1 回の一括だけで上限を超えるときは、鍵の順で上限までを覚え、残りは捨てる。
        let room = self.cache_limit.saturating_sub(self.cache.len());
        self.cache.extend(fresh.into_iter().take(room));
        out
    }
}
