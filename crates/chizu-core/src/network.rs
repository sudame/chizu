//! 道路と歩道の網。
//!
//! 網はチャンクに分けず、街全体で 1 つのグラフとして持つ。ノードはマスの中心に置き、
//! 区間は縦横と 45 度の斜めの 8 方向の直線に限る。
//!
//! 網は次の決まりを保つ。
//! - 2 本の区間が同じマスを通るのは、そのマスが両方の端のノードのときだけである。
//!   既存の区間の途中のマスを通る道路は引けない。
//! - 2 本の区間が同じ 1 歩（隣り合う 2 マスのあいだ）を通ることはない。
//! - 斜めの区間どうしがマスの角で交わることはない。
//! - どの区間にもつながっていないノードは残さない。

use std::collections::BTreeSet;
use std::marker::PhantomData;

use crate::chunk::Layer;
use crate::command::{InvalidCommand, NodeId, RoadKind};
use crate::round;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::{Mm, Money, WorldPos};

/// 1 マスの辺の長さ（mm）。
pub const TILE_MM: i64 = 8_000;

/// 斜めに 1 マス進む長さ（mm）。8m の √2 倍を mm に丸めた値である。
pub const DIAGONAL_TILE_MM: i64 = 11_314;

/// 1 回のコマンドで引ける道路の長さの上限（マス）。
pub const MAX_ROAD_TILES: u64 = 1_024;

/// 道路の 1 マスあたりの建設費。斜めは長さに比例して高くなる。
pub fn road_cost_per_tile(kind: RoadKind) -> Money {
    match kind {
        RoadKind::Small => Money(250),
        RoadKind::Medium => Money(450),
        RoadKind::Large => Money(750),
    }
}

/// 区間の ID。添え字と、使い回したことを見分ける世代の組である。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SegmentId {
    pub index: u32,
    pub generation: u16,
}

/// 車線の ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LaneId {
    pub index: u32,
    pub generation: u16,
}

/// 世代つきの添え字の ID。
pub trait ArenaId: Copy {
    fn from_parts(index: u32, generation: u16) -> Self;
    fn parts(self) -> (u32, u16);
}

macro_rules! impl_arena_id {
    ($($t:ty),*) => {
        $(
            impl ArenaId for $t {
                fn from_parts(index: u32, generation: u16) -> Self {
                    Self { index, generation }
                }
                fn parts(self) -> (u32, u16) {
                    (self.index, self.generation)
                }
            }

            impl StateHash for $t {
                fn write_state(&self, h: &mut StateHasher) {
                    h.write(&self.index);
                    h.write(&self.generation);
                }
            }
        )*
    };
}

impl_arena_id!(NodeId, SegmentId, LaneId);

/// 世代つきの添え字で引く表。空き番号は後入れ先出しで使い回す。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Arena<I, T> {
    slots: Vec<(u16, Option<T>)>,
    free: Vec<u32>,
    _id: PhantomData<I>,
}

impl<I: ArenaId, T> Arena<I, T> {
    fn new() -> Self {
        Arena {
            slots: Vec::new(),
            free: Vec::new(),
            _id: PhantomData,
        }
    }

    fn insert(&mut self, value: T) -> I {
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.1 = Some(value);
            return I::from_parts(index, slot.0);
        }
        let index = u32::try_from(self.slots.len()).expect("ID の添え字が u32 に収まらない");
        self.slots.push((0, Some(value)));
        I::from_parts(index, 0)
    }

    fn get(&self, id: I) -> Option<&T> {
        let (index, generation) = id.parts();
        match self.slots.get(index as usize) {
            Some((g, Some(v))) if *g == generation => Some(v),
            _ => None,
        }
    }

    fn get_mut(&mut self, id: I) -> Option<&mut T> {
        let (index, generation) = id.parts();
        match self.slots.get_mut(index as usize) {
            Some((g, Some(v))) if *g == generation => Some(v),
            _ => None,
        }
    }

    fn remove(&mut self, id: I) -> Option<T> {
        self.get(id)?;
        let (index, _) = id.parts();
        let slot = &mut self.slots[index as usize];
        // 世代は u16 で一周するまで同じ値に戻らない。一周した古い ID は見分けられない。
        slot.0 = slot.0.wrapping_add(1);
        self.free.push(index);
        slot.1.take()
    }

    fn iter(&self) -> impl Iterator<Item = (I, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, (g, v))| {
            let index = u32::try_from(i).expect("ID の添え字が u32 に収まらない");
            v.as_ref().map(|v| (I::from_parts(index, *g), v))
        })
    }
}

