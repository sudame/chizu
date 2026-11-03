//! 車を、ノードの前後で曲線につないで描く。
//!
//! 中核は、車の位置をリンクの上の距離で持ち、リンクはノードの中心からノードの中心までである。
//! 交差点の手前で待つ車はノードの中心に止まり、ノードを越えると次のリンクの向きへ一度に変わる。
//! そのまま描くと、待つ車が交差点の真ん中に重なり、曲がる車が直角に向きを変える。
//!
//! 画面では、リンクの両端の短い窓の中だけ、リンクの上の距離を描く位置に写し替える。窓の外は
//! 中核の位置のまま描くので、区間の途中を横切る人や、乗り降りする建物の前と位置がそろう。
//! 状態は変えない。
//!
//! - 終わりの窓（`QUEUE_MM`）：ノードの中心（距離がリンクの長さ）を、車の前の端が停止線に
//!   かかる所に写し、窓の中を一様に縮める。止まった列は、停止線の手前に詰めて並ぶ。
//! - 始まりの窓（`TURN_MM`）：入ってきた車線の停止線の手前から、出ていく車線の交差点の出口までの
//!   二次ベジエ曲線と、その先の窓の終わりまでのまっすぐな線に写す。向きは曲線の接線にする。
//! - 窓が重なる短いリンクでは、停止線の位置を保ったまま、あいだを一様に縮める。
//!
//! 2 本だけがつながるノードで向きが変わる角も、道路の端までを曲線でつなぐ。
//!
//! 経路を探し直すと、中核は抜けてきたリンクを忘れる。曲線の途中の車が飛ばないよう、
//! 描いた曲線の始まりを [`Entry`] として呼ぶ側に覚えさせ、次の回に渡してもらう。

use chizu_core::movement::{CAR_LENGTH, MIN_GAP};
use chizu_core::network::TILE_MM;
use chizu_core::{CarPath, Dir, Heading, LinkId, Mm, Network, NodeId};

/// ノードを越えてから曲線の上に描く距離（mm）。中核が交差点の中にいるとみなす距離（`JUNCTION_CLEAR`、
/// 車の長さと車間）と同じにする。
const TURN_MM: f64 = (CAR_LENGTH + MIN_GAP) as f64;

/// リンクの終わりで、停止線の手前に列を詰める窓の長さ（mm）。車間 3 台ぶんにする。いちばん奥の停止線
/// （大きい道路の斜めの腕で 9.3m）まで寄せても、縮めた車間が描く車の長さより長く残る。
const QUEUE_MM: f64 = 3.0 * (CAR_LENGTH + MIN_GAP) as f64;

/// 窓が重なる短いリンクで、始まりの窓のあとを縮める割合の下限。列の車が重ならないようにする。
const MIN_SCALE: f64 = 0.55;

// 画面の停止線の位置。web/src/map/roads.ts の ZEBRA_LENGTH と、detail.ts の STOP_LINE_GAP、
// STOP_LINE_WIDTH（マス）と同じにする。
pub(crate) const ZEBRA_LENGTH: f64 = 0.16 * TILE_MM as f64;
const STOP_LINE_GAP: f64 = 0.05 * TILE_MM as f64;
const STOP_LINE_WIDTH: f64 = 0.04 * TILE_MM as f64;
/// 描く車の長さの半分。web/src/map/movers.ts の車の半分の大きさ（0.2 マス）と同じにする。
const CAR_HALF: f64 = 0.2 * TILE_MM as f64;

/// 曲線の長さを測るときに分ける数。
const ARC_SAMPLES: usize = 8;

/// 描く位置（mm）と、進む向きの角度（x から y へ回る向き、ラジアン）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drawn {
    pub x: f64,
    pub y: f64,
    pub angle: f64,
}

/// ノードを越えた車の曲線の始まり。`node` と `dir` は、いまのリンクの始まりのノードの中心と向き、
/// `p0` と `d0` は、入ってきた車線の停止線の手前の点と、そこでの向きである。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    node: V,
    dir: V,
    p0: V,
    d0: V,
}

