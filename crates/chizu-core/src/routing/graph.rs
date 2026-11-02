//! 経路探索のグラフ。リンクを頂点、ノードでのつながりを辺にする。

use core::fmt;

use crate::command::{NodeId, RoadKind};
use crate::movement::crossing::{self, JAYWALK_DETOUR_MM};
use crate::network::{Dir, LaneId, LaneMode, Line, LinkId, Network, SegmentId, SegmentKind};
use crate::tile::TilePos;
use crate::units::MmPerTick;

use super::Delays;

/// 歩く速さ（時速 5km）。
pub const WALK_SPEED: MmPerTick = MmPerTick(1_389);

/// 区間の種類ごとの車の最高速度。
pub fn speed_limit(kind: SegmentKind) -> MmPerTick {
    match kind {
        // 時速 40km、50km、60km、100km。
        SegmentKind::Road(RoadKind::Small) => MmPerTick(11_111),
        SegmentKind::Road(RoadKind::Medium) => MmPerTick(13_889),
        SegmentKind::Road(RoadKind::Large) => MmPerTick(16_667),
        SegmentKind::Highway => MmPerTick(27_778),
    }
}

/// 長さ `mm` を速さ `speed` で進む時間（ms）。端数は切り上げる。あふれたら `None` を返す。
fn travel_ms(mm: u64, speed: MmPerTick) -> Option<u64> {
    let speed = u64::try_from(speed.0).ok().filter(|s| *s > 0)?;
    Some(mm.checked_mul(1_000)?.div_ceil(speed))
}

/// 網の中身が食い違っていて、グラフを作れない理由。
///
/// 道路を引く・消すコマンドで変えた網では起きない。読み戻した街の状態が、検査の値を通ったまま
/// 中身の整合を崩していたときに返す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// ノードかリンクが指すノードがない。
    MissingNode(NodeId),
    /// ノードかリンクが指す区間がない。
    MissingSegment(SegmentId),
    /// 区間が指す車線がない。
    MissingLane(LaneId),
    /// ノードが、そのノードを端に持たない区間を指す。
    NodeNotOnSegment { node: NodeId, segment: SegmentId },
    /// 区間の両端が 8 方向の直線で結べない。
    NotStraight(SegmentId),
    /// 長さや所要時間、頂点や辺の数が収まらない。
    Overflow,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::MissingNode(n) => write!(f, "ノード {n:?} がない"),
            GraphError::MissingSegment(s) => write!(f, "区間 {s:?} がない"),
            GraphError::MissingLane(l) => write!(f, "車線 {l:?} がない"),
            GraphError::NodeNotOnSegment { node, segment } => {
                write!(f, "ノード {node:?} は区間 {segment:?} の端でない")
            }
            GraphError::NotStraight(s) => write!(f, "区間 {s:?} の両端が 8 方向の直線で結べない"),
            GraphError::Overflow => write!(f, "グラフの大きさか費用があふれた"),
        }
    }
}

/// グラフを作る前に、[`Network`] の読み出しが panic しない網かを確かめる。
///
/// [`Network::links`] や [`Network::segment_line`] は網の整合を前提にしているので、
/// 先に区間の端のノード、区間の車線、ノードの区間を見ておく。
fn check(net: &Network) -> Result<(), GraphError> {
    for (id, seg) in net.segments() {
        segment_line(net, id)?;
        for l in &seg.lanes {
            net.lane(*l).ok_or(GraphError::MissingLane(*l))?;
        }
    }
    for (id, node) in net.nodes() {
        for s in &node.segments {
            let seg = net.segment(*s).ok_or(GraphError::MissingSegment(*s))?;
            if seg.a != id && seg.b != id {
                return Err(GraphError::NodeNotOnSegment {
                    node: id,
                    segment: *s,
                });
            }
        }
    }
    Ok(())
}

