//! テストとベンチマークに使う、種から決定論的に作る街。

use chizu_core::citizens::draw_household;
use chizu_core::network::TILE_MM;
use chizu_core::rail::choice::{LineLive, LineShape, line_stops_of};
use chizu_core::routing::{ROUTE_VARIANTS, RouteRequest};
use chizu_core::{
    ApplyOutcome, CHUNK_SIDE, ChunkCoord, Command, LaneMode, Line, LinkId, Money, Network,
    Population, Rail, RngPurpose, RoadKind, ServiceKind, Tick, TilePos, TileRect, World, ZoneKind,
    rng,
};
use chizu_core::{LaneOffset, LineId, SidewalkPos, StationId};
use std::collections::BTreeMap;

pub mod perf;

/// [`rail_trains_city`] の路線の数。街に置ける路線の上限である。
pub const RAIL_BENCH_LINES: usize = 64;

/// [`rail_trains_city`] の路線ごとの列車の本数。64 本の路線と合わせて 512 本にする。
pub const RAIL_BENCH_TRAINS: u8 = 8;

/// 1 組の幅（マス）。
const RAIL_UNIT_WIDTH: i64 = 260;

/// 1 組の高さ（マス）。北の駅のホームから南の道路までの 14 行と、間の 4 行である。
const RAIL_UNIT_HEIGHT: i64 = 18;

/// 分岐と共有する区間のある 2 本の路線を 1 組にして、`lines / 2` 組並べた街。路線ごとに [`RAIL_BENCH_TRAINS`] 本を
/// 走らせるよう決めてある（上限は 8 本）。
///
/// 1 組は、西と東の両側で線路が分かれる形である。真ん中の駅 M の両側に共有する線路があり、西は南の駅 V1 と
/// 北の駅 V2、東は南の駅 E と北の駅 N へ、分岐で分かれる。路線は V1–M–E と V2–M–N で、M とその両側の線路を共有する。
/// 4 つの終点はそれぞれ 1 本の路線だけが使う線路の先にあるので、ホームを 2 つ使えて、終点で折り返せる本数
/// （`turn_cap`）が 8 本を上回る。終点を 2 本で分け合うと、折り返せる本数が足りずに列車が出そろわない（#375）。
/// 原点のまわりの 21×21 チャンクを買い、水辺のない所を左上から順に探して置く。
pub fn rail_trains_city(seed: u64, lines: usize) -> World {
    const R: i32 = 10;
    assert!(lines.is_multiple_of(2), "路線は 2 本ずつ置く");
    let mut w = World::new_with_money(seed, Money(i64::MAX / 4)).with_unlimited_land();
    // 買った土地に辺で接するチャンクしか買えないので、内側から外へ買い広げる。
    for ring in 2..=R {
        for cy in -ring..=ring {
            for cx in -ring..=ring {
                if cx.abs().max(cy.abs()) == ring {
                    w.apply(&Command::BuyChunk {
                        coord: ChunkCoord::new(cx, cy),
                    });
                }
            }
        }
    }
    for cy in -R..=R {
        for cx in -R..=R {
            w.apply(&Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            });
        }
    }
    let side = CHUNK_SIDE as i64;
    let (min, max) = (-i64::from(R) * side, (i64::from(R) + 1) * side - 1);
    let mut made = 0;
    let mut y = min + RAIL_UNIT_HEIGHT;
    while made < lines && y <= max {
        let mut x = min;
        while made < lines && x + RAIL_UNIT_WIDTH <= max {
            let land = (x..=x + RAIL_UNIT_WIDTH).all(|tx| {
                (y - RAIL_UNIT_HEIGHT + 4..=y).all(|ty| w.is_buildable(TilePos::new(tx, ty)))
            });
            if land && place_rail_pair(&mut w, x, y) {
                made += 2;
                x += RAIL_UNIT_WIDTH + 4;
            } else {
                x += 8;
            }
        }
        y += RAIL_UNIT_HEIGHT;
    }
    assert_eq!(made, lines, "路線を置ききれない");
    w
}

