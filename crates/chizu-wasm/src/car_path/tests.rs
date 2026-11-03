use super::*;
use chizu_core::{Command, LaneMode, LaneOffset, RoadKind, TilePos, World};

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn control_point_is_where_the_lanes_meet() {
    // 東へ進んで北へ曲がる。y は画面の下向きなので、北は -y である。
    let p = control((-5.0, 1.0), (1.0, 0.0), (2.0, -5.0), (0.0, -1.0));
    assert!(close(p.0, 2.0) && close(p.1, 1.0), "{p:?}");
    // まっすぐなら真ん中。
    let p = control((-5.0, 1.0), (1.0, 0.0), (5.0, 1.0), (1.0, 0.0));
    assert!(close(p.0, 0.0) && close(p.1, 1.0), "{p:?}");
    // 折り返しも真ん中。
    let p = control((-5.0, 1.0), (1.0, 0.0), (-5.0, -1.0), (-1.0, 0.0));
    assert!(close(p.0, -5.0) && close(p.1, 0.0), "{p:?}");
}

#[test]
fn bezier_starts_and_ends_along_the_lanes() {
    let (p0, p1, p2) = ((-5.0, 1.0), (2.0, 1.0), (2.0, -5.0));
    let (a, da) = bezier(p0, p1, p2, 0.0);
    assert_eq!(a, p0);
    assert!(close(da.1.atan2(da.0), 0.0));
    let (b, db) = bezier(p0, p1, p2, 1.0);
    assert_eq!(b, p2);
    assert!(close(db.1.atan2(db.0), -core::f64::consts::FRAC_PI_2));
    // 途中の向きは、入る向きと出る向きのあいだにある。
    let (_, dm) = bezier(p0, p1, p2, 0.5);
    let m = dm.1.atan2(dm.0);
    assert!(m < 0.0 && m > -core::f64::consts::FRAC_PI_2);
}

#[test]
fn angle_codes_match_the_eight_headings() {
    assert_eq!(angle_code(0.0), 0);
    assert_eq!(angle_code(core::f64::consts::FRAC_PI_2), 16_384);
    assert_eq!(angle_code(core::f64::consts::PI), 32_768);
    assert_eq!(angle_code(-core::f64::consts::FRAC_PI_4), 57_344);
    assert_eq!(angle_code(-1e-12), 0);
}

/// 道路の始まりと終わりのマスと、道路の種類。
type Road = ((i64, i64), (i64, i64), RoadKind);

/// `roads` の順に、種類を指定して道路を引いた街。
fn town_of(roads: &[Road]) -> World {
    let mut w = World::new(1);
    for (seq, &(from, to, kind)) in roads.iter().enumerate() {
        let c = Command::BuildRoad {
            from: TilePos::new(from.0, from.1),
            to: TilePos::new(to.0, to.1),
            kind,
        };
        assert_eq!(
            w.apply_recorded(seq as u64, &c),
            chizu_core::ApplyOutcome::Applied
        );
    }
    w
}

const M: RoadKind = RoadKind::Medium;

/// (0, 10)〜(20, 10) の東西の道路と、(10, 0)〜(10, 20) の南北の道路が (10, 10) で交わる街。
fn crossroads() -> World {
    town_of(&[((0, 10), (20, 10), M), ((10, 0), (10, 20), M)])
}

fn link_toward(net: &Network, from: (i64, i64), to: (i64, i64)) -> LinkId {
    let (from, to) = (TilePos::new(from.0, from.1), TilePos::new(to.0, to.1));
    net.segments()
        .find_map(|(s, _)| {
            let line = net.segment_line(s)?;
            let dir = if line.start == from && line.end() == to {
                Dir::Forward
            } else if line.start == to && line.end() == from {
                Dir::Backward
            } else {
                return None;
            };
            Some(LinkId {
                segment: s,
                dir,
                mode: LaneMode::Car,
            })
        })
        .expect("区間がない")
}

fn path(link: LinkId, offset: u32, prev: Option<(LinkId, (Mm, Mm))>) -> CarPath {
    CarPath {
        link,
        offset: LaneOffset(offset),
        len: LaneOffset(80_000),
        prev,
        lane_shift: (Mm(0), Mm(0)),
    }
}

fn at(net: &Network, p: &CarPath, shift: (Mm, Mm)) -> Drawn {
    draw(net, &mut Paths::new(), p, shift, None, None)
        .unwrap()
        .drawn
}

const ZERO: (Mm, Mm) = (Mm(0), Mm(0));
const NODE: f64 = 10.0 * 8_000.0 + 4_000.0;