/// 区間の直線。[`Network::segment_line`] と同じだが、食い違いを `Err` で返す。
fn segment_line(net: &Network, id: SegmentId) -> Result<Line, GraphError> {
    let s = net.segment(id).ok_or(GraphError::MissingSegment(id))?;
    let tile = |n: NodeId| {
        net.node(n)
            .map(|n| n.tile())
            .ok_or(GraphError::MissingNode(n))
    };
    Line::straight(tile(s.a)?, tile(s.b)?).map_err(|_| GraphError::NotStraight(id))
}

fn to_u32(v: usize) -> Result<u32, GraphError> {
    u32::try_from(v).map_err(|_| GraphError::Overflow)
}

/// ノードでの車線どうしのつながり。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Connector {
    pub node: NodeId,
    pub from: LaneId,
    pub to: LaneId,
    /// 信号の段階。信号を入れるまでは 0 である。
    pub signal_group: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edge {
    pub to: u32,
    /// 曲がるのにかかる費用（ms）。歩道で横断歩道を外した腕を越えるときのほかは 0 である（[`turn_cost`]）。
    pub cost: u64,
}

/// 1 つの手段のグラフ。頂点の番号はリンクの ID の順に振る。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    mode: LaneMode,
    links: Vec<LinkId>,
    /// リンクを端から端まで通る費用（ms）。自由に走れたときの所要時間に、混み具合の遅れを足したもの。
    cost: Vec<u64>,
    /// 自由に走れたときの、リンクを端から端まで通る所要時間（ms）。
    free: Vec<u64>,
    /// 頂点ごとの、強く連結な成分の番号。
    component: Vec<u32>,
    /// 成分ごとの頂点の数。
    component_len: Vec<u32>,
    /// リンクの終わりのノードのマス。ヒューリスティックに使う。
    end_tile: Vec<TilePos>,
    /// 頂点 `v` から出る辺は `edges[edge_start[v]..edge_start[v + 1]]` で、行き先の番号の順に並ぶ。
    edge_start: Vec<u32>,
    edges: Vec<Edge>,
    /// 頂点 `v` に入る辺。`to` は元の頂点で、元の頂点の番号の順に並ぶ。
    rev_start: Vec<u32>,
    rev_edges: Vec<Edge>,
    /// グラフの中でいちばん速いリンクの速さ。
    max_speed: MmPerTick,
}

/// リンクの始まりと終わりのノード。
fn link_ends(net: &Network, link: LinkId) -> Result<(NodeId, NodeId), GraphError> {
    let s = net
        .segment(link.segment)
        .ok_or(GraphError::MissingSegment(link.segment))?;
    Ok(match link.dir {
        Dir::Forward => (s.a, s.b),
        Dir::Backward => (s.b, s.a),
    })
}

/// ノード `n` から出る、手段 `mode` のリンク。区間の ID の順に並ぶ。
fn departures(net: &Network, n: NodeId, mode: LaneMode) -> Result<Vec<LinkId>, GraphError> {
    let node = net.node(n).ok_or(GraphError::MissingNode(n))?;
    let mut out = Vec::new();
    for s in &node.segments {
        let seg = net.segment(*s).ok_or(GraphError::MissingSegment(*s))?;
        let dir = if seg.a == n {
            Dir::Forward
        } else {
            Dir::Backward
        };
        let link = LinkId {
            segment: *s,
            dir,
            mode,
        };
        if net.link_lanes(link).next().is_some() {
            out.push(link);
        }
    }
    Ok(out)
}

/// ノード `n` で、`from` から `to` へ曲がる費用（ms）。歩道では、横断歩道を外した腕を、ほかに回れずに越える
/// 1 本ごとに、[`JAYWALK_DETOUR_MM`] を歩く時間を足す（`movement::crossing`）。車は 0 である。
fn turn_cost(net: &Network, n: NodeId, from: LinkId, to: LinkId) -> Result<u64, GraphError> {
    if from.mode != LaneMode::Sidewalk {
        return Ok(0);
    }
    let count = crossing::removed_crossings(net, n, from.segment, None, to.segment);
    let per = travel_ms(JAYWALK_DETOUR_MM, WALK_SPEED).ok_or(GraphError::Overflow)?;
    u64::try_from(count)
        .ok()
        .and_then(|c| c.checked_mul(per))
        .ok_or(GraphError::Overflow)
}