/// `x` から東へ、y の道路に面した駅 V1、M、E と、y − 10 の道路に面した駅 V2、N、線路を置き、V1–M–E と V2–M–N の
/// 路線を作る。置けなければ偽を返す。
fn place_rail_pair(w: &mut World, x: i64, y: i64) -> bool {
    let e = x + RAIL_UNIT_WIDTH - 3;
    let station = |east: i64, row: i64| Command::PlaceBuilding {
        kind: ServiceKind::Station,
        at: TilePos::new(east, row + 1),
        rotation: 2,
    };
    let rail = |from: (i64, i64), to: (i64, i64)| Command::BuildRail {
        from: TilePos::new(from.0, from.1),
        to: TilePos::new(to.0, to.1),
    };
    let road = |from: i64, to: i64, row: i64| Command::BuildRoad {
        from: TilePos::new(from, row),
        to: TilePos::new(to, row),
        kind: RoadKind::Small,
    };
    let (south, north) = (y - 2, y - 12);
    let commands = [
        road(x, x + RAIL_UNIT_WIDTH, y),
        road(x, x + 40, y - 10),
        road(x + 121, x + RAIL_UNIT_WIDTH, y - 10),
        station(x + 12, south),
        station(x + 12, north),
        station(x + 90, south),
        station(e, south),
        station(e, north),
        rail((x + 12, south), (x + 81, south)),
        rail((x + 12, north), (x + 50, north)),
        // 西の分岐。北西の線路が、南の線路へ東南東に10 マス進んで合わさる。
        rail((x + 50, north), (x + 60, south)),
        rail((x + 90, south), (e - 9, south)),
        // 東の分岐。南の線路から北東へ 10 マス進み、北の駅へ東へ向かう。
        rail((x + 108, south), (x + 118, north)),
        rail((x + 118, north), (e - 9, north)),
    ];
    if !commands.iter().all(|c| w.apply(c) == ApplyOutcome::Applied) {
        return false;
    }
    let at = |tx, row| {
        w.rail()
            .station_at(TilePos::new(tx, row))
            .expect("駅がない")
    };
    let (west_south, west_north, mid) = (at(x + 12, south), at(x + 12, north), at(x + 90, south));
    let (east_south, east_north) = (at(e, south), at(e, north));
    // 真ん中の M を通る共有の区間を挟んで、西と東の端がそれぞれ別々の終点になる。
    for (from, to) in [(west_south, east_south), (west_north, east_north)] {
        if w.apply(&Command::CreateLine { from, to: mid }) != ApplyOutcome::Applied {
            return false;
        }
        let (line, _) = w.rail().lines().last().expect("路線がない");
        let rest = [
            Command::ExtendLine { line, station: to },
            Command::SetLineTrains {
                line,
                trains: RAIL_BENCH_TRAINS,
            },
        ];
        if !rest.iter().all(|c| w.apply(c) == ApplyOutcome::Applied) {
            return false;
        }
    }
    true
}

/// [`rail_trains_city`] の線路の網を、列車がすべて現れるまで回し、さらにふだんの走り方になるまで
/// [`RAIL_BENCH_WARMUP`] tick 回したもの。
pub fn rail_trains_running(seed: u64, lines: usize) -> Rail {
    let w = rail_trains_city(seed, lines);
    let mut rail = w.rail().clone();
    let want = lines * usize::from(RAIL_BENCH_TRAINS);
    let mut tick = w.tick();
    for _ in 0..50_000 {
        rail.step(tick);
        tick = tick.next();
        if rail.train_count() == want {
            break;
        }
    }
    assert_eq!(rail.train_count(), want, "列車が出そろわない");
    for _ in 0..RAIL_BENCH_WARMUP {
        rail.step(tick);
        tick = tick.next();
    }
    assert_eq!(rail.train_count(), want, "列車が消えた");
    assert_eq!(rail.tally().deadlock, 0, "行き詰まった");
    rail
}

/// 列車が出そろってから、測り始めるまでに回す tick。出そろった直後は、駅で間を空けて待つ列車が
/// 同じ tick に出るので、ふだんより重い。
pub const RAIL_BENCH_WARMUP: u64 = 3_000;

/// [`rail_choice_inputs`] の駅の数。
pub const RAIL_CHOICE_STATIONS: u32 = 200;

