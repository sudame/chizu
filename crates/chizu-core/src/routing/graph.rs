//! 経路探索のグラフ。リンクを頂点、ノードでのつながりを辺にする。

use crate::command::{NodeId, RoadKind};
use crate::network::{Dir, LaneId, LaneMode, LinkId, Network, SegmentKind};
use crate::tile::TilePos;
use crate::units::MmPerTick;

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

/// 長さ `mm` を速さ `speed` で進む時間（ms）。端数は切り上げる。
pub(crate) fn travel_ms(mm: u64, speed: MmPerTick) -> u64 {
    let speed = u64::try_from(speed.0).expect("速さが負");
    mm.checked_mul(1_000)
        .expect("所要時間があふれた")
        .div_ceil(speed)
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
    /// 曲がるのにかかる費用（ms）。右折禁止や信号の待ちを入れるまでは 0 である。
    pub cost: u64,
}

/// 1 つの手段のグラフ。頂点の番号はリンクの ID の順に振る。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    mode: LaneMode,
    links: Vec<LinkId>,
    /// リンクを端から端まで通る費用（ms）。
    cost: Vec<u64>,
    /// リンクの終わりのノードのマス。ヒューリスティックに使う。
    end_tile: Vec<TilePos>,
    /// 頂点 `v` から出る辺は `edges[edge_start[v]..edge_start[v + 1]]` で、行き先の番号の順に並ぶ。
    edge_start: Vec<u32>,
    edges: Vec<Edge>,
    /// グラフの中でいちばん速いリンクの速さ。
    max_speed: MmPerTick,
}

/// リンクの始まりと終わりのノード。
fn link_ends(net: &Network, link: LinkId) -> (NodeId, NodeId) {
    let s = net.segment(link.segment).expect("リンクの区間がない");
    match link.dir {
        Dir::Forward => (s.a, s.b),
        Dir::Backward => (s.b, s.a),
    }
}

/// ノード `n` から出る、手段 `mode` のリンク。区間の ID の順に並ぶ。
fn departures(net: &Network, n: NodeId, mode: LaneMode) -> Vec<LinkId> {
    let node = net.node(n).expect("ノードがない");
    node.segments
        .iter()
        .filter_map(|s| {
            let seg = net.segment(*s).expect("ノードの区間がない");
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
            net.link_lanes(link).next().is_some().then_some(link)
        })
        .collect()
}

/// ノード `n` で、`from` から曲がり先 `to` に進めるか。車は行き止まりでだけ折り返せる。
/// ただし「外の世界」ノードは出入りする所なので、車はそこで折り返さない。歩行者はどこでも折り返せる。
fn may_turn(net: &Network, n: NodeId, from: LinkId, to: LinkId, out_count: usize) -> bool {
    let dead_end = out_count == 1 && !net.node(n).expect("ノードがない").outside;
    from.segment != to.segment || from.mode == LaneMode::Sidewalk || dead_end
}

impl Graph {
    /// 網から手段 `mode` のグラフを作る。
    pub fn build(net: &Network, mode: LaneMode) -> Graph {
        let links: Vec<LinkId> = net.links().filter(|l| l.mode == mode).collect();
        let mut cost = Vec::with_capacity(links.len());
        let mut end_tile = Vec::with_capacity(links.len());
        let mut max_speed = MmPerTick(1);
        for l in &links {
            let seg = net.segment(l.segment).expect("リンクの区間がない");
            let speed = match mode {
                LaneMode::Car => speed_limit(seg.kind),
                LaneMode::Sidewalk => WALK_SPEED,
            };
            max_speed = max_speed.max(speed);
            let length = net
                .segment_line(l.segment)
                .expect("リンクの区間がない")
                .length();
            let length = u64::try_from(length.0).expect("長さが負");
            cost.push(travel_ms(length, speed));
            let end = link_ends(net, *l).1;
            end_tile.push(net.node(end).expect("ノードがない").tile());
        }
        let mut edge_start = Vec::with_capacity(links.len() + 1);
        let mut edges = Vec::new();
        edge_start.push(0);
        for l in &links {
            let node = link_ends(net, *l).1;
            let out = departures(net, node, mode);
            for to in &out {
                if may_turn(net, node, *l, *to, out.len()) {
                    let to = links.binary_search(to).expect("グラフにないリンク");
                    edges.push(Edge {
                        to: u32::try_from(to).expect("頂点が u32 に収まらない"),
                        cost: 0,
                    });
                }
            }
            edge_start.push(u32::try_from(edges.len()).expect("辺が u32 に収まらない"));
        }
        Graph {
            mode,
            links,
            cost,
            end_tile,
            edge_start,
            edges,
            max_speed,
        }
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

/// ノードでの車線のつながりを作る。ノード、元の車線、先の車線の順に並ぶ。
///
/// リンクの車線は区間に並べた順に番号を振り、先の車線 `j` には元の車線 `min(j, 元の数 - 1)` を、
/// 先の車線より多い元の車線には先の最後の車線をつなぐ。どの車線も 1 本以上につながる。
pub fn connectors(net: &Network) -> Vec<Connector> {
    let mut out = Vec::new();
    for mode in [LaneMode::Car, LaneMode::Sidewalk] {
        for from in net.links().filter(|l| l.mode == mode) {
            let node = link_ends(net, from).1;
            let next = departures(net, node, mode);
            let from_lanes: Vec<LaneId> = net.link_lanes(from).collect();
            for to in next
                .iter()
                .filter(|t| may_turn(net, node, from, **t, next.len()))
            {
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
    out
}