/// ノード `n` で、`from` から曲がり先 `to` に進めるか。車は行き止まりでだけ折り返せる。
/// ただし「外の世界」ノードは出入りする所なので、車はそこで折り返さない。歩行者はどこでも折り返せる。
fn may_turn(
    net: &Network,
    n: NodeId,
    from: LinkId,
    to: LinkId,
    out_count: usize,
) -> Result<bool, GraphError> {
    let outside = net.node(n).ok_or(GraphError::MissingNode(n))?.outside;
    let dead_end = out_count == 1 && !outside;
    Ok(from.segment != to.segment || from.mode == LaneMode::Sidewalk || dead_end)
}

impl Graph {
    /// 網から手段 `mode` のグラフを作る。網の中身が食い違っていたら panic する。
    /// 道路を引く・消すコマンドで変えた網は食い違わないので、ふだんはこちらを使う。
    pub fn build(net: &Network, mode: LaneMode) -> Graph {
        Graph::try_build(net, mode).expect("網の中身が食い違っている")
    }

    /// 網から手段 `mode` のグラフを作る。網の中身が食い違っていたら `Err` を返す。
    pub fn try_build(net: &Network, mode: LaneMode) -> Result<Graph, GraphError> {
        check(net)?;
        let links: Vec<LinkId> = net.links().filter(|l| l.mode == mode).collect();
        let mut cost = Vec::with_capacity(links.len());
        let mut end_tile = Vec::with_capacity(links.len());
        let mut max_speed = MmPerTick(1);
        for l in &links {
            let seg = net
                .segment(l.segment)
                .ok_or(GraphError::MissingSegment(l.segment))?;
            let speed = match mode {
                LaneMode::Car => speed_limit(seg.kind),
                LaneMode::Sidewalk => WALK_SPEED,
            };
            max_speed = max_speed.max(speed);
            let length = segment_line(net, l.segment)?
                .checked_length()
                .and_then(|m| u64::try_from(m.0).ok())
                .ok_or(GraphError::Overflow)?;
            cost.push(travel_ms(length, speed).ok_or(GraphError::Overflow)?);
            let end = link_ends(net, *l)?.1;
            end_tile.push(net.node(end).ok_or(GraphError::MissingNode(end))?.tile());
        }
        let mut edge_start = Vec::with_capacity(links.len() + 1);
        let mut edges = Vec::new();
        edge_start.push(0);
        for l in &links {
            let node = link_ends(net, *l)?.1;
            let out = departures(net, node, mode)?;
            for to in &out {
                if may_turn(net, node, *l, *to, out.len())? {
                    let cost = turn_cost(net, node, *l, *to)?;
                    let to = links
                        .binary_search(to)
                        .map_err(|_| GraphError::MissingSegment(to.segment))?;
                    edges.push(Edge {
                        to: to_u32(to)?,
                        cost,
                    });
                }
            }
            edge_start.push(to_u32(edges.len())?);
        }
        let (rev_start, rev_edges) = reverse(&edge_start, &edges);
        let (component, component_len) = components(&edge_start, &edges);
        Ok(Graph {
            component,
            component_len,
            mode,
            links,
            free: cost.clone(),
            cost,
            end_tile,
            edge_start,
            edges,
            rev_start,
            rev_edges,
            max_speed,
        })
    }

    pub fn mode(&self) -> LaneMode {
        self.mode
    }

    pub fn vertex_count(&self) -> usize {
        self.links.len()
    }

    pub(crate) fn vertex(&self, link: LinkId) -> Option<u32> {
        let v = self.links.binary_search(&link).ok()?;
        Some(u32::try_from(v).expect("頂点が u32 に収まらない"))
    }

