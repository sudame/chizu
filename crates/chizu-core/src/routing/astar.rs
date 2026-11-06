//! 最短経路の探索。1 対 1 の A* と、根を共有する依頼をまとめて解く 1 対多のダイクストラ。
//!
//! 費用の同じ最短経路がいくつあっても、どの解き方でも同じ経路を返す。候補の数が 1 なら、最短経路のうち、
//! 出発のリンクから順に頂点の番号を比べていちばん小さいものを返す。候補が 2 つ以上なら、1 回の探索から
//! 候補（variant）ごとに 1 本ずつ取り出す。候補 `k` の経路は、出発のリンクから進むたびに、最短経路を続けられる
//! 辺のうち [`Graph::rank`] のいちばん小さいものを選んだものである。どちらの選び方も頂点と候補だけで決まるので、
//! どの依頼を束ねたかや、覚えた結果があったかで経路が変わらない。

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

/// 根から前向きに広げた距離から、`src` から `dst` への経路を、候補 `0..variants` の順に取り出す。
fn path_from_forward(
    g: &Graph,
    s: &Scratch,
    m: &mut Marks,
    src: u32,
    dst: u32,
    variants: u8,
) -> Option<Vec<Route>> {
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
    walk_all(g, src, dst, cost, variants, |u, e| {
        m.is_marked(e.to)
            && add(add(s.dist[u as usize], e.cost), g.cost(e.to)) == s.dist[e.to as usize]
    })
}

/// 根から逆向きに広げた距離から、`src` から根への経路を、候補 `0..variants` の順に取り出す。
fn path_from_backward(
    g: &Graph,
    s: &Scratch,
    src: u32,
    dst: u32,
    variants: u8,
) -> Option<Vec<Route>> {
    if !s.is_closed(src) {
        return None;
    }
    let cost = s.dist[src as usize];
    walk_all(g, src, dst, cost, variants, |u, e| {
        s.dist(e.to)
            .is_some_and(|dv| add(add(g.cost(u), e.cost), dv) == s.dist[u as usize])
    })
}

/// 候補 `0..variants` のそれぞれで [`walk`] し、候補の順に返す。最短経路が途中で分かれなければ、
/// どの候補も同じ経路になるので、1 本だけを返す。
fn walk_all(
    g: &Graph,
    src: u32,
    dst: u32,
    cost: u64,
    variants: u8,
    tight: impl Fn(u32, &Edge) -> bool,
) -> Option<Vec<Route>> {
    if variants == 1 {
        return Some(vec![walk(g, src, dst, cost, None, 0, &tight).0]);
    }
    let (first, branched) = walk(g, src, dst, cost, Some(0), 0, &tight);
    let len = first.links.len();
    let mut out = vec![first];
    if branched {
        out.extend((1..variants).map(|k| walk(g, src, dst, cost, Some(k), len, &tight).0));
    }
    Some(out)
}

/// `src` から、最短経路を続けられる辺のうち、候補 `variant` での行き先の順がいちばん小さいものを選んで進む。
/// 順が同じなら、行き先の番号の小さいものを選ぶ。`variant` が `None` なら、散らさずに行き先の番号の小さいものを選ぶ。
/// 最短経路が途中で分かれたかも返す。`capacity` は経路のリンクの数の見込みである。
fn walk(
    g: &Graph,
    src: u32,
    dst: u32,
    cost: u64,
    variant: Option<u8>,
    capacity: usize,
    tight: &impl Fn(u32, &Edge) -> bool,
) -> (Route, bool) {
    let mut links = Vec::with_capacity(capacity);
    links.push(g.link(src));
    let mut u = src;
    let mut branched = false;
    while u != dst {
        let Some(variant) = variant else {
            // 辺は行き先の番号の順に並ぶので、最初に見つかった辺の行き先がいちばん小さい。
            u = g
                .edges(u)
                .iter()
                .find(|e| tight(u, e))
                .expect("最短経路が続かない")
                .to;
            links.push(g.link(u));
            continue;
        };
        let mut next: Option<(u32, Option<u64>)> = None;
        for e in g.edges(u).iter().filter(|e| tight(u, e)) {
            next = Some(match next {
                None => (e.to, None),
                Some((best, rank)) => {
                    branched = true;
                    let best_rank = rank.unwrap_or_else(|| g.rank(variant, best));
                    let r = g.rank(variant, e.to);
                    if (r, e.to) < (best_rank, best) {
                        (e.to, Some(r))
                    } else {
                        (best, Some(best_rank))
                    }
                }
            });
        }
        u = next.expect("最短経路が続かない").0;
        links.push(g.link(u));
    }
    (Route { links, cost }, branched)
}

