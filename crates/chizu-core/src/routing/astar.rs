//! A* による最短経路。

use core::cmp::Reverse;
use std::collections::BinaryHeap;

use super::graph::Graph;
use crate::network::LinkId;

/// 見つけた経路。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// 出発のリンクから着くリンクまで、通る順に並べたリンク。
    pub links: Vec<LinkId>,
    /// 出発のリンクと着くリンクを端から端まで通るとした費用（ms）。
    pub cost: u64,
}

/// 探索の作業領域。探索ごとに印を変えて、配列を埋め直さずに使い回す。
#[derive(Clone, Debug, Default)]
pub struct Scratch {
    stamp: u32,
    seen: Vec<u32>,
    dist: Vec<u64>,
    parent: Vec<u32>,
    closed: Vec<bool>,
    heap: BinaryHeap<Reverse<(u64, u64, u32)>>,
}

impl Scratch {
    fn reset(&mut self, n: usize) {
        if self.seen.len() != n || self.stamp == u32::MAX {
            self.seen = vec![0; n];
            self.dist = vec![0; n];
            self.parent = vec![0; n];
            self.closed = vec![false; n];
            self.stamp = 0;
        }
        self.stamp += 1;
        self.heap.clear();
    }

    fn dist(&self, v: u32) -> Option<u64> {
        (self.seen[v as usize] == self.stamp).then(|| self.dist[v as usize])
    }

    fn set(&mut self, v: u32, d: u64, parent: u32) {
        let i = v as usize;
        if self.seen[i] != self.stamp {
            self.seen[i] = self.stamp;
            self.closed[i] = false;
        }
        self.dist[i] = d;
        self.parent[i] = parent;
    }
}

/// `from` から `to` への費用の最も小さい経路。つながっていないか、どちらかのリンクがグラフにないときは `None`。
///
/// 待ち行列は（推定の総費用、残りの推定、頂点の番号）の小さい順に取り出す。同じ費用の経路が
/// いくつあっても、取り出す順と親を書き換える条件（真に小さくなったときだけ）が決まっているので、
/// 同じ入力からは同じ経路を返す。
pub fn astar(g: &Graph, from: LinkId, to: LinkId, s: &mut Scratch) -> Option<Route> {
    let (src, dst) = (g.vertex(from)?, g.vertex(to)?);
    s.reset(g.vertex_count());
    let d0 = g.cost(src);
    s.set(src, d0, src);
    let h0 = g.heuristic(src, dst);
    s.heap.push(Reverse((
        d0.checked_add(h0).expect("経路の費用があふれた"),
        h0,
        src,
    )));
    while let Some(Reverse((_, _, v))) = s.heap.pop() {
        if s.closed[v as usize] {
            continue;
        }
        s.closed[v as usize] = true;
        let dv = s.dist(v).expect("待ち行列の頂点に距離がない");
        if v == dst {
            return Some(Route {
                links: path(g, s, src, dst),
                cost: dv,
            });
        }
        for e in g.edges(v) {
            let d = dv
                .checked_add(e.cost)
                .and_then(|d| d.checked_add(g.cost(e.to)))
                .expect("経路の費用があふれた");
            if s.dist(e.to).is_some_and(|old| old <= d) {
                continue;
            }
            s.set(e.to, d, v);
            let h = g.heuristic(e.to, dst);
            let f = d.checked_add(h).expect("経路の費用があふれた");
            s.heap.push(Reverse((f, h, e.to)));
        }
    }
    None
}

fn path(g: &Graph, s: &Scratch, src: u32, dst: u32) -> Vec<LinkId> {
    let mut out = vec![g.link(dst)];
    let mut v = dst;
    while v != src {
        v = s.parent[v as usize];
        out.push(g.link(v));
    }
    out.reverse();
    out
}