impl<I, T: StateHash> StateHash for Arena<I, T> {
    fn write_state(&self, h: &mut StateHasher) {
        h.write_len(self.slots.len());
        for (generation, value) in &self.slots {
            h.write(generation);
            h.write(value);
        }
        h.write(&self.free);
    }
}

/// 区間の向き。`Forward` はノード `a` から `b` へ向かう。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Dir {
    Forward = 0,
    Backward = 1,
}

/// 車線を使うもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LaneMode {
    Car = 0,
    Sidewalk = 1,
}

/// ノードでの交通の整理。信号は信号を入れるときに足す。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum NodeControl {
    #[default]
    None = 0,
}

/// 区間の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Road(RoadKind),
}

impl SegmentKind {
    /// 片側の車の車線の数。
    pub fn car_lanes_per_side(self) -> u8 {
        match self {
            SegmentKind::Road(k) => k as u8,
        }
    }

    pub fn has_sidewalks(self) -> bool {
        matches!(self, SegmentKind::Road(_))
    }

    fn code(self) -> u8 {
        match self {
            SegmentKind::Road(k) => k as u8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    /// マスの中心のワールド座標。
    pub pos: WorldPos,
    pub layer: Layer,
    pub control: NodeControl,
    /// このノードを端に持つ区間。ID の順に並べる。
    pub segments: Vec<SegmentId>,
}

impl Node {
    pub fn tile(&self) -> TilePos {
        TilePos::new(
            round::div_floor(self.pos.x.0, TILE_MM),
            round::div_floor(self.pos.y.0, TILE_MM),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub a: NodeId,
    pub b: NodeId,
    pub kind: SegmentKind,
    /// 車の車線（`a` から `b` へ、`b` から `a` へ）、歩道（同じ順）の順に並べる。
    pub lanes: Vec<LaneId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lane {
    pub segment: SegmentId,
    pub dir: Dir,
    pub mode: LaneMode,
    pub length: Mm,
}

/// 区間の片側の、同じものが使う車線をまとめた「向きつき区間」。経路探索の単位にする。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LinkId {
    pub segment: SegmentId,
    pub dir: Dir,
    pub mode: LaneMode,
}

/// マスの中心を結ぶ 8 方向の直線。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub start: TilePos,
    /// 1 歩の向き。x と y はそれぞれ -1、0、1 のどれかで、両方が 0 にはならない。
    pub step: (i64, i64),
    /// 歩数。1 以上である。
    pub len: u64,
}

impl Line {
    /// `from` から `to` への直線。8 方向でない、長さが 0、または長すぎるときはエラー。
    pub fn between(from: TilePos, to: TilePos) -> Result<Line, InvalidCommand> {
        let dx = to.x.checked_sub(from.x);
        let dy = to.y.checked_sub(from.y);
        let (Some(dx), Some(dy)) = (dx, dy) else {
            return Err(InvalidCommand::RoadTooLong { from, to });
        };
        let (ax, ay) = (dx.unsigned_abs(), dy.unsigned_abs());
        if ax == 0 && ay == 0 {
            return Err(InvalidCommand::ZeroLengthRoad(from));
        }
        if ax != 0 && ay != 0 && ax != ay {
            return Err(InvalidCommand::RoadNotStraight { from, to });
        }
        let len = ax.max(ay);
        if len > MAX_ROAD_TILES {
            return Err(InvalidCommand::RoadTooLong { from, to });
        }
        Ok(Line {
            start: from,
            step: (dx.signum(), dy.signum()),
            len,
        })
    }

    pub fn is_diagonal(&self) -> bool {
        self.step.0 != 0 && self.step.1 != 0
    }

    /// `k` 歩めのマス。
    pub fn tile(&self, k: u64) -> TilePos {
        let k = i64::try_from(k).expect("歩数が i64 に収まらない");
        let at = |start: i64, step: i64| {
            step.checked_mul(k)
                .and_then(|d| start.checked_add(d))
                .expect("マスの座標があふれた")
        };
        TilePos::new(at(self.start.x, self.step.0), at(self.start.y, self.step.1))
    }

    pub fn end(&self) -> TilePos {
        self.tile(self.len)
    }

    /// 始点から終点までのマス。両端を含む。
    pub fn tiles(&self) -> impl Iterator<Item = TilePos> + '_ {
        (0..=self.len).map(|k| self.tile(k))
    }

    /// マス `t` を通るなら、その歩数。
    pub fn position_of(&self, t: TilePos) -> Option<u64> {
        // 軸ごとの歩数。その軸に進まないときは、ずれがないことだけを確かめて `None` を返す。
        let along = |d: i64, step: i64| -> Result<Option<i64>, ()> {
            match step {
                0 if d == 0 => Ok(None),
                0 => Err(()),
                _ => d.checked_mul(step).map(Some).ok_or(()),
            }
        };
        let kx = along(t.x.checked_sub(self.start.x)?, self.step.0).ok()?;
        let ky = along(t.y.checked_sub(self.start.y)?, self.step.1).ok()?;
        let k = match (kx, ky) {
            (Some(a), Some(b)) if a != b => return None,
            (Some(k), _) | (None, Some(k)) => k,
            (None, None) => return None,
        };
        u64::try_from(k).ok().filter(|k| *k <= self.len)
    }

    /// 長さ（mm）。
    pub fn length(&self) -> Mm {
        let per_step = if self.is_diagonal() {
            DIAGONAL_TILE_MM
        } else {
            TILE_MM
        };
        Mm(per_step)
            .checked_mul(i64::try_from(self.len).expect("歩数が i64 に収まらない"))
            .expect("道路の長さがあふれた")
    }
}

/// 道路を引くときの建設費。端数は切り上げる。
pub fn road_cost(kind: RoadKind, line: &Line) -> Money {
    let per_tile = road_cost_per_tile(kind).0;
    let total = per_tile
        .checked_mul(line.length().0)
        .expect("建設費があふれた");
    Money(round::div_ceil(total, TILE_MM))
}

/// 網に道路を引けなかった理由。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoadConflict {
    /// 既存の区間と同じ 1 歩を通る。
    Overlap,
    /// 斜めの区間とマスの角で交わる。
    CornerCrossing,
    /// 既存の区間の途中のマスを通る。
    MidSegment,
}

/// 道路と歩道の網。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    nodes: Arena<NodeId, Node>,
    segments: Arena<SegmentId, Segment>,
    lanes: Arena<LaneId, Lane>,
}

