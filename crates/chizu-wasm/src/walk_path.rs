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
use chizu_core::{Network, NodeId, SegmentId};
use smallvec::SmallVec;

use crate::car_path::{
    Drawn, V, ZEBRA_LENGTH, arc_length, bezier, center, control, dist, geom, is_bend, is_junction,
    mm, node_half,
};

/// 窓の中で、角の位置を窓の長さの何割までに抑えるか。窓の中の人の進みが、中核の半分より遅くならず、
/// 後ろへも戻らないようにする。
const MAX_STOP: f64 = 0.5;

/// 描く道筋の 1 区切り。
#[derive(Clone, Copy, Debug)]
enum Piece {
    Line(V, V),
    Quad(V, V, V),
}

type Pieces = SmallVec<[Piece; 8]>;

impl Piece {
    fn length(&self) -> f64 {
        match *self {
            Piece::Line(a, b) => dist(a, b),
            Piece::Quad(a, c, b) => arc_length(a, c, b),
        }
    }

    /// 長さ `s` の所の位置と向き。`length` はこの区切りの長さである。
    fn at(&self, s: f64, length: f64) -> (V, V) {
        let t = if length > 0.0 { s / length } else { 1.0 };
        match *self {
            Piece::Line(a, b) => (
                (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t),
                (b.0 - a.0, b.1 - a.1),
            ),
            Piece::Quad(a, c, b) => bezier(a, c, b, t.clamp(0.0, 1.0)),
        }
    }
}

/// 区切りを順に並べた道筋の、長さの割合 `f` の所。
fn along_pieces(pieces: &[Piece], f: f64) -> Option<Drawn> {
    let lens: SmallVec<[f64; 8]> = pieces.iter().map(Piece::length).collect();
    let mut left = f.clamp(0.0, 1.0) * lens.iter().sum::<f64>();
    let mut found = None;
    for (k, (p, &l)) in pieces.iter().zip(&lens).enumerate() {
        found = Some(p.at(left.min(l), l));
        if left <= l || k + 1 == pieces.len() {
            break;
        }
        left -= l;
    }
    let (pos, d) = found?;
    let d = if d.0.abs() + d.1.abs() < 1e-9 {
        (1.0, 0.0)
    } else {
        d
    };
    Some(Drawn {
        x: pos.0,
        y: pos.1,
        angle: d.1.atan2(d.0),
    })
}

/// 角の位置と、腕の向きの単位ベクトル、腕の区間。
#[derive(Clone, Copy, Debug)]
struct Point {
    p: V,
    u: V,
    seg: SegmentId,
}

type Points = SmallVec<[Point; 8]>;

/// ノードの中心と、道路の端までのいちばん広い半分の幅（mm）。1 回の描画で 1 度だけ求める。
struct Node {
    id: NodeId,
    c: V,
    half: f64,
    junction: bool,
}

impl Node {
    fn of(net: &Network, id: NodeId) -> Option<Node> {
        Some(Node {
            id,
            c: center(net, id)?,
            half: node_half(net, id),
            junction: is_junction(net, id),
        })
    }

    /// 腕の 1 歩 `step` の向きへ、角の位置までの距離（mm）。交差点なら横断歩道の縞の真ん中、
    /// 角（2 本だけがつながるノード）なら道路の端である。
    fn along(&self, step: (i64, i64)) -> f64 {
        let edge = if step.0 != 0 && step.1 != 0 {
            self.half * core::f64::consts::SQRT_2
        } else {
            self.half
        };
        edge + if self.junction {
            ZEBRA_LENGTH / 2.0
        } else {
            0.0
        }
    }
}

/// ノードから区間 `seg` の腕の向きへ出る 1 歩。
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

/// 歩く人が窓の中で写し替える長さ。リンクの長さ `len` の半分と、歩き終わる距離 `end` を超えない。
fn window(len: f64, end: f64) -> f64 {
    f64::from(WALK_WINDOW_MM).min(len / 2.0).min(end).max(0.0)
}

/// 窓の長さ `w` の中に収まる、角までの距離。
fn capped(along: f64, w: f64) -> f64 {
    along.min(MAX_STOP * w)
}

