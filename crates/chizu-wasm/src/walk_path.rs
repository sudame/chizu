//! 歩く人を、交差点で歩道の角を回る道筋の上に描く。
//!
//! 中核は、歩く人の位置をリンクの上の距離で持ち、リンクはノードの中心からノードの中心までである。
//! 交差点を渡っているあいだは、リンクの終わり（ノードの中心）にいるものとし、渡る腕の横断歩道を順に並べた
//! 道のりの上の位置を、リンクの終わりを越えた距離で表す。そのまま描くと、人が交差点の中央を通り、
//! 渡るあいだも角に立ったままになる。
//!
//! 画面では、リンクの両端の短い窓（`WALK_WINDOW_MM`）の中だけ、リンクの上の距離を描く位置に写し替える。
//! 窓の外は中核の位置のまま描くので、区間の途中を横切る人や、車と位置がそろう。状態は変えない。
//!
//! - 終わりの窓：リンクの終わりの手前から、歩道の角、横断歩道の手前の角まで、折れ線と二次ベジエ曲線の
//!   道筋に沿って描く。窓の始まりでは中核の位置と同じで、窓の中を一様に進める。
//! - 渡るあいだ：渡る腕の横断歩道の縞の上を、中核の進み具合に合わせて進む。渡り終えた所から次の横断歩道の
//!   手前の角（最後なら、出ていく腕の歩道の角）までの道筋は、その横断歩道を渡るあいだに進む。
//! - 始まりの窓：出ていく腕の歩道の角から、窓の終わりの中核の位置まで、まっすぐに写す。
//!
//! 角の位置は、横断歩道の縞（`web/src/map/roads.ts` の `zebra`）の真ん中の位置と、歩道の中心線に
//! 合わせる。区間の途中の車と同じ式で道路の端を求める（`car_path`）。

use chizu_core::movement::crossing;
use chizu_core::movement::{WALK_WINDOW_MM, WalkAhead, WalkPath};
use chizu_core::{Network, NodeId, SegmentId, SegmentKind};

use crate::car_path::{
    Drawn, V, ZEBRA_LENGTH, arc_length, bezier, center, control, dist, edge, geom, is_bend,
    is_junction, mm,
};

/// 出ていく腕の角から窓の終わりまでを写すとき、角の位置を窓の長さの何割までに抑えるか。人が後ろへ
/// 戻らないようにする。
const MAX_STOP: f64 = 0.8;

/// 描く道筋の 1 区切り。
#[derive(Clone, Copy, Debug)]
enum Piece {
    Line(V, V),
    Quad(V, V, V),
}

impl Piece {
    fn length(&self) -> f64 {
        match *self {
            Piece::Line(a, b) => dist(a, b),
            Piece::Quad(a, c, b) => arc_length(a, c, b),
        }
    }

    /// 長さ `s` の所の位置と向き。
    fn at(&self, s: f64, length: f64) -> (V, V) {
        match *self {
            Piece::Line(a, b) => {
                let t = if length > 0.0 { s / length } else { 1.0 };
                (
                    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t),
                    (b.0 - a.0, b.1 - a.1),
                )
            }
            Piece::Quad(a, c, b) => {
                let t = if length > 0.0 { s / length } else { 1.0 };
                bezier(a, c, b, t.clamp(0.0, 1.0))
            }
        }
    }
}

/// 区切りを順に並べた道筋の、長さの割合 `f` の所。
fn along_pieces(pieces: &[Piece], f: f64) -> Drawn {
    let lens: Vec<f64> = pieces.iter().map(Piece::length).collect();
    let total: f64 = lens.iter().sum();
    let mut left = f.clamp(0.0, 1.0) * total;
    let mut found = ((0.0, 0.0), (1.0, 0.0));
    for (k, (p, &l)) in pieces.iter().zip(&lens).enumerate() {
        found = p.at(left.min(l), l);
        if left <= l || k + 1 == pieces.len() {
            break;
        }
        left -= l;
    }
    let (pos, d) = found;
    let d = if d.0.abs() + d.1.abs() < 1e-9 {
        (1.0, 0.0)
    } else {
        d
    };
    Drawn {
        x: pos.0,
        y: pos.1,
        angle: d.1.atan2(d.0),
    }
}

