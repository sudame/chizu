//! 車を、交差点の中の通り抜けの区間では曲線に沿って描く。
//!
//! 中核は、車の位置をリンクの上の距離で持つ。交差点の手前で待つ車は停止線で止まり、停止線から
//! 出ていくリンクの出口までが、交差点の中の通り抜けの区間である（`chizu_core::movement::stop_line`）。
//! 通り抜けの区間の外は、中核の位置をそのまま描く。区間の途中を横切る人や、乗り降りする建物の前と位置がそろう。
//!
//! 通り抜けの区間では、入ってきた車線の停止線の点から、出ていく車線の出口の点までの二次ベジエ曲線の上に、
//! 区間の中で進んだ距離の割合で描く。向きは曲線の接線にする。まっすぐ進む車は、車線に沿って進む。
//! ノードを越える前は、停止線を越えたときに決めた次のリンクを、越えたあとは抜けてきたリンクを使うので、
//! ノードの前後で同じ曲線の上を進む。2 本の区間が向きを変えてつながる角も、道路の端から端までを曲線でつなぐ。

use chizu_core::{CarPath, Dir, LinkId, Mm, Network, NodeId};

/// 曲線の長さを測るときに分ける数。
const ARC_SAMPLES: usize = 8;

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

fn dist(a: V, b: V) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn mm((x, y): (Mm, Mm)) -> V {
    (x.0 as f64, y.0 as f64)
}

/// リンクの、始まりと終わりのノードの中心、1 歩の向き（長さ 1 にしたもの）、長さ（mm）。
struct LinkGeom {
    from: NodeId,
    to: NodeId,
    start: V,
    dir: V,
    len: f64,
}

impl LinkGeom {
    /// 始まりから `along` だけ進み、`side` だけずれた点。
    fn at(&self, along: f64, side: V) -> V {
        add(add(self.start, scale(self.dir, along)), side)
    }
}

