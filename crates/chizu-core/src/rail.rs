//! 線路の網と駅（設計 3.10 の 1 と 2）。
//!
//! 線路は道路の網（[`Network`](crate::network::Network)）とは別の網として持つ。道路の経路探索とその覚えた結果は、
//! 線路を引いても変わらない。ノードはマスの中心に置き、線路はノードとノードを結ぶ 8 方向の直線である。
//! 1 本の線路は、上りと下りの 2 本を並べた複線である。
//!
//! 網は次の決まりを保つ。
//! - 2 本の線路が同じマスを通るのは、そのマスが両方の端のノードのときだけである。
//!   途中のマスで既存の線路に当たる線路を引くと、当たったマスで既存の線路を分けてノードを置く（分岐）。
//! - 2 本の線路が同じ 1 歩を通ることはなく、斜めの線路どうしがマスの角で交わることもない。
//! - 1 つのノードに集まる線路は 3 本まで（分岐 1 つ）である。線路を引くときは、ノードで向きを変えられるのは
//!   45 度までとする（[`arms_ok`]）。取り壊しで分岐の根元を消したときは、V 字のノードが残ってよい
//!   （まっすぐと 45 度に分かれる分岐なら 45 度、左右へ 45 度ずつ分かれる Y 字なら 90 度で開く）。
//! - ノードのあいだの長さは [`RAIL_BLOCK_MAX_TILES`] マス（400m）を超えない。長い線路には、引くときに
//!   区切りのノードを等しい間隔で置く。
//! - どの線路にもつながっていないノードは残さない。
//! - 駅の線路（駅の中の真ん中の列）は、駅を置くときに作り、駅を取り壊すときに消す。
//!
//! 道路のマスと線路のマスは、踏切（[`crossing`]）のマスでだけ重なる。踏切にできるかは、呼ぶ側の
//! [`World`](crate::world::World) が確かめる。踏切のマスの両隣（線路に沿って）にはノードを置き、踏切の線路を
//! 短い区間にする。列車は待てる所でだけ止まって先を待つので、踏切の上で止まることがない。
//!
//! 路線と列車（設計 3.10 の 3）は [`line`] と [`train`] にある。線路を引くか取り壊すと、路線ごとに進路を選び直す。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::chunk::Layer;
use crate::command::{Heading, ServiceKind};
use crate::network::{Arena, DIAGONAL_TILE_MM, Line, LinkId, TILE_MM, impl_arena_id};
use crate::round;
use crate::service::Facility;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::Money;
use crate::world::DenyReason;

/// ノードのあいだの長さの上限（マス）。400m で、列車が最高の速さから止まるまでの距離より長い。
/// 斜めの線路は、長さが同じ 400m を超えない歩数（[`block_steps`]）で区切る。
pub const RAIL_BLOCK_MAX_TILES: u64 = 50;

/// 1 つのノードに集まれる線路の数。分岐 1 つまでである。
pub const MAX_TRACKS_PER_NODE: usize = 3;

/// 線路の 1 マスあたりの建設費。斜めは長さに比例して高くなる。
pub const RAIL_COST_PER_TILE: Money = Money(500);

/// 駅の間口（マス）。道路に沿う向きの長さで、駅の線路とホームの長さでもある。
pub const STATION_FRONTAGE: u64 = 10;

/// 駅の奥行き（マス）。道路の側から、駅舎とホーム、複線の線路、ホームの 3 列である。
pub const STATION_DEPTH: u64 = 3;

/// 駅の線路が通る列。道路の側から数えて 0 から始める。
pub const STATION_TRACK_ROW: u64 = 1;

/// 線路の網のノードの ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RailNodeId {
    pub index: u32,
    pub generation: u16,
}

/// 線路の ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrackId {
    pub index: u32,
    pub generation: u16,
}

/// 駅の ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StationId {
    pub index: u32,
    pub generation: u16,
}

/// 路線の ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineId {
    pub index: u32,
    pub generation: u16,
}

/// 列車の ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TrainId {
    pub index: u32,
    pub generation: u16,
}

/// 乗る人の ID。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RiderId {
    pub index: u32,
    pub generation: u16,
}

impl_arena_id!(RailNodeId, TrackId, StationId, LineId, TrainId, RiderId);

