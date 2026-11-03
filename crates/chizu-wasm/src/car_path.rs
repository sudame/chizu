//! 車を、ノードの前後で曲線につないで描く。
//!
//! 中核は、車の位置をリンクの上の距離で持ち、リンクはノードの中心からノードの中心までである。
//! 交差点の手前で待つ車はノードの中心に止まり、ノードを越えると次のリンクの向きへ一度に変わる。
//! そのまま描くと、待つ車が交差点の真ん中に重なり、曲がる車が直角に向きを変える。
//!
//! 画面では、リンクの上の距離を次のように描く位置に写す。状態は変えない。
//!
//! - リンクの終わりの交差点では、ノードの中心（距離がリンクの長さ）を、車の前の端が停止線に
//!   かかる所に写す。止まった列は、そのぶん後ろへずれる。
//! - リンクの始まりの交差点では、ノードを越えてから `TURN_MM` 進むまでを、入ってきた車線の
//!   停止線の手前から、出ていく車線の交差点の出口までの二次ベジエ曲線に写す。向きは曲線の接線にする。
//! - そのあいだは、リンクの上の距離を一様に縮めて写す。ひとつ手前の交差点にはみ出さない。
//!
//! 2 本だけがつながるノードで向きが変わる角も、道路の端までを曲線でつなぐ。

use chizu_core::movement::{CAR_LENGTH, MIN_GAP};
use chizu_core::network::TILE_MM;
use chizu_core::{CarPath, Dir, Heading, LinkId, Mm, Network, NodeId};

/// ノードを越えてから曲線の上に描く距離（mm）。中核が交差点の中にいるとみなす距離（`JUNCTION_CLEAR`、
/// 車の長さと車間）と同じにする。
const TURN_MM: f64 = (CAR_LENGTH + MIN_GAP) as f64;

// 画面の停止線の位置。web/src/map/roads.ts の ZEBRA_LENGTH と、detail.ts の STOP_LINE_GAP、
// STOP_LINE_WIDTH（マス）と同じにする。
const ZEBRA_LENGTH: f64 = 0.16 * TILE_MM as f64;
const STOP_LINE_GAP: f64 = 0.05 * TILE_MM as f64;
const STOP_LINE_WIDTH: f64 = 0.04 * TILE_MM as f64;
/// 描く車の長さの半分。web/src/map/movers.ts の車の半分の大きさ（0.2 マス）と同じにする。
const CAR_HALF: f64 = 0.2 * TILE_MM as f64;

/// 停止線と交差点の出口に写す距離の和の、リンクの長さに対する上限。残りを、ノードのあいだの
/// まっすぐな所に充てる。
const ENDS_SHARE: f64 = 0.75;

/// 描く位置（mm）と、進む向きの角度（x から y へ回る向き、ラジアン）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drawn {
    pub x: f64,
    pub y: f64,
    pub angle: f64,
}

type V = (f64, f64);

fn add(a: V, b: V) -> V {
    (a.0 + b.0, a.1 + b.1)
}

fn sub(a: V, b: V) -> V {
    (a.0 - b.0, a.1 - b.1)
}

fn scale(a: V, k: f64) -> V {
    (a.0 * k, a.1 * k)
}

fn cross(a: V, b: V) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

fn mm((x, y): (Mm, Mm)) -> V {
    (x.0 as f64, y.0 as f64)
}

/// リンクの、始まりと終わりのノード、1 歩の向き（長さ 1 にしたもの）、腕の向き。
struct LinkGeom {
    from: NodeId,
    to: NodeId,
    dir: V,
    step: (i64, i64),
}

fn geom(net: &Network, link: LinkId) -> Option<LinkGeom> {
    let seg = net.segment(link.segment)?;
    let line = net.segment_line(link.segment)?;
    let (from, to, step) = match link.dir {
        Dir::Forward => (seg.a, seg.b, line.step),
        Dir::Backward => (seg.b, seg.a, (-line.step.0, -line.step.1)),
    };
    let len = ((step.0 * step.0 + step.1 * step.1) as f64).sqrt();
    Some(LinkGeom {
        from,
        to,
        dir: (step.0 as f64 / len, step.1 as f64 / len),
        step,
    })
}

fn center(net: &Network, n: NodeId) -> Option<V> {
    let p = net.node(n)?.pos;
    Some((p.x.0 as f64, p.y.0 as f64))
}

fn heading_of(step: (i64, i64)) -> Option<Heading> {
    Heading::ALL.into_iter().find(|h| h.step() == step)
}

/// ノード `n` につながる区間の、道路の端（歩道を含む）までのいちばん広い半分の幅（mm）。
/// web/src/map/roads.ts の RoadNode の half と同じ。
fn node_half(net: &Network, n: NodeId) -> f64 {
    let Some(node) = net.node(n) else {
        return 0.0;
    };
    node.segments
        .iter()
        .filter_map(|s| net.segment(*s))
        .map(|s| {
            let k = s.kind;
            i64::from(k.car_lanes_per_side()) * k.lane_width_mm() + k.sidewalk_width_mm()
        })
        .max()
        .unwrap_or(0) as f64
}