impl Default for Network {
    fn default() -> Self {
        Network::new()
    }
}

fn tile_center(t: TilePos) -> WorldPos {
    let c = |v: i64| {
        Mm(v.checked_mul(TILE_MM)
            .and_then(|v| v.checked_add(TILE_MM / 2))
            .expect("マスのワールド座標があふれた"))
    };
    WorldPos::new(c(t.x), c(t.y))
}

impl Network {
    pub fn new() -> Network {
        Network {
            nodes: Arena::new(),
            segments: Arena::new(),
            lanes: Arena::new(),
        }
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id)
    }

    pub fn segment(&self, id: SegmentId) -> Option<&Segment> {
        self.segments.get(id)
    }

    pub fn lane(&self, id: LaneId) -> Option<&Lane> {
        self.lanes.get(id)
    }

    /// ノードを ID の順に返す。
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        self.nodes.iter()
    }

    /// 区間を ID の順に返す。
    pub fn segments(&self) -> impl Iterator<Item = (SegmentId, &Segment)> {
        self.segments.iter()
    }

    /// 車線を ID の順に返す。
    pub fn lanes(&self) -> impl Iterator<Item = (LaneId, &Lane)> {
        self.lanes.iter()
    }

    /// 区間がノード `a` からノード `b` へ通るマスの直線。
    pub fn segment_line(&self, id: SegmentId) -> Option<Line> {
        let s = self.segment(id)?;
        let a = self.nodes.get(s.a).expect("区間の端のノードがない").tile();
        let b = self.nodes.get(s.b).expect("区間の端のノードがない").tile();
        Some(Line::between(a, b).expect("区間が 8 方向の直線でない"))
    }

    /// 網のリンクを、区間の ID、向き、使うものの順に返す。
    pub fn links(&self) -> impl Iterator<Item = LinkId> + '_ {
        self.segments.iter().flat_map(move |(id, s)| {
            let mut links: Vec<LinkId> = s
                .lanes
                .iter()
                .map(|l| {
                    let l = self.lanes.get(*l).expect("区間の車線がない");
                    LinkId {
                        segment: id,
                        dir: l.dir,
                        mode: l.mode,
                    }
                })
                .collect();
            links.sort();
            links.dedup();
            links
        })
    }

    /// リンクに含まれる車線。区間に並べた順に返す。
    pub fn link_lanes(&self, link: LinkId) -> impl Iterator<Item = LaneId> + '_ {
        self.segment(link.segment)
            .into_iter()
            .flat_map(|s| s.lanes.iter().copied())
            .filter(move |l| {
                self.lanes
                    .get(*l)
                    .is_some_and(|l| l.dir == link.dir && l.mode == link.mode)
            })
    }

    /// マス `t` を通る区間。
    fn segments_through(&self, t: TilePos) -> Vec<SegmentId> {
        self.segments
            .iter()
            .map(|(id, _)| id)
            .filter(|s| {
                self.segment_line(*s)
                    .is_some_and(|l| l.position_of(t).is_some())
            })
            .collect()
    }

    /// マス `t` にあるノード。
    pub fn node_at(&self, t: TilePos) -> Option<NodeId> {
        self.segments_through(t).into_iter().find_map(|s| {
            let s = self.segment(s)?;
            [s.a, s.b]
                .into_iter()
                .find(|n| self.nodes.get(*n).is_some_and(|n| n.tile() == t))
        })
    }

    fn add_node(&mut self, t: TilePos) -> NodeId {
        self.nodes.insert(Node {
            pos: tile_center(t),
            layer: Layer::Ground,
            control: NodeControl::None,
            segments: Vec::new(),
        })
    }

    fn add_segment(&mut self, a: NodeId, b: NodeId, kind: SegmentKind) -> SegmentId {
        let line = Line::between(self.nodes[a].tile(), self.nodes[b].tile())
            .expect("区間が 8 方向の直線でない");
        let length = line.length();
        let id = self.segments.insert(Segment {
            a,
            b,
            kind,
            lanes: Vec::new(),
        });
        let mut lanes = Vec::new();
        let mut push = |mode, count| {
            for dir in [Dir::Forward, Dir::Backward] {
                for _ in 0..count {
                    lanes.push(self.lanes.insert(Lane {
                        segment: id,
                        dir,
                        mode,
                        length,
                    }));
                }
            }
        };
        push(LaneMode::Car, kind.car_lanes_per_side());
        push(LaneMode::Sidewalk, u8::from(kind.has_sidewalks()));
        self.segments[id].lanes = lanes;
        for n in [a, b] {
            let list = &mut self.nodes[n].segments;
            let at = list.partition_point(|s| *s < id);
            list.insert(at, id);
        }
        id
    }

    /// 区間を消す。端のノードは残す。
    fn remove_segment(&mut self, id: SegmentId) -> Segment {
        let s = self.segments.remove(id).expect("消す区間がない");
        for l in &s.lanes {
            self.lanes.remove(*l);
        }
        for n in [s.a, s.b] {
            self.nodes[n].segments.retain(|x| *x != id);
        }
        s
    }

    /// 道路を引けるか確かめる。マスが建設できる土地かは呼ぶ側で確かめる。
    fn check_road(&self, line: &Line) -> Result<(), RoadConflict> {
        let shares_step = |p: TilePos, q: TilePos| {
            self.segments_through(p).iter().any(|s| {
                self.segment_line(*s)
                    .is_some_and(|l| l.position_of(q).is_some())
            })
        };
        for k in 0..line.len {
            let (p, q) = (line.tile(k), line.tile(k + 1));
            if shares_step(p, q) {
                return Err(RoadConflict::Overlap);
            }
            if line.is_diagonal() {
                let corner_a = TilePos::new(q.x, p.y);
                let corner_b = TilePos::new(p.x, q.y);
                if shares_step(corner_a, corner_b) {
                    return Err(RoadConflict::CornerCrossing);
                }
            }
        }
        for t in line.tiles() {
            if !self.segments_through(t).is_empty() && self.node_at(t).is_none() {
                return Err(RoadConflict::MidSegment);
            }
        }
        Ok(())
    }

    /// 道路を引く。途中で既存のノードを通ったら、そこで区間を分けてつなぐ。
    /// 引けないときは網を変えない。
    pub fn build_road(&mut self, line: &Line, kind: RoadKind) -> Result<(), RoadConflict> {
        self.check_road(line)?;
        let mut cuts = BTreeSet::from([0, line.len]);
        for k in 1..line.len {
            if self.node_at(line.tile(k)).is_some() {
                cuts.insert(k);
            }
        }
        let nodes: Vec<NodeId> = cuts
            .into_iter()
            .map(|k| {
                let t = line.tile(k);
                self.node_at(t).unwrap_or_else(|| self.add_node(t))
            })
            .collect();
        for w in nodes.windows(2) {
            self.add_segment(w[0], w[1], SegmentKind::Road(kind));
        }
        Ok(())
    }

    /// 長方形にかかる区間を消し、つながりのなくなったノードも消す。消した区間の数を返す。
    pub fn bulldoze(&mut self, area: TileRect) -> usize {
        let targets: Vec<SegmentId> = self
            .segments
            .iter()
            .map(|(id, _)| id)
            .filter(|s| {
                let line = self.segment_line(*s).expect("区間がない");
                line.tiles().any(|t| rect_contains(area, t))
            })
            .collect();
        for s in &targets {
            let seg = self.remove_segment(*s);
            for n in [seg.a, seg.b] {
                if self.nodes.get(n).is_some_and(|n| n.segments.is_empty()) {
                    self.nodes.remove(n);
                }
            }
        }
        targets.len()
    }
}

