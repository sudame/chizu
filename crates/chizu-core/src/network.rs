//! 道路と歩道の網。
//!
//! 網はチャンクに分けず、街全体で 1 つのグラフとして持つ。ノードはマスの中心に置き、
//! 区間は縦横と 45 度の斜めの 8 方向の直線に限る。
//!
//! 網は次の決まりを保つ。
//! - 2 本の区間が同じマスを通るのは、そのマスが両方の端のノードのときだけである。
//!   途中のマスで交わる道路を引くと、交わるマスで両方の区間を分ける。
//! - 2 本の区間が同じ 1 歩（隣り合う 2 マスのあいだ）を通ることはない。
//! - 斜めの区間どうしがマスの角で交わることはない。
//! - どの区間にもつながっていないノードは残さない。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use smallvec::SmallVec;

use crate::chunk::Layer;
use crate::command::{Heading, InvalidCommand, NodeId, RoadKind};
use crate::round;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{CHUNK_SIDE, ChunkCoord, TilePos, TileRect};
use crate::units::{Mm, Money, WorldPos};
use crate::world::DenyReason;

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SegmentId {
    pub index: u32,
    pub generation: u16,
}

/// 車線の ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum Dir {
    Forward = 0,
    Backward = 1,
}

/// 車線を使うもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum LaneMode {
    Car = 0,
    Sidewalk = 1,
}

/// ノードでの交通の整理。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum NodeControl {
    #[default]
    None = 0,
    /// 感応式の信号。灯火は tick と交差点のまわりの車と歩く人から決まるので、状態は持たない（`movement::signal`）。
    Signal = 1,
}

/// 区間の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SegmentKind {
    Road(RoadKind),
    /// 外部接続の高速道路。歩道はない。
    Highway,
}

impl SegmentKind {
    /// 片側の車の車線の数。
    pub fn car_lanes_per_side(self) -> u8 {
        match self {
            SegmentKind::Road(k) => k as u8,
            SegmentKind::Highway => 2,
        }
    }

    pub fn has_sidewalks(self) -> bool {
        matches!(self, SegmentKind::Road(_))
    }

    /// 画面に描く車線 1 本の幅（mm）。道路が 1 マスに収まり、半分の幅が小 0.3、中 0.4、大 0.5、
    /// 高速 0.5 マスになるよう、種類ごとに決める。シミュレーションには使わない。
    /// 画面の `web/src/map/lanes.ts` と同じにする。
    pub fn lane_width_mm(self) -> i64 {
        match self {
            SegmentKind::Road(RoadKind::Small) => 1_760,
            SegmentKind::Road(RoadKind::Medium) => 1_280,
            SegmentKind::Road(RoadKind::Large) => 1_120,
            SegmentKind::Highway => 2_000,
        }
    }

    /// 画面に描く歩道 1 本の幅（mm）。歩道のない道路は 0 である。
    pub fn sidewalk_width_mm(self) -> i64 {
        if self.has_sidewalks() { 640 } else { 0 }
    }

