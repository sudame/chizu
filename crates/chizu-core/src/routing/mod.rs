//! 経路探索。
//!
//! 網から車と歩行者のグラフを作り、A* で解く。網が変わったら [`Router::rebuild`] でグラフを
//! 作り直し、覚えた結果も捨てる。探索の結果は状態に入れない。同じ網と同じ依頼からは、
//! 覚えた結果の有無やスレッドの数に関わらず同じ経路を返す。

mod astar;
mod graph;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

pub use astar::{Route, Scratch, astar};
pub use graph::{Connector, Graph, WALK_SPEED, connectors, speed_limit};

use crate::network::{LaneMode, LinkId, Network};

/// 1 日を分ける時間帯の数。重みは時間帯ごとに写す。
pub const TIME_BANDS: u8 = 96;

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

/// A* で解き、`(出発のリンク, 着くリンク, 手段)` ごとに結果を覚える方式。時間帯は覚える範囲を区切る。
#[derive(Clone, Debug)]
pub struct AstarRouter {
    car: Graph,
    walk: Graph,
    connectors: Vec<Connector>,
    band: u8,
    cache: BTreeMap<RouteRequest, Option<Arc<Route>>>,
}

impl AstarRouter {
    pub fn new(net: &Network) -> AstarRouter {
        AstarRouter {
            car: Graph::build(net, LaneMode::Car),
            walk: Graph::build(net, LaneMode::Sidewalk),
            connectors: connectors(net),
            band: 0,
            cache: BTreeMap::new(),
        }
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

    fn solve(&self, r: &RouteRequest, s: &mut Scratches) -> Option<Arc<Route>> {
        astar(self.graph(r.mode), r.from, r.to, s.get(r.mode)).map(Arc::new)
    }
}

impl Router for AstarRouter {
    fn rebuild(&mut self, net: &Network) {
        *self = AstarRouter {
            band: self.band,
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
        let this = &*self;
        #[cfg(feature = "parallel")]
        let solved: Vec<_> = misses
            .par_iter()
            .map_init(Scratches::default, |s, r| this.solve(r, s))
            .collect();
        #[cfg(not(feature = "parallel"))]
        let solved: Vec<_> = {
            let mut s = Scratches::default();
            misses.iter().map(|r| this.solve(r, &mut s)).collect()
        };
        self.cache.extend(misses.into_iter().zip(solved));
        requests.iter().map(|r| self.cache[r].clone()).collect()
    }
}