/// 手段の選び方のベンチマークに使う、[`city_grid_100k`] の上の駅と路線。
pub struct RailChoiceInputs {
    /// 駅の ID の順の、出入口のマスと位置。
    pub doors: Vec<(StationId, TilePos, SidewalkPos)>,
    /// 路線ごとの止まる所と、区間の時間と本数。路線の ID の順に並べる。
    pub lines: Vec<LineShape>,
    /// 路線ごとの、走っている本数と区間の時間。
    pub live: BTreeMap<LineId, LineLive>,
}

/// [`city_grid_100k`]（約 4.6km 四方）に、駅 [`RAIL_CHOICE_STATIONS`] 個と路線 [`RAIL_BENCH_LINES`] 本を置いた形。
///
/// 駅は 15 × 14 の格子（約 330m おき）から 200 個を選んで置く。路線は、ある駅から格子の隣の駅へ種で選んで
/// 進み、まだ通っていない駅を 12〜32 個つないだもので、4 本に 1 本は環状にする。線路は引かず、路線の形
/// （止まる所の並び）と、走っている本数（4〜8 本）と、区間の時間（40〜120 tick）だけを作る。
/// 出入口の位置は、駅のマスにいちばん近い東西の通りの、東向きの歩道の上に置く（駅まで歩く表を作るため）。
/// 駅のマス（徒歩圏を測る所）は格子のまま変えない。
pub fn rail_choice_inputs(seed: u64) -> RailChoiceInputs {
    const COLS: i64 = 15;
    const ROWS: i64 = 14;
    const STEP: i64 = 41;
    let station = |i: u32| StationId {
        index: i,
        generation: 0,
    };
    let pos = |i: u32| {
        let i = i64::from(i);
        TilePos::new(6 + (i % COLS) * STEP, 6 + (i / COLS) * STEP)
    };
    // `city_grid_100k` の通りは 12 マスおきで、東の端は x = 576 である。
    let doors = (0..RAIL_CHOICE_STATIONS)
        .map(|i| {
            let t = pos(i);
            let door = SidewalkPos {
                tile: TilePos::new(t.x.min(575), (t.y + 6) / 12 * 12),
                step: (1, 0),
                along: LaneOffset(0),
            };
            (station(i), t, door)
        })
        .collect();
    let n = i64::from(RAIL_CHOICE_STATIONS);
    let mut lines = Vec::new();
    let mut live = BTreeMap::new();
    for k in 0..u32::try_from(RAIL_BENCH_LINES).expect("路線が多すぎる") {
        let mut r = rng(seed, RngPurpose::JobAssignment, u64::from(k), Tick(7));
        let want = 12 + r.below(21) as usize;
        let mut at = i64::from(r.below(RAIL_CHOICE_STATIONS));
        let mut seq = vec![at];
        while seq.len() < want {
            let (c, row) = (at % COLS, at / COLS);
            let next: Vec<i64> = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .into_iter()
                .map(|(dc, dr)| (c + dc, row + dr))
                .filter(|(c, row)| (0..COLS).contains(c) && (0..ROWS).contains(row))
                .map(|(c, row)| row * COLS + c)
                .filter(|j| *j < n && !seq.contains(j))
                .collect();
            if next.is_empty() {
                break;
            }
            let len = u32::try_from(next.len()).expect("隣が多すぎる");
            at = next[r.below(len) as usize];
            seq.push(at);
        }
        if seq.len() < 2 {
            continue;
        }
        let stations: Vec<StationId> = seq
            .iter()
            .map(|i| station(u32::try_from(*i).expect("駅の番号")))
            .collect();
        let id = LineId {
            index: k,
            generation: 0,
        };
        let stops = line_stops_of(&stations, k % 4 == 3);
        let segments: Vec<u32> = stops.iter().map(|_| 40 + r.below(81)).collect();
        let trains = 4 + r.below(5);
        live.insert(id, LineLive::new(trains, &segments));
        lines.push(LineShape {
            line: id,
            stops,
            segments,
            trains,
        });
    }
    RailChoiceInputs { doors, lines, live }
}