    pub(crate) fn link(&self, v: u32) -> LinkId {
        self.links[v as usize]
    }

    /// リンクの費用を、自由に走れたときの所要時間に `delays` の遅れを足したものにする。
    /// グラフにないリンクの遅れは捨てる。費用は下がらないので、ヒューリスティックは下限のままである。
    pub(crate) fn set_delays(&mut self, delays: &Delays) {
        self.cost.clone_from(&self.free);
        for (link, ms) in delays.iter() {
            if let Some(v) = self.vertex(link) {
                let v = v as usize;
                self.cost[v] = self.free[v].saturating_add(ms);
            }
        }
    }

    /// リンク `from` の終わりからリンク `to` へ、必ず経路があると言えるか。同じ強く連結な成分にあれば
    /// 経路がある。`from` と `to` が同じなら、出て戻ってくる輪があるときだけ `true` を返す。
    /// `false` は「ない」ではなく「ここでは分からない」で、探して確かめる。
    pub(crate) fn surely_connected(&self, from: LinkId, to: LinkId) -> bool {
        let (Some(a), Some(b)) = (self.vertex(from), self.vertex(to)) else {
            return false;
        };
        let c = self.component[a as usize];
        c == self.component[b as usize] && (a != b || self.component_len[c as usize] > 1)
    }

    /// リンクを端から端まで通る費用（ms）。
    pub fn link_cost(&self, link: LinkId) -> Option<u64> {
        self.vertex(link).map(|v| self.cost[v as usize])
    }

    pub(crate) fn cost(&self, v: u32) -> u64 {
        self.cost[v as usize]
    }

    pub(crate) fn edges(&self, v: u32) -> &[Edge] {
        let v = v as usize;
        &self.edges[self.edge_start[v] as usize..self.edge_start[v + 1] as usize]
    }

    /// 頂点 `v` に入る辺。`to` は元の頂点である。
    pub(crate) fn rev_edges(&self, v: u32) -> &[Edge] {
        let v = v as usize;
        &self.rev_edges[self.rev_start[v] as usize..self.rev_start[v + 1] as usize]
    }

    /// 頂点 `v` を出てから頂点 `target` を通り終えるまでの費用の下限。
    ///
    /// 区間は 8 方向の直線なので、2 つのノードのあいだの道のりは 8 方向の距離（オクタイル）より
    /// 短くならない。それをいちばん速いリンクの速さで割り、切り捨てる。
    pub(crate) fn heuristic(&self, v: u32, target: u32) -> u64 {
        let (p, q) = (self.end_tile[v as usize], self.end_tile[target as usize]);
        let dx = p.x.abs_diff(q.x);
        let dy = p.y.abs_diff(q.y);
        let (diag, straight) = (dx.min(dy), dx.max(dy) - dx.min(dy));
        let mm = diag
            .checked_mul(crate::network::DIAGONAL_TILE_MM as u64)
            .and_then(|d| {
                straight
                    .checked_mul(crate::network::TILE_MM as u64)
                    .and_then(|s| s.checked_add(d))
            })
            .expect("距離があふれた");
        let speed = u64::try_from(self.max_speed.0).expect("速さが負");
        mm.checked_mul(1_000).expect("所要時間があふれた") / speed
    }
}

/// 辺の向きを逆にした CSR。元の頂点を番号の順になめるので、入る辺は元の頂点の順に並ぶ。
fn reverse(start: &[u32], edges: &[Edge]) -> (Vec<u32>, Vec<Edge>) {
    let n = start.len() - 1;
    let mut rev_start = vec![0_u32; n + 1];
    for e in edges {
        rev_start[e.to as usize + 1] += 1;
    }
    for v in 0..n {
        rev_start[v + 1] += rev_start[v];
    }
    let mut fill: Vec<u32> = rev_start[..n].to_vec();
    let mut rev = vec![Edge { to: 0, cost: 0 }; edges.len()];
    for u in 0..n {
        for e in &edges[start[u] as usize..start[u + 1] as usize] {
            let at = &mut fill[e.to as usize];
            rev[*at as usize] = Edge {
                to: u32::try_from(u).expect("頂点が u32 に収まらない"),
                cost: e.cost,
            };
            *at += 1;
        }
    }
    (rev_start, rev)
}

