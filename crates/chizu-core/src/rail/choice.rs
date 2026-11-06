//! 鉄道の候補の見積もり（設計 3.10 の 4 と 7）。
//!
//! 移動の出発と行き先の建物の正面のマスから [`STATION_WALK_RADIUS`] マス以内の駅を、それぞれ近い順に
//! [`NEAR_STATIONS`] つ選び、9 つの組のうちいちばん費用の小さいものを鉄道の候補にする。費用は移動の秒で、
//! 駅まで歩く時間、待ち、乗る時間、駅から歩く時間に、運賃を [`FARE_SECONDS`] 倍して足す。
//!
//! - 駅の空間索引（[`RailPlanner`]）は、駅の出入口のマスを [`STATION_WALK_RADIUS`] マス四方の区切りごとに並べた表である。
//! - 駅の組の表は、（乗る駅、降りる駅）の組ごとに、乗り換えなしで結ぶ路線と向きと、乗る所と降りる所
//!   （止まる要素の順の番号）を、路線の ID の順に並べて持つ。乗り換え 1 回の進路も、組ごとに 1 つまで持つ
//!   （[`TransferRoute`]）。路線の形と走らせる本数だけから作るので、路線か線路か道路か本数が変わったときだけ
//!   作り直す。時間はこの表に持たず、引くたびに路線ごとの区間の時間（測った値を混ぜたもの）の和と、その時点の
//!   走っている本数から求める（[`RideChoice`]）。
//! - 候補になる路線は、止まっておらず、走っている列車が 1 本以上あるものに限る。乗り換えるときは両方の路線が
//!   これを満たすものに限る。
//! - 乗り換えの費用は、乗り換える路線の待ちに、乗り換えの手間 [`TRANSFER_SECONDS`] を足したものである。

use std::collections::{BTreeMap, BTreeSet};

use smallvec::SmallVec;

use super::line::{RailLine, StepKind};
use super::rider::{FARE_SECONDS, QueueKey, Transfer, estimate_segments};
use super::{LineId, Rail, StationId};
use crate::movement::{SidewalkPos, facility_spot};
use crate::network::{Network, TILE_MM};
use crate::routing::WALK_SPEED;
use crate::tile::TilePos;

/// 駅まで歩く範囲（マス）。乗る駅と降りる駅の候補を探す範囲である。
pub const STATION_WALK_RADIUS: i64 = 64;

/// 出発の側と行き先の側で、それぞれ候補にする駅の数。
pub const NEAR_STATIONS: usize = 3;

/// 乗り換えの手間（移動の秒）。乗り換える駅での待ちとは別に、乗り換え 1 回ごとに費用へ足す。同じ駅の中で
/// ホームを移る手間と、待ちの見積もりが外れる備えである（設計 3.10 の「作る順の 7 で決めた細部」）。
pub const TRANSFER_SECONDS: u64 = 60;

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

/// 乗り換え 1 回の進路。乗る駅から `first` に乗って駅 `at` で降り、`second` に乗り換えて降りる駅へ向かう。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransferRoute {
    pub first: Direct,
    pub at: StationId,
    pub second: Direct,
}

/// 駅の組を結ぶ進路。乗り換えなしの路線を ID の順にすべてと、乗り換え 1 回の進路を 1 つまで持つ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PairRoutes {
    pub direct: SmallVec<[Direct; 2]>,
    pub transfer: Option<TransferRoute>,
}

/// 駅の組の表を作るのに使う、路線の形。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineShape {
    pub line: LineId,
    /// 止まる所。進路の止まる要素の順に並べる。
    pub stops: Vec<StopInfo>,
    /// 止まる要素ごとの区間の時間の見込み（tick）。進路の形だけから求めた値で、乗り換えの進路を組ごとに 1 つへ
    /// 絞るのに使う。数が `stops` と違えば、その路線を乗り換えに使わない。
    pub segments: Vec<u32>,
    /// 走らせる本数。0 の路線（止まった路線も）は乗り換えに使わない。
    pub trains: u32,
}

impl LineShape {
    /// 区間の時間をすべて `segment` tick とし、本数 1 で走らせる形。
    pub fn uniform(line: LineId, stops: Vec<StopInfo>, segment: u32) -> LineShape {
        let segments = vec![segment; stops.len()];
        LineShape {
            line,
            stops,
            segments,
            trains: 1,
        }
    }
}