/// `count` 件の、家から職場への移動の出発と行き先の、正面のマスと、出入口の歩道のリンクとその始まりからの距離。
/// [`commute_requests`] の車のリンクと同じ区間、同じ向きの歩道のリンクの真ん中である。
pub fn commute_doors(
    seed: u64,
    net: &Network,
    count: u32,
) -> Vec<[(TilePos, LinkId, LaneOffset); 2]> {
    let mid = |l: LinkId| {
        let line = net.segment_line(l.segment).expect("リンクがない");
        let walk = LinkId {
            mode: LaneMode::Sidewalk,
            ..l
        };
        let along =
            LaneOffset(u32::try_from(line.len / 2 * TILE_MM.unsigned_abs()).expect("長すぎる"));
        let spot = SidewalkPos::on_any_link(net, walk, along).expect("歩道がない");
        (spot.tile, walk, along)
    };
    commute_requests(seed, net, count, 8)
        .into_iter()
        .map(|r| [mid(r.from), mid(r.to)])
        .collect()
}

/// `count` 件の、家から職場への移動の出発と行き先の建物の正面のマス。[`commute_requests`] のリンクの真ん中のマスである。
pub fn commute_tiles(seed: u64, net: &Network, count: u32) -> Vec<(TilePos, TilePos)> {
    let mid = |l: LinkId| {
        let line = net.segment_line(l.segment).expect("リンクがない");
        line.tile(line.len / 2)
    };
    commute_requests(seed, net, count, 8)
        .into_iter()
        .map(|r| (mid(r.from), mid(r.to)))
        .collect()
}

/// 東西に約 2km 離れた住宅（西）と工場（東）を通りで結び、通りの南の駅 2 つを線路でつないで、本数 `trains` の
/// 路線を走らせる街。chizu-core の `rail_town` と同じ形で、住宅と工場の区画を通りに沿って広げてある。
/// 本数が少ないほど 1 本の列車に乗る人が増える。
pub fn rail_riders_town(trains: u8) -> World {
    let seed = (5..)
        .find(|s| chizu_core::world::highway_line(*s).start.y < 0)
        .expect("北から入る種がない");
    let mut w = World::new_with_money(seed, Money(1_000_000_000)).with_unlimited_land();
    let at = TilePos::new;
    let apply = |w: &mut World, c: Command| {
        let out = w.apply(&c);
        assert!(
            matches!(out, ApplyOutcome::Applied | ApplyOutcome::AppliedPartly(_)),
            "{c:?}: {out:?}"
        );
    };
    let road = |from: TilePos, to: TilePos| Command::BuildRoad {
        from,
        to,
        kind: RoadKind::Medium,
    };
    // 内側から外へ、まわりのチャンクを買い広げる。
    for ring in 2_i32..=4 {
        for cy in -ring..=ring {
            for cx in -ring..=ring {
                if i32::max(cx.abs(), cy.abs()) == ring {
                    w.apply(&Command::BuyChunk {
                        coord: ChunkCoord::new(cx, cy),
                    });
                }
            }
        }
    }
    apply(&mut w, road(at(-200, 0), at(200, 0)));
    let inner = chizu_core::world::highway_line(w.seed()).start;
    let corner = at(inner.x, 0);
    if inner != corner {
        apply(&mut w, road(inner, corner));
    }
    if !(-200..=200).contains(&corner.x) {
        apply(&mut w, road(corner, at(corner.x.clamp(-200, 200), 0)));
    }
    for x in [-140, 125] {
        apply(
            &mut w,
            Command::PlaceBuilding {
                kind: ServiceKind::Station,
                at: at(x, 1),
                rotation: 0,
            },
        );
    }
    let west = w.rail().station_at(at(-140, 2)).expect("西の駅がない");
    let east = w.rail().station_at(at(125, 2)).expect("東の駅がない");
    apply(
        &mut w,
        Command::BuildRail {
            from: at(-131, 2),
            to: at(125, 2),
        },
    );
    for (a, b, zone) in [
        ((-200, -6), (-110, -1), chizu_core::Zone::Residential),
        ((110, -6), (190, -1), chizu_core::Zone::Industrial),
    ] {
        apply(
            &mut w,
            Command::PaintZone {
                area: TileRect::from_corners(at(a.0, a.1), at(b.0, b.1)),
                zone: Some(zone),
            },
        );
    }
    let xs: Vec<i64> = [-80, -60, -40, -20, 20, 40, 60, 80]
        .into_iter()
        .filter(|x| (x - inner.x).abs() > 14)
        .collect();
    for (kind, x) in [
        (ServiceKind::PowerPlant, xs[0]),
        (ServiceKind::WaterPump, xs[1]),
        (ServiceKind::WaterPump, xs[2]),
    ] {
        apply(
            &mut w,
            Command::PlaceBuilding {
                kind,
                at: at(x, -1),
                rotation: 2,
            },
        );
    }
    apply(
        &mut w,
        Command::CreateLine {
            from: west,
            to: east,
        },
    );
    let line = w.rail().lines().next().expect("路線がない").0;
    apply(&mut w, Command::SetLineTrains { line, trains });
    w
}