#[test]
fn a_car_waiting_at_the_junction_is_drawn_behind_the_stop_line() {
    let w = crossroads();
    let net = w.network();
    let east = link_toward(net, (0, 10), (10, 10));
    let d = at(net, &path(east, 80_000, None), ZERO);
    // 中くらいの道路の端は 0.4 マス、横断歩道 0.16、すき間 0.05、停止線 0.04、車の半分 0.2 マス。
    let want = NODE - (0.4 + 0.16 + 0.05 + 0.04 + 0.2) * 8_000.0;
    assert!(close(d.x, want), "{} != {want}", d.x);
    assert!(close(d.angle, 0.0));
}

#[test]
fn a_car_in_the_middle_of_a_link_is_drawn_where_the_core_has_it() {
    let w = crossroads();
    let net = w.network();
    let east = link_toward(net, (0, 10), (10, 10));
    let on = link_toward(net, (10, 10), (20, 10));
    // 始まりが交差点のリンクでも、始まりの窓を過ぎれば中核の位置のまま描く。
    for o in [7_000, 8_000, 40_000, 59_000] {
        let d = at(net, &path(on, o, Some((east, ZERO))), ZERO);
        assert!((d.x - (NODE + f64::from(o))).abs() < 1e-3, "{o}: {}", d.x);
    }
    // 始まりが行き止まりのリンクも、終わりの窓の手前までは中核の位置のまま描く。
    for o in [0, 8_000, 40_000, 59_000] {
        let d = at(net, &path(east, o, None), ZERO);
        assert!(
            (d.x - (4_000.0 + f64::from(o))).abs() < 1e-3,
            "{o}: {}",
            d.x
        );
    }
}

#[test]
fn a_queue_keeps_its_order_and_spacing() {
    let w = crossroads();
    let net = w.network();
    let east = link_toward(net, (0, 10), (10, 10));
    let spacing = (CAR_LENGTH + MIN_GAP) as f64;
    let xs: Vec<f64> = (0..8)
        .map(|k| {
            let o = 80_000 - k * (CAR_LENGTH + MIN_GAP);
            at(net, &path(east, o, None), ZERO).x
        })
        .collect();
    for p in xs.windows(2) {
        // 並びは変わらず、描いた車の長さ（0.4 マス）より離れる。
        assert!(p[0] - p[1] > 2.0 * CAR_HALF, "{xs:?}");
        assert!(p[0] - p[1] <= spacing + 1e-6, "{xs:?}");
    }
}

/// 1 マスのリンクの終わりで待つ車の前の端が、停止線（横断歩道の外側）の手前にあるか。
/// `before` は `from` の手前のノードで、そこから `from` へまっすぐ入ってくる。
fn stops_behind_the_line(w: &World, [before, from, to]: [(i64, i64); 3], len: u32, half: f64) {
    let net = w.network();
    let link = link_toward(net, from, to);
    let came = link_toward(net, before, from);
    let mut p = path(link, len, None);
    p.len = LaneOffset(len);
    let d = at(net, &p, ZERO);
    let g = geom(net, link).unwrap();
    let start = center(net, g.from).unwrap();
    let along = (d.x - start.0) * g.dir.0 + (d.y - start.1) * g.dir.1;
    let front = along + CAR_HALF;
    // 停止線の外側の、終わりのノードからの距離。detail.ts の stopLine と同じ式である。
    let line = half + ZEBRA_LENGTH + STOP_LINE_GAP + STOP_LINE_WIDTH;
    assert!(
        (f64::from(len) - front - line).abs() < 1e-3,
        "前の端 {front}、停止線 {}",
        f64::from(len) - line
    );
    // 中核で車間を空けて後ろにいる車は、待つ車と重ならない。
    let spacing = CAR_LENGTH + MIN_GAP;
    for o in [0, (len - spacing) / 2, len - spacing] {
        p.offset = LaneOffset(o);
        p.prev = Some((came, ZERO));
        let behind = at(net, &p, ZERO);
        let gap = (d.x - behind.x).hypot(d.y - behind.y);
        assert!(gap >= 2.0 * CAR_HALF, "{o}: {gap}");
    }
}

#[test]
fn on_a_one_tile_link_the_waiting_car_stays_behind_the_stop_line() {
    let w = town_of(&[
        ((0, 10), (20, 10), M),
        ((10, 0), (10, 20), M),
        ((11, 0), (11, 20), M),
    ]);
    stops_behind_the_line(&w, [(0, 10), (10, 10), (11, 10)], 8_000, 3_200.0);
    let w = town_of(&[
        ((0, 0), (20, 20), M),
        ((10, 0), (10, 20), M),
        ((11, 0), (11, 20), M),
    ]);
    let diagonal = 11_314;
    let half = 3_200.0 * core::f64::consts::SQRT_2;
    stops_behind_the_line(&w, [(0, 0), (10, 10), (11, 11)], diagonal, half);
}

