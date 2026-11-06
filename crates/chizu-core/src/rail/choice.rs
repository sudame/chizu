//! 鉄道の候補の見積もり（設計 3.10 の 4 と 7）。
//!
//! 移動の出発と行き先の建物の正面のマスから [`STATION_WALK_RADIUS`] マス以内の駅を、それぞれ近い順に
//! [`NEAR_STATIONS`] つ選び、9 つの組のうちいちばん費用の小さいものを鉄道の候補にする。費用は移動の秒で、
//! 駅まで歩く時間、待ち、乗る時間、駅から歩く時間に、運賃を [`FARE_SECONDS`] 倍して足す。
//!
//! - 駅の空間索引（[`RailPlanner`]）は、駅の出入口のマスを [`STATION_WALK_RADIUS`] マス四方の区切りごとに並べた表である。
//! - 駅の組の表は、（乗る駅、降りる駅）の組ごとに、乗り換えなしで結ぶ路線と向きと、乗る所と降りる所
//!   （止まる要素の順の番号）を、路線の ID の順に並べて持つ。路線の形だけから作るので、路線か線路か道路が
//!   変わったときだけ作り直す。時間はこの表に持たず、引くたびに路線ごとの区間の時間（測った値を混ぜたもの）の
//!   和と、その時点の走っている本数から求める（[`RideChoice`]）。
//! - 候補になる路線は、止まっておらず、走っている列車が 1 本以上あるものに限る。

use std::collections::{BTreeMap, BTreeSet};

use smallvec::SmallVec;

use super::line::{RailLine, StepKind};
use super::rider::{FARE_SECONDS, QueueKey};
use super::{LineId, Rail, StationId};
use crate::movement::{SidewalkPos, facility_spot};
use crate::network::{Network, TILE_MM};
use crate::routing::WALK_SPEED;
use crate::tile::TilePos;

/// 駅まで歩く範囲（マス）。乗る駅と降りる駅の候補を探す範囲である。
pub const STATION_WALK_RADIUS: i64 = 64;

/// 家の正面のマス `at` が、出入口のマス `door` の徒歩圏（直線で [`STATION_WALK_RADIUS`] マス以内）に入るか。
/// 候補の駅の探し方（[`RailPlanner`]）と、画面の主題図の円と世帯の数が、同じ物差しを使うためにある。
pub fn in_walk_range(at: TilePos, door: TilePos) -> bool {
    let (dx, dy) = (door.x - at.x, door.y - at.y);
    dx * dx + dy * dy <= STATION_WALK_RADIUS * STATION_WALK_RADIUS
}

/// 出発の側と行き先の側で、それぞれ候補にする駅の数。
pub const NEAR_STATIONS: usize = 3;

/// 歩く道のりを、直線の距離から見積もる割合（分子、分母）。碁盤の目の街で直線より遠回りする分である。
pub const WALK_DETOUR: (u64, u64) = (13, 10);

/// 車の移動の時間を直線の距離から見積もる速さ（mm/秒）。車で通勤した人だけの「直線距離 ÷ 測った通勤の時間」の
/// 中央値で決めた（設計 3.10 の「作る順の 6 で決めた細部」）。
pub const CAR_STRAIGHT_LINE_MM_PER_SECOND: u64 = 2_800;

/// 直線の距離 `mm` を歩く時間の見積もり（移動の秒）。
pub fn walk_seconds(mm: u64) -> u64 {
    mm * WALK_DETOUR.0 / WALK_DETOUR.1 / u64::from(WALK_SPEED.0.unsigned_abs())
}

/// 直線の距離 `mm` を車で行く時間の見積もり（移動の秒）。
pub fn car_seconds(mm: u64) -> u64 {
    mm / CAR_STRAIGHT_LINE_MM_PER_SECOND
}