/// 乗る人のいちばん多く降りる 1 tick の直前の街と、乗る人のいちばん多い時点の街。
pub struct RidersPeak {
    /// 降りた人のいちばん多い tick の直前の街。読み戻して 1 tick 進めると、その tick になる。
    pub before_peak: Vec<u8>,
    /// その tick に鉄道を離れた人数。
    pub alighted: u64,
    /// 駅で待つ人と列車に乗る人の和がいちばん多い時点の街。
    pub crowded: Vec<u8>,
    /// その時点の乗る人の数。
    pub riders: usize,
}

/// [`rail_riders_town`] を `days` 日回し、鉄道を離れた人（降りた人、待ちきれずに歩いた人、途中で降ろされた人）が
/// いちばん多かった tick の直前の街と、乗る人のいちばん多かった時点の街を書き出す。
pub fn rail_riders_peak(trains: u8, days: u64) -> RidersPeak {
    let left = |w: &World| {
        let t = w.rail().ride_tally();
        t.alighted + t.gave_up + t.evicted
    };
    let mut w = rail_riders_town(trains);
    let (mut peak, mut crowd) = ((0, 0), (0, 0));
    for _ in 0..days * chizu_core::TICKS_PER_DAY {
        let before = left(&w);
        w.step();
        let n = left(&w) - before;
        if n > peak.0 {
            peak = (n, w.tick().0);
        }
        let r = w.rail().rider_count();
        if r > crowd.0 {
            crowd = (r, w.tick().0);
        }
    }
    let mut w = rail_riders_town(trains);
    let mut out = RidersPeak {
        before_peak: Vec::new(),
        alighted: peak.0,
        crowded: Vec::new(),
        riders: crowd.0,
    };
    while out.before_peak.is_empty() || out.crowded.is_empty() {
        if w.tick().0 + 1 == peak.1 {
            out.before_peak = w.to_snapshot();
        }
        if w.tick().0 == crowd.1 {
            out.crowded = w.to_snapshot();
        }
        w.step();
    }
    out
}

pub use chizu_core::citizens::MAX_HOUSEHOLD_SIZE;

/// 種 `seed` から `count` 人の合成の住民と世帯を作る。
///
/// 世帯は 1〜[`MAX_HOUSEHOLD_SIZE`] 人で、最初の 1 人は 18〜79 歳、残りは 0〜79 歳である。
/// 最後の世帯は、合計がちょうど `count` 人になるように人数を削る。
/// 住む建物と職場はまだない。
pub fn synthetic_population(seed: u64, count: u32) -> Population {
    let mut p = Population::default();
    let mut made = 0_u32;
    let mut household_no = 0_u64;
    while made < count {
        let mut r = rng(seed, RngPurpose::Migration, household_no, Tick::ZERO);
        household_no += 1;
        let (money, ages) = draw_household(&mut r, count - made);
        let h = p.add_household(money);
        for age in &ages {
            p.add_citizen(h, *age).expect("足したばかりの世帯がない");
        }
        made += ages.len() as u32;
    }
    p
}

/// 種 `seed` の街に、開始地点を中心とする 7×7 チャンクの土地を買い、12 マスごとの格子の道路を引き、
/// 街区ごとに 4 種類の区画を順に塗り、ところどころに発電所と浄水場を置き、外の世界とつないだ街。
/// 水辺にかかる道路と区画は飛ばす。建物と住民はまだない。
pub fn zoned_grid_city(seed: u64) -> World {
    grid_city(seed, 3, false)
}

