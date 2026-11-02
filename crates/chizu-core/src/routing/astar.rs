//! 最短経路の探索。1 対 1 の A* と、根を共有する依頼をまとめて解く 1 対多のダイクストラ。
//!
//! 費用の同じ最短経路がいくつあっても、どの解き方でも同じ経路を返す。返すのは、最短経路のうち、
//! 出発のリンクから順に頂点の番号を比べていちばん小さいものである。こうすると、どの依頼を
//! 束ねたかや、覚えた結果があったかで経路が変わらない。

use core::cmp::Reverse;
use serde::{Deserialize, Serialize};
use std::collections::BinaryHeap;

use super::graph::{Edge, Graph};
use crate::network::LinkId;

/// 見つけた経路。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// 出発のリンクから着くリンクまで、通る順に並べたリンク。
    pub links: Vec<LinkId>,
    /// 出発のリンクと着くリンクを端から端まで通るとした費用（ms）。
    pub cost: u64,
}

/// 探索の向き。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Toward {
    /// 根から辺の向きに広げる。根は出発のリンクで、距離は根の始まりからその頂点の終わりまで。
    Forward,
    /// 根から辺を逆にたどる。根は着くリンクで、距離はその頂点の始まりから根の終わりまで。
    Backward,
}

/// 探索の作業領域。探索ごとに印を変えて、配列を埋め直さずに使い回す。
#[derive(Clone, Debug, Default)]
pub struct Scratch {
    stamp: u32,
    seen: Vec<u32>,
    dist: Vec<u64>,
    closed: Vec<bool>,
    marks: Marks,
    heap: BinaryHeap<Reverse<(u64, u64, u32)>>,
    /// 配列を作り直した回数。
    allocations: u32,
}

impl Scratch {
    /// 配列を作り直した回数。グラフの頂点の数が変わるたびに増える。
    pub fn allocations(&self) -> u32 {
        self.allocations
    }

    fn reset(&mut self, n: usize) {
        if self.seen.len() != n || self.stamp == u32::MAX {
            self.allocations = self.allocations.saturating_add(1);
            self.seen = vec![0; n];
            self.dist = vec![0; n];
            self.closed = vec![false; n];
            self.stamp = 0;
        }
        self.stamp += 1;
        self.heap.clear();
    }

    /// 印の作業領域を借りる。返すときは [`Scratch::put_marks`] を呼ぶ。
    pub(crate) fn take_marks(&mut self) -> Marks {
        std::mem::take(&mut self.marks)
    }

    pub(crate) fn put_marks(&mut self, m: Marks) {
        self.marks = m;
    }

    fn dist(&self, v: u32) -> Option<u64> {
        (self.seen[v as usize] == self.stamp).then(|| self.dist[v as usize])
    }

    fn is_closed(&self, v: u32) -> bool {
        self.seen[v as usize] == self.stamp && self.closed[v as usize]
    }

    fn set(&mut self, v: u32, d: u64) {
        let i = v as usize;
        if self.seen[i] != self.stamp {
            self.seen[i] = self.stamp;
            self.closed[i] = false;
        }
        self.dist[i] = d;
    }
}

/// 最短経路の上の頂点に付ける印。経路を取り出すスレッドごとに 1 つ持つ。
#[derive(Clone, Debug, Default)]
pub(crate) struct Marks {
    stamp: u32,
    mark: Vec<u32>,
    stack: Vec<u32>,
}

impl Marks {
    fn begin(&mut self, n: usize) {
        if self.mark.len() != n || self.stamp == u32::MAX {
            self.mark = vec![0; n];
            self.stamp = 0;
        }
        self.stamp += 1;
    }

    fn set(&mut self, v: u32) {
        self.mark[v as usize] = self.stamp;
    }

    fn is_marked(&self, v: u32) -> bool {
        self.mark[v as usize] == self.stamp
    }
}

fn add(a: u64, b: u64) -> u64 {
    a.checked_add(b).expect("経路の費用があふれた")
}

fn adjacent(g: &Graph, v: u32, toward: Toward) -> &[Edge] {
    match toward {
        Toward::Forward => g.edges(v),
        Toward::Backward => g.rev_edges(v),
    }
}

/// `root` から広げる。`done` が真を返したら、その頂点の推定の総費用より大きい頂点を取り出す前に止める。
/// 同じ推定の頂点は最後まで閉じるので、止めた時点で最短経路の上の頂点はすべて閉じている。
fn search(
    g: &Graph,
    root: u32,
    toward: Toward,
    s: &mut Scratch,
    h: impl Fn(u32) -> u64,
    mut done: impl FnMut(u32) -> bool,
) {
    s.reset(g.vertex_count());
    let d0 = g.cost(root);
    s.set(root, d0);
    s.heap.push(Reverse((add(d0, h(root)), h(root), root)));
    let mut bound = None;
    while let Some(Reverse((f, _, v))) = s.heap.pop() {
        if bound.is_some_and(|b| f > b) {
            break;
        }
        if s.is_closed(v) {
            continue;
        }
        s.closed[v as usize] = true;
        if bound.is_none() && done(v) {
            bound = Some(f);
        }
        let dv = s.dist[v as usize];
        for e in adjacent(g, v, toward) {
            let d = add(add(dv, e.cost), g.cost(e.to));
            if s.dist(e.to).is_some_and(|old| old <= d) {
                continue;
            }
            s.set(e.to, d);
            let hv = h(e.to);
            s.heap.push(Reverse((add(d, hv), hv, e.to)));
        }
    }
}