/// 腕 `seg` の角の位置（歩道の中心線の上）。`cw` は、腕の時計回りの側かどうか（出ていく向きの右）。
/// `cap` があれば、ノードからの距離をそれ以下に抑える。
fn corner(net: &Network, node: &Node, seg: SegmentId, cw: bool, cap: Option<f64>) -> Option<Point> {
    let step = arm_step(net, node.id, seg)?;
    let u = unit(step);
    let kind = net.segment(seg)?.kind;
    let d = (i64::from(kind.car_lanes_per_side()) * kind.lane_width_mm()
        + kind.sidewalk_width_mm() / 2) as f64;
    // 腕の向きの左は画面で (u.1, -u.0) の向きである。
    let side = if cw { -d } else { d };
    let along = node.along(step);
    let along = cap.map_or(along, |c| along.min(c));
    Some(Point {
        p: (
            node.c.0 + u.0 * along + u.1 * side,
            node.c.1 + u.1 * along - u.0 * side,
        ),
        u,
        seg,
    })
}

/// 窓を使うノードか。3 本以上がつながる交差点か、向きを変える角である。
fn windowed(net: &Network, node: NodeId) -> bool {
    is_junction(net, node) || is_bend(net, node)
}

/// 角 `a` から角 `b` への区切り。同じ腕の角どうしならまっすぐ、違う腕なら歩道の中心線が交わる所を
/// 通る曲線で結ぶ。
fn step_piece(a: &Point, b: &Point) -> Piece {
    if a.seg == b.seg {
        Piece::Line(a.p, b.p)
    } else {
        let inward = (-a.u.0, -a.u.1);
        Piece::Quad(a.p, control(a.p, inward, b.p, b.u), b.p)
    }
}

/// 通る道筋の区切りのうち `want` のもの。`None` なら、リンク上の点 `from` から最初の横断歩道の手前の
/// 角（渡らなければ出ていく腕の角）まで、`Some(k)` なら、k 番めに渡る横断歩道と、渡り終えた所から次の
/// 横断歩道の手前の角（最後なら出ていく腕の角）までである。
fn stage(
    points: &[Point],
    arms: &[SegmentId],
    junction: bool,
    from: V,
    want: Option<usize>,
) -> Pieces {
    let mut out = Pieces::new();
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return out;
    };
    if want.is_none() {
        out.push(Piece::Line(from, first.p));
    }
    if !junction {
        // 2 本だけがつながる角では、車道を渡らない。入る角から出る角へ、1 本の曲線で結ぶ。
        if want.is_none() {
            out.push(step_piece(first, last));
        }
        return out;
    }
    let mut next = 0;
    let mut now = None;
    for w in points.windows(2) {
        if w[0].seg == w[1].seg && arms.get(next) == Some(&w[0].seg) {
            now = Some(next);
            next += 1;
        }
        if now == want {
            out.push(step_piece(&w[0], &w[1]));
        }
    }
    out
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
    // 歩き出す出入口と、歩き終わる出入口は、中核の位置と描く位置が合うように、窓の外に置く。
    let begin = f64::from(wp.from.0).min(len);
    let end = wp.to.map_or(len, |t| f64::from(t.0).min(len));
    let from_node = wp.entered && windowed(net, g.from);
    let ws = if from_node { window(len, end) } else { 0.0 };
    if from_node && ws >= 1.0 && o < ws {
        let node = Node::of(net, g.from)?;
        let stop = capped(node.along(arm_step(net, g.from, wp.link.segment)?), ws);
        return Some(on_link(stop + o * (ws - stop) / ws));
    }
    let ahead = wp
        .ahead
        .as_ref()
        .filter(|a| a.node == g.to && windowed(net, g.to));
    let we = ahead.map_or(0.0, |_| {
        f64::from(WALK_WINDOW_MM)
            .min(len - ws)
            .min(len - begin)
            .max(0.0)
    });
    let ahead = ahead.filter(|_| we >= 1.0);
    let crossing = o > len;
    let Some(ahead) = ahead else {
        if crossing {
            // 道筋が求められないときも、交差点の中心には描かない。
            let node = Node::of(net, g.to)?;
            let step = arm_step(net, g.to, wp.link.segment)?;
            return Some(on_link(len - node.along((-step.0, -step.1)).min(len / 2.0)));
        }
        return None;
    };
    if !crossing && o <= len - we {
        return None;
    }
    let node = Node::of(net, g.to)?;
    let from = on_link(len - we);
    match pass(net, ahead, &node, we) {
        Some(points) => {
            let from = (from.x, from.y);
            if !crossing {
                let pieces = stage(&points, &ahead.arms, node.junction, from, None);
                return along_pieces(&pieces, (o - (len - we)) / we);
            }
            // 渡っているあいだ。渡る腕の横断歩道を順に並べた道のりの上の位置から、どの横断歩道のどこかを求める。
            let mut x = o - len;
            for (k, &arm) in ahead.arms.iter().enumerate() {
                let w = f64::from(crossing::width(net, arm));
                if x < w || k + 1 == ahead.arms.len() {
                    let pieces = stage(&points, &ahead.arms, node.junction, from, Some(k));
                    let f = if w > 0.0 { (x / w).min(1.0) } else { 1.0 };
                    return along_pieces(&pieces, f);
                }
                x -= w;
            }
            None
        }
        None => {
            let along = capped(node.along(arm_step(net, g.to, wp.link.segment)?), we);
            crossing.then(|| on_link(len - along))
        }
    }
}