/// [`zoned_grid_city`] を 11×11 チャンクに広げ、5 列に 1 列の街区を区画にせず発電所と浄水場で埋めた街。
/// 住民が 10 万人を超えるだけの土地と、そのぶんの電気と水道がある。
pub fn serviced_grid_city(seed: u64) -> World {
    grid_city(seed, 5, true)
}

/// 開始地点を中心とする (2 × `radius` + 1) 四方のチャンクの格子の街。
fn grid_city(seed: u64, radius: i32, serviced: bool) -> World {
    let mut w = World::new_with_money(seed, Money(1_000_000_000)).with_unlimited_land();
    for ring in 2..=radius {
        for cy in -ring..=ring {
            for cx in -ring..=ring {
                if cx.abs().max(cy.abs()) == ring {
                    w.apply(&Command::BuyChunk {
                        coord: ChunkCoord::new(cx, cy),
                    });
                }
            }
        }
    }
    // 角のチャンクは辺で接するまで買えないので、もう一周して買い残しを埋める。
    for cy in -radius..=radius {
        for cx in -radius..=radius {
            w.apply(&Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            });
        }
    }
    let side = CHUNK_SIDE as i64;
    let (min, max, step) = (
        -i64::from(radius) * side,
        (i64::from(radius) + 1) * side - 1,
        12_i64,
    );
    let lines: Vec<i64> = (0..)
        .map(|i| min + i * step)
        .take_while(|v| *v <= max)
        .collect();
    for &at in &lines {
        for (from, to) in [
            (TilePos::new(min, at), TilePos::new(max, at)),
            (TilePos::new(at, min), TilePos::new(at, max)),
        ] {
            // 水辺で途切れるので、街区 1 つぶんずつ引く。
            let n = (to.x - from.x).max(to.y - from.y) / step;
            for k in 0..n {
                let d = (to.x - from.x).signum() * step;
                let e = (to.y - from.y).signum() * step;
                w.apply(&Command::BuildRoad {
                    from: TilePos::new(from.x + d * k, from.y + e * k),
                    to: TilePos::new(from.x + d * (k + 1), from.y + e * (k + 1)),
                    kind: RoadKind::Small,
                });
            }
        }
    }
    for (by, y) in lines.iter().enumerate() {
        for (bx, x) in lines.iter().enumerate() {
            if serviced && bx % 5 == 0 {
                // `dx` は施設の西の端。南の辺に向きを変えて置くと、角のマスから西へ延びるので、東の端を渡す。
                let place = |kind: ServiceKind, dx: i64, dy: i64, rotation: u8| {
                    let east = if rotation == 2 {
                        kind.size().0 as i64 - 1
                    } else {
                        0
                    };
                    Command::PlaceBuilding {
                        kind,
                        at: TilePos::new(x + dx + east, y + dy),
                        rotation,
                    }
                };
                // 区画を塗らず、北と南の辺に施設を並べる。3 つに 1 つは浄水場だけ、残りは発電所 2 つと浄水場 1 つにする。
                for (dy, rotation) in [(1, 0), (step - 1, 2)] {
                    if by % 3 == 0 {
                        for dx in [1, 3, 5, 7, 9] {
                            w.apply(&place(ServiceKind::WaterPump, dx, dy, rotation));
                        }
                    } else {
                        w.apply(&place(ServiceKind::PowerPlant, 1, dy, rotation));
                        w.apply(&place(ServiceKind::PowerPlant, 5, dy, rotation));
                        w.apply(&place(ServiceKind::WaterPump, 9, dy, rotation));
                    }
                }
                continue;
            }
            let zone = ZoneKind::ALL[(bx + by) % 4];
            w.apply(&Command::PaintZone {
                area: TileRect::from_corners(
                    TilePos::new(x + 1, y + 1),
                    TilePos::new(x + step - 1, y + step - 1),
                ),
                zone: Some(zone.into()),
            });
            // ところどころの街区の北の辺に、発電所 1 つと浄水場 2 つを置く。
            if (bx + by) % 4 == 0 && bx % 3 == 0 {
                let place = |kind, dx| Command::PlaceBuilding {
                    kind,
                    at: TilePos::new(x + dx, y + 1),
                    rotation: 0,
                };
                w.apply(&place(ServiceKind::PowerPlant, 4));
                w.apply(&place(ServiceKind::WaterPump, 8));
                w.apply(&place(ServiceKind::WaterPump, 10));
            }
        }
    }
    // 高速道路の内側のノードから、縦横いちばん近い格子の道路まで引いて、外の世界とつなぐ。
    let inner = chizu_core::world::highway_line(seed).start;
    let snap = |v: i64| min + (v - min).div_euclid(step) * step;
    for to in [
        TilePos::new(snap(inner.x), inner.y),
        TilePos::new(snap(inner.x) + step, inner.y),
        TilePos::new(inner.x, snap(inner.y)),
        TilePos::new(inner.x, snap(inner.y) + step),
    ] {
        if to != inner {
            w.apply(&Command::BuildRoad {
                from: inner,
                to,
                kind: RoadKind::Small,
            });
        }
    }
    w
}