/// マス `a` と `b` の中心どうしの直線の距離（mm）。
pub fn tile_distance_mm(a: TilePos, b: TilePos) -> u64 {
    let dx = a.x.abs_diff(b.x);
    let dy = a.y.abs_diff(b.y);
    let sq = dx
        .checked_mul(dx)
        .and_then(|x| dy.checked_mul(dy).and_then(|y| x.checked_add(y)))
        .expect("距離があふれた");
    sq.isqrt()
        .checked_mul(TILE_MM.unsigned_abs())
        .expect("距離があふれた")
}

/// 路線の止まる所。進路の止まる要素の順に並べる。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopInfo {
    pub station: StationId,
    /// ここに着いた列車に乗る人の向き。
    pub board_dir: u8,
    /// 往復する路線の終点か。終点の先へは乗ったまま進まない。
    pub terminal: bool,
}

/// 路線 `line` の止まる所。
pub fn line_stops(line: &RailLine) -> Vec<StopInfo> {
    line.stops()
        .into_iter()
        .filter_map(|i| {
            let s = line.step(i);
            Some(StopInfo {
                station: s.station?,
                board_dir: line.board_dir_at_arrival(i),
                terminal: s.kind == StepKind::Terminal,
            })
        })
        .collect()
}

/// 駅の並び `stations` の路線の止まる所。進路から求める [`line_stops`] と同じ並びになる。ベンチマークで、
/// 線路を引かずに路線の形だけを作るのに使う。
pub fn line_stops_of(stations: &[StationId], looped: bool) -> Vec<StopInfo> {
    let stop = |station, board_dir, terminal| StopInfo {
        station,
        board_dir,
        terminal,
    };
    if looped {
        return stations.iter().map(|s| stop(*s, 0, false)).collect();
    }
    let n = stations.len();
    let mut out: Vec<StopInfo> = stations[1..n - 1]
        .iter()
        .map(|s| stop(*s, 0, false))
        .collect();
    out.push(stop(stations[n - 1], 1, true));
    out.extend(stations[1..n - 1].iter().rev().map(|s| stop(*s, 1, false)));
    out.push(stop(stations[0], 0, true));
    out
}

/// 乗り換えなしで駅の組を結ぶ路線。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Direct {
    pub line: LineId,
    pub dir: u8,
    /// 乗る所と降りる所の、止まる要素の順の番号。
    pub board: u16,
    pub alight: u16,
}

/// 駅の空間索引と、駅の組の表。路線の形と駅の出入口から作り直せるので、状態には入れない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RailPlanner {
    /// 出入口のある駅。ID の順に並べ、出入口のマスと位置を添える。
    doors: Vec<(StationId, TilePos, SidewalkPos)>,
    /// [`STATION_WALK_RADIUS`] マス四方の区切りごとの、`doors` の添え字。
    cells: BTreeMap<(i64, i64), Vec<u32>>,
    table: BTreeMap<(StationId, StationId), SmallVec<[Direct; 2]>>,
}

fn cell_of(t: TilePos) -> (i64, i64) {
    (
        t.x.div_euclid(STATION_WALK_RADIUS),
        t.y.div_euclid(STATION_WALK_RADIUS),
    )
}

impl RailPlanner {
    /// いまの鉄道と道路の網から作る。出入口のない駅（道路に面していない駅）は候補にしない。
    pub fn build(rail: &Rail, net: &Network) -> RailPlanner {
        let doors: Vec<(StationId, TilePos, SidewalkPos)> = rail
            .stations()
            .filter_map(|(id, s)| {
                let door = facility_spot(net, &s.site())?;
                Some((id, door.tile, door))
            })
            .collect();
        let lines: Vec<(LineId, Vec<StopInfo>)> = rail
            .lines()
            .map(|(id, line)| (id, line_stops(line)))
            .collect();
        RailPlanner::from_parts(doors, &lines)
    }