/// `from` から `to` への費用の最も小さい経路。つながっていないか、どちらかのリンクがグラフにないときは `None`。
///
/// 費用の同じ経路があれば、頂点の番号を順に比べていちばん小さいものを返す。
///
/// 待ち行列は（推定の総費用、残りの推定、頂点の番号）の小さい順に取り出す。
pub fn astar(g: &Graph, from: LinkId, to: LinkId, s: &mut Scratch) -> Option<Route> {
    astar_variants(g, from, to, s, 1).map(|mut v| v.swap_remove(0))
}

/// [`astar`] と同じく探し、候補 `0..variants` の経路を候補の順に返す。`variants` は 1 以上であること。
/// 最短経路が途中で分かれず、どの候補も同じ経路になるときは、1 本だけを返す。
pub fn astar_variants(
    g: &Graph,
    from: LinkId,
    to: LinkId,
    s: &mut Scratch,
    variants: u8,
) -> Option<Vec<Route>> {
    assert!(variants > 0, "候補の数が 0");
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
    let route = path_from_forward(g, s, &mut m, src, dst, variants);
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

/// [`search_many`] の結果から、根 `root` と `other` を結ぶ経路を、候補 `0..variants` の順に取り出す。
/// どの候補も同じ経路になるときは、1 本だけを返す。
/// `s` は読むだけなので、`Marks` をスレッドごとに分ければ、同じ探索から並列に取り出せる。
pub(crate) fn extract(
    g: &Graph,
    s: &Scratch,
    m: &mut Marks,
    root: u32,
    other: LinkId,
    toward: Toward,
    variants: u8,
) -> Option<Vec<Route>> {
    let o = g.vertex(other)?;
    match toward {
        Toward::Forward => path_from_forward(g, s, m, root, o, variants),
        Toward::Backward => path_from_backward(g, s, o, root, variants),
    }
}

/// 根の並び `roots`（リンクと、そのリンクの頂点に置く始めの距離）から `toward` の向きに広げ、入口までの距離が
/// `limit` 以下の頂点をすべて、リンクの ID の順に距離を添えて返す。根はいつも入る。グラフにない根は飛ばす。
///
/// 距離は [`search`] と同じく、頂点のリンクを端から端まで通る費用を含む。`Forward` なら根の距離に続けて
/// 辺の費用と行き先のリンクの費用を足し、`Backward` なら辺を逆にたどって元のリンクの費用を足す。入口までの
/// 距離は、そこから頂点のリンクの費用を引いたもの（`Forward` ならリンクの始まり、`Backward` ならリンクの終わり
/// までの距離）である。長いリンクの途中の点まで測れるよう、入口が上限の中なら、リンクの先が上限を超えても入れる。
/// 広げるのは、距離が `limit` 以下の頂点からだけである。最短の距離は広げる順によらず決まるので、結果は根の並びの
/// 順にもよらない。
pub fn reach_within(
    g: &Graph,
    roots: &[(LinkId, u64)],
    toward: Toward,
    limit: u64,
    s: &mut Scratch,
) -> Vec<(LinkId, u64)> {
    s.reset(g.vertex_count());
    let mut root_of: Vec<u32> = Vec::new();
    for (link, d0) in roots {
        let Some(v) = g.vertex(*link) else {
            continue;
        };
        root_of.push(v);
        if s.dist(v).is_some_and(|old| old <= *d0) {
            continue;
        }
        s.set(v, *d0);
        s.heap.push(Reverse((*d0, 0, v)));
    }
    let mut out = Vec::new();
    // 上限を超えた頂点からは広げないので、待ち行列には広げた頂点の隣までしか入らない。最後まで取り出す。
    while let Some(Reverse((dv, _, v))) = s.heap.pop() {
        if s.is_closed(v) {
            continue;
        }
        s.closed[v as usize] = true;
        let entry = dv.saturating_sub(g.cost(v));
        if entry <= limit || root_of.contains(&v) {
            out.push((v, dv));
        }
        if dv > limit {
            continue;
        }
        for e in adjacent(g, v, toward) {
            let d = add(add(dv, e.cost), g.cost(e.to));
            if s.dist(e.to).is_some_and(|old| old <= d) {
                continue;
            }
            s.set(e.to, d);
            s.heap.push(Reverse((d, 0, e.to)));
        }
    }
    // 頂点の番号はリンクの ID の順に振ってあるので、番号で並べればリンクの ID の順になる。
    out.sort_unstable();
    out.into_iter().map(|(v, d)| (g.link(v), d)).collect()
}

/// 根 `root` を共有する依頼をまとめて解き、`others` の順に結果を返す。費用の同じ経路があれば、頂点の番号を順に比べていちばん小さいものを返す。
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
        .map(|l| extract(g, s, &mut m, r, *l, toward, 1).map(|mut v| v.swap_remove(0)))
        .collect();
    s.marks = m;
    out
}