/// 格子状の道路網。`blocks` × `blocks` の街区で、1 街区の辺は `block_tiles` マスである。
///
/// `arterial_every` 本ごとに、東西を太い道路、南北を中くらいの道路にし、ほかは細い道路にする。
pub fn grid_network(blocks: i64, block_tiles: i64, arterial_every: i64) -> Network {
    let mut n = Network::new();
    let side = blocks * block_tiles;
    for i in 0..=blocks {
        let at = i * block_tiles;
        let arterial = i % arterial_every == 0;
        let roads = [
            (TilePos::new(0, at), TilePos::new(side, at), RoadKind::Large),
            (
                TilePos::new(at, 0),
                TilePos::new(at, side),
                RoadKind::Medium,
            ),
        ];
        for (from, to, kind) in roads {
            let kind = if arterial { kind } else { RoadKind::Small };
            let line = Line::between(from, to).expect("格子の道路が引けない");
            n.build_road(&line, kind).expect("格子の道路が重なった");
        }
    }
    n
}

/// 人口 10 万の街の規模の格子。48 × 48 の街区（1 辺 96m、街全体で約 4.6km 四方）である。
pub fn city_grid_100k() -> Network {
    grid_network(48, 12, 4)
}

/// `count` 人ぶんの、家から職場への車の通勤の依頼。
///
/// 家はすべての車のリンクから一様に選ぶ。職場は車のリンクの約 `1 / office_every` に集まっているとし、
/// そこから一様に選ぶ。職場のリンクは 1 本ずつ `1 / office_every` の確率で選ぶので、区間の向きに片寄らない。
pub fn commute_requests(
    seed: u64,
    net: &Network,
    count: u32,
    office_every: u32,
) -> Vec<RouteRequest> {
    let links: Vec<LinkId> = net.links().filter(|l| l.mode == LaneMode::Car).collect();
    let len = u32::try_from(links.len()).expect("リンクが多すぎる");
    let mut offices: Vec<LinkId> = (0_u64..)
        .zip(&links)
        .filter(|(i, _)| rng(seed, RngPurpose::JobAssignment, *i, Tick(1)).chance(1, office_every))
        .map(|(_, l)| *l)
        .collect();
    if offices.is_empty() {
        offices.extend(links.first());
    }
    let office_len = u32::try_from(offices.len()).expect("リンクが多すぎる");
    (0..count)
        .map(|i| {
            let mut r = rng(seed, RngPurpose::JobAssignment, u64::from(i), Tick::ZERO);
            RouteRequest {
                from: links[r.below(len) as usize],
                to: offices[r.below(office_len) as usize],
                mode: LaneMode::Car,
                // 実際の街と同じく、費用の同じ経路の候補を人ごとに散らす。
                variant: u8::try_from(i % u32::from(ROUTE_VARIANTS)).expect("候補が多すぎる"),
            }
        })
        .collect()
}