pub mod choice;
pub mod crossing;
pub mod line;
pub mod rider;
pub mod train;

pub use crossing::{CROSSING_WARN_TICKS, CrossingTally};
pub use line::{LineCapacity, RailLine, ShortSpan, Step, StepKind};
pub use rider::{
    Exit, ExitFrom, FARE_SECONDS, FARE_WEIGHT_POOR_PERMILLE, FARE_WEIGHT_RICH_PERMILLE,
    INITIAL_FARE, MAX_FARE, QueueKey, RAIL_GIVE_UP, RidePlan, RideTally, Rider, StationWalks,
    TRAIN_CAPACITY, Transfer, fare_weight_permille, money_seconds,
};
pub use train::{Block, RailTally, TRAIN_BLOCK_LEN_MM, Train, TrainState};

/// 線路の網のノード。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RailNode {
    pub tile: TilePos,
    pub layer: Layer,
    /// このノードを端に持つ線路。ID の順に並べる。
    pub tracks: Vec<TrackId>,
}

/// ノード `a` と `b` を結ぶ直線の複線。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub a: RailNodeId,
    pub b: RailNodeId,
    pub layer: Layer,
    /// 駅の中の線路なら、その駅。
    pub station: Option<StationId>,
}

/// 駅。敷地の形と出入口は施設と同じ決まりで持つ（[`Station::site`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    pub area: TileRect,
    /// 道路に面した角のマス（`PlaceBuilding` の `at`）。
    pub front: TilePos,
    pub toward_road: (i64, i64),
    /// 面した道路の歩道のリンク。施設と同じく、道路が変わるたびに付け直し、道路がなくなると `None` になる。
    pub entrance: Option<LinkId>,
    /// 駅の中の線路。両端のノードで、駅の外の線路とつなぐ。
    pub track: TrackId,
}

impl Station {
    /// 施設と同じ形にしたもの。出入口の位置（`facility_spot`）など、施設の決まりをそのまま使うためにある。
    pub fn site(&self) -> Facility {
        Facility {
            kind: ServiceKind::Station,
            area: self.area,
            front: self.front,
            toward_road: self.toward_road,
            entrance: self.entrance,
        }
    }
}

/// 取り壊しで消えたもの。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RailRemoval {
    /// 消えた線路の直線。駅の線路も含む。
    pub tracks: Vec<Line>,
    /// 消えた駅の敷地。
    pub stations: Vec<TileRect>,
}

impl RailRemoval {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty() && self.stations.is_empty()
    }

    /// 使い道を書き直すマス。消えた線路と駅の敷地のマスである。
    pub fn tiles(&self) -> Vec<TilePos> {
        let tracks = self
            .tracks
            .iter()
            .flat_map(|l| l.tiles().collect::<Vec<_>>());
        let stations = self
            .stations
            .iter()
            .flat_map(|r| crate::zone::tiles_of(*r).collect::<Vec<_>>());
        tracks.chain(stations).collect()
    }
}

/// 線路の網と駅。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rail {
    nodes: Arena<RailNodeId, RailNode>,
    tracks: Arena<TrackId, Track>,
    stations: Arena<StationId, Station>,
    lines: Arena<LineId, RailLine>,
    trains: Arena<TrainId, Train>,
    /// マスごとの、そのマスを通る線路。ID の順に並べる。線路から作り直せるので、状態ハッシュには入れない。
    tiles: BTreeMap<TilePos, SmallVec<[TrackId; 2]>>,
    /// 読み取り用の数。状態ハッシュには入れない。
    tally: RailTally,
    /// 踏切のマスと、閉じていれば閉じた tick。閉じた tick は、列車が踏切に入ってよいか（閉じてから
    /// [`CROSSING_WARN_TICKS`] tick たったか）を決めるので、踏切があるときは状態ハッシュに入れる。
    crossings: BTreeMap<TilePos, Option<u64>>,
    /// 踏切の読み取り用の数。状態ハッシュには入れない。
    crossing_tally: CrossingTally,
    /// 駅で待っている人と、列車に乗っている人。
    riders: Arena<RiderId, Rider>,
    /// 休ませている移動から、乗る人を引く表。`riders` から作り直せるので、状態ハッシュには入れない。
    by_trip: BTreeMap<crate::id::TripId, RiderId>,
    /// 駅の待ち行列。（駅、路線、向き）ごとに、着いた順に並べる。
    queues: BTreeMap<QueueKey, VecDeque<RiderId>>,
    /// 列車ごとの、乗っている人。乗った順に並べる。
    aboard: BTreeMap<TrainId, Vec<RiderId>>,
    /// 直前に出た列車に乗りきれずに人が残った（駅、路線、向き）。待ちの見積もりに運転間隔を 1 つ足す。
    left_behind: BTreeSet<QueueKey>,
    /// 列車ごとの、前に止まった所（止まる要素の順の番号）と着いた tick。区間の時間を測るのに使う。
    last_stop: BTreeMap<TrainId, (usize, u64)>,
    /// 運賃。1 回の乗車ごとに街で 1 つの額である。
    fare: u16,
    /// この前に取り出してから集めた運賃。`World` が tick ごとに所持金へ移すので、書き出さない。
    #[serde(skip)]
    fare_due: i64,
    /// 歩きに戻る人。`World` がすぐに取り出すので、書き出さない。
    #[serde(skip)]
    exits: Vec<Exit>,
    /// 乗る人の読み取り用の数。状態ハッシュには入れない。
    ride_tally: RideTally,
}