/// 根から前向きに広げた距離から、`src` から `dst` への経路を取り出す。
fn path_from_forward(g: &Graph, s: &Scratch, m: &mut Marks, src: u32, dst: u32) -> Option<Route> {
    if !s.is_closed(dst) {
        return None;
    }
    // 着くリンクから逆にたどり、最短経路の上にある頂点に印を付ける。
    m.begin(g.vertex_count());
    m.set(dst);
    m.stack.clear();
    m.stack.push(dst);
    while let Some(v) = m.stack.pop() {
        let dv = s.dist[v as usize];
        for e in g.rev_edges(v) {
            let u = e.to;
            if s.is_closed(u)
                && !m.is_marked(u)
                && add(add(s.dist[u as usize], e.cost), g.cost(v)) == dv
            {
                m.set(u);
                m.stack.push(u);
            }
        }
    }
    let cost = s.dist[dst as usize];
    let m = &*m;
    walk(g, src, dst, cost, |u, e| {
        m.is_marked(e.to)
            && add(add(s.dist[u as usize], e.cost), g.cost(e.to)) == s.dist[e.to as usize]
    })
}

/// 根から逆向きに広げた距離から、`src` から根への経路を取り出す。
fn path_from_backward(g: &Graph, s: &Scratch, src: u32, dst: u32) -> Option<Route> {
    if !s.is_closed(src) {
        return None;
    }
    let cost = s.dist[src as usize];
    walk(g, src, dst, cost, |u, e| {
        s.dist(e.to)
            .is_some_and(|dv| add(add(g.cost(u), e.cost), dv) == s.dist[u as usize])
    })
}

/// `src` から、最短経路を続けられる辺のうち行き先の番号がいちばん小さいものを選んで進む。
fn walk(
    g: &Graph,
    src: u32,
    dst: u32,
    cost: u64,
    tight: impl Fn(u32, &Edge) -> bool,
) -> Option<Route> {
    let mut links = vec![g.link(src)];
    let mut u = src;
    while u != dst {
        u = g
            .edges(u)
            .iter()
            .find(|e| tight(u, e))
            .expect("最短経路が続かない")
            .to;
        links.push(g.link(u));
    }
    Some(Route { links, cost })
}

/// `from` から `to` への費用の最も小さい経路。つながっていないか、どちらかのリンクがグラフにないときは `None`。
///
/// 待ち行列は（推定の総費用、残りの推定、頂点の番号）の小さい順に取り出す。
pub fn astar(g: &Graph, from: LinkId, to: LinkId, s: &mut Scratch) -> Option<Route> {
    let (src, dst) = (g.vertex(from)?, g.vertex(to)?);
    search(
        g,
        src,
        Toward::Forward,
        s,
        |v| g.heuristic(v, dst),
        |v| v == dst,
    );
    let mut m = std::mem::take(&mut s.marks);
    let route = path_from_forward(g, s, &mut m, src, dst);
    s.marks = m;
    route
}

/// 根 `root` を共有する依頼のための探索。`others` がすべて閉じたところで止める。
/// 根がグラフになければ `None`。経路は [`extract`] で取り出す。
pub(crate) fn search_many(
    g: &Graph,
    root: LinkId,
    others: &[LinkId],
    toward: Toward,
    s: &mut Scratch,
) -> Option<u32> {
    let r = g.vertex(root)?;
    let mut want: Vec<u32> = others.iter().filter_map(|l| g.vertex(*l)).collect();
    want.sort_unstable();
    want.dedup();
    let mut left = want.len();
    search(
        g,
        r,
        toward,
        s,
        |_| 0,
        |v| {
            if want.binary_search(&v).is_ok() {
                left -= 1;
            }
            left == 0
        },
    );
    Some(r)
}

/// [`search_many`] の結果から、根 `root` と `other` を結ぶ経路を取り出す。`s` は読むだけなので、
/// `Marks` をスレッドごとに分ければ、同じ探索から並列に取り出せる。
pub(crate) fn extract(
    g: &Graph,
    s: &Scratch,
    m: &mut Marks,
    root: u32,
    other: LinkId,
    toward: Toward,
) -> Option<Route> {
    let o = g.vertex(other)?;
    match toward {
        Toward::Forward => path_from_forward(g, s, m, root, o),
        Toward::Backward => path_from_backward(g, s, o, root),
    }
}

/// 根 `root` を共有する依頼をまとめて解き、`others` の順に結果を返す。
///
/// `Forward` なら `root` は出発のリンクで `others` は着くリンク、`Backward` なら `root` は着くリンクで
/// `others` は出発のリンクである。`others` がすべて閉じたところで止める。
pub fn one_to_many(
    g: &Graph,
    root: LinkId,
    others: &[LinkId],
    toward: Toward,
    s: &mut Scratch,
) -> Vec<Option<Route>> {
    let Some(r) = search_many(g, root, others, toward, s) else {
        return vec![None; others.len()];
    };
    let mut m = std::mem::take(&mut s.marks);
    let out = others
        .iter()
        .map(|l| extract(g, s, &mut m, r, *l, toward))
        .collect();
    s.marks = m;
    out
}