impl core::ops::Index<NodeId> for Arena<NodeId, Node> {
    type Output = Node;
    fn index(&self, id: NodeId) -> &Node {
        self.get(id).expect("ノードがない")
    }
}

impl core::ops::IndexMut<NodeId> for Arena<NodeId, Node> {
    fn index_mut(&mut self, id: NodeId) -> &mut Node {
        self.get_mut(id).expect("ノードがない")
    }
}

impl core::ops::Index<SegmentId> for Arena<SegmentId, Segment> {
    type Output = Segment;
    fn index(&self, id: SegmentId) -> &Segment {
        self.get(id).expect("区間がない")
    }
}

impl core::ops::IndexMut<SegmentId> for Arena<SegmentId, Segment> {
    fn index_mut(&mut self, id: SegmentId) -> &mut Segment {
        self.get_mut(id).expect("区間がない")
    }
}

fn rect_contains(r: TileRect, t: TilePos) -> bool {
    (r.min.x..=r.max.x).contains(&t.x) && (r.min.y..=r.max.y).contains(&t.y)
}

impl StateHash for Node {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.pos);
        h.write(&(self.layer as u8));
        h.write(&(self.control as u8));
        h.write(&self.segments);
    }
}

impl StateHash for Segment {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.a);
        h.write(&self.b);
        h.write(&self.kind.code());
        h.write(&self.lanes);
    }
}