impl Default for Rail {
    fn default() -> Self {
        Rail::new()
    }
}

/// 線路を引くときの建設費。端数は切り上げる。
pub fn rail_cost(line: &Line) -> Money {
    let total = RAIL_COST_PER_TILE
        .0
        .checked_mul(line.length().0)
        .expect("建設費があふれた");
    Money(round::div_ceil(total, TILE_MM))
}

/// 区切りのノードのあいだの歩数の上限。縦横は [`RAIL_BLOCK_MAX_TILES`]、斜めは長さが同じ 400m を超えない歩数である。
pub fn block_steps(diagonal: bool) -> u64 {
    let max_mm = RAIL_BLOCK_MAX_TILES * TILE_MM as u64;
    if diagonal {
        max_mm / DIAGONAL_TILE_MM as u64
    } else {
        RAIL_BLOCK_MAX_TILES
    }
}

/// 2 つの向きの、45 度を単位にした開き（0〜4）。
fn spread(a: Heading, b: Heading) -> u8 {
    let d = (a as u8).abs_diff(b as u8);
    d.min(8 - d)
}

/// ノードから出ていく線路の向きの組が、決まりに合うか。
///
/// 2 本なら、片方から入った列車がもう片方へ 45 度までの曲がりで抜けられる（向きの開きが 135 度以上）。
/// 3 本なら分岐で、1 本（根元）から残りの 2 本のどちらへも 45 度までの曲がりで抜けられる。
/// 1 本（線路の端）はいつでもよい。4 本以上は分岐が 2 つになるので合わない。
pub fn arms_ok(arms: &[Heading]) -> bool {
    let through = |a: Heading, b: Heading| spread(a, b) >= 3;
    match arms {
        [] | [_] => true,
        [a, b] => through(*a, *b),
        [a, b, c] => {
            (through(*a, *b) && through(*a, *c))
                || (through(*b, *a) && through(*b, *c))
                || (through(*c, *a) && through(*c, *b))
        }
        _ => false,
    }
}

fn heading_of(step: (i64, i64)) -> Heading {
    Heading::of_step(step).expect("線路の 1 歩が 8 方向でない")
}

fn rect_contains(r: TileRect, t: TilePos) -> bool {
    (r.min.x..=r.max.x).contains(&t.x) && (r.min.y..=r.max.y).contains(&t.y)
}

fn overlaps(a: TileRect, b: TileRect) -> bool {
    a.min.x <= b.max.x && b.min.x <= a.max.x && a.min.y <= b.max.y && b.min.y <= a.max.y
}

impl Rail {
    pub fn new() -> Rail {
        Rail {
            nodes: Arena::new(),
            tracks: Arena::new(),
            stations: Arena::new(),
            lines: Arena::new(),
            trains: Arena::new(),
            tiles: BTreeMap::new(),
            tally: RailTally::default(),
            crossings: BTreeMap::new(),
            crossing_tally: CrossingTally::default(),
            riders: Arena::new(),
            by_trip: BTreeMap::new(),
            queues: BTreeMap::new(),
            aboard: BTreeMap::new(),
            left_behind: BTreeSet::new(),
            last_stop: BTreeMap::new(),
            fare: INITIAL_FARE,
            fare_due: 0,
            exits: Vec::new(),
            ride_tally: RideTally::default(),
        }
    }