/// ノードでの車線のつながりを作る。ノード、元の車線、先の車線の順に並ぶ。
///
/// リンクの車線は区間に並べた順に番号を振り、先の車線 `j` には元の車線 `min(j, 元の数 - 1)` を、
/// 先の車線より多い元の車線には先の最後の車線をつなぐ。どの車線も 1 本以上につながる。
/// 網の中身が食い違っていたら panic する。
pub fn connectors(net: &Network) -> Vec<Connector> {
    try_connectors(net).expect("網の中身が食い違っている")
}

/// [`connectors`] と同じだが、網の中身が食い違っていたら `Err` を返す。
pub fn try_connectors(net: &Network) -> Result<Vec<Connector>, GraphError> {
    check(net)?;
    let mut out = Vec::new();
    for mode in [LaneMode::Car, LaneMode::Sidewalk] {
        for from in net.links().filter(|l| l.mode == mode) {
            let node = link_ends(net, from)?.1;
            let next = departures(net, node, mode)?;
            let from_lanes: Vec<LaneId> = net.link_lanes(from).collect();
            for to in &next {
                if !may_turn(net, node, from, *to, next.len())? {
                    continue;
                }
                let to_lanes: Vec<LaneId> = net.link_lanes(*to).collect();
                let last_from = from_lanes.len() - 1;
                let last_to = to_lanes.len() - 1;
                let pairs = (0..to_lanes.len())
                    .map(|j| (j.min(last_from), j))
                    .chain((to_lanes.len()..from_lanes.len()).map(|i| (i, last_to)));
                out.extend(pairs.map(|(i, j)| Connector {
                    node,
                    from: from_lanes[i],
                    to: to_lanes[j],
                    signal_group: 0,
                }));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// 強く連結な成分を、Tarjan の方法で求める。再帰を使わず、頂点の番号の順に始める。
/// 頂点ごとの成分の番号と、成分ごとの頂点の数を返す。
fn components(edge_start: &[u32], edges: &[Edge]) -> (Vec<u32>, Vec<u32>) {
    let n = edge_start.len() - 1;
    const NONE: u32 = u32::MAX;
    let mut index = vec![NONE; n];
    let mut low = vec![0_u32; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut component = vec![NONE; n];
    let mut lens: Vec<u32> = Vec::new();
    let mut next = 0_u32;
    // (頂点, 次に見る辺の位置)
    let mut work: Vec<(u32, u32)> = Vec::new();
    for root in 0..n {
        if index[root] != NONE {
            continue;
        }
        work.push((root as u32, edge_start[root]));
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root as u32);
        on_stack[root] = true;
        while let Some(&mut (v, ref mut e)) = work.last_mut() {
            let vu = v as usize;
            if *e < edge_start[vu + 1] {
                let w = edges[*e as usize].to as usize;
                *e += 1;
                if index[w] == NONE {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w as u32);
                    on_stack[w] = true;
                    work.push((w as u32, edge_start[w]));
                } else if on_stack[w] {
                    low[vu] = low[vu].min(index[w]);
                }
                continue;
            }
            work.pop();
            if let Some(&(p, _)) = work.last() {
                low[p as usize] = low[p as usize].min(low[vu]);
            }
            if low[vu] == index[vu] {
                let id = u32::try_from(lens.len()).expect("成分の数が u32 に収まらない");
                let mut len = 0_u32;
                loop {
                    let w = stack.pop().expect("成分の頂点がない") as usize;
                    on_stack[w] = false;
                    component[w] = id;
                    len += 1;
                    if w == vu {
                        break;
                    }
                }
                lens.push(len);
            }
        }
    }
    (component, lens)
}