    /// 駅の出入口と、路線ごとの止まる所から作る。`doors` は駅の ID の順に並べる。
    pub fn from_parts(
        doors: Vec<(StationId, TilePos, SidewalkPos)>,
        lines: &[(LineId, Vec<StopInfo>)],
    ) -> RailPlanner {
        let mut cells: BTreeMap<(i64, i64), Vec<u32>> = BTreeMap::new();
        for (i, (_, t, _)) in doors.iter().enumerate() {
            let i = u32::try_from(i).expect("駅が多すぎる");
            cells.entry(cell_of(*t)).or_default().push(i);
        }
        RailPlanner {
            doors,
            cells,
            table: build_table(lines),
        }
    }

    /// 駅の組の表の、組の数。
    pub fn pairs(&self) -> usize {
        self.table.len()
    }

    /// マス `at` から [`STATION_WALK_RADIUS`] マス以内の出入口の駅を、近い順（同じなら ID の順）に
    /// [`NEAR_STATIONS`] つまで返す。値は `doors` の添え字と直線の距離（mm）である。
    fn near(&self, at: TilePos) -> SmallVec<[(u32, u64); NEAR_STATIONS]> {
        let (cx, cy) = cell_of(at);
        let mut found: SmallVec<[(i64, StationId, u32); 8]> = SmallVec::new();
        for x in cx - 1..=cx + 1 {
            for y in cy - 1..=cy + 1 {
                let Some(list) = self.cells.get(&(x, y)) else {
                    continue;
                };
                for i in list {
                    let (id, t, _) = self.doors[*i as usize];
                    if in_walk_range(at, t) {
                        let (dx, dy) = (t.x - at.x, t.y - at.y);
                        found.push((dx * dx + dy * dy, id, *i));
                    }
                }
            }
        }
        found.sort_unstable();
        found
            .into_iter()
            .take(NEAR_STATIONS)
            .map(|(_, _, i)| {
                let (_, t, _) = self.doors[i as usize];
                (i, tile_distance_mm(at, t))
            })
            .collect()
    }
}

/// 路線ごとの止まる所から、駅の組の表を作る。路線は ID の順に渡す。
fn build_table(
    lines: &[(LineId, Vec<StopInfo>)],
) -> BTreeMap<(StationId, StationId), SmallVec<[Direct; 2]>> {
    let mut table: BTreeMap<(StationId, StationId), SmallVec<[Direct; 2]>> = BTreeMap::new();
    for (line, stops) in lines {
        let m = stops.len();
        for k in 0..m {
            let from = stops[k];
            for d in 1..m {
                let j = (k + d) % m;
                let to = stops[j];
                if to.station != from.station {
                    table
                        .entry((from.station, to.station))
                        .or_default()
                        .push(Direct {
                            line: *line,
                            dir: from.board_dir,
                            board: u16::try_from(k).expect("駅が多すぎる"),
                            alight: u16::try_from(j).expect("駅が多すぎる"),
                        });
                }
                if to.terminal {
                    break;
                }
            }
        }
    }
    table
}

/// 路線の、いまの走っている本数と区間の時間。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineLive {
    /// 走っている列車の数。0 の路線は候補にしない。
    pub running: u32,
    /// 止まる要素の順の、区間の時間の累積の和。先頭は 0 で、最後は 1 周の時間である。
    pub prefix: Vec<u64>,
}

impl LineLive {
    /// 止まる要素ごとの区間の時間 `segments` から作る。
    pub fn new(running: u32, segments: &[u32]) -> LineLive {
        let mut prefix = Vec::with_capacity(segments.len() + 1);
        let mut acc = 0_u64;
        prefix.push(0);
        for s in segments {
            acc += u64::from(*s);
            prefix.push(acc);
        }
        LineLive { running, prefix }
    }

    fn total(&self) -> u64 {
        self.prefix.last().copied().unwrap_or(0)
    }

    /// 止まる要素の順の番号 `from` から `to` まで乗る時間。番号が区間の時間の数を超えれば `None`。
    fn ride(&self, from: u16, to: u16) -> Option<u64> {
        let a = *self.prefix.get(usize::from(from))?;
        let b = *self.prefix.get(usize::from(to))?;
        Some(if b > a { b - a } else { self.total() - a + b })
    }