#[test]
fn a_turning_car_follows_a_smooth_arc() {
    let w = crossroads();
    let net = w.network();
    let east = link_toward(net, (0, 10), (10, 10));
    let north = link_toward(net, (10, 10), (10, 0));
    let shown = (Mm(0), Mm(-1_000));
    let mut last = 0.0_f64;
    let mut prev_pos: Option<(f64, f64)> = None;
    let wait = at(net, &path(east, 80_000, None), shown);
    let mut paths = Paths::new();
    for o in (0..=7_000).step_by(250) {
        let p = path(north, o, Some((east, shown)));
        let d = draw(net, &mut paths, &p, ZERO, None, None).unwrap().drawn;
        // 向きは東から北へ、少しずつ回る。
        assert!(d.angle <= last + 1e-9, "{o}: {} > {last}", d.angle);
        assert!(last - d.angle < 0.5, "{o}: 向きが飛んだ");
        last = d.angle;
        if let Some(p) = prev_pos {
            assert!((d.x - p.0).hypot(d.y - p.1) < 1_000.0, "{o}: 位置が飛んだ");
        } else {
            // 曲がり始めは、停止線の手前で待っていた所と同じ。
            assert!(
                (d.x - wait.x).abs() < 1e-3 && (d.y - wait.y).abs() < 1e-3,
                "{d:?} {wait:?}"
            );
        }
        prev_pos = Some((d.x, d.y));
    }
    assert!(close(last, -core::f64::consts::FRAC_PI_2));
    // 窓を抜けた所は、中核の位置と同じ。
    let p = prev_pos.unwrap();
    assert!(
        (p.0 - NODE).abs() < 1e-3 && (p.1 - (NODE - 7_000.0)).abs() < 1e-3,
        "{p:?}"
    );
}

#[test]
fn a_car_going_straight_stays_in_its_lane() {
    let w = crossroads();
    let net = w.network();
    let east = link_toward(net, (0, 10), (10, 10));
    let on = link_toward(net, (10, 10), (20, 10));
    let lane = (Mm(0), Mm(-1_920));
    for o in (0..=7_000).step_by(500) {
        let mut p = path(on, o, Some((east, lane)));
        p.lane_shift = lane;
        let d = at(net, &p, lane);
        assert!((d.y - (NODE - 1_920.0)).abs() < 1e-3, "{o}: {d:?}");
        assert!(close(d.angle, 0.0));
    }
}

#[test]
fn a_rerouted_car_keeps_to_the_arc_it_was_on() {
    let w = crossroads();
    let net = w.network();
    let east = link_toward(net, (0, 10), (10, 10));
    let north = link_toward(net, (10, 10), (10, 0));
    let mut paths = Paths::new();
    let first = draw(
        net,
        &mut paths,
        &path(north, 3_000, Some((east, ZERO))),
        ZERO,
        None,
        None,
    )
    .unwrap();
    let entry = first.entry.expect("曲線の上にいる");
    // 経路を探し直して、抜けてきたリンクを忘れても、覚えた曲線の始まりで同じ所に、同じ道筋で描く。
    let again = draw(
        net,
        &mut paths,
        &path(north, 3_000, None),
        ZERO,
        Some(&entry),
        None,
    )
    .unwrap();
    assert_eq!(again.drawn, first.drawn);
    assert_eq!(again.entry, Some(entry));
    assert_eq!(again.route.map(|r| r.id), first.route.map(|r| r.id));
    // 別のノードを越えたあとの車には使わない。
    let other = link_toward(net, (10, 10), (20, 10));
    let d = draw(
        net,
        &mut paths,
        &path(other, 3_000, None),
        ZERO,
        Some(&entry),
        None,
    )
    .unwrap();
    assert!(close(d.drawn.angle, 0.0));
}

#[test]
fn a_car_moving_lanes_off_its_path_is_drawn_where_the_core_has_it() {
    let w = crossroads();
    let net = w.network();
    let east = link_toward(net, (0, 10), (10, 10));
    let on = link_toward(net, (10, 10), (20, 10));
    let mut p = path(on, 20_000, Some((east, ZERO)));
    p.lane_shift = (Mm(0), Mm(-1_920));
    // 区間の途中で車線を移っているあいだ（ノードを越えたときのずれから移るのではない）は、道筋の上にいない。
    let placed = draw(net, &mut Paths::new(), &p, (Mm(0), Mm(800)), None, None).unwrap();
    assert!(placed.route.is_none());
    assert!((placed.drawn.y - (NODE + 800.0)).abs() < 1e-6);
    // ノードを越えたときのずれから今の車線へ移っているあいだは、曲線から続く道筋の上にいる。
    let settling = draw(net, &mut Paths::new(), &p, (Mm(0), Mm(-800)), None, None).unwrap();
    assert!(settling.route.is_some());
    assert!((settling.drawn.y - (NODE - 1_920.0)).abs() < 1e-6);
}