    /// 線路も駅も一度も置いていないか。置いて取り壊したあとは偽である（ID の使い回しの順が変わるため）。
    /// 状態ハッシュは、これが真のとき鉄道の欄を書かない。
    pub fn is_unused(&self) -> bool {
        self.nodes.is_unused() && self.tracks.is_unused() && self.stations.is_unused()
    }

    /// 状態ハッシュに鉄道の欄を書かなくてよいか。線路も駅も一度も置いておらず、運賃が初期値のときである。
    pub fn is_blank(&self) -> bool {
        self.is_unused() && self.fare == INITIAL_FARE
    }

    pub fn node(&self, id: RailNodeId) -> Option<&RailNode> {
        self.nodes.get(id)
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.get(id)
    }

    pub fn station(&self, id: StationId) -> Option<&Station> {
        self.stations.get(id)
    }

    /// ノードを ID の順に返す。
    pub fn nodes(&self) -> impl Iterator<Item = (RailNodeId, &RailNode)> {
        self.nodes.iter()
    }

    /// 線路を ID の順に返す。
    pub fn tracks(&self) -> impl Iterator<Item = (TrackId, &Track)> {
        self.tracks.iter()
    }

    /// 駅を ID の順に返す。
    pub fn stations(&self) -> impl Iterator<Item = (StationId, &Station)> {
        self.stations.iter()
    }

    /// 線路がノード `a` からノード `b` へ通るマスの直線。
    pub fn track_line(&self, id: TrackId) -> Option<Line> {
        let t = self.tracks.get(id)?;
        let a = self.nodes.get(t.a).expect("線路の端のノードがない").tile;
        let b = self.nodes.get(t.b).expect("線路の端のノードがない").tile;
        Some(Line::straight(a, b).expect("線路が 8 方向の直線でない"))
    }

    /// 線路が通っているマスの数。駅の中の線路のマスも数える。
    pub fn rail_tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// マス `t` を通る線路。ID の順に並ぶ。
    pub fn tracks_through(&self, t: TilePos) -> &[TrackId] {
        self.tiles.get(&t).map_or(&[], |s| s.as_slice())
    }

    /// マス `t` を線路が通っているか。
    pub fn is_rail_tile(&self, t: TilePos) -> bool {
        !self.tracks_through(t).is_empty()
    }

    /// マス `t` にあるノード。
    pub fn node_at(&self, t: TilePos) -> Option<RailNodeId> {
        self.tracks_through(t).iter().find_map(|id| {
            let tr = self.tracks.get(*id)?;
            [tr.a, tr.b]
                .into_iter()
                .find(|n| self.nodes.get(*n).is_some_and(|n| n.tile == t))
        })
    }

    /// マス `t` を敷地に含む駅。
    pub fn station_at(&self, t: TilePos) -> Option<StationId> {
        self.stations
            .iter()
            .find(|(_, s)| rect_contains(s.area, t))
            .map(|(id, _)| id)
    }

    /// マス `t` が駅の線路の端のノードか。駅の範囲のうち、線路を始めても終えてもよいのはこのマスだけである。
    pub fn is_station_end(&self, t: TilePos) -> bool {
        self.node_at(t).is_some_and(|n| {
            self.nodes[n]
                .tracks
                .iter()
                .any(|id| self.tracks[*id].station.is_some())
        })
    }

    /// 隣り合うマス `p` と `q` のあいだの 1 歩を、どれかの線路が通るか。
    fn shares_step(&self, p: TilePos, q: TilePos) -> bool {
        self.tracks_through(p).iter().any(|id| {
            self.track_line(*id)
                .is_some_and(|l| l.position_of(q).is_some())
        })
    }

    /// 斜めの直線 `line` が、どれかの斜めの線路とマスの角で交わるか。道路を引くときに線路と比べるのに使う。
    pub fn crosses_corner(&self, line: &Line) -> bool {
        line.is_diagonal()
            && (0..line.len).any(|k| {
                let (p, q) = (line.tile(k), line.tile(k + 1));
                self.shares_step(TilePos::new(q.x, p.y), TilePos::new(p.x, q.y))
            })
    }