    /// 運転間隔。
    fn headway(&self) -> u64 {
        self.total() / u64::from(self.running.max(1))
    }
}

/// 鉄道の候補。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RideOffer {
    /// 費用（移動の秒）。
    pub cost: u64,
    pub board: StationId,
    pub line: LineId,
    pub dir: u8,
    pub alight: StationId,
    /// 乗る駅の出入口。
    pub door: SidewalkPos,
}

/// 1 tick の移動の選び方に使う、駅の組の表といまの路線の様子。
#[derive(Clone, Debug)]
pub struct RideChoice<'a> {
    planner: &'a RailPlanner,
    live: BTreeMap<LineId, LineLive>,
    left_behind: BTreeSet<QueueKey>,
    fare_seconds: u64,
}

impl<'a> RideChoice<'a> {
    /// いまの鉄道から作る。
    pub fn new(planner: &'a RailPlanner, rail: &Rail) -> RideChoice<'a> {
        let mut running = BTreeMap::<LineId, u32>::new();
        for (_, t) in rail.trains() {
            *running.entry(t.line).or_default() += 1;
        }
        let live = rail
            .lines()
            .filter(|(_, l)| !l.stopped)
            .filter_map(|(id, l)| {
                let n = running.get(&id).copied().unwrap_or(0);
                (n > 0).then(|| (id, LineLive::new(n, l.segment_ticks())))
            })
            .collect();
        RideChoice::from_parts(planner, live, rail.left_behind().clone(), rail.fare())
    }

    /// 路線の様子を直接渡して作る。ベンチマークで使う。
    pub fn from_parts(
        planner: &'a RailPlanner,
        live: BTreeMap<LineId, LineLive>,
        left_behind: BTreeSet<QueueKey>,
        fare: u16,
    ) -> RideChoice<'a> {
        RideChoice {
            planner,
            live,
            left_behind,
            fare_seconds: u64::from(fare) * FARE_SECONDS,
        }
    }

    /// 候補になる路線が 1 本もないか。
    pub fn is_empty(&self) -> bool {
        self.live.is_empty() || self.planner.doors.is_empty()
    }

    /// 正面のマスが `from` の建物から、正面のマスが `to` の建物へ向かう鉄道の候補。
    /// 費用の同じ組は、（乗る駅、降りる駅、路線）の ID の小さい順に選ぶ。
    pub fn offer(&self, from: TilePos, to: TilePos) -> Option<RideOffer> {
        if self.is_empty() {
            return None;
        }
        let starts = self.planner.near(from);
        if starts.is_empty() {
            return None;
        }
        let goals = self.planner.near(to);
        let mut best: Option<((u64, StationId, StationId, LineId), RideOffer)> = None;
        for (a, wa) in &starts {
            let (sa, _, door) = self.planner.doors[*a as usize];
            for (b, wb) in &goals {
                let (sb, _, _) = self.planner.doors[*b as usize];
                let Some(directs) = self.planner.table.get(&(sa, sb)) else {
                    continue;
                };
                for d in directs {
                    let Some(live) = self.live.get(&d.line) else {
                        continue;
                    };
                    // 表を作ったあとに路線の形が変わっていれば、使わない（表は路線が変わるたびに作り直す）。
                    let Some(ride) = live.ride(d.board, d.alight) else {
                        continue;
                    };
                    let headway = live.headway();
                    let mut wait = headway / 2;
                    if self.left_behind.contains(&(sa, d.line, d.dir)) {
                        wait += headway;
                    }
                    let cost =
                        walk_seconds(*wa) + wait + ride + walk_seconds(*wb) + self.fare_seconds;
                    let key = (cost, sa, sb, d.line);
                    if best.as_ref().is_none_or(|(k, _)| key < *k) {
                        best = Some((
                            key,
                            RideOffer {
                                cost,
                                board: sa,
                                line: d.line,
                                dir: d.dir,
                                alight: sb,
                                door,
                            },
                        ));
                    }
                }
            }
        }
        best.map(|(_, o)| o)
    }
}