#[test]
fn paths_are_found_once_per_network_and_never_reused() {
    let mut w = World::new_with_money(1, chizu_core::Money(1_000_000_000));
    for (from, to) in [((0, 10), (20, 10)), ((10, 0), (10, 20))] {
        let c = Command::BuildRoad {
            from: TilePos::new(from.0, from.1),
            to: TilePos::new(to.0, to.1),
            kind: M,
        };
        assert_eq!(w.apply(&c), chizu_core::ApplyOutcome::Applied);
    }
    let mut paths = Paths::new();
    let route = |w: &World, paths: &mut Paths| {
        let net = w.network();
        paths.sync(net);
        let east = link_toward(net, (0, 10), (10, 10));
        let north = link_toward(net, (10, 10), (10, 0));
        let p = path(north, 3_000, Some((east, ZERO)));
        draw(net, paths, &p, ZERO, None, None)
            .unwrap()
            .route
            .unwrap()
    };
    let a = route(&w, &mut paths);
    let sent = paths.take();
    assert_eq!(sent[0], 1.0, "初めて送るときは、前の道筋を忘れさせる");
    assert_eq!(sent[1], 1.0);
    assert_eq!(sent[2], f64::from(a.id));
    // 同じ網なら、同じ車線の組は同じ道筋で、送り直さない。
    assert_eq!(route(&w, &mut paths).id, a.id);
    assert_eq!(paths.take(), [0.0, 0.0]);
    // 網を変えたら、前の道筋は使わず、新しい番号の道筋を求め直す。
    let c = Command::BuildRoad {
        from: TilePos::new(14, 5),
        to: TilePos::new(14, 15),
        kind: M,
    };
    assert_eq!(w.apply(&c), chizu_core::ApplyOutcome::Applied);
    let b = route(&w, &mut paths);
    assert!(b.id > a.id);
    assert!(paths.get(a.id).is_none());
    let sent = paths.take();
    assert_eq!(sent[0], 1.0);
    assert_eq!(sent[2], f64::from(b.id));
}

/// リンク `link` の、端から数えて `index` 番目の車線のずれ。中核の `lane_shift::target` と同じ式である。
fn lane_of(net: &Network, link: LinkId, index: i64) -> (Mm, Mm) {
    let kind = net.segment(link.segment).unwrap().kind;
    let per_side = i64::from(kind.car_lanes_per_side());
    let lane = kind.lane_width_mm();
    let d = (per_side - index) * lane - lane / 2;
    let (sx, sy) = geom(net, link).unwrap().step;
    let (x, y) = if sx != 0 && sy != 0 {
        let c = d * 70_711 / 100_000;
        (c * sy, -c * sx)
    } else {
        (d * sy, -d * sx)
    };
    (Mm(x), Mm(y))
}

/// 中核の `lane_shift::blended` と同じ式で、`from` から `to` へ、移り始めてから `offset` 進んだ所のずれ。
fn blended(from: (Mm, Mm), to: (Mm, Mm), offset: u32) -> (Mm, Mm) {
    let at = i64::from(offset).clamp(0, 3_000);
    let mix = |a: i64, b: i64| Mm((a * (3_000 - at) + b * at) / 3_000);
    (mix(from.0.0, to.0.0), mix(from.1.0, to.1.0))
}

fn lanes(net: &Network, l: LinkId) -> i64 {
    i64::from(net.segment(l.segment).unwrap().kind.car_lanes_per_side())
}

/// 画面の補間（web/src/map/mover-interpolation.ts）を写したもの。前に見せていた位置と道筋から、次の tick の
/// 位置へ、どう動かすか。
#[derive(Clone, Debug)]
enum Plan {
    /// 位置と向きをまっすぐ移す。
    Linear(V, f64, V, f64),
    /// 道筋の区切り（番号、始まりの長さ、終わりの長さ）を順にたどる。
    Along(Vec<(u32, f64, f64)>),
}

/// 画面に見せている車。位置と向きと、道筋の上にいればその番号と長さ。
#[derive(Clone, Copy, Debug)]
struct Shown {
    p: V,
    angle: f64,
    on: Option<(u32, f64)>,
}

/// 2 つの道筋がつながっているか。前の道筋の終わりと、次の道筋の始まりが同じ所にある。
fn joins(paths: &Paths, a: u32, b: u32) -> bool {
    let (Some(a), Some(b)) = (paths.get(a), paths.get(b)) else {
        return false;
    };
    dist(a.at(a.length()).0, b.at(0.0).0) < 40.0
}