/// ノード `node` から区間 `seg` の腕の向きへ出る 1 歩。
fn arm_step(net: &Network, node: NodeId, seg: SegmentId) -> Option<(i64, i64)> {
    let s = net.segment(seg)?;
    let line = net.segment_line(seg)?;
    Some(if s.a == node {
        line.step
    } else {
        (-line.step.0, -line.step.1)
    })
}

fn unit(step: (i64, i64)) -> V {
    let len = ((step.0 * step.0 + step.1 * step.1) as f64).sqrt();
    (step.0 as f64 / len, step.1 as f64 / len)
}

/// ノード `node` から区間 `seg` の腕へ、角の位置までの距離（mm）。交差点なら横断歩道の縞の真ん中、
/// 角（2 本だけがつながるノード）なら道路の端である。
fn corner_along(net: &Network, node: NodeId, seg: SegmentId) -> Option<f64> {
    let step = arm_step(net, node, seg)?;
    let zebra = is_junction(net, node);
    Some(edge(net, node, step) + if zebra { ZEBRA_LENGTH / 2.0 } else { 0.0 })
}

/// ノード `node` の腕 `seg` の角の位置（歩道の中心線の上）と、腕の向きの単位ベクトル。
/// `cw` は、腕の時計回りの側かどうか（出ていく向きの右）。
fn corner(net: &Network, node: NodeId, seg: SegmentId, cw: bool) -> Option<(V, V)> {
    let step = arm_step(net, node, seg)?;
    let u = unit(step);
    let kind: SegmentKind = net.segment(seg)?.kind;
    let d = (i64::from(kind.car_lanes_per_side()) * kind.lane_width_mm()
        + kind.sidewalk_width_mm() / 2) as f64;
    // 腕の向きの左は画面で (u.1, -u.0) の向きである。
    let side = if cw { -d } else { d };
    let c = center(net, node)?;
    let along = corner_along(net, node, seg)?;
    Some((
        (
            c.0 + u.0 * along + u.1 * side,
            c.1 + u.1 * along - u.0 * side,
        ),
        u,
    ))
}

/// 窓を使うノードか。3 本以上がつながる交差点か、向きを変える角である。
fn windowed(net: &Network, node: NodeId) -> bool {
    is_junction(net, node) || is_bend(net, node)
}

/// 角 `a` から角 `b` への区切り。同じ腕の角どうしならまっすぐ、違う腕なら歩道の中心線が交わる所を
/// 通る曲線で結ぶ。
fn step_piece(a: (V, V, SegmentId), b: (V, V, SegmentId)) -> Piece {
    if a.2 == b.2 {
        Piece::Line(a.0, b.0)
    } else {
        let inward = (-a.1.0, -a.1.1);
        Piece::Quad(a.0, control(a.0, inward, b.0, b.1), b.0)
    }
}

/// 通る道筋を、横断歩道を渡る前の区切りと、渡る横断歩道ごとの区切りに分けたもの。
struct Pass {
    /// 最初の横断歩道の手前の角（渡らなければ、出ていく腕の角）まで。リンク上の点から始まる。
    approach: Vec<Piece>,
    /// 渡る横断歩道と、渡り終えた所から次の横断歩道の手前の角（最後なら、出ていく腕の角）まで。
    crossings: Vec<Vec<Piece>>,
}

fn pass(net: &Network, ahead: &WalkAhead, from: V) -> Option<Pass> {
    let node = ahead.node;
    let points: Vec<(V, V, SegmentId)> = ahead
        .corners
        .iter()
        .map(|&(s, cw)| corner(net, node, s, cw).map(|(p, u)| (p, u, s)))
        .collect::<Option<_>>()?;
    let (first, last) = (points.first()?, points.last()?);
    let mut pass = Pass {
        approach: vec![Piece::Line(from, first.0)],
        crossings: Vec::new(),
    };
    if !is_junction(net, node) {
        // 2 本だけがつながる角では、車道を渡らない。入る角から出る角へ、1 本の曲線で結ぶ。
        pass.approach.push(step_piece(*first, *last));
        return Some(pass);
    }
    let mut arms = ahead.arms.iter().peekable();
    for w in points.windows(2) {
        let piece = step_piece(w[0], w[1]);
        let crossing = w[0].2 == w[1].2 && arms.peek().is_some_and(|&&s| s == w[0].2);
        if crossing {
            arms.next();
            pass.crossings.push(vec![piece]);
        } else if let Some(stage) = pass.crossings.last_mut() {
            stage.push(piece);
        } else {
            pass.approach.push(piece);
        }
    }
    Some(pass)
}