impl StateHash for Lane {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.segment);
        h.write(&(self.dir as u8));
        h.write(&(self.mode as u8));
        h.write(&self.length);
    }
}

impl StateHash for Network {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.nodes);
        h.write(&self.segments);
        h.write(&self.lanes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn line(fx: i64, fy: i64, tx: i64, ty: i64) -> Line {
        Line::between(TilePos::new(fx, fy), TilePos::new(tx, ty)).unwrap()
    }

    fn build(n: &mut Network, fx: i64, fy: i64, tx: i64, ty: i64) -> Result<(), RoadConflict> {
        n.build_road(&line(fx, fy, tx, ty), RoadKind::Small)
    }

    fn degree(n: &Network, x: i64, y: i64) -> usize {
        let id = n.node_at(TilePos::new(x, y)).expect("ノードがない");
        n.node(id).unwrap().segments.len()
    }

    /// 網の決まりと、ノード・区間・車線・索引の食い違いがないことを確かめる。
    fn check_invariants(n: &Network) {
        for (id, node) in n.nodes() {
            assert!(!node.segments.is_empty(), "つながりのないノード {id:?}");
            assert!(node.segments.is_sorted());
            for s in &node.segments {
                let seg = n.segment(*s).expect("ノードが消えた区間を指す");
                assert!(seg.a == id || seg.b == id);
            }
        }
        let ends = |s: SegmentId| {
            let s = n.segment(s).unwrap();
            [s.a, s.b].map(|e| n.node(e).unwrap().tile())
        };
        let tiles: BTreeSet<TilePos> = n.nodes().map(|(_, node)| node.tile()).collect();
        assert_eq!(
            tiles.len(),
            n.nodes().count(),
            "同じマスにノードが 2 つある"
        );
        // すべての区間の 1 歩を、向きをそろえて集める。
        let mut steps = BTreeSet::new();
        for (id, _) in n.segments() {
            let l = n.segment_line(id).unwrap();
            for k in 0..l.len {
                let (p, q) = (l.tile(k), l.tile(k + 1));
                assert!(steps.insert((p.min(q), p.max(q))), "1 歩の重なり {p:?}");
            }
        }
        for (p, q) in &steps {
            if p.x != q.x && p.y != q.y {
                let (a, b) = (TilePos::new(q.x, p.y), TilePos::new(p.x, q.y));
                assert!(!steps.contains(&(a.min(b), a.max(b))), "角での交わり {p:?}");
            }
        }
        let mut lane_count = 0;
        for (id, seg) in n.segments() {
            assert_ne!(seg.a, seg.b);
            for end in [seg.a, seg.b] {
                assert!(n.node(end).unwrap().segments.contains(&id));
            }
            let l = n.segment_line(id).unwrap();
            for lane in &seg.lanes {
                let lane = n.lane(*lane).unwrap();
                assert_eq!((lane.segment, lane.length), (id, l.length()));
            }
            lane_count += seg.lanes.len();
            // 同じマスを通る別の区間とは、両方の端のノードでだけ交わる。
            for t in l.tiles() {
                for other in n.segments_through(t) {
                    if other != id {
                        assert!(ends(id).contains(&t) && ends(other).contains(&t), "{t:?}");
                    }
                }
            }
        }
        assert_eq!(n.lanes().count(), lane_count);
    }

    #[test]
    fn straight_road_makes_one_segment_with_lanes_and_sidewalks() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 10, 0).unwrap();
        let (id, seg) = n.segments().next().unwrap();
        assert_eq!((n.segments().count(), n.nodes().count()), (1, 2));
        assert_eq!(seg.lanes.len(), 4);
        assert_eq!(n.lane(seg.lanes[0]).unwrap().length, Mm(80_000));
        let a = n.node(seg.a).unwrap();
        assert_eq!(a.pos, WorldPos::new(Mm(4_000), Mm(4_000)));
        assert_eq!(n.links().count(), 4);
        let sidewalk = LinkId {
            segment: id,
            dir: Dir::Backward,
            mode: LaneMode::Sidewalk,
        };
        assert_eq!(n.link_lanes(sidewalk).collect::<Vec<_>>(), [seg.lanes[3]]);
        check_invariants(&n);
    }

    #[test]
    fn medium_road_has_two_car_lanes_per_side() {
        let mut n = Network::new();
        n.build_road(&line(0, 0, 3, 3), RoadKind::Medium).unwrap();
        let (id, seg) = n.segments().next().unwrap();
        assert_eq!(seg.lanes.len(), 6);
        let link = LinkId {
            segment: id,
            dir: Dir::Forward,
            mode: LaneMode::Car,
        };
        assert_eq!(n.link_lanes(link).count(), 2);
        let length = n.lane(seg.lanes[0]).unwrap().length;
        assert_eq!(length, Mm(3 * DIAGONAL_TILE_MM));
    }

    #[test]
    fn overlaps_and_corner_crossings_are_refused() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 10, 0).unwrap();
        build(&mut n, 0, 1, 1, 2).unwrap();
        let before = n.clone();
        assert_eq!(build(&mut n, 3, 0, 20, 0), Err(RoadConflict::Overlap));
        assert_eq!(build(&mut n, 10, 0, 2, 0), Err(RoadConflict::Overlap));
        assert_eq!(build(&mut n, 1, 1, 0, 2), Err(RoadConflict::CornerCrossing));
        assert_eq!(build(&mut n, 5, -5, 5, 5), Err(RoadConflict::MidSegment));
        assert_eq!(build(&mut n, 5, 0, 5, 5), Err(RoadConflict::MidSegment));
        assert_eq!(n, before);
        // 端のノードで同じ向きにつなぐのは重なりではない。
        build(&mut n, 10, 0, 15, 0).unwrap();
        assert_eq!(degree(&n, 10, 0), 2);
        check_invariants(&n);
    }

    #[test]
    fn roads_through_a_node_are_cut_there() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 10, 0).unwrap();
        build(&mut n, 10, -5, 10, 5).unwrap();
        build(&mut n, 0, 0, 0, 5).unwrap();
        build(&mut n, 5, 5, 15, -5).unwrap();
        assert_eq!(degree(&n, 10, 0), 5);
        assert_eq!((n.segments().count(), n.nodes().count()), (6, 7));
        check_invariants(&n);
    }

    #[test]
    fn bulldoze_removes_touched_segments_and_lonely_nodes() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 5, 0).unwrap();
        build(&mut n, 5, 0, 10, 0).unwrap();
        build(&mut n, 5, 0, 5, 5).unwrap();
        let area = TileRect::from_corners(TilePos::new(4, 3), TilePos::new(6, 3));
        assert_eq!(n.bulldoze(area), 1);
        assert_eq!((n.segments().count(), n.nodes().count()), (2, 3));
        assert_eq!(degree(&n, 5, 0), 2);
        assert_eq!(n.bulldoze(area), 0);
        check_invariants(&n);
        let all = TileRect::from_corners(TilePos::new(0, 0), TilePos::new(10, 0));
        assert_eq!(n.bulldoze(all), 2);
        assert_eq!(n.nodes().count(), 0);
    }

    #[test]
    fn freed_ids_are_reused_last_in_first_out_with_a_new_generation() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 5, 0).unwrap();
        build(&mut n, 0, 2, 5, 2).unwrap();
        let ids: Vec<_> = n.segments().map(|(id, _)| id).collect();
        n.bulldoze(TileRect::from_corners(
            TilePos::new(0, 0),
            TilePos::new(5, 2),
        ));
        build(&mut n, 0, 4, 5, 4).unwrap();
        let (id, _) = n.segments().next().unwrap();
        assert_eq!((id.index, id.generation), (ids[1].index, 1));
        assert!(n.segment(ids[1]).is_none());
    }

    #[test]
    fn position_of_finds_tiles_on_the_line() {
        let l = line(2, 2, -2, 6);
        assert_eq!(l.position_of(TilePos::new(0, 4)), Some(2));
        assert_eq!(l.position_of(TilePos::new(-2, 6)), Some(4));
        assert_eq!(l.position_of(TilePos::new(-3, 7)), None);
        assert_eq!(l.position_of(TilePos::new(1, 2)), None);
        assert_eq!(l.position_of(TilePos::new(i64::MIN, i64::MAX)), None);
        assert_eq!(line(0, 0, 3, 0).position_of(TilePos::new(2, 1)), None);
    }

    #[test]
    fn diagonal_cost_is_rounded_up() {
        assert_eq!(road_cost(RoadKind::Small, &line(0, 0, 4, 0)), Money(1_000));
        // 250 × 11,314 ÷ 8,000 を切り上げる。
        assert_eq!(road_cost(RoadKind::Small, &line(0, 0, 1, 1)), Money(354));
    }

    proptest! {
        #[test]
        fn edits_keep_the_network_consistent(
            edits in proptest::collection::vec(
                (0_u8..5, 0_i64..12, 0_i64..12, -1_i64..=1, -1_i64..=1, 1_i64..8),
                1..40,
            )
        ) {
            let mut n = Network::new();
            for (op, x, y, sx, sy, len) in edits {
                if op == 0 {
                    let area = TileRect::from_corners(TilePos::new(x, y), TilePos::new(x + len / 2, y));
                    n.bulldoze(area);
                } else if (sx, sy) != (0, 0) {
                    let before = n.clone();
                    if build(&mut n, x, y, x + sx * len, y + sy * len).is_err() {
                        prop_assert_eq!(&n, &before);
                    }
                }
                check_invariants(&n);
            }
        }
    }
}