fn plan(paths: &Paths, from: &Shown, to: &Placed) -> Plan {
    let linear = Plan::Linear(from.p, from.angle, (to.drawn.x, to.drawn.y), to.drawn.angle);
    let Some(r) = to.route else {
        return linear;
    };
    let Some(p) = paths.get(r.id) else {
        return linear;
    };
    let s1 = f64::from(r.frac) * p.length();
    let Some((a, s0)) = from.on else {
        return linear;
    };
    if a == r.id {
        return Plan::Along(vec![(a, s0, s1)]);
    }
    let Some(pa) = paths.get(a) else {
        return linear;
    };
    if r.via != 0
        && let Some(pv) = paths.get(r.via)
        && joins(paths, a, r.via)
        && joins(paths, r.via, r.id)
    {
        return Plan::Along(vec![
            (a, s0, pa.length()),
            (r.via, 0.0, pv.length()),
            (r.id, 0.0, s1),
        ]);
    }
    if joins(paths, a, r.id) {
        return Plan::Along(vec![(a, s0, pa.length()), (r.id, 0.0, s1)]);
    }
    linear
}

/// 計画 `plan` の割合 `t` の所の位置、向き、道筋。
fn eval(paths: &Paths, plan: &Plan, t: f64) -> Shown {
    match plan {
        Plan::Linear(a, ha, b, hb) => Shown {
            p: add(*a, scale(sub(*b, *a), t)),
            angle: ha + turn_between(*ha, *hb) * t,
            on: None,
        },
        Plan::Along(pieces) => {
            let total: f64 = pieces.iter().map(|(_, s0, s1)| s1 - s0).sum();
            let mut left = total * t;
            for (k, &(id, s0, s1)) in pieces.iter().enumerate() {
                let span = s1 - s0;
                if left <= span || k + 1 == pieces.len() {
                    let s = s0 + left.min(span);
                    let (p, angle) = paths.get(id).unwrap().at(s);
                    return Shown {
                        p,
                        angle,
                        on: Some((id, s)),
                    };
                }
                left -= span;
            }
            unreachable!()
        }
    }
}

/// 1 台の車の、tick ごとの道筋。`legs` は通るリンクと、そのリンクの車線のずれ。
/// 車は最初のリンクの `start` から `speed` ずつ進み、最後のリンクの `stop` まで描く。
fn drive(
    net: &Network,
    paths: &mut Paths,
    legs: &[(LinkId, (Mm, Mm))],
    start: u32,
    stop: u32,
    speed: u32,
) -> Vec<Placed> {
    let lens: Vec<u32> = legs.iter().map(|(l, _)| link_len(net, *l) as u32).collect();
    let mut out = Vec::new();
    let mut last: Option<Last> = None;
    let mut entry: Option<Entry> = None;
    // 中核は、ノードを越えた車を越えた先の所に置き、そこからずれを移す（`lane_shift::Blend`）。
    let mut entered: Option<(usize, u32)> = None;
    let mut d = start;
    loop {
        // d はリンクを並べた道のりの上の距離。
        let mut k = 0;
        let mut o = d;
        while k + 1 < legs.len() && o > lens[k] {
            o -= lens[k];
            k += 1;
        }
        if k + 1 == legs.len() && o > stop {
            break;
        }
        let (link, lane) = legs[k];
        let prev = (k > 0).then(|| legs[k - 1]);
        let p = CarPath {
            link,
            offset: LaneOffset(o),
            len: LaneOffset(lens[k]),
            prev,
            lane_shift: lane,
        };
        if entered.is_none_or(|(leg, _)| leg != k) {
            entered = Some((k, o));
        }
        let since = entered.map_or(0, |(_, at)| o - at);
        let shift = prev.map_or(lane, |(_, from)| blended(from, lane, since));
        let placed = draw(net, paths, &p, shift, entry.as_ref(), last.as_ref()).unwrap();
        entry = placed.entry;
        last = placed.route.map(|r| r.last);
        out.push(placed);
        d += speed;
    }
    out
}

/// 車の向き `angle` の単位ベクトル。
fn heading(angle: f64) -> V {
    (angle.cos(), angle.sin())
}