    /// マス `t` から、いまある線路が出ていく向き。ノードならその線路の向き、線路の途中なら前と後ろの 2 つである。
    fn arms_at(&self, t: TilePos) -> SmallVec<[Heading; 4]> {
        let mut arms = SmallVec::new();
        if let Some(n) = self.node_at(t) {
            for id in &self.nodes[n].tracks {
                let tr = &self.tracks[*id];
                let other = if tr.a == n { tr.b } else { tr.a };
                let o = self.nodes[other].tile;
                arms.push(heading_of(((o.x - t.x).signum(), (o.y - t.y).signum())));
            }
        } else if let Some(id) = self.tracks_through(t).first() {
            let step = self.track_line(*id).expect("線路がない").step;
            arms.push(heading_of(step));
            arms.push(heading_of((-step.0, -step.1)));
        }
        arms
    }

    /// 直線のうち、ノードを置く歩数。両端と、既存の線路に当たるマスと、`extra`（踏切の両隣）である。
    fn cuts(&self, line: &Line, extra: &BTreeSet<u64>) -> BTreeSet<u64> {
        let mut cuts = BTreeSet::from([0, line.len]);
        cuts.extend(extra.iter().copied().filter(|k| *k <= line.len));
        for k in 1..line.len {
            if self.is_rail_tile(line.tile(k)) {
                cuts.insert(k);
            }
        }
        cuts
    }

    /// 線路を引けるか確かめる。土地、道路、駅、施設とのかかわりは呼ぶ側で確かめる。
    /// `extra` は、ほかにノードを置く歩数（踏切の両隣）である。
    pub(crate) fn check_rail(&self, line: &Line, extra: &BTreeSet<u64>) -> Result<(), DenyReason> {
        for k in 0..line.len {
            let (p, q) = (line.tile(k), line.tile(k + 1));
            if self.shares_step(p, q) {
                return Err(DenyReason::Overlap);
            }
            if line.is_diagonal()
                && self.shares_step(TilePos::new(q.x, p.y), TilePos::new(p.x, q.y))
            {
                return Err(DenyReason::CornerCrossing);
            }
        }
        let forward = heading_of(line.step);
        let back = heading_of((-line.step.0, -line.step.1));
        for k in self.cuts(line, extra) {
            let mut arms = self.arms_at(line.tile(k));
            if k > 0 {
                arms.push(back);
            }
            if k < line.len {
                arms.push(forward);
            }
            if arms.len() > MAX_TRACKS_PER_NODE {
                return Err(DenyReason::RailTooManyBranches);
            }
            if !arms_ok(&arms) {
                return Err(DenyReason::RailTooSharp);
            }
        }
        Ok(())
    }

    /// 線路を引く。途中で既存の線路に当たれば、そこで分けてつなぐ。長い区間には区切りのノードを置く。
    /// `extra` の歩数（踏切の両隣）にもノードを置く。引けないときは網を変えない。
    pub fn build_rail(&mut self, line: &Line, extra: &BTreeSet<u64>) -> Result<(), DenyReason> {
        self.check_rail(line, extra)?;
        let nodes: Vec<(u64, RailNodeId)> = self
            .cuts(line, extra)
            .into_iter()
            .map(|k| {
                let t = line.tile(k);
                let n = if let Some(n) = self.node_at(t) {
                    n
                } else if let Some(id) = self.tracks_through(t).first().copied() {
                    self.split_track(id, t)
                } else {
                    self.add_node(t)
                };
                (k, n)
            })
            .collect();
        let max = block_steps(line.is_diagonal());
        for w in nodes.windows(2) {
            let ((k0, n0), (k1, n1)) = (w[0], w[1]);
            let len = k1 - k0;
            let pieces = len.div_ceil(max);
            let mut prev = n0;
            for i in 1..pieces {
                let n = self.add_node(line.tile(k0 + len * i / pieces));
                self.add_track(prev, n);
                prev = n;
            }
            self.add_track(prev, n1);
        }
        self.refresh_lines(&BTreeSet::new());
        Ok(())
    }