/// ノード `n` が、2 本の区間を向きを変えてつなぐ角か。
fn is_bend(net: &Network, n: NodeId) -> bool {
    let Some(node) = net.node(n) else {
        return false;
    };
    let [s, t] = node.segments.as_slice() else {
        return false;
    };
    let arm = |s: chizu_core::SegmentId| {
        let seg = net.segment(s)?;
        let line = net.segment_line(s)?;
        Some(if seg.a == n {
            line.step
        } else {
            (-line.step.0, -line.step.1)
        })
    };
    match (arm(*s), arm(*t)) {
        (Some(a), Some(b)) => a != (-b.0, -b.1),
        _ => false,
    }
}

fn is_junction(net: &Network, n: NodeId) -> bool {
    net.node(n).is_some_and(|x| x.segments.len() >= 3)
}

/// ノード `n` から腕の向き `arm`（ノードから出ていく 1 歩）へ、道路の端までの距離（mm）。
fn edge(net: &Network, n: NodeId, arm: (i64, i64)) -> f64 {
    let half = node_half(net, n);
    if arm.0 != 0 && arm.1 != 0 {
        half * core::f64::consts::SQRT_2
    } else {
        half
    }
}

/// リンク `link` の、始まりのノードから出口までの距離と、終わりのノードの手前で待つ車の中心までの距離（mm）。
/// 和がリンクの長さの `ENDS_SHARE` を超えれば、同じ割合で縮める。
fn ends(net: &Network, link: LinkId, g: &LinkGeom, len: f64) -> (f64, f64) {
    let exit = if is_junction(net, g.from) || is_bend(net, g.from) {
        edge(net, g.from, g.step)
    } else {
        0.0
    };
    let back = (-g.step.0, -g.step.1);
    let stop = if is_junction(net, g.to) {
        let kind = net.segment(link.segment).map(|s| s.kind);
        let walks = kind.is_some_and(|k| k.has_sidewalks());
        let zebra = walks
            && heading_of(back)
                .and_then(|h| net.node(g.to).map(|n| !n.crosswalk_removed(h)))
                .unwrap_or(false);
        edge(net, g.to, back)
            + if zebra { ZEBRA_LENGTH } else { 0.0 }
            + STOP_LINE_GAP
            + STOP_LINE_WIDTH
            + CAR_HALF
    } else if is_bend(net, g.to) {
        edge(net, g.to, back)
    } else {
        0.0
    };
    let room = len * ENDS_SHARE;
    let k = if exit + stop > room {
        room / (exit + stop)
    } else {
        1.0
    };
    (exit * k, stop * k)
}

/// 二次ベジエ曲線の、`t` の位置と接線。
fn bezier(p0: V, p1: V, p2: V, t: f64) -> (V, V) {
    let u = 1.0 - t;
    let p = add(
        add(scale(p0, u * u), scale(p1, 2.0 * u * t)),
        scale(p2, t * t),
    );
    let d = add(scale(sub(p1, p0), 2.0 * u), scale(sub(p2, p1), 2.0 * t));
    (p, d)
}

/// 点 `p0` から向き `d0` へ進む直線と、点 `p2` へ向き `d2` で入る直線の交わる所を、曲線の制御点にする。
/// 平行か、交わる所が `p0` の後ろか `p2` の先なら、2 点の真ん中にする。
pub fn control(p0: V, d0: V, p2: V, d2: V) -> V {
    let mid = scale(add(p0, p2), 0.5);
    let den = cross(d0, d2);
    if den.abs() < 1e-9 {
        return mid;
    }
    let w = sub(p2, p0);
    let a = cross(w, d2) / den;
    // p0 + a·d0 = p2 + b·d2。交わる所は p0 の先（a > 0）で、p2 の手前（b < 0）にある。
    let b = cross(w, d0) / den;
    if a <= 0.0 || b >= 0.0 {
        return mid;
    }
    add(p0, scale(d0, a))
}

/// 車を描く位置と向き。`shift` は中核が求めた、車線に寄せるずれである。網から道筋を求められなければ `None`。
pub fn draw(net: &Network, path: &CarPath, shift: (Mm, Mm)) -> Option<Drawn> {
    let g = geom(net, path.link)?;
    let len = f64::from(path.len.0);
    let o = f64::from(path.offset.0).min(len);
    if len <= 0.0 {
        return None;
    }
    let (exit, stop) = ends(net, path.link, &g, len);
    let start = center(net, g.from)?;
    // ノードを越えてから曲線の上に描く距離。出口がなければ曲線にしない。
    let turn = if exit > 0.0 { TURN_MM.min(len) } else { 0.0 };
    if o < turn
        && let Some((prev, shown)) = path.prev
        && let Some(pg) = geom(net, prev)
        && pg.to == g.from
    {
        let plen = net
            .segment_line(prev.segment)
            .and_then(|l| l.checked_length())
            .map_or(0.0, |m| m.0 as f64);
        let (_, pstop) = ends(net, prev, &pg, plen);
        let p0 = add(sub(start, scale(pg.dir, pstop)), mm(shown));
        let p2 = add(add(start, scale(g.dir, exit)), mm(path.lane_shift));
        let p1 = control(p0, pg.dir, p2, g.dir);
        let (p, d) = bezier(p0, p1, p2, o / turn);
        let d = if d.0.abs() + d.1.abs() < 1e-9 {
            g.dir
        } else {
            d
        };
        return Some(Drawn {
            x: p.0,
            y: p.1,
            angle: d.1.atan2(d.0),
        });
    }
    let along = if o < turn {
        exit * o / turn
    } else {
        exit + (o - turn) * (len - stop - exit) / (len - turn).max(1.0)
    };
    let p = add(add(start, scale(g.dir, along)), mm(shift));
    Some(Drawn {
        x: p.0,
        y: p.1,
        angle: g.dir.1.atan2(g.dir.0),
    })
}