/// 道筋の上を `step` mm ずつ動かして、車の絵が車道 `cw` からはみ出す量のいちばん大きい値と、
/// 1 歩で向きが変わるいちばん大きい値。
fn sweep(paths: &Paths, id: u32, cw: &Carriageway, step: f64) -> (f64, f64) {
    let p = paths.get(id).unwrap();
    let (mut worst, mut turn) = (0.0_f64, 0.0_f64);
    let mut last: Option<f64> = None;
    let mut s = 0.0;
    while s <= p.length() {
        let (c, a) = p.at(s);
        worst = worst.max(cw.overshoot(c, heading(a), -1.0));
        if let Some(l) = last {
            turn = turn.max(turn_between(l, a).abs());
        }
        last = Some(a);
        s += step;
    }
    (worst, turn)
}

/// 車の絵が車道の外に出るのを許す量（mm）。どの形でも少し出る交差点では、出る量のいちばん小さい形を選ぶので、
/// その分を許す。倍率 128 でも 1 ピクセル（62.5mm）の 4 分の 1 である。
const SLACK: f64 = 16.0;

/// 交差点の形ごとに確かめた結果。
#[derive(Default, Debug)]
struct Report {
    moves: usize,
    /// 補間の途中で、許す量を超えてはみ出した量のいちばん大きい値。
    over: f64,
    /// 許した量のいちばん大きい値。
    allowed: f64,
    /// 道筋の上を 25mm 動かしたときに、向きが変わるいちばん大きい値。
    turn: f64,
    /// 折り返しに近い動きの数。
    sharp: usize,
}

/// ノード `n` に入るリンクと出るリンクのすべての組（折り返しを除く）と、すべての車線の組で、実際の速さで
/// 車を進め、補間の途中の車の絵が車道からはみ出さないこと、道筋が飛ばないこと、tick と tick のあいだを
/// 道筋に沿って補間できることを確かめる。
fn every_move(w: &World, node: (i64, i64)) -> Report {
    let net = w.network();
    let n = net
        .node_at(TilePos::new(node.0, node.1))
        .expect("ノードがない");
    let cw = Carriageway::of(net, n).unwrap();
    let links = |s: chizu_core::SegmentId| {
        [Dir::Forward, Dir::Backward].map(|dir| LinkId {
            segment: s,
            dir,
            mode: LaneMode::Car,
        })
    };
    let all: Vec<LinkId> = net
        .node(n)
        .unwrap()
        .segments
        .iter()
        .flat_map(|&s| links(s))
        .collect();
    let mut paths = Paths::new();
    paths.sync(net);
    let mut report = Report::default();
    for &inn in all.iter().filter(|l| geom(net, **l).unwrap().to == n) {
        for &out in all.iter().filter(|l| geom(net, **l).unwrap().from == n) {
            if out.segment == inn.segment {
                continue;
            }
            for i in 0..lanes(net, inn) {
                for j in 0..lanes(net, out) {
                    let shown = lane_of(net, inn, i);
                    let lane = lane_of(net, out, j);
                    report.moves += 1;
                    check_move(
                        net,
                        &mut paths,
                        &cw,
                        [(inn, shown), (out, lane)],
                        &mut report,
                    );
                }
            }
        }
    }
    assert!(report.moves > 0);
    eprintln!("{node:?}: {report:?}");
    report
}