pub(crate) type V = (f64, f64);

pub(crate) fn add(a: V, b: V) -> V {
    (a.0 + b.0, a.1 + b.1)
}

pub(crate) fn sub(a: V, b: V) -> V {
    (a.0 - b.0, a.1 - b.1)
}

pub(crate) fn scale(a: V, k: f64) -> V {
    (a.0 * k, a.1 * k)
}

fn cross(a: V, b: V) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

pub(crate) fn dist(a: V, b: V) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

pub(crate) fn mm((x, y): (Mm, Mm)) -> V {
    (x.0 as f64, y.0 as f64)
}

/// リンクの、始まりと終わりのノード、1 歩の向き（長さ 1 にしたもの）、腕の向き。
pub(crate) struct LinkGeom {
    pub(crate) from: NodeId,
    pub(crate) to: NodeId,
    pub(crate) dir: V,
    pub(crate) step: (i64, i64),
}

pub(crate) fn geom(net: &Network, link: LinkId) -> Option<LinkGeom> {
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

pub(crate) fn center(net: &Network, n: NodeId) -> Option<V> {
    let p = net.node(n)?.pos;
    Some((p.x.0 as f64, p.y.0 as f64))
}

fn heading_of(step: (i64, i64)) -> Option<Heading> {
    Heading::ALL.into_iter().find(|h| h.step() == step)
}

/// ノード `n` につながる区間の、道路の端（歩道を含む）までのいちばん広い半分の幅（mm）。
/// web/src/map/roads.ts の RoadNode の half と同じ。
pub(crate) fn node_half(net: &Network, n: NodeId) -> f64 {
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
pub(crate) fn is_bend(net: &Network, n: NodeId) -> bool {
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

pub(crate) fn is_junction(net: &Network, n: NodeId) -> bool {
    net.node(n).is_some_and(|x| x.segments.len() >= 3)
}

/// ノード `n` から腕の向き `arm`（ノードから出ていく 1 歩）へ、道路の端までの距離（mm）。
pub(crate) fn edge(net: &Network, n: NodeId, arm: (i64, i64)) -> f64 {
    let half = node_half(net, n);
    if arm.0 != 0 && arm.1 != 0 {
        half * core::f64::consts::SQRT_2
    } else {
        half
    }
}

/// リンクの上の距離を描く位置へ写すための、リンク 1 本の寸法（mm）。
#[derive(Clone, Copy, Debug)]
struct Layout {
    len: f64,
    /// 始まりの窓の長さ。始まりのノードで曲がらなければ 0。
    turn: f64,
    /// 始まりの窓の終わりを写す、始まりのノードからの距離。窓が重ならなければ `turn` と同じ。
    settle: f64,
    /// 曲線の終わり（交差点の出口）の、始まりのノードからの距離。
    exit: f64,
    /// 終わりのノードの手前で待つ車の中心までの、終わりのノードからの距離。
    stop: f64,
}

impl Layout {
    fn of(net: &Network, link: LinkId, g: &LinkGeom, len: f64) -> Layout {
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
        let stop = stop.min(len);
        let turn = if exit > 0.0 {
            TURN_MM.min(len / 2.0)
        } else {
            0.0
        };
        // 窓が重ならなければ、始まりの窓の終わりは中核の位置のまま。重なれば、停止線の位置を保ち、
        // あいだを縮める割合が下限を下回らない所まで、始まりの窓の終わりを手前に寄せる。
        let settle = if len - QUEUE_MM > turn {
            turn
        } else {
            (len - stop - MIN_SCALE * (len - turn)).clamp(0.0, turn)
        };
        Layout {
            len,
            turn,
            settle,
            exit: exit.min(settle),
            stop,
        }
    }

    /// 始まりの窓のあとの距離 `o` を描く、始まりのノードからの距離。
    fn along(&self, o: f64) -> f64 {
        let queue = self.len - QUEUE_MM;
        if queue > self.turn {
            if o <= queue {
                o
            } else {
                queue + (o - queue) * (QUEUE_MM - self.stop) / QUEUE_MM
            }
        } else {
            let span = (self.len - self.turn).max(1.0);
            self.settle + (o - self.turn) * (self.len - self.stop - self.settle) / span
        }
    }
}

fn link_len(net: &Network, link: LinkId) -> f64 {
    net.segment_line(link.segment)
        .and_then(|l| l.checked_length())
        .map_or(0.0, |m| m.0 as f64)
}

/// 二次ベジエ曲線の、`t` の位置と接線。
pub(crate) fn bezier(p0: V, p1: V, p2: V, t: f64) -> (V, V) {
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

/// 二次ベジエ曲線の長さを、折れ線で近づけて求める。
pub(crate) fn arc_length(p0: V, p1: V, p2: V) -> f64 {
    let mut total = 0.0;
    let mut last = p0;
    for k in 1..=ARC_SAMPLES {
        let (p, _) = bezier(p0, p1, p2, k as f64 / ARC_SAMPLES as f64);
        total += dist(last, p);
        last = p;
    }
    total
}

/// 車を描く位置と向き。`shift` は中核が求めた、車線に寄せるずれである。`remembered` は、前の回に
/// この車について返した曲線の始まりで、中核が抜けてきたリンクを忘れたときに代わりに使う。
/// 描く位置と、曲線の上にいれば次の回に渡す曲線の始まりを返す。網から道筋を求められなければ `None`。
pub fn draw(
    net: &Network,
    path: &CarPath,
    shift: (Mm, Mm),
    remembered: Option<&Entry>,
) -> Option<(Drawn, Option<Entry>)> {
    let g = geom(net, path.link)?;
    let len = f64::from(path.len.0);
    if len <= 0.0 {
        return None;
    }
    let o = f64::from(path.offset.0).min(len);
    let lay = Layout::of(net, path.link, &g, len);
    let start = center(net, g.from)?;
    let straight = |along: f64, side: V| Drawn {
        x: start.0 + g.dir.0 * along + side.0,
        y: start.1 + g.dir.1 * along + side.1,
        angle: g.dir.1.atan2(g.dir.0),
    };
    if o >= lay.turn {
        return Some((straight(lay.along(o), mm(shift)), None));
    }
    let fresh = path.prev.and_then(|(prev, shown)| {
        let pg = geom(net, prev)?;
        (pg.to == g.from).then_some(())?;
        let pstop = Layout::of(net, prev, &pg, link_len(net, prev)).stop;
        Some(Entry {
            node: start,
            dir: g.dir,
            p0: add(sub(start, scale(pg.dir, pstop)), mm(shown)),
            d0: pg.dir,
        })
    });
    let same_place = |e: &&Entry| dist(e.node, start) < 1.0 && dist(e.dir, g.dir) < 1e-6;
    let Some(entry) = fresh.or_else(|| remembered.filter(same_place).copied()) else {
        // 抜けてきた所が分からなければ、ノードの中心から窓の終わりまでをまっすぐ描く。
        return Some((straight(lay.settle * o / lay.turn, mm(shift)), None));
    };
    // 曲線の終わりから、窓の終わりまではまっすぐ進む。窓の中の距離を、曲線とまっすぐな所の長さで分ける。
    let lane = mm(path.lane_shift);
    let p2 = add(add(start, scale(g.dir, lay.exit)), lane);
    let p1 = control(entry.p0, entry.d0, p2, g.dir);
    let arc = arc_length(entry.p0, p1, p2);
    let s = o / lay.turn * (arc + lay.settle - lay.exit);
    if s >= arc || arc < 1.0 {
        return Some((straight(lay.exit + (s - arc).max(0.0), lane), Some(entry)));
    }
    let (p, d) = bezier(entry.p0, p1, p2, s / arc);
    let d = if d.0.abs() + d.1.abs() < 1e-9 {
        g.dir
    } else {
        d
    };
    Some((
        Drawn {
            x: p.0,
            y: p.1,
            angle: d.1.atan2(d.0),
        },
        Some(entry),
    ))
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

    /// 道路の始まりと終わりのマス。
    type Road = ((i64, i64), (i64, i64));

    /// 中くらいの道路を `roads` の順に引いた街。
    fn town(roads: &[Road]) -> World {
        let mut w = World::new(1);
        for (seq, (from, to)) in roads.iter().enumerate() {
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

    /// (0, 10)〜(20, 10) の東西の道路と、(10, 0)〜(10, 20) の南北の道路が (10, 10) で交わる街。
    fn crossroads() -> World {
        town(&[((0, 10), (20, 10)), ((10, 0), (10, 20))])
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
        draw(net, p, shift, None).unwrap().0
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
            assert!(close(d.x, NODE + f64::from(o)), "{o}: {}", d.x);
        }
        // 始まりが行き止まりのリンクも、終わりの窓の手前までは中核の位置のまま描く。
        for o in [0, 8_000, 40_000, 59_000] {
            let d = at(net, &path(east, o, None), ZERO);
            assert!(close(d.x, 4_000.0 + f64::from(o)), "{o}: {}", d.x);
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
            close(f64::from(len) - front, line),
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
        let w = town(&[
            ((0, 10), (20, 10)),
            ((10, 0), (10, 20)),
            ((11, 0), (11, 20)),
        ]);
        stops_behind_the_line(&w, [(0, 10), (10, 10), (11, 10)], 8_000, 3_200.0);
        let w = town(&[((0, 0), (20, 20)), ((10, 0), (10, 20)), ((11, 0), (11, 20))]);
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
        for o in (0..=7_000).step_by(250) {
            let d = at(net, &path(north, o, Some((east, shown))), ZERO);
            // 向きは東から北へ、少しずつ回る。
            assert!(d.angle <= last + 1e-9, "{o}: {} > {last}", d.angle);
            assert!(last - d.angle < 0.5, "{o}: 向きが飛んだ");
            last = d.angle;
            if let Some(p) = prev_pos {
                assert!((d.x - p.0).hypot(d.y - p.1) < 1_000.0, "{o}: 位置が飛んだ");
            } else {
                // 曲がり始めは、停止線の手前で待っていた所と同じ。
                assert!(close(d.x, wait.x) && close(d.y, wait.y), "{d:?} {wait:?}");
            }
            prev_pos = Some((d.x, d.y));
        }
        assert!(close(last, -core::f64::consts::FRAC_PI_2));
        // 窓を抜けた所は、中核の位置と同じ。
        let p = prev_pos.unwrap();
        assert!(close(p.0, NODE) && close(p.1, NODE - 7_000.0), "{p:?}");
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
            assert!(close(d.y, NODE - 1_920.0), "{o}: {d:?}");
            assert!(close(d.angle, 0.0));
        }
    }

    #[test]
    fn a_rerouted_car_keeps_to_the_arc_it_was_on() {
        let w = crossroads();
        let net = w.network();
        let east = link_toward(net, (0, 10), (10, 10));
        let north = link_toward(net, (10, 10), (10, 0));
        let (on_arc, entry) =
            draw(net, &path(north, 3_000, Some((east, ZERO))), ZERO, None).unwrap();
        let entry = entry.expect("曲線の上にいる");
        // 経路を探し直して、抜けてきたリンクを忘れても、覚えた曲線の始まりで同じ所に描く。
        let (again, kept) = draw(net, &path(north, 3_000, None), ZERO, Some(&entry)).unwrap();
        assert_eq!(again, on_arc);
        assert_eq!(kept, Some(entry));
        // 別のノードを越えたあとの車には使わない。
        let other = link_toward(net, (10, 10), (20, 10));
        let (d, _) = draw(net, &path(other, 3_000, None), ZERO, Some(&entry)).unwrap();
        assert!(close(d.angle, 0.0));
    }
}