/// 通る角の位置を、通る順に並べる。入る角は窓の長さ `we` に収まる所に、出る角は次のリンクの窓に収まる所に置く。
fn pass(net: &Network, ahead: &WalkAhead, node: &Node, we: f64) -> Option<Points> {
    let last = ahead.corners.len().checked_sub(1)?;
    let wn = window(f64::from(ahead.next_len.0), f64::from(ahead.next_end.0));
    ahead
        .corners
        .iter()
        .enumerate()
        .map(|(k, &(seg, cw))| {
            let cap = match k {
                0 => Some(MAX_STOP * we),
                k if k == last => Some(MAX_STOP * wn),
                _ => None,
            };
            corner(net, node, seg, cw, cap)
        })
        .collect()
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
                from: LaneOffset(0),
                to: None,
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
        let corners = crossing::corner_route(net, node, (from, false), (to, far), None);
        assert!(!arms.is_empty());
        let len = 80_000;
        let path = |offset: u32| WalkPath {
            link: from,
            offset: LaneOffset(offset),
            len: LaneOffset(len),
            lane_shift: (Mm(0), Mm(2_880)),
            entered: false,
            from: LaneOffset(0),
            to: None,
            ahead: Some(chizu_core::WalkAhead {
                node,
                corners: corners.clone(),
                arms: arms.clone(),
                next_len: LaneOffset(80_000),
                next_end: LaneOffset(80_000),
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

    // ここから、網の変化、出入口、横断歩道の取り外し、1 マスのリンクの確かめ。

    use chizu_core::{Heading, Money, SidewalkPos};

    fn big_world() -> World {
        World::new_with_money(1, Money(1_000_000_000))
    }

    fn road(w: &mut World, a: (i64, i64), b: (i64, i64)) {
        let c = Command::BuildRoad {
            from: TilePos::new(a.0, a.1),
            to: TilePos::new(b.0, b.1),
            kind: RoadKind::Medium,
        };
        let out = w.apply(&c);
        assert_eq!(out, chizu_core::ApplyOutcome::Applied, "{a:?}-{b:?}");
    }

    fn link_of(w: &World, from: (i64, i64), to: (i64, i64)) -> LinkId {
        let net = w.network();
        let (from, to) = (TilePos::new(from.0, from.1), TilePos::new(to.0, to.1));
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
    }

    fn link_len(w: &World, l: LinkId) -> u32 {
        w.network().segment_line(l.segment).unwrap().length().0 as u32
    }

    fn spawn(w: &mut World, a: (LinkId, u32), b: (LinkId, u32)) {
        let at = |(l, o): (LinkId, u32)| {
            SidewalkPos::on_link(w.network(), l, LaneOffset(o)).expect("歩道がない")
        };
        let (pa, pb) = (at(a), at(b));
        w.spawn_walk(pa, pb).expect("歩く人を出せない");
    }

    fn rect30() -> TileRect {
        TileRect {
            min: TilePos::new(0, 0),
            max: TilePos::new(30, 30),
        }
    }

    /// 描く位置。道筋がなければ中核の位置と同じ。
    fn drawn_or_core(w: &World, m: &chizu_core::Mover) -> V {
        let core = (
            m.pos.x.0 as f64 + m.shift.0.0 as f64,
            m.pos.y.0 as f64 + m.shift.1.0 as f64,
        );
        m.walk
            .as_ref()
            .and_then(|p| draw(w.network(), p))
            .map_or(core, |d| (d.x, d.y))
    }

    /// 交差点の手前の窓に入った人がいる街を作る。
    fn town_with_walker_near_end() -> World {
        let mut w = big_world();
        road(&mut w, (0, 10), (20, 10));
        road(&mut w, (10, 0), (10, 20));
        let west = link_of(&w, (0, 10), (10, 10));
        let east = link_of(&w, (10, 10), (20, 10));
        spawn(&mut w, (west, 16_000), (east, 40_000));
        for _ in 0..500 {
            w.step();
            let near = w
                .movers_in(rect30())
                .iter()
                .any(|m| m.walk.as_ref().is_some_and(|p| p.ahead.is_some()));
            if near {
                return w;
            }
        }
        panic!("人が交差点の手前に着かない");
    }

    #[test]
    fn changing_the_network_before_the_next_step_does_not_panic() {
        // 網を変えた直後は、次の step で経路を直すまで、経路のリンクが網にない。壊す場合と、T 字につないで
        // 区間を分ける場合の両方で、描く準備と描く位置の計算が落ちない。
        for tee in [false, true] {
            let mut w = town_with_walker_near_end();
            if tee {
                road(&mut w, (5, 5), (5, 15));
            } else {
                let area = TileRect {
                    min: TilePos::new(3, 10),
                    max: TilePos::new(3, 10),
                };
                let out = w.apply(&Command::Bulldoze { area });
                assert_eq!(out, chizu_core::ApplyOutcome::Applied);
            }
            for m in w.movers_in(rect30()) {
                let _ = drawn_or_core(&w, &m);
            }
        }
    }

    #[test]
    fn a_crosswalk_removed_while_crossing_keeps_the_walker_off_the_middle() {
        let mut w = town_with_walker_near_end();
        let node = {
            let west = link_of(&w, (0, 10), (10, 10));
            w.network().segment(west.segment).unwrap().b
        };
        let mut toggled = false;
        let mut last: Option<V> = None;
        let mut seen = 0;
        for _ in 0..400 {
            w.step();
            for m in w.movers_in(rect30()) {
                let Some(p) = m.walk.as_ref() else { continue };
                let d = drawn_or_core(&w, &m);
                assert!(dist(d, NODE) > 2_500.0, "交差点の中心に描かれた {d:?}");
                if let Some(l) = last {
                    assert!(dist(d, l) < 3_000.0, "飛んだ {l:?} -> {d:?}");
                }
                last = Some(d);
                seen += 1;
                if !toggled && p.offset > p.len {
                    toggled = true;
                    let off = Command::SetCrosswalk {
                        node,
                        arm: Heading::S,
                        on: false,
                    };
                    w.apply(&off);
                }
            }
        }
        assert!(toggled && seen > 0);
    }

    /// 中核の位置（歩道のずれつき）の、リンク `link` の始まりから `o` の所。
    fn core_at(w: &World, link: LinkId, o: u32, lane: (Mm, Mm)) -> V {
        let net = w.network();
        let g = geom(net, link).unwrap();
        let s = center(net, g.from).unwrap();
        let o = f64::from(o);
        (
            s.0 + g.dir.0 * o + lane.0.0 as f64,
            s.1 + g.dir.1 * o + lane.1.0 as f64,
        )
    }

    /// (10, 10) で、東西、南北、斜めの道路が交わる街。
    fn star() -> World {
        let mut w = big_world();
        road(&mut w, (0, 10), (20, 10));
        road(&mut w, (10, 0), (10, 20));
        road(&mut w, (0, 0), (20, 20));
        w
    }

    fn ahead_of(w: &World, from: LinkId, to: LinkId) -> chizu_core::WalkAhead {
        use chizu_core::movement::crossing;
        let net = w.network();
        let seg = net.segment(from.segment).unwrap();
        let node = match from.dir {
            Dir::Forward => seg.b,
            Dir::Backward => seg.a,
        };
        let (far, arms) = crossing::depart(net, node, (from, false), to, None, None);
        let len = link_len(w, to);
        chizu_core::WalkAhead {
            node,
            corners: crossing::corner_route(net, node, (from, false), (to, far), None),
            arms,
            next_len: LaneOffset(len),
            next_end: LaneOffset(len),
        }
    }

    #[test]
    fn a_door_inside_the_window_keeps_the_drawn_position_on_the_core_position() {
        let w = star();
        let lane = (Mm(0), Mm(2_880));
        let diag_lane = (Mm(-2_036), Mm(2_036));
        // 着く人。交差点を抜けた先の出入口が、始まりの窓の中にある。直交は 8,000、斜めは 5,657。
        for (to, door, lane) in [
            (link_of(&w, (10, 10), (20, 10)), 8_000_u32, lane),
            (link_of(&w, (10, 10), (20, 20)), 5_657, diag_lane),
        ] {
            let len = link_len(&w, to);
            for o in [0, door / 2, door - 1, door] {
                let wp = WalkPath {
                    link: to,
                    offset: LaneOffset(o),
                    len: LaneOffset(len),
                    lane_shift: lane,
                    entered: true,
                    from: LaneOffset(0),
                    to: Some(LaneOffset(door)),
                    ahead: None,
                };
                let want = core_at(&w, to, o, lane);
                let got = draw(w.network(), &wp).map_or(want, |d| (d.x, d.y));
                if o + 1 >= door {
                    assert!(dist(got, want) < 2.0, "{o}: {got:?} {want:?}");
                }
            }
        }
        // 出る人。出入口が、終わりの窓の中にある。
        for (from, door_from_end, lane) in [
            (link_of(&w, (0, 10), (10, 10)), 8_000_u32, lane),
            (link_of(&w, (0, 0), (10, 10)), 5_657, diag_lane),
        ] {
            let len = link_len(&w, from);
            let to = link_of(&w, (10, 10), (20, 10));
            let ahead = ahead_of(&w, from, to);
            let door = len - door_from_end;
            for o in [door, door + 1, door + door_from_end / 2] {
                let wp = WalkPath {
                    link: from,
                    offset: LaneOffset(o),
                    len: LaneOffset(len),
                    lane_shift: lane,
                    entered: false,
                    from: LaneOffset(door),
                    to: None,
                    ahead: Some(ahead.clone()),
                };
                let want = core_at(&w, from, o, lane);
                let got = draw(w.network(), &wp).map_or(want, |d| (d.x, d.y));
                if o <= door + 1 {
                    assert!(dist(got, want) < 2.0, "{o}: {got:?} {want:?}");
                }
            }
        }
    }

    /// 1 マスのリンク `short` の上で、描く位置が進む向きに戻らず、中核の半分近い速さで進むこと。
    fn check_one_tile(w: &mut World, before: LinkId, short: LinkId, after: (LinkId, u32)) {
        let start = link_len(w, before) - 16_000;
        spawn(w, (before, start), after);
        let g = geom(w.network(), short).unwrap();
        let along = |a: V, b: V| (a.0 - b.0) * g.dir.0 + (a.1 - b.1) * g.dir.1;
        let mut prev: Option<(V, V)> = None;
        let mut frames = 0;
        for _ in 0..2_000 {
            w.step();
            for m in w.movers_in(rect30()) {
                let Some(p) = m.walk.as_ref().filter(|p| p.link == short) else {
                    prev = None;
                    continue;
                };
                if p.offset > p.len {
                    prev = None;
                    continue;
                }
                let core = (
                    m.pos.x.0 as f64 + m.shift.0.0 as f64,
                    m.pos.y.0 as f64 + m.shift.1.0 as f64,
                );
                let d = drawn_or_core(w, &m);
                if let Some((pd, pc)) = prev {
                    let (moved, core_moved) = (along(d, pd), along(core, pc));
                    let at = p.offset.0;
                    assert!(moved > -1.0, "戻った: {moved} ({at})");
                    assert!(
                        moved >= 0.4 * core_moved - 1.0,
                        "遅い: {moved} / {core_moved} ({at})"
                    );
                    frames += 1;
                }
                prev = Some((d, core));
            }
        }
        assert!(frames > 3);
    }

    #[test]
    fn on_a_one_tile_link_the_walker_neither_steps_back_nor_crawls() {
        let mut w = big_world();
        road(&mut w, (0, 10), (30, 10));
        road(&mut w, (10, 0), (10, 20));
        road(&mut w, (11, 0), (11, 20));
        let before = link_of(&w, (0, 10), (10, 10));
        let short = link_of(&w, (10, 10), (11, 10));
        let after = (link_of(&w, (11, 10), (30, 10)), 40_000);
        check_one_tile(&mut w, before, short, after);

        let mut w = big_world();
        road(&mut w, (0, 0), (30, 30));
        road(&mut w, (10, 0), (10, 20));
        road(&mut w, (11, 0), (11, 20));
        let before = link_of(&w, (0, 0), (10, 10));
        let short = link_of(&w, (10, 10), (11, 11));
        let after = (link_of(&w, (11, 11), (30, 30)), 40_000);
        check_one_tile(&mut w, before, short, after);
    }
}