/// 入るリンクからノードを曲がって出ていくリンクへ進む車を確かめる。
fn check_move(
    net: &Network,
    paths: &mut Paths,
    cw: &Carriageway,
    legs: [(LinkId, (Mm, Mm)); 2],
    report: &mut Report,
) {
    let [(inn, shown), (out, lane)] = legs;
    let in_len = link_len(net, inn) as u32;
    let out_len = link_len(net, out) as u32;
    // 曲がる道筋。停止線の手前から窓の終わりまで。
    let probe = CarPath {
        link: out,
        offset: LaneOffset(0),
        len: LaneOffset(out_len),
        prev: Some((inn, shown)),
        lane_shift: lane,
    };
    let turn = draw(net, paths, &probe, shown, None, None)
        .unwrap()
        .route
        .expect("曲がる道筋がない");
    let id = turn.id;
    // 斜めの腕は、区間の途中でも車線の上の車が車道の外に出る。その量までは許す。
    let p = paths.get(id).unwrap();
    let straight_over = |at: V, a: f64| cw.overshoot(at, heading(a), -1.0);
    let (p0, a0) = p.at(0.0);
    let (pe, ae) = p.at(p.length());
    let allowed = straight_over(sub(p0, scale(heading(a0), 6_000.0)), a0)
        .max(straight_over(add(pe, scale(heading(ae), 6_000.0)), ae))
        .max(straight_over(p0, a0));
    report.allowed = report.allowed.max(allowed);
    let (worst, step_turn) = sweep(paths, id, cw, 25.0);
    report.turn = report.turn.max(step_turn);
    assert!(
        worst <= allowed + SLACK,
        "{inn:?} -> {out:?} {shown:?} {lane:?}: 道筋が車道の外に {worst} 出た（許す量 {allowed}）"
    );
    // 25mm 動かして向きが、選ぶ形の上限ほどより大きく変わらない。折り返しに近い（115 度より大きく曲がる）
    // ものは、入る車線と出る車線が近く、どの形も上限より急になるので、いちばん緩い形を選ぶ。
    let sharp = turn_between(a0, ae).abs() > 2.0;
    let curvature = if sharp {
        report.sharp += 1;
        step_turn / 25.0
    } else {
        MAX_CURVATURE
    };
    assert!(
        step_turn <= 25.0 * curvature * 1.05,
        "{inn:?} -> {out:?} {shown:?} {lane:?}: 向きが飛んだ {step_turn}"
    );
    for speed in [2_500, 5_000, 11_111, 13_889, 16_667] {
        for phase in (0..speed).step_by(1_250) {
            let start = in_len.saturating_sub(3 * speed) + phase;
            let ticks = drive(
                net,
                paths,
                &legs,
                start,
                (TURN_MM as u32 + 2 * speed).min(out_len),
                speed,
            );
            let mut shown_at: Option<Shown> = None;
            for placed in &ticks {
                let Some(from) = shown_at else {
                    shown_at = Some(Shown {
                        p: (placed.drawn.x, placed.drawn.y),
                        angle: placed.drawn.angle,
                        on: placed
                            .route
                            .map(|r| (r.id, f64::from(r.frac) * paths.get(r.id).unwrap().length())),
                    });
                    continue;
                };
                let pl = plan(paths, &from, placed);
                assert!(
                    matches!(pl, Plan::Along(_)),
                    "{inn:?} -> {out:?} {speed}/{phase}: 道筋に沿って補間しない {from:?} -> {placed:?}"
                );
                let mut last_angle = from.angle;
                let moved = match &pl {
                    Plan::Along(pieces) => pieces.iter().map(|(_, a, b)| b - a).sum::<f64>(),
                    Plan::Linear(..) => 0.0,
                };
                for k in 0..=10 {
                    let s = eval(paths, &pl, f64::from(k) / 10.0);
                    let over = cw.overshoot(s.p, heading(s.angle), -1.0);
                    report.over = report.over.max(over - allowed);
                    assert!(
                        over <= allowed + SLACK,
                        "{inn:?} -> {out:?} {shown:?} {lane:?} {speed}/{phase} t {k}: 補間の途中で {over} はみ出した（許す量 {allowed}）"
                    );
                    // 向きは、道筋の上を動いた長さに見合う分だけ変わる。
                    let jump = turn_between(last_angle, s.angle).abs();
                    let can = if k == 0 {
                        1e-6
                    } else {
                        moved / 10.0 * curvature * 1.05 + 1e-6
                    };
                    assert!(
                        jump <= can,
                        "{inn:?} -> {out:?} {speed}/{phase} t {k}: 向きが {jump} 飛んだ（{can} まで）"
                    );
                    last_angle = s.angle;
                }
                shown_at = Some(eval(paths, &pl, 1.0));
            }
        }
    }
}

#[test]
fn turning_cars_stay_on_the_carriageway_at_a_crossroads() {
    every_move(&crossroads(), (10, 10));
}

#[test]
fn turning_cars_stay_on_the_carriageway_at_a_t_junction_of_two_kinds() {
    // #209 の交差点と同じ形。南北の中くらいの道路に、東から小さい道路がつながる。
    let w = town_of(&[
        ((10, 0), (10, 20), M),
        ((10, 10), (20, 10), RoadKind::Small),
    ]);
    every_move(&w, (10, 10));
}

#[test]
fn turning_cars_stay_on_the_carriageway_where_kinds_differ() {
    // 大きい道路と小さい道路の十字路。車線の数も幅も違う。
    let w = town_of(&[
        ((0, 10), (20, 10), RoadKind::Large),
        ((10, 0), (10, 20), RoadKind::Small),
    ]);
    every_move(&w, (10, 10));
    // 大きい道路に、中くらいの道路が北からつながる T 字路。
    let w = town_of(&[((0, 10), (20, 10), RoadKind::Large), ((10, 0), (10, 10), M)]);
    every_move(&w, (10, 10));
}

#[test]
fn turning_cars_stay_on_the_carriageway_with_a_diagonal_arm() {
    // 中くらいの道路の十字路に、北西から斜めの小さい道路がつながる。
    let w = town_of(&[
        ((0, 10), (20, 10), M),
        ((10, 0), (10, 20), M),
        ((2, 2), (10, 10), RoadKind::Small),
    ]);
    let r = every_move(&w, (10, 10));
    assert_eq!(r.allowed, 0.0, "{r:?}");
}