/// 歩く人を描く位置と向き。窓の外にいるなど、写し替えない人は `None` で、中核の位置のまま描く。
pub fn draw(net: &Network, wp: &WalkPath) -> Option<Drawn> {
    let g = geom(net, wp.link)?;
    let len = f64::from(wp.len.0);
    if len <= 0.0 {
        return None;
    }
    let o = f64::from(wp.offset.0);
    let start = center(net, g.from)?;
    let lat = mm(wp.lane_shift);
    let on_link = |along: f64| Drawn {
        x: start.0 + g.dir.0 * along + lat.0,
        y: start.1 + g.dir.1 * along + lat.1,
        angle: g.dir.1.atan2(g.dir.0),
    };
    let window = f64::from(WALK_WINDOW_MM);
    let from_node = wp.entered && windowed(net, g.from);
    let ahead = wp
        .ahead
        .as_ref()
        .filter(|a| a.node == g.to && windowed(net, g.to));
    let ws = if from_node {
        window.min(len / 2.0)
    } else {
        0.0
    };
    let we = if ahead.is_some() {
        window.min(len - ws)
    } else {
        0.0
    };
    if from_node && o < ws {
        let stop = corner_along(net, g.from, wp.link.segment)?.min(MAX_STOP * ws);
        return Some(on_link(stop + o * (ws - stop) / ws));
    }
    let ahead = ahead?;
    if o <= len - we {
        return None;
    }
    let pass = pass(net, ahead, {
        let d = on_link(len - we);
        (d.x, d.y)
    })?;
    if o <= len {
        return Some(along_pieces(&pass.approach, (o - (len - we)) / we));
    }
    // 渡っているあいだ。渡る腕の横断歩道を順に並べた道のりの上の位置から、どの横断歩道のどこかを求める。
    let mut x = o - len;
    let last = pass.crossings.len().checked_sub(1)?;
    for (k, (stage, &arm)) in pass.crossings.iter().zip(&ahead.arms).enumerate() {
        let w = f64::from(crossing::width(net, arm));
        if x < w || k == last {
            let f = if w > 0.0 { (x / w).min(1.0) } else { 1.0 };
            return Some(along_pieces(stage, f));
        }
        x -= w;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movers::spawn_dev_walks;
    use chizu_core::network::TILE_MM;
    use chizu_core::{
        Command, Dir, LaneMode, LaneOffset, LinkId, Mm, RoadKind, TilePos, TileRect, World,
    };

    const TILE: f64 = TILE_MM as f64;
    /// 交差点 (10, 10) の中心。
    const NODE: V = (10.0 * TILE + TILE / 2.0, 10.0 * TILE + TILE / 2.0);

    /// (0, 10)〜(20, 10) の東西の道路と、(10, 0)〜(10, 20) の南北の道路が (10, 10) で交わる街。
    fn crossroads() -> World {
        let mut w = World::new(1);
        for (seq, (from, to)) in [((0, 10), (20, 10)), ((10, 0), (10, 20))]
            .iter()
            .enumerate()
        {
            let c = Command::BuildRoad {
                from: TilePos::new(from.0, from.1),
                to: TilePos::new(to.0, to.1),
                kind: RoadKind::Medium,
            };
            w.apply_recorded(seq as u64, &c);
        }
        w
    }

    fn view() -> TileRect {
        TileRect {
            min: TilePos::new(0, 0),
            max: TilePos::new(20, 20),
        }
    }

    /// 1 人の `tick` ごとの描く位置、中核の位置、渡っているか。
    type Seen = Vec<(V, V, bool)>;

    /// 歩く人を 1 人出して、`tick` ごとに描き、道筋を持つ間の位置を集める。
    fn trace(from: (i64, i64), to: (i64, i64)) -> Seen {
        let mut w = crossroads();
        let n = spawn_dev_walks(
            &mut w,
            TilePos::new(from.0, from.1),
            TilePos::new(to.0, to.1),
            1,
        );
        assert_eq!(n, 1);
        let mut out = Vec::new();
        for _ in 0..3_000 {
            w.step();
            for m in w.movers_in(view()) {
                let Some(walk) = m.walk.as_ref() else {
                    continue;
                };
                let core = (
                    m.pos.x.0 as f64 + m.shift.0.0 as f64,
                    m.pos.y.0 as f64 + m.shift.1.0 as f64,
                );
                let d = draw(w.network(), walk).map_or(core, |d| (d.x, d.y));
                out.push((d, core, walk.offset >= walk.len));
            }
        }
        out
    }

    #[test]
    fn walkers_do_not_pass_through_the_middle_of_the_junction() {
        // 東西に直進、東から北へ曲がる、南から西へ曲がる。
        for (from, to) in [
            ((2, 10), (18, 10)),
            ((18, 10), (10, 2)),
            ((10, 18), (2, 10)),
        ] {
            let seen = trace(from, to);
            assert!(!seen.is_empty());
            let nearest = |f: fn(&(V, V, bool)) -> V| {
                seen.iter()
                    .map(|s| dist(f(s), NODE))
                    .fold(f64::MAX, f64::min)
            };
            // 中核の位置は、渡っているあいだ交差点の中心にある。描いた位置は、歩道の中心線より内側へ入らない。
            assert!(
                nearest(|s| s.0) > 3_500.0,
                "{from:?}->{to:?}: {}",
                nearest(|s| s.0)
            );
            assert!(nearest(|s| s.1) < 3_000.0, "{from:?}->{to:?}");
        }
    }

    #[test]
    fn a_walker_never_jumps_and_crosses_on_the_stripes() {
        let seen = trace((2, 10), (18, 10));
        for p in seen.windows(2) {
            // 1 tick の歩みは 1.4m ほど。写し替えで速くなっても、飛ぶほどにはならない。
            assert!(dist(p[0].0, p[1].0) < 3_000.0, "{:?} -> {:?}", p[0], p[1]);
        }
        // 渡っているあいだは、腕の縞の真ん中（交差点の中心から道路の端の 0.4 マス + 縞の半分）の上にいる。
        let stripe = 0.4 * TILE + ZEBRA_LENGTH / 2.0;
        let crossing: Vec<_> = seen.iter().filter(|s| s.2).collect();
        assert!(!crossing.is_empty());
        // 縞は腕に直角で、腕の向きの位置は縞の真ん中。渡る途中の位置は、縞の真ん中の線の上にある。
        let on_stripe = |d: &V| {
            let (dx, dy) = ((d.0 - NODE.0).abs(), (d.1 - NODE.1).abs());
            ((dx - stripe).abs() < 1.0 && dy < 2_881.0)
                || ((dy - stripe).abs() < 1.0 && dx < 2_881.0)
        };
        assert!(crossing.iter().any(|s| on_stripe(&s.0)));
    }

    #[test]
    fn the_middle_of_a_link_stays_where_the_core_has_it() {
        let seen = trace((2, 10), (18, 10));
        // 写し替えの大きさは窓の長さを超えない。
        let window = f64::from(WALK_WINDOW_MM);
        assert!(seen.iter().all(|(d, c, _)| dist(*d, *c) < window + TILE));
    }

    #[test]
    fn the_start_window_meets_the_core_at_its_end() {
        let w = crossroads();
        let net = w.network();
        let seg = net
            .segments()
            .find_map(|(s, _)| {
                let l = net.segment_line(s)?;
                (l.start == TilePos::new(10, 10) && l.end() == TilePos::new(20, 10)).then_some(s)
            })
            .expect("区間がない");
        let link = LinkId {
            segment: seg,
            dir: Dir::Forward,
            mode: LaneMode::Sidewalk,
        };
        let lane = (Mm(0), Mm(2_880));
        let at = |o: u32| {
            let wp = WalkPath {
                link,
                offset: LaneOffset(o),
                len: LaneOffset(80_000),
                lane_shift: lane,
                entered: true,
                ahead: None,
            };
            draw(net, &wp)
        };
        // 角の歩道の上から始まり、中核の位置へ向かって窓の中を進む。
        let first = at(0).expect("窓の中");
        let stop = 0.4 * TILE + ZEBRA_LENGTH / 2.0;
        assert!((first.x - (NODE.0 + stop)).abs() < 1e-6, "{first:?}");
        assert!((first.y - (NODE.1 + 2_880.0)).abs() < 1e-6);
        let mut last = first.x;
        for o in (0..WALK_WINDOW_MM).step_by(500) {
            let d = at(o).expect("窓の中");
            assert!(d.x >= last);
            last = d.x;
        }
        // 窓の外は中核の位置のまま。窓の端でも飛ばない。
        assert!(at(WALK_WINDOW_MM).is_none());
        let edge_drawn = at(WALK_WINDOW_MM - 1).expect("窓の中");
        let want = NODE.0 + f64::from(WALK_WINDOW_MM - 1);
        assert!((edge_drawn.x - want).abs() < 5.0, "{edge_drawn:?}");
    }

    #[test]
    fn a_walker_waits_at_the_corner_and_crosses_stripe_by_stripe() {
        use chizu_core::movement::crossing;
        let w = crossroads();
        let net = w.network();
        let node = net
            .segments()
            .find_map(|(s, seg)| {
                let l = net.segment_line(s)?;
                (l.start == TilePos::new(0, 10)).then_some(seg.b)
            })
            .expect("区間がない");
        let link_of = |from: TilePos, to: TilePos| {
            net.segments()
                .find_map(|(s, _)| {
                    let l = net.segment_line(s)?;
                    let dir = if l.start == from && l.end() == to {
                        Dir::Forward
                    } else if l.start == to && l.end() == from {
                        Dir::Backward
                    } else {
                        return None;
                    };
                    Some(LinkId {
                        segment: s,
                        dir,
                        mode: LaneMode::Sidewalk,
                    })
                })
                .expect("区間がない")
        };
        let t = TilePos::new;
        let (from, to) = (link_of(t(0, 10), t(10, 10)), link_of(t(10, 10), t(20, 10)));
        // 東へ歩いて直進する、右の歩道の人。
        let (far, arms) = crossing::depart(net, node, (from, false), to, None, None);
        let corners = crossing::corner_route(net, node, (from, false), (to, far));
        assert!(!arms.is_empty());
        let len = 80_000;
        let path = |offset: u32| WalkPath {
            link: from,
            offset: LaneOffset(offset),
            len: LaneOffset(len),
            lane_shift: (Mm(0), Mm(2_880)),
            entered: false,
            ahead: Some(chizu_core::WalkAhead {
                node,
                corners: corners.clone(),
                arms: arms.to_vec(),
            }),
        };
        let at = |offset: u32| draw(net, &path(offset)).expect("窓の中");
        // 窓の始まりそのものは、中核の位置のまま描く。
        let draw_is_core = |offset: u32| draw(net, &path(offset)).is_none();
        let stripe = 0.4 * TILE + ZEBRA_LENGTH / 2.0;
        // 南の歩道を東へ歩く人は、直進するために南の腕を渡る。渡る前は、その縞の手前の歩道の角に立つ。
        // 交差点の真ん中ではない。
        let wait = at(len);
        assert!((wait.x - (NODE.0 - 2_880.0)).abs() < 1.0, "{wait:?}");
        assert!((wait.y - (NODE.1 + stripe)).abs() < 1.0, "{wait:?}");
        // 渡り始めると縞の上を進み、真ん中では車道の真ん中にいる。
        let width = crossing::width(net, arms[0]);
        let half = at(len + width / 2);
        assert!((half.y - (NODE.1 + stripe)).abs() < 1.0, "{half:?}");
        assert!((half.x - NODE.0).abs() < 1_000.0, "{half:?}");
        // 窓の始まりは中核の位置のまま、そこから角へ連続してつながる。
        let window = len - WALK_WINDOW_MM.min(len / 2);
        assert!(draw_is_core(window));
        let begin = at(window + 1);
        assert!(
            (begin.x - (NODE.0 - f64::from(len - window - 1))).abs() < 5.0,
            "{begin:?}"
        );
        let mut prev = begin;
        for o in (window + 1..=len).step_by(250) {
            let d = at(o);
            assert!(dist((d.x, d.y), (prev.x, prev.y)) < 1_000.0);
            prev = d;
        }
    }
}