/// 角度（ラジアン）を、x 軸から y 軸へ回る 1 周を 65536 とした値に直す。
pub fn angle_code(angle: f64) -> u16 {
    let turns = angle / core::f64::consts::TAU;
    let code = (turns * 65536.0).round().rem_euclid(65536.0);
    code as u16
}

#[cfg(test)]
mod tests {
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

    /// (0, 10)〜(20, 10) の東西の道路と、(10, 0)〜(10, 20) の南北の道路が (10, 10) で交わる街。
    fn crossroads() -> World {
        let mut w = World::new(1);
        for (seq, (from, to)) in [((0, 10), (20, 10)), ((10, 0), (10, 20))]
            .into_iter()
            .enumerate()
        {
            let c = Command::BuildRoad {
                from: TilePos::new(from.0, from.1),
                to: TilePos::new(to.0, to.1),
                kind: RoadKind::Medium,
            };
            let applied = w.apply_recorded(seq as u64, &c);
            assert_eq!(applied, chizu_core::ApplyOutcome::Applied);
        }
        w
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

    const ZERO: (Mm, Mm) = (Mm(0), Mm(0));

    #[test]
    fn a_car_waiting_at_the_junction_is_drawn_behind_the_stop_line() {
        let w = crossroads();
        let net = w.network();
        let east = link_toward(net, (0, 10), (10, 10));
        let d = draw(net, &path(east, 80_000, None), ZERO).unwrap();
        // 中くらいの道路の端は 0.4 マス、横断歩道 0.16、すき間 0.05、停止線 0.04、車の半分 0.2 マス。
        let node_x = 10.0 * 8_000.0 + 4_000.0;
        let want = node_x - (0.4 + 0.16 + 0.05 + 0.04 + 0.2) * 8_000.0;
        assert!(close(d.x, want), "{} != {want}", d.x);
        assert!(close(d.angle, 0.0));
        // リンクの始まりから少し離れた所は、ほぼ中核の位置のまま描く。
        let early = draw(net, &path(east, 8_000, None), ZERO).unwrap();
        assert!(
            (early.x - (4_000.0 + 8_000.0)).abs() < 2_000.0,
            "{}",
            early.x
        );
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
                draw(net, &path(east, o, None), ZERO).unwrap().x
            })
            .collect();
        for p in xs.windows(2) {
            // 並びは変わらず、描いた車の長さ（0.4 マス）より離れる。
            assert!(p[0] - p[1] > 2.0 * CAR_HALF, "{xs:?}");
            assert!(p[0] - p[1] <= spacing + 1e-6, "{xs:?}");
        }
        // 列の後ろは、ひとつ手前の交差点にはみ出さない。
        let first = draw(net, &path(east, 0, None), ZERO).unwrap();
        assert!(close(first.x, 4_000.0));
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
        let wait = draw(net, &path(east, 80_000, None), shown).unwrap();
        for o in (0..=7_000).step_by(500) {
            let d = draw(net, &path(north, o, Some((east, shown))), ZERO).unwrap();
            // 向きは東から北へ、少しずつ回る。
            assert!(d.angle <= last + 1e-9, "{o}: {} > {last}", d.angle);
            assert!(last - d.angle < 0.5, "{o}: 向きが飛んだ");
            last = d.angle;
            if let Some(p) = prev_pos {
                assert!((d.x - p.0).hypot(d.y - p.1) < 2_000.0, "{o}: 位置が飛んだ");
            } else {
                // 曲がり始めは、停止線の手前で待っていた所と同じ。
                assert!(close(d.x, wait.x) && close(d.y, wait.y), "{d:?} {wait:?}");
            }
            prev_pos = Some((d.x, d.y));
        }
        assert!(close(last, -core::f64::consts::FRAC_PI_2));
        // 曲がり終えた所と、その先のまっすぐな所はつながる。
        let after = draw(net, &path(north, 7_001, Some((east, shown))), ZERO).unwrap();
        let p = prev_pos.unwrap();
        assert!((after.x - p.0).hypot(after.y - p.1) < 10.0);
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
            let d = draw(net, &p, lane).unwrap();
            assert!(close(d.y, 10.0 * 8_000.0 + 4_000.0 - 1_920.0), "{o}: {d:?}");
            assert!(close(d.angle, 0.0));
        }
    }
}