#[test]
fn turning_cars_on_wide_diagonal_arms_stick_out_no_more_than_on_the_arm() {
    // 斜めの中くらいと大きい道路の腕は、画面の車道の幅を 0.2 マスに抑えるので、区間の途中でも
    // 車線の上の車が車道の外に出る。曲がる車は、その量より外へ出ない。
    for kind in [M, RoadKind::Large] {
        let w = town_of(&[
            ((0, 10), (20, 10), kind),
            ((10, 0), (10, 20), kind),
            ((2, 2), (10, 10), kind),
            ((18, 2), (10, 10), kind),
        ]);
        every_move(&w, (10, 10));
    }
}

#[test]
fn turning_cars_stay_on_the_carriageway_at_bends() {
    // 中くらいの道路が、北へ小さい道路に曲がってつながる角。
    let w = town_of(&[((0, 10), (10, 10), M), ((10, 10), (10, 0), RoadKind::Small)]);
    every_move(&w, (10, 10));
    // 大きい道路が、北東へ斜めに曲がる角。
    let w = town_of(&[
        ((0, 10), (10, 10), RoadKind::Large),
        ((10, 10), (18, 2), RoadKind::Large),
    ]);
    every_move(&w, (10, 10));
}

/// (0, 10)〜(30, 10) の東西の道路と、(10, 0)〜(10, 20)、(11, 0)〜(11, 20) の南北の道路の街。
/// (10, 10) と (11, 10) のあいだは 1 マスのリンクになる。
fn one_tile_town() -> World {
    town_of(&[
        ((0, 10), (30, 10), M),
        ((10, 0), (10, 20), M),
        ((11, 0), (11, 20), M),
    ])
}

#[test]
fn turning_cars_stay_on_the_carriageway_around_a_one_tile_link() {
    let w = one_tile_town();
    every_move(&w, (10, 10));
    every_move(&w, (11, 10));
}

#[test]
fn a_car_crossing_a_one_tile_link_in_one_tick_follows_both_turns() {
    // 北から南へ下りて (10, 10) で東へ曲がり、1 マスのリンクを抜けて、(11, 10) で北へ曲がる。
    // 1 tick に 1 マス以上進むので、前の tick と次の tick のあいだに 1 マスのリンクを丸ごと通り抜ける。
    let w = one_tile_town();
    let net = w.network();
    let down = link_toward(net, (10, 0), (10, 10));
    let short = link_toward(net, (10, 10), (11, 10));
    let up = link_toward(net, (11, 10), (11, 0));
    let a = Carriageway::of(net, net.node_at(TilePos::new(10, 10)).unwrap()).unwrap();
    let b = Carriageway::of(net, net.node_at(TilePos::new(11, 10)).unwrap()).unwrap();
    let mut paths = Paths::new();
    paths.sync(net);
    let mut chained = 0;
    for (i, j, k) in [(0, 0, 0), (1, 0, 1), (0, 1, 0), (1, 1, 1)] {
        let legs = [
            (down, lane_of(net, down, i)),
            (short, lane_of(net, short, j)),
            (up, lane_of(net, up, k)),
        ];
        for speed in [11_111, 13_889, 16_667] {
            for phase in (0..speed).step_by(1_000) {
                let start = link_len(net, down) as u32 - 2 * speed + phase;
                let ticks = drive(net, &mut paths, &legs, start, 7_000 + speed, speed);
                let mut from: Option<Shown> = None;
                for placed in &ticks {
                    let Some(f) = from else {
                        let r = placed.route.unwrap();
                        from = Some(Shown {
                            p: (placed.drawn.x, placed.drawn.y),
                            angle: placed.drawn.angle,
                            on: Some((r.id, f64::from(r.frac) * paths.get(r.id).unwrap().length())),
                        });
                        continue;
                    };
                    let pl = plan(&paths, &f, placed);
                    let Plan::Along(pieces) = &pl else {
                        panic!("{speed}/{phase}: 道筋に沿って補間しない {f:?} -> {placed:?}");
                    };
                    chained += usize::from(pieces.len() == 3);
                    for t in 0..=20 {
                        let s = eval(&paths, &pl, f64::from(t) / 20.0);
                        let h = heading(s.angle);
                        let over = a.overshoot(s.p, h, -1.0).min(b.overshoot(s.p, h, -1.0));
                        assert!(
                            over <= SLACK,
                            "{i}{j}{k} {speed}/{phase} t {t}: {over} はみ出した {s:?}"
                        );
                    }
                    from = Some(eval(&paths, &pl, 1.0));
                }
            }
        }
    }
    assert!(chained > 0, "1 マスのリンクを丸ごと通り抜ける tick がない");
}