/// 駅の空間索引と、駅の組の表。路線の形と駅の出入口から作り直せるので、状態には入れない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RailPlanner {
    /// 出入口のある駅。ID の順に並べ、出入口のマスと位置を添える。
    doors: Vec<(StationId, TilePos, SidewalkPos)>,
    /// [`STATION_WALK_RADIUS`] マス四方の区切りごとの、`doors` の添え字。
    cells: BTreeMap<(i64, i64), Vec<u32>>,
    /// （乗る駅、降りる駅）の順に並べた、駅の組の表。
    table: Vec<((StationId, StationId), PairRoutes)>,
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
        RailPlanner::build_with(rail, net, true)
    }

    /// [`RailPlanner::build`] と同じだが、`transfers` が偽なら乗り換えの進路を表に入れない。
    pub fn build_with(rail: &Rail, net: &Network, transfers: bool) -> RailPlanner {
        let doors: Vec<(StationId, TilePos, SidewalkPos)> = rail
            .stations()
            .filter_map(|(id, s)| {
                let door = facility_spot(net, &s.site())?;
                Some((id, door.tile, door))
            })
            .collect();
        let lines: Vec<LineShape> = rail
            .lines()
            .map(|(id, line)| LineShape {
                line: id,
                stops: line_stops(line),
                segments: estimate_segments(line),
                trains: if line.stopped || !transfers {
                    0
                } else {
                    u32::from(line.allowed())
                },
            })
            .collect();
        RailPlanner::from_shapes(doors, &lines)
    }

    /// 駅の出入口と、路線ごとの止まる所から作る。`doors` は駅の ID の順に並べる。乗り換えの進路は、
    /// どの路線も区間の時間を 100 tick、本数を 1 とみて絞る。
    pub fn from_parts(
        doors: Vec<(StationId, TilePos, SidewalkPos)>,
        lines: &[(LineId, Vec<StopInfo>)],
    ) -> RailPlanner {
        let shapes: Vec<LineShape> = lines
            .iter()
            .map(|(id, stops)| LineShape::uniform(*id, stops.clone(), 100))
            .collect();
        RailPlanner::from_shapes(doors, &shapes)
    }

    /// 駅の出入口と、路線の形から作る。`doors` は駅の ID の順に、`lines` は路線の ID の順に並べる。
    pub fn from_shapes(
        doors: Vec<(StationId, TilePos, SidewalkPos)>,
        lines: &[LineShape],
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

    /// 駅の組の表のうち、乗り換え 1 回の進路を持つ組の数。
    pub fn transfer_pairs(&self) -> usize {
        self.table
            .iter()
            .filter(|(_, r)| r.transfer.is_some())
            .count()
    }

    /// 駅 `from` から駅 `to` への進路。
    pub fn routes(&self, from: StationId, to: StationId) -> Option<&PairRoutes> {
        let i = self
            .table
            .binary_search_by_key(&(from, to), |(k, _)| *k)
            .ok()?;
        Some(&self.table[i].1)
    }

    /// マス `at` から [`STATION_WALK_RADIUS`] マス以内の出入口の駅を、近い順（同じなら ID の順）に
    /// [`NEAR_STATIONS`] つまで返す。値は `doors` の添え字と直線の距離（mm）である。
    fn near(&self, at: TilePos) -> SmallVec<[(u32, u64); NEAR_STATIONS]> {
        let (cx, cy) = cell_of(at);
        let r2 = STATION_WALK_RADIUS * STATION_WALK_RADIUS;
        let mut found: SmallVec<[(i64, StationId, u32); 8]> = SmallVec::new();
        for x in cx - 1..=cx + 1 {
            for y in cy - 1..=cy + 1 {
                let Some(list) = self.cells.get(&(x, y)) else {
                    continue;
                };
                for i in list {
                    let (id, t, _) = self.doors[*i as usize];
                    let (dx, dy) = (t.x - at.x, t.y - at.y);
                    let d2 = dx * dx + dy * dy;
                    if d2 <= r2 {
                        found.push((d2, id, *i));
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

/// 乗り換えの進路を絞るときの、1 つの路線に乗る区切りと、その費用の見込み（待ちと乗る時間の tick）。
#[derive(Clone, Copy, Debug)]
struct Hop {
    direct: Direct,
    /// 路線の添え字（路線の ID の順）。
    li: u16,
    cost: u64,
}

impl Hop {
    fn key(&self) -> (u64, LineId) {
        (self.cost, self.direct.line)
    }
}

/// 行き先ごとの、路線の違う安い区切りを 2 つまで。乗り換える前と後で同じ路線を選ばないよう、2 つ持つ。
type Best2 = [Option<Hop>; 2];

/// `best` に区切り `h` を入れる。同じ路線は安いほうだけを残す。
fn keep_best2(best: &mut Best2, h: Hop) {
    match *best {
        [None, _] => best[0] = Some(h),
        [Some(b0), _] if b0.direct.line == h.direct.line => {
            if h.key() < b0.key() {
                best[0] = Some(h);
            }
        }
        [Some(b0), _] if h.key() < b0.key() => {
            // 前のいちばん安い区切りは 2 つ目に下げる。前の 2 つ目は、`h` と同じ路線でも違う路線でも、
            // どちらより高いので捨てる。
            best[1] = Some(b0);
            best[0] = Some(h);
        }
        [Some(_), None] => best[1] = Some(h),
        [Some(_), Some(b1)] => {
            if h.key() < b1.key() {
                best[1] = Some(h);
            }
        }
    }
}

/// 路線ごとの、止まる所の駅の番号、区間の時間の累積の和、待ちの見込み。
struct LineInfo {
    at: Vec<usize>,
    prefix: Vec<u64>,
    /// 運転間隔の見込みの半分。`None` の路線は乗り換えに使わない。
    wait: Option<u64>,
}

impl LineInfo {
    /// 止まる所の番号 `k` から、進む向きに `j` まで乗る時間の見込み。
    fn ride(&self, k: usize, j: usize) -> u64 {
        let (a, b) = (self.prefix[k], self.prefix[j]);
        if j > k {
            b - a
        } else {
            self.prefix[self.prefix.len() - 1] - a + b
        }
    }
}

/// 路線ごとの止まる所から、駅の組の表を作る。路線は ID の順に渡す。
///
/// 乗り換えなしの路線は、組ごとにすべて持つ。乗り換え 1 回の進路は、組ごとに見込みの費用（両方の路線の待ちと
/// 乗る時間）がいちばん小さいもの 1 つを持つ。乗る駅ごとに、1 本で行ける駅（乗り換える駅）へは路線の違う安い
/// 区切りを 2 つだけ残し、乗り換え先の路線ごとに止まる所を進む順に 1 度たどって選ぶ。手間は「駅の数 × 路線の
/// 止まる所の数の和」で、組ごとに乗り換える駅をすべて試す S² × S にはならない。費用の同じ進路は（乗る路線、
/// 乗り換える駅、乗り換え先の路線）の ID の小さい順に選ぶ。
fn build_table(lines: &[LineShape]) -> Vec<((StationId, StationId), PairRoutes)> {
    let mut ids: Vec<StationId> = lines
        .iter()
        .flat_map(|l| l.stops.iter().map(|s| s.station))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let n = ids.len();
    let dense = |s: StationId| ids.binary_search(&s).expect("駅の番号がない");
    let infos: Vec<LineInfo> = lines
        .iter()
        .map(|l| {
            let usable = l.trains > 0 && l.segments.len() == l.stops.len();
            let live = LineLive::new(l.trains, &l.segments);
            LineInfo {
                at: l.stops.iter().map(|s| dense(s.station)).collect(),
                wait: usable.then(|| live.headway() / 2),
                prefix: live.prefix,
            }
        })
        .collect();
    // 駅ごとの止まる所。（路線の添え字、止まる所の番号）を、路線の ID の順、番号の順に並べる。
    let mut at: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    for (li, info) in infos.iter().enumerate() {
        for (k, x) in info.at.iter().enumerate() {
            at[*x].push((li, k));
        }
    }
    let order = |v: usize| u16::try_from(v).expect("駅が多すぎる");
    // 駅ごとの、1 本で行ける駅への乗り換えなしの路線（すべて）と、乗り換えに使える安い区切り（2 つまで）。
    // どちらも行き先の駅の番号の順に並べる。
    let mut rows: Vec<Vec<(usize, SmallVec<[Direct; 2]>)>> = Vec::with_capacity(n);
    let mut reach: Vec<Vec<(usize, Best2)>> = Vec::with_capacity(n);
    let mut directs: Vec<SmallVec<[Direct; 2]>> = vec![SmallVec::new(); n];
    let mut best: Vec<Best2> = vec![[None, None]; n];
    let mut seen = vec![false; n];
    let mut touched: Vec<usize> = Vec::new();
    for from in &at {
        for &(li, k) in from {
            let (shape, info) = (&lines[li], &infos[li]);
            let m = shape.stops.len();
            let src = shape.stops[k];
            for d in 1..m {
                let j = (k + d) % m;
                let to = shape.stops[j];
                if to.station != src.station {
                    let b = info.at[j];
                    if !seen[b] {
                        seen[b] = true;
                        touched.push(b);
                    }
                    let direct = Direct {
                        line: shape.line,
                        dir: src.board_dir,
                        board: order(k),
                        alight: order(j),
                    };
                    directs[b].push(direct);
                    if let Some(wait) = info.wait {
                        let cost = wait + info.ride(k, j);
                        let li = u16::try_from(li).expect("路線が多すぎる");
                        keep_best2(&mut best[b], Hop { direct, li, cost });
                    }
                }
                if to.terminal {
                    break;
                }
            }
        }
        touched.sort_unstable();
        let mut row = Vec::with_capacity(touched.len());
        let mut hops = Vec::new();
        for b in touched.drain(..) {
            seen[b] = false;
            row.push((b, std::mem::take(&mut directs[b])));
            let h = std::mem::take(&mut best[b]);
            if h[0].is_some() {
                hops.push((b, h));
            }
        }
        rows.push(row);
        reach.push(hops);
    }
    // 乗る駅ごとに、乗り換え 1 回の進路を、降りる駅ごとに 1 つ選ぶ。乗り換え先の路線ごとに、止まる所を進む順に
    // たどり、そこまでに乗り換えられた所のうち、いちばん安く乗れる所を覚えながら進む。路線の止まる所の数の和に
    // 比例する手間で、乗る駅 1 つぶんが済む。
    let runs: Vec<Option<Runs>> = lines
        .iter()
        .zip(&infos)
        .map(|(l, info)| info.wait.map(|w| Runs::new(l, info, w)))
        .collect();
    // 乗り換える駅ごとの、乗る駅から行く路線の違う安い区切り 2 つ（費用と路線の添え字）。
    let mut into: Vec<[(i64, u16); 2]> = vec![[(0, NO_LINE); 2]; n];
    // 降りる駅ごとの、いちばんよい進路の選ぶ順（[`pack_key`]）と、乗り換え先の路線の乗る所と降りる所。
    let mut picks: Vec<u128> = vec![u128::MAX; n];
    let mut seconds: Vec<(u16, u16)> = vec![(0, 0); n];
    let mut table = Vec::new();
    for (a, row) in rows.into_iter().enumerate() {
        for (x, h) in &reach[a] {
            for (s, hop) in h.iter().enumerate() {
                if let Some(hop) = hop {
                    into[*x][s] = (i64::try_from(hop.cost).expect("費用があふれた"), hop.li);
                }
            }
        }
        for (li2, line_runs) in runs.iter().enumerate() {
            let Some(r) = line_runs else {
                continue;
            };
            let l2 = u16::try_from(li2).expect("路線が多すぎる");
            // 駅 `x` で乗り換えるときの、乗り換える前の費用と路線。乗り換え先と同じ路線は使わない。
            let board = |x: u32| -> (i64, u16) {
                let h = &into[x as usize];
                h[usize::from(h[0].1 == l2)]
            };
            let offer = |b: &RunStop, cost: i64, by: &Boarding| {
                if b.x as usize == a {
                    return;
                }
                let cost = u64::try_from(cost).expect("費用が負");
                let key = pack_key(cost, by.l1, by.x, l2);
                if key < picks[b.x as usize] {
                    picks[b.x as usize] = key;
                    seconds[b.x as usize] = (by.j, b.j);
                }
            };
            r.sweep(board, offer);
        }
        for (x, _) in &reach[a] {
            into[*x] = [(0, NO_LINE); 2];
        }
        // 乗り換えなしの行と乗り換えの行を、降りる駅の番号の順に合わせて並べる。
        let mut direct = row.into_iter().peekable();
        for (b, pick) in picks.iter_mut().enumerate() {
            let mut routes = PairRoutes::default();
            if direct.peek().is_some_and(|(d, _)| *d == b) {
                routes.direct = direct.next().expect("行がない").1;
            }
            let key = std::mem::replace(pick, u128::MAX);
            if key != u128::MAX {
                let (l1, x, l2) = unpack_key(key);
                let x = x as usize;
                let k = reach[a]
                    .binary_search_by_key(&x, |(t, _)| *t)
                    .expect("乗り換える駅への区切りがない");
                let first = reach[a][k]
                    .1
                    .iter()
                    .flatten()
                    .find(|h| h.li == l1)
                    .expect("乗り換える前の路線がない")
                    .direct;
                let shape = &lines[usize::from(l2)];
                let (board, alight) = seconds[b];
                routes.transfer = Some(TransferRoute {
                    first,
                    at: ids[x],
                    second: Direct {
                        line: shape.line,
                        dir: shape.stops[usize::from(board)].board_dir,
                        board,
                        alight,
                    },
                });
            }
            if !routes.direct.is_empty() || routes.transfer.is_some() {
                table.push(((ids[a], ids[b]), routes));
            }
        }
    }
    table
}

/// 区切りの路線がないことを表す路線の添え字。
const NO_LINE: u16 = u16::MAX;

/// 乗り換えの進路を選ぶ順を 1 つの整数に詰める。（費用、乗る路線、乗り換える駅、乗り換え先の路線）の小さい順で、
/// 路線の添え字は路線の ID の順、駅の番号は駅の ID の順なので、ID の代わりに比べてよい。
fn pack_key(cost: u64, l1: u16, x: u32, l2: u16) -> u128 {
    (u128::from(cost) << 64) | (u128::from(l1) << 48) | (u128::from(x) << 16) | u128::from(l2)
}

/// [`pack_key`] で詰めた（乗る路線、乗り換える駅、乗り換え先の路線）。
fn unpack_key(key: u128) -> (u16, u32, u16) {
    let l1 = u16::try_from((key >> 48) & 0xffff).expect("路線の添え字");
    let x = u32::try_from((key >> 16) & 0xffff_ffff).expect("駅の番号");
    let l2 = u16::try_from(key & 0xffff).expect("路線の添え字");
    (l1, x, l2)
}

/// 乗り換え先の路線の止まる所を、乗ったまま進める並びに分けたもの。往復の路線は、終点から次の終点までの並びが
/// 2 つある。並びの先頭の終点で乗った人は、並びの終わりまで進める。環状の路線は、どこで乗っても 1 周の手前まで
/// 進める。
struct Runs {
    /// 止まる所。往復の路線は並びを続けて持ち、並びの終わりを `ends` に持つ。環状の路線は進路の順に 1 周ぶん持つ。
    stops: Vec<RunStop>,
    ends: Vec<usize>,
    looped: bool,
    /// 1 周の時間と、待ちの見込み。
    total: i64,
    wait: i64,
}

/// [`Runs`] の止まる所。
#[derive(Clone, Copy, Debug)]
struct RunStop {
    /// 駅の番号。
    x: u32,
    /// 止まる所の番号。
    j: u16,
    /// 次の止まる所までの区間の時間。
    seg: i64,
    /// 進路の先頭の止まる所から、ここまでの区間の時間の和。
    before: i64,
}

/// 乗り換えの候補。乗り換える前の費用から乗る所までの区間の時間の和を引いた値 `val` と、乗る路線、乗り換える駅、
/// 乗り換え先の路線に乗る止まる所の番号。（`val`、乗る路線、乗り換える駅）の小さい順に選ぶ。
#[derive(Clone, Copy, Debug)]
struct Boarding {
    val: i64,
    l1: u16,
    x: u32,
    j: u16,
}

impl Boarding {
    const NONE: Boarding = Boarding {
        val: i64::MAX,
        l1: NO_LINE,
        x: u32::MAX,
        j: 0,
    };

    fn key(&self) -> (i64, u16, u32) {
        (self.val, self.l1, self.x)
    }

    fn min(self, other: Boarding) -> Boarding {
        if other.key() < self.key() {
            other
        } else {
            self
        }
    }
}

impl Runs {
    fn new(shape: &LineShape, info: &LineInfo, wait: u64) -> Runs {
        let signed = |v: u64| i64::try_from(v).expect("区間の時間があふれた");
        let m = shape.stops.len();
        let stop = |k: usize| RunStop {
            x: u32::try_from(info.at[k]).expect("駅が多すぎる"),
            j: u16::try_from(k).expect("駅が多すぎる"),
            seg: signed(info.prefix[k + 1] - info.prefix[k]),
            before: signed(info.prefix[k]),
        };
        let mut stops = Vec::new();
        let mut ends = Vec::new();
        for t in (0..m).filter(|k| shape.stops[*k].terminal) {
            stops.push(stop(t));
            let mut k = t;
            for _ in 1..m {
                k = (k + 1) % m;
                stops.push(stop(k));
                if shape.stops[k].terminal {
                    break;
                }
            }
            ends.push(stops.len());
        }
        let looped = ends.is_empty();
        if looped {
            stops = (0..m).map(stop).collect();
        }
        Runs {
            stops,
            ends,
            looped,
            total: signed(info.prefix[m]),
            wait: signed(wait),
        }
    }

    /// 乗り換えられる駅（`board` が乗り換える前の費用と路線を返す駅。路線が [`NO_LINE`] なら乗り換えられない）から、
    /// 乗ったまま行ける止まる所 `b` ごとに、いちばん安く乗れる候補を `offer(b, 費用, 候補)` で渡す。費用は、
    /// 乗り換える前の費用と、この路線の待ちと乗る時間の和である。
    fn sweep(
        &self,
        board: impl Fn(u32) -> (i64, u16),
        mut offer: impl FnMut(&RunStop, i64, &Boarding),
    ) {
        let cand = |s: &RunStop, base: i64| -> Boarding {
            let (c, l1) = board(s.x);
            if l1 == NO_LINE {
                Boarding::NONE
            } else {
                Boarding {
                    val: c - base,
                    l1,
                    x: s.x,
                    j: s.j,
                }
            }
        };
        if self.looped {
            // `k` より前の止まる所で乗れば before[k] − before[j]、後で乗れば 1 周を足した時間で着く。
            let m = self.stops.len();
            let mut after = vec![Boarding::NONE; m + 1];
            for j in (0..m).rev() {
                after[j] = after[j + 1].min(cand(&self.stops[j], self.stops[j].before));
            }
            let mut before = Boarding::NONE;
            for (k, s) in self.stops.iter().enumerate() {
                let mut late = after[k + 1];
                if late.l1 != NO_LINE {
                    late.val += self.total;
                }
                let b = before.min(late);
                if b.l1 != NO_LINE {
                    offer(s, b.val + s.before + self.wait, &b);
                }
                before = before.min(cand(s, s.before));
            }
            return;
        }
        let mut start = 0;
        for &end in &self.ends {
            let run = &self.stops[start..end];
            start = end;
            let mut acc = 0_i64;
            let mut best = Boarding::NONE;
            for w in 0..run.len() - 1 {
                best = best.min(cand(&run[w], acc));
                acc += run[w].seg;
                if best.l1 != NO_LINE {
                    offer(&run[w + 1], best.val + acc + self.wait, &best);
                }
            }
        }
    }
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

    pub(crate) fn total(&self) -> u64 {
        self.prefix.last().copied().unwrap_or(0)
    }

    /// 止まる要素の順の番号 `from` から、進む向きに `to` まで乗る時間。番号が区間の時間の数を超えれば `None`。
    fn ride(&self, from: u16, to: u16) -> Option<u64> {
        let a = *self.prefix.get(usize::from(from))?;
        let b = *self.prefix.get(usize::from(to))?;
        Some(if to > from {
            b - a
        } else {
            self.total() - a + b
        })
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
    /// 乗り換え。`alight` は乗り換える駅になる。
    pub transfer: Option<Transfer>,
}

/// 費用の同じ候補を選ぶ順。（費用、乗る駅、降りる駅、路線、乗り換える駅、乗り換え先の路線）。
type OfferKey = (
    u64,
    StationId,
    StationId,
    LineId,
    Option<StationId>,
    Option<LineId>,
);

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

    /// 駅 `at` から `d` に乗る区切りの、待ちと乗る時間。路線が候補にならないか、表を作ったあとに路線の形が
    /// 変わっていれば `None`（表は路線が変わるたびに作り直す）。
    fn leg_cost(&self, at: StationId, d: &Direct) -> Option<u64> {
        let live = self.live.get(&d.line)?;
        let ride = live.ride(d.board, d.alight)?;
        let headway = live.headway();
        let mut wait = headway / 2;
        if self.left_behind.contains(&(at, d.line, d.dir)) {
            wait += headway;
        }
        Some(wait + ride)
    }

    /// 正面のマスが `from` の建物から、正面のマスが `to` の建物へ向かう鉄道の候補。乗り換え 1 回の進路も比べる。
    /// 費用の同じ組は、（乗る駅、降りる駅、路線、乗り換える駅、乗り換え先の路線）の ID の小さい順に選ぶ。
    /// 乗り換えのない進路は、乗り換える駅と路線を持たないので、同じ費用の乗り換える進路より先になる。
    pub fn offer(&self, from: TilePos, to: TilePos) -> Option<RideOffer> {
        if self.is_empty() {
            return None;
        }
        let starts = self.planner.near(from);
        if starts.is_empty() {
            return None;
        }
        let goals = self.planner.near(to);
        let mut best: Option<(OfferKey, RideOffer)> = None;
        let mut consider = |key: OfferKey, offer: RideOffer| {
            if best.as_ref().is_none_or(|(k, _)| key < *k) {
                best = Some((key, offer));
            }
        };
        for (a, wa) in &starts {
            let (sa, _, door) = self.planner.doors[*a as usize];
            for (b, wb) in &goals {
                let (sb, _, _) = self.planner.doors[*b as usize];
                let Some(routes) = self.planner.routes(sa, sb) else {
                    continue;
                };
                let walk = walk_seconds(*wa) + walk_seconds(*wb) + self.fare_seconds;
                for d in &routes.direct {
                    let Some(leg) = self.leg_cost(sa, d) else {
                        continue;
                    };
                    let cost = walk + leg;
                    consider(
                        (cost, sa, sb, d.line, None, None),
                        RideOffer {
                            cost,
                            board: sa,
                            line: d.line,
                            dir: d.dir,
                            alight: sb,
                            door,
                            transfer: None,
                        },
                    );
                }
                let Some(t) = &routes.transfer else {
                    continue;
                };
                let (Some(first), Some(second)) =
                    (self.leg_cost(sa, &t.first), self.leg_cost(t.at, &t.second))
                else {
                    continue;
                };
                let cost = walk + first + TRANSFER_SECONDS + second;
                consider(
                    (cost, sa, sb, t.first.line, Some(t.at), Some(t.second.line)),
                    RideOffer {
                        cost,
                        board: sa,
                        line: t.first.line,
                        dir: t.first.dir,
                        alight: t.at,
                        door,
                        transfer: Some(Transfer {
                            line: t.second.line,
                            dir: t.second.dir,
                            alight: sb,
                        }),
                    },
                );
            }
        }
        best.map(|(_, o)| o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::units::LaneOffset;
    use std::collections::{BTreeMap, BTreeSet};

    fn station(index: u32) -> StationId {
        StationId {
            index,
            generation: 0,
        }
    }

    fn line(index: u32) -> LineId {
        LineId {
            index,
            generation: 0,
        }
    }

    /// 駅 `i` の出入口を、マス (`x`, 0) に置く。
    fn door(i: u32, x: i64) -> (StationId, TilePos, SidewalkPos) {
        let t = TilePos::new(x, 0);
        let pos = SidewalkPos {
            tile: t,
            step: (1, 0),
            along: LaneOffset(0),
        };
        (station(i), t, pos)
    }

    /// 駅の並び `stations` の往復の路線を、区間の時間 `segment` で 1 本走らせる形。
    fn shape(id: u32, stations: &[u32], segment: u32) -> LineShape {
        let s: Vec<StationId> = stations.iter().map(|i| station(*i)).collect();
        LineShape::uniform(line(id), line_stops_of(&s, false), segment)
    }

    fn choice<'a>(planner: &'a RailPlanner, lines: &[LineShape]) -> RideChoice<'a> {
        let live = lines
            .iter()
            .map(|l| (l.line, LineLive::new(l.trains, &l.segments)))
            .collect::<BTreeMap<_, _>>();
        RideChoice::from_parts(planner, live, BTreeSet::new(), 5)
    }

    /// 駅 0 と駅 3 の出入口だけを、遠く離して置く。乗り換える駅（1、2）は、どちらの建物からも遠い。
    fn far_doors() -> Vec<(StationId, TilePos, SidewalkPos)> {
        vec![door(0, 0), door(1, 500), door(2, 600), door(3, 1_000)]
    }

    /// 直接行ける路線がない組は、乗り換え 1 回で結ぶ。費用は、両方の路線の待ちと乗る時間に、乗り換えの手間と
    /// 駅まで歩く時間と運賃を足したものである。
    #[test]
    fn a_pair_without_a_direct_line_is_joined_by_one_transfer() {
        let lines = [shape(0, &[0, 1], 100), shape(1, &[1, 3], 100)];
        let planner = RailPlanner::from_shapes(far_doors(), &lines);
        let o = choice(&planner, &lines)
            .offer(TilePos::new(0, 0), TilePos::new(1_000, 0))
            .expect("乗り換えの候補がない");
        assert_eq!(
            (o.board, o.line, o.alight),
            (station(0), line(0), station(1))
        );
        assert_eq!(
            o.transfer,
            Some(Transfer {
                line: line(1),
                dir: 0,
                alight: station(3),
            })
        );
        // 往復の 1 周は 200 tick で、1 本なので待ちは 100。乗るのは 100。
        assert_eq!(
            o.cost,
            (100 + 100) * 2 + TRANSFER_SECONDS + 5 * FARE_SECONDS
        );
    }

    /// 直接行ける路線があっても、乗り換えたほうが安いときだけ乗り換える。
    #[test]
    fn transfers_only_when_cheaper_than_the_direct_line() {
        for (slow, transfer) in [(100, false), (220, false), (300, true)] {
            let lines = [
                shape(0, &[0, 3], slow),
                shape(1, &[0, 1], 100),
                shape(2, &[1, 3], 100),
            ];
            let planner = RailPlanner::from_shapes(far_doors(), &lines);
            let o = choice(&planner, &lines)
                .offer(TilePos::new(0, 0), TilePos::new(1_000, 0))
                .unwrap();
            assert_eq!(o.transfer.is_some(), transfer, "直接の区間 {slow} tick");
            let direct = slow * 2 + 5 * FARE_SECONDS as u32;
            let via = 400 + TRANSFER_SECONDS as u32 + 5 * FARE_SECONDS as u32;
            assert_eq!(o.cost, u64::from(direct.min(via)));
            if !transfer {
                assert_eq!((o.line, o.alight), (line(0), station(3)));
            }
        }
    }

    /// 費用が同じなら、乗り換えない進路を選び、乗り換える進路どうしは（路線、乗り換える駅、乗り換え先の路線）の
    /// ID の小さい順に選ぶ。
    #[test]
    fn ties_follow_the_id_order() {
        // 路線 0 は駅 0、1、2 を、駅 1 と 2 のあいだを 0 tick で結ぶ。駅 1 で路線 2 に、駅 2 で路線 1 に
        // 乗り換えても同じ費用になる。乗り換える駅の ID が先に比べられ、駅 1 を選ぶ。
        let mut first = shape(0, &[0, 1, 2], 100);
        first.segments = vec![0, 0, 100, 100];
        let lines = [first, shape(1, &[2, 3], 100), shape(2, &[1, 3], 100)];
        let planner = RailPlanner::from_shapes(far_doors(), &lines);
        let o = choice(&planner, &lines)
            .offer(TilePos::new(0, 0), TilePos::new(1_000, 0))
            .unwrap();
        assert_eq!((o.line, o.alight), (line(0), station(1)));
        assert_eq!(o.transfer.map(|t| t.line), Some(line(2)));
        // 乗り換える駅が同じなら、路線の ID の小さいほうに乗る。
        let lines = [
            shape(0, &[0, 1], 100),
            shape(1, &[0, 1], 100),
            shape(2, &[1, 3], 100),
        ];
        let planner = RailPlanner::from_shapes(far_doors(), &lines);
        let o = choice(&planner, &lines)
            .offer(TilePos::new(0, 0), TilePos::new(1_000, 0))
            .unwrap();
        assert_eq!(o.line, line(0));
        // 乗り換えない進路と同じ費用なら、乗り換えない。直接の路線の区間を、乗り換えの費用と同じにする。
        let via = 400 + TRANSFER_SECONDS;
        let lines = [
            shape(0, &[0, 3], u32::try_from(via / 2).unwrap()),
            shape(1, &[0, 1], 100),
            shape(2, &[1, 3], 100),
        ];
        let planner = RailPlanner::from_shapes(far_doors(), &lines);
        let o = choice(&planner, &lines)
            .offer(TilePos::new(0, 0), TilePos::new(1_000, 0))
            .unwrap();
        assert_eq!(o.cost, via + 5 * FARE_SECONDS);
        assert_eq!(o.transfer, None);
    }

    /// 乗り換えの進路を絞っても、組ごとの見込みの費用は、すべての（乗る路線、乗り換える駅、乗り換え先の路線）を
    /// 試したときのいちばん小さい値と同じになる。
    #[test]
    fn the_pruned_transfer_table_matches_a_full_search() {
        let mut r = crate::rng::rng(
            7,
            crate::rng::RngPurpose::JobAssignment,
            0,
            crate::units::Tick(0),
        );
        for _ in 0..40 {
            let stations = 6 + r.below(10);
            let lines: Vec<LineShape> = (0..2 + r.below(6))
                .map(|k| {
                    let mut seq: Vec<u32> = (0..stations).collect();
                    for i in (1..seq.len()).rev() {
                        let j = r.below(u32::try_from(i + 1).unwrap()) as usize;
                        seq.swap(i, j);
                    }
                    seq.truncate(2 + r.below(stations - 1) as usize);
                    let s: Vec<StationId> = seq.iter().map(|i| station(*i)).collect();
                    let looped = s.len() > 2 && r.below(3) == 0;
                    let stops = line_stops_of(&s, looped);
                    let segments = stops.iter().map(|_| 10 + r.below(200)).collect();
                    LineShape {
                        line: line(k),
                        stops,
                        segments,
                        trains: r.below(4),
                    }
                })
                .collect();
            let table = build_table(&lines);
            // すべてを試す見込みの費用。
            let hop = |l: &LineShape, from: StationId| -> Vec<(StationId, u64)> {
                if l.trains == 0 {
                    return Vec::new();
                }
                let live = LineLive::new(l.trains, &l.segments);
                let m = l.stops.len();
                let mut out = Vec::new();
                for k in 0..m {
                    if l.stops[k].station != from {
                        continue;
                    }
                    for d in 1..m {
                        let j = (k + d) % m;
                        let to = l.stops[j];
                        if to.station != from {
                            let ride = live.ride(k as u16, j as u16).unwrap();
                            out.push((to.station, live.headway() / 2 + ride));
                        }
                        if to.terminal {
                            break;
                        }
                    }
                }
                out
            };
            for a in 0..stations {
                for b in 0..stations {
                    if a == b {
                        continue;
                    }
                    let mut want: Option<u64> = None;
                    for l1 in &lines {
                        for (x, c1) in hop(l1, station(a)) {
                            for l2 in lines.iter().filter(|l| l.line != l1.line) {
                                for (to, c2) in hop(l2, x) {
                                    if to == station(b) {
                                        want = Some(want.map_or(c1 + c2, |w| w.min(c1 + c2)));
                                    }
                                }
                            }
                        }
                    }
                    let got = table
                        .iter()
                        .find(|(k, _)| *k == (station(a), station(b)))
                        .and_then(|(_, r)| r.transfer)
                        .map(|t| {
                            let cost = |d: &Direct| {
                                let l = lines.iter().find(|l| l.line == d.line).unwrap();
                                let live = LineLive::new(l.trains, &l.segments);
                                live.headway() / 2 + live.ride(d.board, d.alight).unwrap()
                            };
                            cost(&t.first) + cost(&t.second)
                        });
                    assert_eq!(got, want, "駅 {a} から {b}");
                }
            }
            // 表は（乗る駅、降りる駅）の順に並ぶ。
            assert!(table.windows(2).all(|w| w[0].0 < w[1].0));
        }
    }
}