/// `count` 人ぶんの、歩道の上の 2 点を結ぶ歩きの出発と行き先。どちらも歩道のリンクから一様に選び、
/// リンクの上の位置も一様に選ぶ。
pub fn walk_ends(seed: u64, net: &Network, count: u32) -> Vec<(SidewalkPos, SidewalkPos)> {
    let links: Vec<LinkId> = net
        .links()
        .filter(|l| l.mode == LaneMode::Sidewalk)
        .collect();
    let len = u32::try_from(links.len()).expect("リンクが多すぎる");
    (0..count)
        .map(|i| {
            let mut r = rng(seed, RngPurpose::DailySchedule, u64::from(i), Tick::ZERO);
            let mut pick = || {
                let link = links[r.below(len) as usize];
                let length = net
                    .segment_line(link.segment)
                    .expect("リンクがない")
                    .length();
                let length = u32::try_from(length.0).expect("長さが u32 に収まらない");
                let offset = LaneOffset(r.below(length));
                SidewalkPos::on_link(net, link, offset).expect("歩道の位置がない")
            };
            (pick(), pick())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::Dir;
    use chizu_core::routing::{AstarRouter, Router};

    #[test]
    fn grid_is_connected_for_commutes() {
        let n = grid_network(6, 4, 3);
        assert_eq!(n.nodes().count(), 49);
        let reqs = commute_requests(1, &n, 500, 10);
        assert_eq!(reqs, commute_requests(1, &n, 500, 10));
        let routes = AstarRouter::new(&n).route_batch(&reqs);
        assert!(routes.iter().all(Option::is_some));
        let forward = reqs.iter().filter(|r| r.to.dir == Dir::Forward).count();
        assert!(
            (150..350).contains(&forward),
            "職場の向きが片寄った {forward}"
        );
    }
    use chizu_core::state_hash_of;

    #[test]
    fn makes_exactly_the_requested_number_of_citizens() {
        for n in [0, 1, 5, 1_000] {
            let p = synthetic_population(3, n);
            assert_eq!(p.citizen_count(), n as usize);
            assert!(p.household_count() <= n as usize);
        }
    }

    #[test]
    fn is_deterministic_and_depends_on_the_seed() {
        let a = state_hash_of(&synthetic_population(1, 10_000));
        assert_eq!(a, state_hash_of(&synthetic_population(1, 10_000)));
        assert_ne!(a, state_hash_of(&synthetic_population(2, 10_000)));
    }

    #[test]
    fn households_and_ages_are_in_range() {
        let p = synthetic_population(9, 10_000);
        let ids: Vec<_> = p.citizen_ids().collect();
        let mut heads = 0;
        for id in &ids {
            let row = p.citizen(*id).unwrap();
            let members = p.members(row.household).unwrap();
            assert!((1..=MAX_HOUSEHOLD_SIZE as usize).contains(&members.len()));
            assert!(row.age < 80);
            if members[0] == *id {
                heads += 1;
                assert!(row.age >= 18);
            }
        }
        assert_eq!(heads, p.household_count());
    }

    #[test]
    fn every_household_size_and_minors_appear() {
        let p = synthetic_population(9, 10_000);
        let mut sizes = [0_usize; MAX_HOUSEHOLD_SIZE as usize + 1];
        let mut minors = 0;
        for id in p.citizen_ids() {
            let row = p.citizen(id).unwrap();
            let members = p.members(row.household).unwrap();
            if members[0] == id {
                sizes[members.len()] += 1;
            } else if row.age < 18 {
                minors += 1;
            }
        }
        // 人数は 1〜4 人から一様に引くので、どの人数の世帯も全体の 1/8 より多い。
        let households = p.household_count();
        for size in 1..=MAX_HOUSEHOLD_SIZE as usize {
            assert!(sizes[size] * 8 > households, "{size} 人の世帯 {sizes:?}");
        }
        assert!(minors > 0);
    }

    /// ベンチマークの列車がそろう前提。どの路線も、終点で折り返せる本数（`turn_cap`）などの上限に抑えられずに、
    /// [`RAIL_BENCH_TRAINS`] 本を走らせられる。崩れると `rail_trains` が「列車が出そろわない」で落ちる。
    #[test]
    fn rail_bench_city_runs_every_requested_train() {
        let w = rail_trains_city(1, RAIL_BENCH_LINES);
        assert_eq!(w.rail().lines().count(), RAIL_BENCH_LINES);
        for (id, line) in w.rail().lines() {
            assert_eq!(
                line.allowed(),
                RAIL_BENCH_TRAINS,
                "{id:?}: 上限 {}、折り返し {}",
                line.cap(),
                line.turn_cap()
            );
        }
    }

    #[test]
    fn golden_hash() {
        assert_eq!(
            state_hash_of(&synthetic_population(42, 1_000)),
            16_390_134_796_798_341_638
        );
    }
}