    fn code(self) -> u8 {
        match self {
            SegmentKind::Road(k) => k as u8,
            SegmentKind::Highway => 0x10,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// マスの中心のワールド座標。
    pub pos: WorldPos,
    pub layer: Layer,
    pub control: NodeControl,
    /// 横断歩道を外した腕の向き。[`Heading`] の値のビットを立てる。腕の向きで持つので、区間を分けたり
    /// 種類を変えたりして区間の ID が変わっても残る。
    pub no_crosswalk: u8,
    /// 「外の世界」につながるノードか。
    pub outside: bool,
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

    /// 向き `arm` の腕の横断歩道を外してあるか。
    pub fn crosswalk_removed(&self, arm: Heading) -> bool {
        self.no_crosswalk & (1 << arm as u8) != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    pub a: NodeId,
    pub b: NodeId,
    pub kind: SegmentKind,
    /// 車の車線（`a` から `b` へ、`b` から `a` へ）、歩道（同じ順）の順に並べる。
    pub lanes: Vec<LaneId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    pub segment: SegmentId,
    pub dir: Dir,
    pub mode: LaneMode,
    pub length: Mm,
}

/// 区間の片側の、同じものが使う車線をまとめた「向きつき区間」。経路探索の単位にする。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
    /// `from` から `to` への、プレイヤーが引ける道路の直線。8 方向でない、長さが 0、
    /// または `MAX_ROAD_TILES` マスより長いときはエラー。
    pub fn between(from: TilePos, to: TilePos) -> Result<Line, InvalidCommand> {
        let line = Line::straight(from, to)?;
        if line.len > MAX_ROAD_TILES {
            return Err(InvalidCommand::RoadTooLong { from, to });
        }
        Ok(line)
    }

    /// `from` から `to` への 8 方向の直線。長さの上限はない。網の中の区間に使う。
    pub fn straight(from: TilePos, to: TilePos) -> Result<Line, InvalidCommand> {
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
        self.checked_length().expect("道路の長さがあふれた")
    }

    /// 長さ（mm）。`i64` に収まらなければ `None` を返す。
    pub fn checked_length(&self) -> Option<Mm> {
        let per_step = if self.is_diagonal() {
            DIAGONAL_TILE_MM
        } else {
            TILE_MM
        };
        Mm(per_step).checked_mul(i64::try_from(self.len).ok()?)
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

/// 既存の区間 1 本の種類を `from` から `to` に変える費用。CS2 と同じく、上げるときは建設費の差額を払い、
/// 下げるときは払い戻さない。
pub fn road_change_cost(from: RoadKind, to: RoadKind, line: &Line) -> Money {
    let diff = road_cost(to, line).0 - road_cost(from, line).0;
    Money(diff.max(0))
}

/// 道路の種類を変える見積もり。[`Network::plan_road_kind`] で作り、[`Network::change_road_kind`] で網に映す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindChange {
    pub kind: RoadKind,
    /// 種類を変える区間。ID の順に並ぶ。
    pub segments: Vec<SegmentId>,
    /// 費用の合計。
    pub cost: Money,
}

/// 道路と歩道の網。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    nodes: Arena<NodeId, Node>,
    segments: Arena<SegmentId, Segment>,
    lanes: Arena<LaneId, Lane>,
    /// 網が変わるたびに 1 上がる。
    version: u64,
    /// チャンクごとの、そのチャンクのマスを通る区間。ID の順に並べる。区間から作り直せるので、状態ハッシュには入れない。
    index: BTreeMap<ChunkCoord, Vec<SegmentId>>,
    /// マスごとの、そのマスを通る区間。ID の順に並べる。`index` と同じく区間から作り直せる。
    tiles: BTreeMap<TilePos, SmallVec<[SegmentId; 2]>>,
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
            version: 0,
            index: BTreeMap::new(),
            tiles: BTreeMap::new(),
        }
    }

    /// 網が変わるたびに 1 上がる版。道路を引く、消す、高速道路を置くたびに上がる。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 網の整合を崩すテストのために、区間を書き換える。
    #[cfg(test)]
    pub(crate) fn segment_mut(&mut self, id: SegmentId) -> Option<&mut Segment> {
        self.segments.get_mut(id)
    }

    /// 網の整合を崩すテストのために、ノードを書き換える。
    #[cfg(test)]
    pub(crate) fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// 網の整合を崩すテストのために、区間から指されたままのノードを消す。
    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn remove_node_unchecked(&mut self, id: NodeId) {
        self.nodes.remove(id);
    }

    /// 網の整合を崩すテストのために、区間から指されたままの車線を消す。
    #[cfg(test)]
    pub(crate) fn remove_lane_unchecked(&mut self, id: LaneId) {
        self.lanes.remove(id);
    }