    /// 駅を置く。`ends` は駅の線路の両端のマスで、どちらにもまだ線路がないこと。
    pub(crate) fn add_station(&mut self, site: &Facility, ends: (TilePos, TilePos)) -> StationId {
        assert!(
            !self.is_rail_tile(ends.0) && !self.is_rail_tile(ends.1),
            "駅の線路の端に線路がある"
        );
        let a = self.add_node(ends.0);
        let b = self.add_node(ends.1);
        let track = self.add_track(a, b);
        let id = self.stations.insert(Station {
            area: site.area,
            front: site.front,
            toward_road: site.toward_road,
            entrance: site.entrance,
            track,
        });
        self.tracks[track].station = Some(id);
        id
    }

    /// 駅の出入口を付け直す。
    pub(crate) fn set_entrances(&mut self, entrances: &[(StationId, Option<LinkId>)]) {
        for (id, link) in entrances {
            if let Some(s) = self.stations.get_mut(*id) {
                s.entrance = *link;
            }
        }
    }

    /// 長方形を取り壊したときに消える駅（ID の順）と線路。長方形にかかる駅と、その駅の線路と、
    /// 範囲にかかった駅の外の線路（ノードからノードまでの区間）である。[`Rail::bulldoze`] はこれを消す。
    pub fn bulldoze_targets(&self, area: TileRect) -> (Vec<StationId>, BTreeSet<TrackId>) {
        let stations: Vec<StationId> = self
            .stations
            .iter()
            .filter(|(_, s)| overlaps(s.area, area))
            .map(|(id, _)| id)
            .collect();
        let mut tracks: BTreeSet<TrackId> = stations
            .iter()
            .filter_map(|id| self.stations.get(*id))
            .map(|s| s.track)
            .collect();
        for (id, tr) in self.tracks.iter() {
            if tr.station.is_none()
                && self
                    .track_line(id)
                    .is_some_and(|l| l.tiles().any(|t| rect_contains(area, t)))
            {
                tracks.insert(id);
            }
        }
        (stations, tracks)
    }

    /// 長方形にかかる駅と線路を消し、つながりのなくなったノードも消す。駅は ID の順に消し、駅の線路も一緒に消す。
    /// 駅の外の線路は、範囲にかかった区間（ノードからノードまで）ごと消す。消すものがなければ網を変えない。
    pub fn bulldoze(&mut self, area: TileRect) -> RailRemoval {
        let mut removal = RailRemoval::default();
        let (gone, tracks) = self.bulldoze_targets(area);
        for id in gone {
            let site = self.stations.get(id).expect("消す駅がない").site();
            self.evict_transfer_waiters(id, site);
            let s = self.stations.remove(id).expect("消す駅がない");
            removal.stations.push(s.area);
        }
        for id in tracks.iter().copied() {
            removal
                .tracks
                .push(self.track_line(id).expect("消す線路がない"));
            let tr = self.remove_track(id);
            for n in [tr.a, tr.b] {
                if self.nodes.get(n).is_some_and(|n| n.tracks.is_empty()) {
                    self.nodes.remove(n);
                }
            }
        }
        if !removal.is_empty() {
            self.refresh_lines(&tracks);
        }
        removal
    }

    /// 駅の外の線路の、建設費の 1 マスあたりの額 × 長さ（mm）の合計。維持費を求めるのに使う。
    /// 駅の線路は駅の建設費に含めるので数えない。
    pub fn track_cost_mm(&self) -> i64 {
        self.tracks
            .iter()
            .filter(|(_, tr)| tr.station.is_none())
            .map(|(id, _)| {
                let line = self.track_line(id).expect("線路がない");
                RAIL_COST_PER_TILE
                    .0
                    .checked_mul(line.length().0)
                    .expect("維持費があふれた")
            })
            .fold(0_i64, |a, b| a.checked_add(b).expect("維持費があふれた"))
    }

    fn add_node(&mut self, t: TilePos) -> RailNodeId {
        self.nodes.insert(RailNode {
            tile: t,
            layer: Layer::Ground,
            tracks: Vec::new(),
        })
    }