fn geom(net: &Network, link: LinkId) -> Option<LinkGeom> {
    let seg = net.segment(link.segment)?;
    let line = net.segment_line(link.segment)?;
    let (from, to, step) = match link.dir {
        Dir::Forward => (seg.a, seg.b, line.step),
        Dir::Backward => (seg.b, seg.a, (-line.step.0, -line.step.1)),
    };
    let n = ((step.0 * step.0 + step.1 * step.1) as f64).sqrt();
    let p = net.node(from)?.pos;
    Some(LinkGeom {
        from,
        to,
        start: (p.x.0 as f64, p.y.0 as f64),
        dir: (step.0 as f64 / n, step.1 as f64 / n),
        len: line.checked_length()?.0 as f64,
    })
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

/// 二次ベジエ曲線の長さを、折れ線で近づけて求める。
fn arc_length(p0: V, p1: V, p2: V) -> f64 {
    let mut total = 0.0;
    let mut last = p0;
    for k in 1..=ARC_SAMPLES {
        let (p, _) = bezier(p0, p1, p2, k as f64 / ARC_SAMPLES as f64);
        total += dist(last, p);
        last = p;
    }
    total
}

/// 点 `p0`（向き `d0`）から点 `p2`（向き `d2`）への曲線の、長さの割合 `f` の所。
fn on_curve(p0: V, d0: V, p2: V, d2: V, f: f64) -> Drawn {
    let p1 = control(p0, d0, p2, d2);
    let f = f.clamp(0.0, 1.0);
    let arc = arc_length(p0, p1, p2);
    // 曲線の上の距離が割合に比例するよう、折れ線の長さで t を求める。
    let mut t = f;
    if arc > 1.0 {
        let want = f * arc;
        let mut acc = 0.0;
        let mut last = p0;
        for k in 1..=ARC_SAMPLES {
            let tk = k as f64 / ARC_SAMPLES as f64;
            let (p, _) = bezier(p0, p1, p2, tk);
            let piece = dist(last, p);
            if acc + piece >= want || k == ARC_SAMPLES {
                let into = if piece > 0.0 {
                    ((want - acc) / piece).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                t = (tk - 1.0 / ARC_SAMPLES as f64) + into / ARC_SAMPLES as f64;
                break;
            }
            acc += piece;
            last = p;
        }
    }
    let (p, d) = bezier(p0, p1, p2, t);
    let d = if d.0.abs() + d.1.abs() < 1e-9 { d2 } else { d };
    Drawn {
        x: p.0,
        y: p.1,
        angle: d.1.atan2(d.0),
    }
}

/// 車を描く位置と向き。`shift` は中核が求めた、車線に寄せるずれである。網から道筋を求められなければ `None`。
pub fn draw(net: &Network, path: &CarPath, shift: (Mm, Mm)) -> Option<Drawn> {
    let g = geom(net, path.link)?;
    let o = f64::from(path.offset.0).min(g.len);
    let stop = f64::from(path.stop.0);
    let exit = f64::from(path.exit.0);
    // 停止線を越え、ノードの手前にいる。
    if let Some((next, next_exit, next_shift)) = path.next
        && o > stop
        && let Some(ng) = geom(net, next).filter(|ng| ng.from == g.to)
    {
        let next_exit = f64::from(next_exit.0);
        let span = g.len - stop + next_exit;
        if span > 0.0 {
            let p0 = g.at(stop, mm(shift));
            let p2 = ng.at(next_exit, mm(next_shift));
            return Some(on_curve(p0, g.dir, p2, ng.dir, (o - stop) / span));
        }
    }
    // ノードを越え、出口の手前にいる。
    if o < exit
        && let Some((prev, prev_stop, shown)) = path.prev
        && let Some(pg) = geom(net, prev).filter(|pg| pg.to == g.from)
    {
        let before = pg.len - f64::from(prev_stop.0);
        let span = before + exit;
        if span > 0.0 {
            let p0 = pg.at(f64::from(prev_stop.0), mm(shown));
            let p2 = g.at(exit, mm(path.exit_shift));
            return Some(on_curve(p0, pg.dir, p2, g.dir, (before + o) / span));
        }
    }
    let p = g.at(o, mm(shift));
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
    use chizu_core::movement::stop_line::{exit_line, stop_line};
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

    const ZERO: (Mm, Mm) = (Mm(0), Mm(0));
    const NODE: f64 = 10.0 * 8_000.0 + 4_000.0;

    fn path(net: &Network, link: LinkId, offset: u32) -> CarPath {
        CarPath {
            link,
            offset: LaneOffset(offset),
            len: LaneOffset(80_000),
            stop: stop_line(net, link),
            exit: exit_line(net, link),
            exit_shift: ZERO,
            prev: None,
            next: None,
        }
    }

    fn at(net: &Network, p: &CarPath) -> Drawn {
        draw(net, p, ZERO).unwrap()
    }

    #[test]
    fn a_car_outside_the_junctions_is_drawn_where_the_core_has_it() {
        let w = crossroads();
        let net = w.network();
        let east = link_toward(net, (0, 10), (10, 10));
        let on = link_toward(net, (10, 10), (20, 10));
        // 停止線で待つ車も、中核の位置のまま描く。窓の中で縮めない。
        let stop = stop_line(net, east).0;
        for o in [0, 8_000, 40_000, 72_000, stop] {
            let d = at(net, &path(net, east, o));
            assert!(close(d.x, 4_000.0 + f64::from(o)), "{o}: {}", d.x);
            assert!(close(d.angle, 0.0));
        }
        let exit = exit_line(net, on).0;
        for o in [exit, 8_000, 40_000, 79_000] {
            let mut p = path(net, on, o);
            p.prev = Some((east, LaneOffset(stop), ZERO));
            let d = at(net, &p);
            assert!(close(d.x, NODE + f64::from(o)), "{o}: {}", d.x);
        }
    }

    #[test]
    fn the_waiting_car_is_behind_the_stop_line() {
        let w = crossroads();
        let net = w.network();
        let east = link_toward(net, (0, 10), (10, 10));
        // 中くらいの道路の端 3.2m、横断歩道 1.28m、すき間 0.4m、停止線 0.32m、車の長さの半分 2.25m。
        let want = 80_000 - 3_200 - 1_280 - 400 - 320 - 2_250;
        assert_eq!(stop_line(net, east).0, want);
        assert_eq!(exit_line(net, east).0, 0);
        let on = link_toward(net, (10, 10), (20, 10));
        assert_eq!(exit_line(net, on).0, 3_200);
    }

    #[test]
    fn a_turning_car_follows_a_smooth_arc_across_the_node() {
        let w = crossroads();
        let net = w.network();
        let east = link_toward(net, (0, 10), (10, 10));
        let north = link_toward(net, (10, 10), (10, 0));
        let stop = stop_line(net, east);
        let exit = exit_line(net, north);
        let lane = (Mm(-1_000), Mm(0));
        let shown = (Mm(0), Mm(-1_000));
        let mut points = Vec::new();
        // ノードの手前（停止線を越えて次のリンクを決めた車）。
        for o in (stop.0..=80_000).step_by(200) {
            let mut p = path(net, east, o);
            p.next = Some((north, exit, lane));
            points.push(draw(net, &p, shown).unwrap());
        }
        // ノードを越えたあと。
        for o in (0..=exit.0 + 2_000).step_by(200) {
            let mut p = path(net, north, o);
            p.prev = Some((east, stop, shown));
            p.exit_shift = lane;
            points.push(draw(net, &p, lane).unwrap());
        }
        // 曲がり始めは、停止線で待っていた所と同じ。
        let wait = draw(net, &path(net, east, stop.0), shown).unwrap();
        assert!(close(points[0].x, wait.x) && close(points[0].y, wait.y));
        let mut last = 0.0_f64;
        for w in points.windows(2) {
            let (a, b) = (w[0], w[1]);
            assert!(
                (a.x - b.x).hypot(a.y - b.y) < 600.0,
                "位置が飛んだ {a:?} {b:?}"
            );
            // 向きは東から北へ、少しずつ回る。
            assert!(b.angle <= last + 1e-9, "{b:?} > {last}");
            assert!(last - b.angle < 0.3, "向きが飛んだ {a:?} {b:?}");
            last = b.angle;
        }
        assert!(close(last, -core::f64::consts::FRAC_PI_2));
        // 出口を過ぎた所は、中核の位置と同じ。
        let end = points.last().unwrap();
        assert!(close(end.x, NODE - 1_000.0) && close(end.y, NODE - f64::from(exit.0 + 2_000)));
    }

    #[test]
    fn a_car_going_straight_stays_in_its_lane() {
        let w = crossroads();
        let net = w.network();
        let east = link_toward(net, (0, 10), (10, 10));
        let on = link_toward(net, (10, 10), (20, 10));
        let stop = stop_line(net, east);
        let exit = exit_line(net, on);
        let lane = (Mm(0), Mm(-1_920));
        for o in (stop.0..=80_000).step_by(500) {
            let mut p = path(net, east, o);
            p.next = Some((on, exit, lane));
            let d = draw(net, &p, lane).unwrap();
            assert!(
                close(d.y, NODE - 1_920.0) && close(d.angle, 0.0),
                "{o}: {d:?}"
            );
            assert!(close(d.x, 4_000.0 + f64::from(o)), "{o}: {d:?}");
        }
        for o in (0..=exit.0).step_by(500) {
            let mut p = path(net, on, o);
            p.prev = Some((east, stop, lane));
            p.exit_shift = lane;
            let d = draw(net, &p, lane).unwrap();
            assert!(
                close(d.y, NODE - 1_920.0) && close(d.angle, 0.0),
                "{o}: {d:?}"
            );
            assert!(close(d.x, NODE + f64::from(o)), "{o}: {d:?}");
        }
    }
}