    fn bump_version(&mut self) {
        self.version = self.version.checked_add(1).expect("網の版があふれた");
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

    /// チャンクのマスを通る区間。ID の順に並ぶ。
    pub fn segments_in_chunk(&self, coord: ChunkCoord) -> &[SegmentId] {
        self.index.get(&coord).map_or(&[], Vec::as_slice)
    }

    /// 区間がノード `a` からノード `b` へ通るマスの直線。
    pub fn segment_line(&self, id: SegmentId) -> Option<Line> {
        let s = self.segment(id)?;
        let a = self.nodes.get(s.a).expect("区間の端のノードがない").tile();
        let b = self.nodes.get(s.b).expect("区間の端のノードがない").tile();
        Some(Line::straight(a, b).expect("区間が 8 方向の直線でない"))
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

    /// マス `t` を道路が通っているか。
    pub fn is_road_tile(&self, t: TilePos) -> bool {
        !self.segments_through(t).is_empty()
    }

    /// マス `t` を通る区間。
    pub fn segments_through(&self, t: TilePos) -> Vec<SegmentId> {
        self.tiles.get(&t).map_or_else(Vec::new, |s| s.to_vec())
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

    fn add_node(&mut self, t: TilePos, outside: bool) -> NodeId {
        self.nodes.insert(Node {
            pos: tile_center(t),
            layer: Layer::Ground,
            control: NodeControl::None,
            no_crosswalk: 0,
            outside,
            segments: Vec::new(),
        })
    }

    fn add_segment(&mut self, a: NodeId, b: NodeId, kind: SegmentKind) -> SegmentId {
        let line = Line::straight(self.nodes[a].tile(), self.nodes[b].tile())
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
        for coord in chunks_of(&line) {
            let list = self.index.entry(coord).or_default();
            let at = list.partition_point(|s| *s < id);
            list.insert(at, id);
        }
        for t in line.tiles() {
            let list = self.tiles.entry(t).or_default();
            let at = list.partition_point(|s| *s < id);
            list.insert(at, id);
        }
        id
    }

    /// 区間を消す。端のノードは残す。
    fn remove_segment(&mut self, id: SegmentId) -> Segment {
        let line = self.segment_line(id).expect("消す区間がない");
        let s = self.segments.remove(id).expect("消す区間がない");
        for l in &s.lanes {
            self.lanes.remove(*l);
        }
        for n in [s.a, s.b] {
            self.nodes[n].segments.retain(|x| *x != id);
        }
        for coord in chunks_of(&line) {
            if let Some(list) = self.index.get_mut(&coord) {
                list.retain(|x| *x != id);
                if list.is_empty() {
                    self.index.remove(&coord);
                }
            }
        }
        for t in line.tiles() {
            if let Some(list) = self.tiles.get_mut(&t) {
                list.retain(|x| *x != id);
                if list.is_empty() {
                    self.tiles.remove(&t);
                }
            }
        }
        s
    }

    /// 区間をマス `t` で 2 つに分け、そこに置いたノードを返す。
    fn split_segment(&mut self, id: SegmentId, t: TilePos) -> NodeId {
        let s = self.remove_segment(id);
        let n = self.add_node(t, false);
        self.add_segment(s.a, n, s.kind);
        self.add_segment(n, s.b, s.kind);
        n
    }

    /// 外部接続の高速道路を置く。`inner` から `outer` へ延び、`outer` を「外の世界」ノードにする。
    pub(crate) fn add_highway(&mut self, inner: TilePos, outer: TilePos) {
        let a = self.add_node(inner, false);
        let b = self.add_node(outer, true);
        self.add_segment(a, b, SegmentKind::Highway);
        self.bump_version();
    }

    /// 道路を引けるか確かめる。マスが建設できる土地かは呼ぶ側で確かめる。
    fn check_road(&self, line: &Line) -> Result<(), DenyReason> {
        for t in line.tiles() {
            for s in self.segments_through(t) {
                let seg = &self.segments[s];
                let at_inner_node = [seg.a, seg.b]
                    .iter()
                    .any(|n| self.nodes[*n].tile() == t && !self.nodes[*n].outside);
                if seg.kind == SegmentKind::Highway && !at_inner_node {
                    return Err(DenyReason::HighwayMidway);
                }
            }
        }
        let shares_step = |p: TilePos, q: TilePos| {
            self.segments_through(p).iter().any(|s| {
                self.segment_line(*s)
                    .is_some_and(|l| l.position_of(q).is_some())
            })
        };
        for k in 0..line.len {
            let (p, q) = (line.tile(k), line.tile(k + 1));
            if shares_step(p, q) {
                return Err(DenyReason::Overlap);
            }
            if line.is_diagonal() {
                let corner_a = TilePos::new(q.x, p.y);
                let corner_b = TilePos::new(p.x, q.y);
                if shares_step(corner_a, corner_b) {
                    return Err(DenyReason::CornerCrossing);
                }
            }
        }
        Ok(())
    }

    /// 道路を引く。途中で既存の区間やノードに当たったら、そこで区間を分けてつなぐ。
    /// 引けないときは網を変えない。
    pub fn build_road(&mut self, line: &Line, kind: RoadKind) -> Result<(), DenyReason> {
        self.check_road(line)?;
        let mut cuts = BTreeSet::from([0, line.len]);
        for k in 1..line.len {
            if !self.segments_through(line.tile(k)).is_empty() {
                cuts.insert(k);
            }
        }
        let nodes: Vec<NodeId> = cuts
            .into_iter()
            .map(|k| {
                let t = line.tile(k);
                if let Some(n) = self.node_at(t) {
                    n
                } else if let Some(s) = self.segments_through(t).first() {
                    self.split_segment(*s, t)
                } else {
                    self.add_node(t, false)
                }
            })
            .collect();
        for w in nodes.windows(2) {
            self.add_segment(w[0], w[1], SegmentKind::Road(kind));
        }
        self.bump_version();
        Ok(())
    }

    /// 直線 `line` と同じ 1 歩を通る区間。ID の順に並ぶ。
    pub fn segments_along(&self, line: &Line) -> Vec<SegmentId> {
        let mut found = BTreeSet::new();
        for k in 0..line.len {
            let (p, q) = (line.tile(k), line.tile(k + 1));
            for s in self.segments_through(p) {
                if self
                    .segment_line(s)
                    .is_some_and(|l| l.position_of(q).is_some())
                {
                    found.insert(s);
                }
            }
        }
        found.into_iter().collect()
    }

    /// 直線 `line` と同じ 1 歩を通る道路の区間を、区間ごと `kind` に変える見積もり。網は変えない。
    ///
    /// 高速道路は外部接続で、歩道もないので変えない。通る道路がなければ `NoRoadToChange`、
    /// どれもすでに `kind` なら `RoadKindUnchanged` を返す。
    pub fn plan_road_kind(&self, line: &Line, kind: RoadKind) -> Result<KindChange, DenyReason> {
        let mut any_road = false;
        let mut segments = Vec::new();
        let mut cost = Money::ZERO;
        for s in self.segments_along(line) {
            let SegmentKind::Road(old) = self.segments[s].kind else {
                continue;
            };
            any_road = true;
            if old == kind {
                continue;
            }
            let l = self.segment_line(s).expect("区間がない");
            cost = cost + road_change_cost(old, kind, &l);
            segments.push(s);
        }
        if !any_road {
            return Err(DenyReason::NoRoadToChange);
        }
        if segments.is_empty() {
            return Err(DenyReason::RoadKindUnchanged);
        }
        Ok(KindChange {
            kind,
            segments,
            cost,
        })
    }

    /// 見積もった区間の種類を変え、変えた区間の新しい ID を古い ID の順に返す。区間はノードを変えずに
    /// 作り直すので、区間と車線の ID が変わる。走っている移動は、呼ぶ前に
    /// [`crate::movement::Trips::note_before_change`] で控え、呼んだあとに
    /// [`crate::movement::Trips::reroute_noted`] でいまの位置から探し直す。
    pub fn change_road_kind(&mut self, change: &KindChange) -> Vec<SegmentId> {
        let ids = change
            .segments
            .iter()
            .map(|id| {
                let s = self.remove_segment(*id);
                assert!(
                    matches!(s.kind, SegmentKind::Road(_)),
                    "高速道路の種類は変えない"
                );
                self.add_segment(s.a, s.b, SegmentKind::Road(change.kind))
            })
            .collect();
        self.bump_version();
        ids
    }

    /// 交差点 `node` に信号を置くか、外す。3 本以上の区間が集まるノードでなければ置けない。
    /// 信号は経路探索に効かないので、網の版は上げない。
    pub fn set_signal(&mut self, node: NodeId, on: bool) -> Result<(), DenyReason> {
        let n = self.nodes.get_mut(node).ok_or(DenyReason::NotJunction)?;
        if on && n.segments.len() < 3 {
            return Err(DenyReason::NotJunction);
        }
        let control = if on {
            NodeControl::Signal
        } else {
            NodeControl::None
        };
        if n.control == control {
            return Err(DenyReason::SignalUnchanged);
        }
        n.control = control;
        Ok(())
    }

    /// ノード `node` から区間 `segment` へ出ていく腕の向き。
    pub fn arm_heading(&self, node: NodeId, segment: SegmentId) -> Option<Heading> {
        let s = self.segment(segment)?;
        let other = if s.a == node { s.b } else { s.a };
        let (p, q) = (self.node(node)?.tile(), self.node(other)?.tile());
        Heading::of_step(((q.x - p.x).signum(), (q.y - p.y).signum()))
    }

    /// ノード `node` の、向き `arm` の腕の区間。
    pub fn arm_segment(&self, node: NodeId, arm: Heading) -> Option<SegmentId> {
        let n = self.node(node)?;
        n.segments
            .iter()
            .copied()
            .find(|s| self.arm_heading(node, *s) == Some(arm))
    }

    /// 交差点 `node` の腕 `segment` に横断歩道があるか。3 本以上の区間が集まるノードの、
    /// 高速道路でない腕のうち、外していないものにある。
    pub fn has_crosswalk(&self, node: NodeId, segment: SegmentId) -> bool {
        let Some(n) = self.node(node).filter(|n| n.segments.len() >= 3) else {
            return false;
        };
        self.segment(segment)
            .is_some_and(|s| s.kind.has_sidewalks())
            && self
                .arm_heading(node, segment)
                .is_some_and(|h| !n.crosswalk_removed(h))
    }

    /// 交差点 `node` の向き `arm` の腕の横断歩道を、戻す（`on` が `true`）か外す。3 本以上の区間が集まる
    /// ノードの、高速道路でない腕でなければ変えられない。横断歩道は歩く人の経路の費用に効くので、網の版を上げる。
    pub fn set_crosswalk(
        &mut self,
        node: NodeId,
        arm: Heading,
        on: bool,
    ) -> Result<(), DenyReason> {
        let n = self.nodes.get(node).ok_or(DenyReason::NotJunction)?;
        if n.segments.len() < 3 {
            return Err(DenyReason::NotJunction);
        }
        let road = self
            .arm_segment(node, arm)
            .and_then(|s| self.segment(s))
            .is_some_and(|s| s.kind.has_sidewalks());
        if !road {
            return Err(DenyReason::NoCrosswalkArm);
        }
        let n = &mut self.nodes[node];
        if n.crosswalk_removed(arm) != on {
            return Err(DenyReason::CrosswalkUnchanged);
        }
        n.no_crosswalk ^= 1 << arm as u8;
        self.bump_version();
        Ok(())
    }

    /// 長方形にかかる道路の区間を消し、つながりのなくなったノードも消す。消した区間の直線を返す。
    /// 高速道路は消さない。消すものがなければ網を変えずにエラーを返す。
    pub fn bulldoze(&mut self, area: TileRect) -> Result<Vec<Line>, DenyReason> {
        let mut targets = BTreeSet::new();
        for (coord, list) in &self.index {
            if !chunk_overlaps(*coord, area) {
                continue;
            }
            for s in list {
                let seg = &self.segments[*s];
                if seg.kind == SegmentKind::Highway {
                    continue;
                }
                let line = self.segment_line(*s).expect("区間がない");
                if line.tiles().any(|t| rect_contains(area, t)) {
                    targets.insert(*s);
                }
            }
        }
        let mut removed = Vec::new();
        for s in &targets {
            removed.push(self.segment_line(*s).expect("区間がない"));
            let (a, b) = (self.segments[*s].a, self.segments[*s].b);
            let arms = [a, b].map(|n| self.arm_heading(n, *s).expect("区間の腕の向きがない"));
            let seg = self.remove_segment(*s);
            for (n, arm) in [seg.a, seg.b].into_iter().zip(arms) {
                if self.nodes.get(n).is_some_and(|n| n.segments.is_empty()) {
                    self.nodes.remove(n);
                } else if let Some(n) = self.nodes.get_mut(n) {
                    // なくなった腕の横断歩道の印は消す。あとでその向きに引いた腕には横断歩道がある。
                    n.no_crosswalk &= !(1 << arm as u8);
                    if n.segments.len() < 3 {
                        // 交差点でなくなったノードの信号と、横断歩道の印は外す。
                        n.control = NodeControl::None;
                        n.no_crosswalk = 0;
                    }
                }
            }
        }
        if removed.is_empty() {
            return Err(DenyReason::NothingToBulldoze);
        }
        self.bump_version();
        Ok(removed)
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

fn chunk_overlaps(coord: ChunkCoord, r: TileRect) -> bool {
    let o = coord.origin();
    let last = CHUNK_SIDE as i64 - 1;
    o.x <= r.max.x && r.min.x <= o.x + last && o.y <= r.max.y && r.min.y <= o.y + last
}

/// 直線が通るチャンク。座標の順に並ぶ。
fn chunks_of(line: &Line) -> BTreeSet<ChunkCoord> {
    line.tiles()
        .map(|t| t.chunk().expect("チャンクの座標が i32 に収まらないマス"))
        .collect()
}

impl StateHash for Node {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.pos);
        h.write(&(self.layer as u8));
        h.write(&(self.control as u8));
        h.write(&self.no_crosswalk);
        h.write(&self.outside);
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
        h.write(&self.version);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn line(fx: i64, fy: i64, tx: i64, ty: i64) -> Line {
        Line::between(TilePos::new(fx, fy), TilePos::new(tx, ty)).unwrap()
    }

    fn build(n: &mut Network, fx: i64, fy: i64, tx: i64, ty: i64) -> Result<(), DenyReason> {
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
        // 索引は、区間から作り直したものと一致する。
        let mut tiles = BTreeMap::<TilePos, SmallVec<[SegmentId; 2]>>::new();
        let mut index = BTreeMap::<ChunkCoord, Vec<SegmentId>>::new();
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
            for c in chunks_of(&l) {
                index.entry(c).or_default().push(id);
            }
            for t in l.tiles() {
                tiles.entry(t).or_default().push(id);
            }
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
        assert_eq!(n.index, index);
        assert_eq!(n.tiles, tiles);
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
    fn joining_the_middle_of_a_segment_splits_it() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 10, 0).unwrap();
        build(&mut n, 5, 0, 5, 5).unwrap();
        assert_eq!((n.segments().count(), n.nodes().count()), (3, 4));
        assert_eq!(degree(&n, 5, 0), 3);
        check_invariants(&n);
    }

    #[test]
    fn crossing_roads_split_both() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 10, 0).unwrap();
        build(&mut n, 5, -5, 5, 5).unwrap();
        // 斜めの道路も、マスの中心で交わればつながる。
        build(&mut n, 0, -5, 10, 5).unwrap();
        assert_eq!(degree(&n, 5, 0), 6);
        assert_eq!((n.segments().count(), n.nodes().count()), (6, 7));
        check_invariants(&n);
    }

    #[test]
    fn overlaps_and_corner_crossings_are_refused() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 10, 0).unwrap();
        build(&mut n, 0, 1, 1, 2).unwrap();
        let before = n.clone();
        assert_eq!(build(&mut n, 3, 0, 20, 0), Err(DenyReason::Overlap));
        assert_eq!(build(&mut n, 10, 0, 2, 0), Err(DenyReason::Overlap));
        assert_eq!(build(&mut n, 1, 1, 0, 2), Err(DenyReason::CornerCrossing));
        assert_eq!(n, before);
        // 端のノードで同じ向きにつなぐのは重なりではない。
        build(&mut n, 10, 0, 15, 0).unwrap();
        assert_eq!(degree(&n, 10, 0), 2);
        check_invariants(&n);
    }

    #[test]
    fn highway_connects_only_at_its_inner_node() {
        let mut n = Network::new();
        n.add_highway(TilePos::new(0, 0), TilePos::new(100, 0));
        let before = n.clone();
        assert_eq!(build(&mut n, 50, -5, 50, 5), Err(DenyReason::HighwayMidway));
        assert_eq!(
            build(&mut n, 100, 0, 100, 5),
            Err(DenyReason::HighwayMidway)
        );
        assert_eq!(n, before);
        build(&mut n, 0, 0, 0, 5).unwrap();
        assert_eq!(degree(&n, 0, 0), 2);
        // 高速道路は消さない。
        let all = TileRect::from_corners(TilePos::new(-10, -10), TilePos::new(200, 200));
        assert_eq!(n.bulldoze(all).map_or(0, |r| r.len()), 1);
        assert_eq!(n.segments().count(), 1);
        assert_eq!(n.nodes().count(), 2);
        check_invariants(&n);
    }

    #[test]
    fn bulldoze_removes_touched_segments_and_lonely_nodes() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 10, 0).unwrap();
        build(&mut n, 5, 0, 5, 5).unwrap();
        let area = TileRect::from_corners(TilePos::new(4, 3), TilePos::new(6, 3));
        assert_eq!(n.bulldoze(area).map_or(0, |r| r.len()), 1);
        assert_eq!((n.segments().count(), n.nodes().count()), (2, 3));
        assert_eq!(degree(&n, 5, 0), 2);
        assert_eq!(n.bulldoze(area).map_or(0, |r| r.len()), 0);
        check_invariants(&n);
        let all = TileRect::from_corners(TilePos::new(0, 0), TilePos::new(10, 0));
        assert_eq!(n.bulldoze(all).map_or(0, |r| r.len()), 2);
        assert_eq!(n.nodes().count(), 0);
        assert!(n.index.is_empty());
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
        ))
        .unwrap();
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
    fn version_goes_up_only_when_the_network_changes() {
        let mut n = Network::new();
        assert_eq!(n.version(), 0);
        build(&mut n, 0, 0, 10, 0).unwrap();
        build(&mut n, 5, -5, 5, 5).unwrap();
        assert_eq!(n.version(), 2);
        assert_eq!(build(&mut n, 2, 0, 8, 0), Err(DenyReason::Overlap));
        let empty = TileRect::from_corners(TilePos::new(50, 50), TilePos::new(60, 60));
        assert_eq!(n.bulldoze(empty), Err(DenyReason::NothingToBulldoze));
        assert_eq!(n.version(), 2);
        let all = TileRect::from_corners(TilePos::new(0, -5), TilePos::new(10, 5));
        assert_eq!(n.bulldoze(all).map(|r| r.len()), Ok(4));
        assert_eq!(n.version(), 3);
    }

    #[test]
    fn diagonal_cost_is_rounded_up() {
        assert_eq!(road_cost(RoadKind::Small, &line(0, 0, 4, 0)), Money(1_000));
        // 250 × 11,314 ÷ 8,000 を切り上げる。
        assert_eq!(road_cost(RoadKind::Small, &line(0, 0, 1, 1)), Money(354));
    }

    #[test]
    fn changing_the_kind_keeps_the_shape_and_nodes() {
        let mut n = Network::new();
        build(&mut n, 0, 0, 10, 0).unwrap();
        build(&mut n, 5, -5, 5, 5).unwrap();
        let nodes: Vec<_> = n.nodes().map(|(id, node)| (id, node.tile())).collect();
        let lines = |n: &Network| {
            let mut l: Vec<_> = n.segments().map(|(id, _)| n.segment_line(id)).collect();
            l.sort_by_key(|l| l.map(|l| (l.start.x, l.start.y, l.len)));
            l
        };
        let before = lines(&n);
        // 横の道路をなぞる。縦の道路は交差点のマスで触れるだけなので変えない。
        let change = n
            .plan_road_kind(&line(0, 0, 10, 0), RoadKind::Large)
            .unwrap();
        assert_eq!(change.segments.len(), 2);
        // 小から大へ、1 マス 500 の差額を 10 マスぶん払う。
        assert_eq!(change.cost, Money(5_000));
        let version = n.version();
        let ids = n.change_road_kind(&change);
        assert_eq!(n.version(), version + 1);
        assert_eq!(
            n.nodes()
                .map(|(id, node)| (id, node.tile()))
                .collect::<Vec<_>>(),
            nodes
        );
        assert_eq!(lines(&n), before);
        for id in ids {
            let seg = n.segment(id).unwrap();
            assert_eq!(seg.kind, SegmentKind::Road(RoadKind::Large));
            assert_eq!(seg.lanes.len(), 8);
        }
        let vertical = n.segments_along(&line(5, -5, 5, 5));
        assert!(
            vertical
                .iter()
                .all(|s| n.segment(*s).unwrap().kind == SegmentKind::Road(RoadKind::Small))
        );
        check_invariants(&n);
        // 下げるときは払い戻さない。
        let down = n
            .plan_road_kind(&line(2, 0, 3, 0), RoadKind::Medium)
            .unwrap();
        assert_eq!((down.segments.len(), down.cost), (1, Money::ZERO));
        n.change_road_kind(&down);
        check_invariants(&n);
    }

    #[test]
    fn changing_the_kind_is_denied_without_a_road_to_change() {
        let mut n = Network::new();
        n.add_highway(TilePos::new(0, 0), TilePos::new(100, 0));
        build(&mut n, 0, 0, 0, 10).unwrap();
        let plan = |n: &Network, l: Line| n.plan_road_kind(&l, RoadKind::Medium);
        // 高速道路と、道路のない所と、道路を横切るだけの直線。
        for l in [line(0, 0, 20, 0), line(5, 5, 9, 5), line(-3, 4, 3, 4)] {
            assert_eq!(plan(&n, l), Err(DenyReason::NoRoadToChange), "{l:?}");
        }
        let small = line(0, 2, 0, 8);
        assert_eq!(
            n.plan_road_kind(&small, RoadKind::Small),
            Err(DenyReason::RoadKindUnchanged)
        );
        // 高速道路と道路をまたいでなぞると、道路だけを変える。
        let both = n
            .plan_road_kind(&line(0, 5, 0, 0), RoadKind::Medium)
            .unwrap();
        assert_eq!(both.segments.len(), 1);
        // 斜めの区間も長さに比例した差額になる。
        build(&mut n, 1, 11, 5, 15).unwrap();
        let diagonal = n
            .plan_road_kind(&line(1, 11, 5, 15), RoadKind::Medium)
            .unwrap();
        assert_eq!(
            diagonal.cost,
            road_cost(RoadKind::Medium, &line(1, 11, 5, 15))
                - road_cost(RoadKind::Small, &line(1, 11, 5, 15))
        );
    }

    /// 高速道路が `inner` から `outer` への 1 本のまま残り、「外の世界」ノードには高速道路しか
    /// つながっていないことを確かめる。
    fn check_highway(n: &Network, inner: TilePos, outer: TilePos) {
        let highways: Vec<_> = n
            .segments()
            .filter(|(_, s)| s.kind == SegmentKind::Highway)
            .collect();
        assert_eq!(highways.len(), 1, "高速道路が 1 本でない");
        let (id, seg) = highways[0];
        let (a, b) = (n.node(seg.a).unwrap(), n.node(seg.b).unwrap());
        assert_eq!(
            (a.tile(), b.tile()),
            (inner, outer),
            "高速道路の端が変わった"
        );
        assert!(!a.outside && b.outside);
        assert_eq!(b.segments, [id], "外の世界ノードに高速道路以外がつながった");
        let line = n.segment_line(id).unwrap();
        for k in 1..line.len {
            assert_eq!(
                n.node_at(line.tile(k)),
                None,
                "高速道路の途中にノードがある"
            );
        }
        let outside = n.nodes().filter(|(_, node)| node.outside).count();
        assert_eq!(outside, 1);
    }

    proptest! {
        #[test]
        fn edits_keep_the_network_consistent(
            edits in proptest::collection::vec(
                (0_u8..5, 56_i64..72, -8_i64..8, -1_i64..=1, -1_i64..=1, 1_i64..12),
                1..40,
            )
        ) {
            let mut n = Network::new();
            // 編集の範囲を横切り、外の世界ノードも範囲の端に入る高速道路。
            let (inner, outer) = (TilePos::new(60, 0), TilePos::new(80, 0));
            n.add_highway(inner, outer);
            for (op, x, y, sx, sy, len) in edits {
                if op == 0 {
                    let area = TileRect::from_corners(TilePos::new(x, y), TilePos::new(x + len / 2, y));
                    let before = n.clone();
                    if n.bulldoze(area).is_err() {
                        prop_assert_eq!(&n, &before);
                    }
                } else if (sx, sy) != (0, 0) {
                    let before = n.clone();
                    if build(&mut n, x, y, x + sx * len, y + sy * len).is_err() {
                        prop_assert_eq!(&n, &before);
                    }
                }
                check_invariants(&n);
                check_highway(&n, inner, outer);
            }
        }
    }
}