    fn add_track(&mut self, a: RailNodeId, b: RailNodeId) -> TrackId {
        let line = Line::straight(self.nodes[a].tile, self.nodes[b].tile)
            .expect("線路が 8 方向の直線でない");
        let id = self.tracks.insert(Track {
            a,
            b,
            layer: Layer::Ground,
            station: None,
        });
        for n in [a, b] {
            let list = &mut self.nodes[n].tracks;
            let at = list.partition_point(|x| *x < id);
            list.insert(at, id);
        }
        for t in line.tiles() {
            let list = self.tiles.entry(t).or_default();
            let at = list.partition_point(|x| *x < id);
            list.insert(at, id);
        }
        id
    }

    /// 線路を消す。端のノードは残す。
    fn remove_track(&mut self, id: TrackId) -> Track {
        let line = self.track_line(id).expect("消す線路がない");
        let tr = self.tracks.remove(id).expect("消す線路がない");
        for n in [tr.a, tr.b] {
            self.nodes[n].tracks.retain(|x| *x != id);
        }
        for t in line.tiles() {
            if let Some(list) = self.tiles.get_mut(&t) {
                list.retain(|x| *x != id);
                if list.is_empty() {
                    self.tiles.remove(&t);
                }
            }
        }
        tr
    }

    /// 線路をマス `t` で 2 つに分け、そこに置いたノードを返す。
    /// 線路の上の列車は、同じ位置の分けた線路へ移す。
    fn split_track(&mut self, id: TrackId, t: TilePos) -> RailNodeId {
        let first_len = Line::straight(self.nodes[self.tracks[id].a].tile, t)
            .expect("分けるマスが線路の上にない")
            .length()
            .0;
        let total = self.track_line(id).expect("分ける線路がない").length().0;
        let tr = self.remove_track(id);
        assert!(tr.station.is_none(), "駅の線路は分けない");
        let n = self.add_node(t);
        let first = self.add_track(tr.a, n);
        let second = self.add_track(n, tr.b);
        self.move_trains_off(id, (first, first_len), (second, total - first_len));
        n
    }
}

impl core::ops::Index<RailNodeId> for Arena<RailNodeId, RailNode> {
    type Output = RailNode;
    fn index(&self, id: RailNodeId) -> &RailNode {
        self.get(id).expect("ノードがない")
    }
}

impl core::ops::IndexMut<RailNodeId> for Arena<RailNodeId, RailNode> {
    fn index_mut(&mut self, id: RailNodeId) -> &mut RailNode {
        self.get_mut(id).expect("ノードがない")
    }
}

impl core::ops::Index<TrackId> for Arena<TrackId, Track> {
    type Output = Track;
    fn index(&self, id: TrackId) -> &Track {
        self.get(id).expect("線路がない")
    }
}

impl core::ops::IndexMut<TrackId> for Arena<TrackId, Track> {
    fn index_mut(&mut self, id: TrackId) -> &mut Track {
        self.get_mut(id).expect("線路がない")
    }
}

impl StateHash for RailNode {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.tile.x);
        h.write(&self.tile.y);
        h.write(&(self.layer as u8));
        h.write(&self.tracks);
    }
}

impl StateHash for Track {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.a);
        h.write(&self.b);
        h.write(&(self.layer as u8));
        h.write(&self.station);
    }
}

impl StateHash for Station {
    fn write_state(&self, h: &mut StateHasher) {
        for t in [self.area.min, self.area.max, self.front] {
            h.write(&t.x);
            h.write(&t.y);
        }
        h.write(&self.toward_road);
        h.write(
            &self
                .entrance
                .map(|e| (e.segment, e.dir as u8, e.mode as u8)),
        );
        h.write(&self.track);
    }
}

impl StateHash for Rail {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.nodes);
        h.write(&self.tracks);
        h.write(&self.stations);
        h.write(&self.lines);
        h.write(&self.trains);
        // 踏切が閉じた tick は、列車が踏切に入ってよいかを決めるので入れる。踏切のない街のハッシュは変えない。
        if !self.crossings.is_empty() {
            h.write(&self.crossings);
        }
        h.write(&self.riders);
        rider::write_queues(&self.queues, h);
        h.write(&self.aboard);
        h.write(&self.left_behind);
        h.write(&self.last_stop);
        h.write(&self.fare);
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod train_tests;

#[cfg(test)]
mod crossing_tests;

#[cfg(test)]
pub(crate) mod rider_tests;
