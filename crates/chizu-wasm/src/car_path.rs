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
//! - 始まりの窓（`TURN_MM`）：入ってきた車線の停止線の手前から、窓の終わりの出ていく車線までの
//!   道筋（[`Turn`]）に写す。入る車線をまっすぐ進み、2 本の車線の交わる所を制御点にした曲線で
//!   向きを変え、出ていく車線をまっすぐ進む。曲線の両端の離し方と車の向きの決め方は、描く車の絵が
//!   画面の車道（ノードの車道の四角と腕の帯、[`Carriageway`]）からはみ出さないものを選ぶ。
//!   まっすぐ進んで車線を移る車は、両端で車線に沿う S 字を通る。
//! - 窓が重なる短いリンクでは、停止線の位置を保ったまま、あいだを一様に縮める。
//!
//! 2 本だけがつながるノードで向きが変わる角も、同じ道筋でつなぐ。
//!
//! 経路を探し直すと、中核は抜けてきたリンクを忘れる。曲線の途中の車が飛ばないよう、
//! 描いた曲線の始まりを [`Entry`] として呼ぶ側に覚えさせ、次の回に渡してもらう。

use chizu_core::movement::{CAR_LENGTH, MIN_GAP};
use chizu_core::network::TILE_MM;
use chizu_core::{CarPath, Dir, Heading, LinkId, Mm, Network, NodeId};
use std::cell::RefCell;
use std::collections::HashMap;

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
const ZEBRA_LENGTH: f64 = 0.16 * TILE_MM as f64;
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
/// `arc` は、選んだ道筋の形と、そのときの出ていく車線のずれである。
/// 毎回選び直さずに済むよう覚えておく。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Entry {
    node: V,
    dir: V,
    p0: V,
    d0: V,
    arc: Option<(Shape, V)>,
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

/// 描く車の幅の半分。web/src/map/movers.ts の車の半分の大きさ（0.065 マス）と同じにする。
const CAR_SIDE: f64 = 0.065 * TILE_MM as f64;
/// 高速道路の車道の半分の幅。web/src/map/roads.ts の HIGHWAY_ASPHALT_HALF と同じにする。
const HIGHWAY_ASPHALT: f64 = 0.45 * TILE_MM as f64;
/// 斜めの区間に描く車道の半分の幅の上限。web/src/map/roads.ts の DIAGONAL_ASPHALT_HALF と同じにする。
const DIAGONAL_ASPHALT: f64 = 0.2 * TILE_MM as f64;

/// 画面に描く車道の半分の幅（mm）。web/src/map/roads.ts の asphaltHalfWidth と同じ。
fn asphalt_half(kind: chizu_core::SegmentKind) -> f64 {
    if kind.has_sidewalks() {
        (i64::from(kind.car_lanes_per_side()) * kind.lane_width_mm()) as f64
    } else {
        HIGHWAY_ASPHALT
    }
}

/// ノードのまわりに画面で描く車道。ノードの車道の四角と、腕ごとの車道の帯を合わせた形である
/// （web/src/map/roads.ts の roadLaneGeometry）。腕の帯は、ノードから腕の向きへ延びる半直線とみなす。
struct Carriageway {
    center: V,
    square: f64,
    /// 腕の向き（長さ 1）と、車道の半分の幅。
    arms: Vec<(V, f64)>,
    /// 車道の縁が内側へ折れる角。腕の帯の縁が、ノードの車道の四角の縁にぶつかる所である。
    /// 曲がる車が角を削ると、この角が車の絵に食い込む。
    corners: Vec<V>,
}

impl Carriageway {
    fn of(net: &Network, n: NodeId) -> Option<Carriageway> {
        let node = net.node(n)?;
        let center = center(net, n)?;
        let mut square: f64 = 0.0;
        let mut arms = Vec::with_capacity(node.segments.len());
        for &s in &node.segments {
            let (Some(seg), Some(line)) = (net.segment(s), net.segment_line(s)) else {
                continue;
            };
            let half = asphalt_half(seg.kind);
            square = square.max(half);
            let step = if seg.a == n {
                line.step
            } else {
                (-line.step.0, -line.step.1)
            };
            let diagonal = step.0 != 0 && step.1 != 0;
            let len = ((step.0 * step.0 + step.1 * step.1) as f64).sqrt();
            let dir = (step.0 as f64 / len, step.1 as f64 / len);
            arms.push((
                dir,
                if diagonal {
                    half.min(DIAGONAL_ASPHALT)
                } else {
                    half
                },
            ));
        }
        let exit = |e: f64, d: f64| {
            if d.abs() < 1e-9 {
                f64::INFINITY
            } else {
                (square * d.signum() - e) / d
            }
        };
        let mut corners = Vec::new();
        for &(dir, half) in &arms {
            for side in [-1.0, 1.0] {
                // 帯の縁の線を、ノードの中心の横から腕の向きへたどり、四角の縁を出る所。
                let e = scale((-dir.1, dir.0), side * half);
                let r = add(e, scale(dir, exit(e.0, dir.0).min(exit(e.1, dir.1))));
                // ほかの腕の帯に入っていれば、車道の縁ではない。
                let covered = arms
                    .iter()
                    .any(|&(d, h)| d != dir && dot(r, d) > 1.0 && cross(d, r).abs() < h - 1.0);
                if !covered {
                    corners.push(add(center, r));
                }
            }
        }
        Some(Carriageway {
            center,
            square,
            arms,
            corners,
        })
    }

    /// 点 `p` が車道の中にあるか。境目から `slack` mm までの外は中とみなす（負なら、その分内側に限る）。
    fn contains(&self, p: V, slack: f64) -> bool {
        let r = sub(p, self.center);
        if r.0.abs() <= self.square + slack && r.1.abs() <= self.square + slack {
            return true;
        }
        self.arms
            .iter()
            .any(|&(dir, half)| dot(r, dir) >= -slack && cross(dir, r).abs() <= half + slack)
    }

    /// 中心 `c`、向き `h`（長さ 1）に描く車の絵が車道からはみ出す所の数。車の絵の 4 隅のうち車道の外に
    /// 出るものと、車道の縁の角のうち車の絵に食い込むものを数える。境目から `slack` mm までは許す。
    fn outside(&self, c: V, h: V, slack: f64) -> usize {
        let l = (-h.1, h.0);
        let corners = [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)]
            .into_iter()
            .filter(|&(u, v)| {
                let p = add(add(c, scale(h, u * CAR_HALF)), scale(l, v * CAR_SIDE));
                !self.contains(p, slack)
            })
            .count();
        let bitten = self
            .corners
            .iter()
            .filter(|&&p| {
                let r = sub(p, c);
                dot(r, h).abs() < CAR_HALF - slack && dot(r, l).abs() < CAR_SIDE - slack
            })
            .count();
        corners + bitten
    }
}

/// 曲線の両端を、2 本の車線の交わる所からどれだけ離すか。交わる所から `p0` まで、`q` までの距離に掛ける割合で、
/// この中から入る側と出る側の組を選ぶ。
const HANDLE_STEPS: [f64; 7] = [1.0, 0.75, 0.55, 0.4, 0.28, 0.18, 0.1];
/// 曲線を選ぶときに、道筋の上で車の絵を確かめる所の数。
const FIT_SAMPLES: usize = 48;
/// 曲線を選ぶときは、車の絵の縁が車道の境目からこれだけ内側に収まるものを探す（mm）。確かめる所の
/// あいだで縁がふくらむ分を見込む。
const FIT_MARGIN: f64 = 30.0;

/// 曲がる道筋の形。曲線の両端を交わる所から離す割合（[`HANDLE_STEPS`]）と、描く車の向きの決め方
/// （[`Aim::Slide`] にするか）である。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shape {
    fa: f64,
    fb: f64,
    slide: bool,
}

/// 道筋の上の車を、どちらへ向けて描くか。
#[derive(Clone, Copy, Debug, PartialEq)]
enum Aim {
    /// 道筋の接線の向き。
    Tangent,
    /// 車の長さの半分の弦の向き。弦は、曲線に入る前は車の長さの半分だけ戻った道筋の上から車の真ん中まで、
    /// 曲線を抜けたあとは車の真ん中から同じだけ進んだ道筋の上までとし、曲線の上ではそのあいだを滑らかに
    /// 移す。曲がり始めは後ろの端が、曲がり終わりは前の端が道筋の近くに残るので、狭い車線から曲がり
    /// 始めても後ろが外へ振れず、狭い車線へ入るときも前が外へ振れない。
    Slide,
}

/// 曲がる道筋。入る車線の `p0` から向き `d0` にまっすぐ進み、3 次ベジエ曲線 `c` で向きを `d2` に変え、
/// 出ていく車線を `q` までまっすぐ進む。曲線の両端の接線は車線の向きなので、位置も向きも飛ばない。
/// 曲がるときの曲線は、2 本の車線の交わる所を制御点にした二次ベジエ曲線を 3 次に直したものである。
/// まっすぐ進んで車線を移るときは、両端で向きが車線に沿う S 字にする。
#[derive(Clone, Copy, Debug)]
struct Turn {
    p0: V,
    d0: V,
    c: [V; 4],
    d2: V,
    /// 曲線の前と後ろのまっすぐな所の長さ。
    before: f64,
    after: f64,
    /// 曲線を `ARC_SAMPLES` に分けた点までの、曲線に沿った長さ。
    marks: [f64; ARC_SAMPLES + 1],
    aim: Aim,
}

/// 3 次ベジエ曲線の、`t` の位置と接線。
fn cubic(c: &[V; 4], t: f64) -> (V, V) {
    let u = 1.0 - t;
    let p = add(
        add(scale(c[0], u * u * u), scale(c[1], 3.0 * u * u * t)),
        add(scale(c[2], 3.0 * u * t * t), scale(c[3], t * t * t)),
    );
    let d = add(
        add(
            scale(sub(c[1], c[0]), 3.0 * u * u),
            scale(sub(c[2], c[1]), 6.0 * u * t),
        ),
        scale(sub(c[3], c[2]), 3.0 * t * t),
    );
    (p, d)
}

fn dot(a: V, b: V) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

/// 長さ 1 にした向き。長さがほとんどなければ `None`。
fn unit(v: V) -> Option<V> {
    let n = v.0.hypot(v.1);
    (n > 1e-6).then(|| scale(v, 1.0 / n))
}

impl Turn {
    /// 2 本の車線の交わる所と、そこまでの `p0` からの距離、そこから `q` までの距離。
    /// 車線が交わらないか、交わる所が `p0` の後ろか `q` の先なら `None`。折り返しに近いものも `None` にする。
    fn corner(p0: V, d0: V, q: V, d2: V) -> Option<(V, f64, f64)> {
        let den = cross(d0, d2);
        if den.abs() < 1e-6 || dot(d0, d2) < -0.8 {
            return None;
        }
        let a = cross(sub(q, p0), d2) / den;
        let p = add(p0, scale(d0, a));
        let b = dot(sub(q, p), d2);
        (a > 0.0 && b > 0.0).then_some((p, a, b))
    }

    /// 形 `shape` の道筋。2 本の車線が同じ向きなら、形によらず [`Aim::Slide`] の S 字にする。
    /// 作れなければ `None`。
    fn new(p0: V, d0: V, q: V, d2: V, shape: Shape) -> Option<Turn> {
        let (c, before, after, aim) = if let Some((p, a, b)) = Turn::corner(p0, d0, q, d2) {
            let (ta, tb) = (a * shape.fa, b * shape.fb);
            let ca = sub(p, scale(d0, ta));
            let cb = add(p, scale(d2, tb));
            let third = |e: V| add(e, scale(sub(p, e), 2.0 / 3.0));
            let aim = if shape.slide {
                Aim::Slide
            } else {
                Aim::Tangent
            };
            ([ca, third(ca), third(cb), cb], a - ta, b - tb, aim)
        } else if dot(d0, d2) > 1.0 - 1e-9 && dot(sub(q, p0), d0) > 0.0 {
            let h = scale(d0, dot(sub(q, p0), d0) / 3.0);
            ([p0, add(p0, h), sub(q, h), q], 0.0, 0.0, Aim::Slide)
        } else {
            return None;
        };
        let mut marks = [0.0; ARC_SAMPLES + 1];
        let mut last = c[0];
        for k in 1..=ARC_SAMPLES {
            let (pt, _) = cubic(&c, k as f64 / ARC_SAMPLES as f64);
            marks[k] = marks[k - 1] + dist(last, pt);
            last = pt;
        }
        Some(Turn {
            p0,
            d0,
            c,
            d2,
            before,
            after,
            marks,
            aim,
        })
    }

    fn arc(&self) -> f64 {
        self.marks[ARC_SAMPLES]
    }

    fn length(&self) -> f64 {
        self.before + self.arc() + self.after
    }

    /// 道筋の始まりから `s` だけ進んだ所と、そこでの接線の向き（長さ 1）。始まりより手前は入る車線を
    /// まっすぐ戻り、終わりより先は出ていく車線をまっすぐ進む。曲線の上は、長さの表から曲線の変数を求め、
    /// 進む距離に合わせて動かす。
    fn pos(&self, s: f64) -> (V, V) {
        let arc = self.arc();
        if s <= self.before {
            return (add(self.p0, scale(self.d0, s)), self.d0);
        }
        if s >= self.before + arc || arc < 1.0 {
            let s = (s - self.before - arc).max(0.0);
            return (add(self.c[3], scale(self.d2, s)), self.d2);
        }
        let s = s - self.before;
        let k = self
            .marks
            .partition_point(|&m| m <= s)
            .clamp(1, ARC_SAMPLES);
        let (m0, m1) = (self.marks[k - 1], self.marks[k]);
        let within = if m1 > m0 { (s - m0) / (m1 - m0) } else { 0.0 };
        let (p, d) = cubic(&self.c, ((k - 1) as f64 + within) / ARC_SAMPLES as f64);
        (p, unit(d).unwrap_or(self.d0))
    }

    /// 道筋の始まりから `s` だけ進んだ所と、そこで描く車の向き（長さ 1、[`Aim`]）。
    fn at(&self, s: f64) -> (V, V) {
        let (p, tangent) = self.pos(s);
        if self.aim == Aim::Tangent {
            return (p, tangent);
        }
        let u = ((s - self.before) / self.arc().max(1.0)).clamp(0.0, 1.0);
        let ahead = u * u * (3.0 - 2.0 * u) * CAR_HALF;
        let chord = sub(self.pos(s + ahead).0, self.pos(s + ahead - CAR_HALF).0);
        (p, unit(chord).unwrap_or(self.d0))
    }

    /// 曲線の前後の車の長さの半分を含めた `FIT_SAMPLES + 1` か所と道筋の両端で、車の絵が車道から
    /// はみ出す所の数を足したもの。そのほかのまっすぐな所は車線の上なので見ない。`cap` に届いたら、
    /// そこで数えるのをやめて `cap` を返す。
    fn misfit(&self, cw: &Carriageway, cap: usize) -> usize {
        let from = (self.before - CAR_HALF).max(0.0);
        let span = (self.before + self.arc() + CAR_HALF).min(self.length()) - from;
        let mut total = 0;
        for s in (0..=FIT_SAMPLES)
            .map(|k| from + span * k as f64 / FIT_SAMPLES as f64)
            .chain([0.0, self.length()])
        {
            let (c, h) = self.at(s);
            total += cw.outside(c, h, -FIT_MARGIN);
            if total >= cap {
                return cap;
            }
        }
        total
    }

    /// 車の絵が車道 `cw` の外に出ない道筋の形を選ぶ。曲線の短いほうの端が交わる所から遠いもの
    /// （緩く曲がるもの）を先に試し、同じ割合なら [`Aim::Slide`] を先に試す。
    /// どれでも出るなら、出る所がいちばん少ないものにする。道筋を作れなければ `None`。
    fn fit(cw: &Carriageway, p0: V, d0: V, q: V, d2: V) -> Option<Shape> {
        let straight = Shape {
            fa: 1.0,
            fb: 1.0,
            slide: true,
        };
        let Some((_, a, b)) = Turn::corner(p0, d0, q, d2) else {
            return Turn::new(p0, d0, q, d2, straight).map(|_| straight);
        };
        let mut shapes: Vec<Shape> = HANDLE_STEPS
            .iter()
            .flat_map(|&fa| HANDLE_STEPS.iter().map(move |&fb| (fa, fb)))
            .flat_map(|(fa, fb)| [true, false].map(|slide| Shape { fa, fb, slide }))
            .collect();
        let key = |s: &Shape| {
            let (ta, tb) = (a * s.fa, b * s.fb);
            (ta.min(tb), ta + tb)
        };
        // 並べ替えは安定なので、同じ割合の組では Slide が先に残る。
        shapes.sort_by(|x, y| {
            let (kx, ky) = (key(x), key(y));
            ky.0.total_cmp(&kx.0).then(ky.1.total_cmp(&kx.1))
        });
        let mut best: Option<(usize, Shape)> = None;
        for shape in shapes {
            let Some(turn) = Turn::new(p0, d0, q, d2, shape) else {
                continue;
            };
            let misfit = turn.misfit(cw, best.map_or(usize::MAX, |(m, _)| m));
            if best.is_none_or(|(m, _)| misfit < m) {
                best = Some((misfit, shape));
            }
            if misfit == 0 {
                break;
            }
        }
        best.map(|(_, s)| s)
    }
}

/// 選んだ道筋の形を覚えておく数の上限。超えたら忘れて選び直す。
const FITTED_LIMIT: usize = 4096;

thread_local! {
    /// 交差点の車道と、入る車線、出ていく車線ごとに選んだ道筋の形。同じ所を曲がる車は多いので、
    /// 選び直さずに済ませる。描画のためだけに覚え、状態には入れない。反復はしない。
    static FITTED: RefCell<HashMap<Vec<u64>, Option<Shape>>> = RefCell::new(HashMap::new());
}

/// [`Turn::fit`] の結果を、同じ車道と同じ車線の組について覚えておいて使う。
fn fitted(cw: &Carriageway, p0: V, d0: V, q: V, d2: V) -> Option<Shape> {
    let mut key: Vec<u64> = [
        p0.0,
        p0.1,
        d0.0,
        d0.1,
        q.0,
        q.1,
        d2.0,
        d2.1,
        cw.center.0,
        cw.center.1,
        cw.square,
    ]
    .iter()
    .map(|v| v.to_bits())
    .collect();
    for &(d, h) in &cw.arms {
        key.extend([d.0.to_bits(), d.1.to_bits(), h.to_bits()]);
    }
    if let Some(shape) = FITTED.with(|m| m.borrow().get(&key).copied()) {
        return shape;
    }
    let shape = Turn::fit(cw, p0, d0, q, d2);
    FITTED.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= FITTED_LIMIT {
            m.clear();
        }
        m.insert(key, shape);
    });
    shape
}

fn link_len(net: &Network, link: LinkId) -> f64 {
    net.segment_line(link.segment)
        .and_then(|l| l.checked_length())
        .map_or(0.0, |m| m.0 as f64)
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
            arc: None,
        })
    });
    let same_place = |e: &&Entry| dist(e.node, start) < 1.0 && dist(e.dir, g.dir) < 1e-6;
    let Some(entry) = fresh.or_else(|| remembered.filter(same_place).copied()) else {
        // 抜けてきた所が分からなければ、ノードの中心から窓の終わりまでをまっすぐ描く。
        return Some((straight(lay.settle * o / lay.turn, mm(shift)), None));
    };
    let lane = mm(path.lane_shift);
    // 曲がる車は、入る車線から出ていく車線へ、車の絵が車道の外に出ない曲線でつなぎ、窓の終わり
    // （中核の位置）までまっすぐ進む。同じ曲線の上にいる間は、前の回に選んだ曲線を使う。
    let q = add(add(start, scale(g.dir, lay.settle)), lane);
    let kept = remembered
        .filter(|e| same_place(e) && dist(e.p0, entry.p0) < 1e-6 && dist(e.d0, entry.d0) < 1e-6)
        .and_then(|e| e.arc)
        .filter(|&(_, l)| dist(l, lane) < 1e-6)
        .map(|(f, _)| f);
    let shape = kept.or_else(|| {
        let cw = Carriageway::of(net, g.from)?;
        fitted(&cw, entry.p0, entry.d0, q, g.dir)
    });
    if let Some(t) = shape.and_then(|f| Turn::new(entry.p0, entry.d0, q, g.dir, f)) {
        let (p, d) = t.at(o / lay.turn * t.length());
        let entry = Entry {
            arc: shape.map(|f| (f, lane)),
            ..entry
        };
        return Some((
            Drawn {
                x: p.0,
                y: p.1,
                angle: d.1.atan2(d.0),
            },
            Some(entry),
        ));
    }
    // まっすぐ進む車（車線を移る車を含む）は、曲線の終わりから窓の終わりまでまっすぐ進む。窓の中の距離を、
    // 曲線とまっすぐな所の長さで分ける。
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

    /// ノード `n` に入るリンクと出るリンクのすべての組（折り返しを除く）と、すべての車線の組で、
    /// 停止線の手前から窓の終わりまで車を動かし、`check` に、入る車線、出るリンク、描いた位置の並びを渡す。
    fn every_move(w: &World, n: NodeId, mut check: impl FnMut(LinkId, LinkId, &[Drawn])) {
        let net = w.network();
        let node = net.node(n).unwrap();
        let links = |s: chizu_core::SegmentId| {
            [Dir::Forward, Dir::Backward].map(|dir| LinkId {
                segment: s,
                dir,
                mode: LaneMode::Car,
            })
        };
        let all: Vec<LinkId> = node.segments.iter().flat_map(|&s| links(s)).collect();
        for &inn in all.iter().filter(|l| geom(net, **l).unwrap().to == n) {
            for &out in all.iter().filter(|l| geom(net, **l).unwrap().from == n) {
                if out.segment == inn.segment {
                    continue;
                }
                let lanes = |l: LinkId| {
                    i64::from(net.segment(l.segment).unwrap().kind.car_lanes_per_side())
                };
                for i in 0..lanes(inn) {
                    for j in 0..lanes(out) {
                        let shown = lane_of(net, inn, i);
                        let lane = lane_of(net, out, j);
                        let in_len = link_len(net, inn) as u32;
                        let mut wait = path(inn, in_len, None);
                        wait.len = LaneOffset(in_len);
                        let mut drawn = vec![at(net, &wait, shown)];
                        let mut entry = None;
                        for o in (0..=TURN_MM as u32).step_by(25) {
                            let mut p = path(out, o, Some((inn, shown)));
                            p.len = LaneOffset(link_len(net, out) as u32);
                            p.lane_shift = lane;
                            let (d, e) = draw(net, &p, lane, entry.as_ref()).unwrap();
                            entry = e;
                            drawn.push(d);
                        }
                        let mut p = path(out, TURN_MM as u32 + 25, None);
                        p.len = LaneOffset(link_len(net, out) as u32);
                        drawn.push(at(net, &p, lane));
                        check(inn, out, &drawn);
                    }
                }
            }
        }
    }

    /// 交差点の車道の外に、描く車の絵が出ないか。位置と向きが飛ばないか。
    fn stays_on_the_carriageway(w: &World, node: (i64, i64)) {
        let net = w.network();
        let n = net
            .node_at(TilePos::new(node.0, node.1))
            .expect("ノードがない");
        let cw = Carriageway::of(net, n).unwrap();
        let mut moves = 0;
        every_move(w, n, |inn, out, drawn| {
            moves += 1;
            for (k, d) in drawn.iter().enumerate() {
                let h = (d.angle.cos(), d.angle.sin());
                assert_eq!(
                    cw.outside((d.x, d.y), h, 1.0),
                    0,
                    "{inn:?} -> {out:?} の {k} 番目 {d:?} が車道の外に出た"
                );
            }
            let turn = |a: &Drawn, b: &Drawn| {
                (b.angle - a.angle + core::f64::consts::PI).rem_euclid(core::f64::consts::TAU)
                    - core::f64::consts::PI
            };
            for (k, p) in drawn.windows(2).enumerate() {
                let step = (p[1].x - p[0].x).hypot(p[1].y - p[0].y);
                // 25mm ずつ動かすので、曲線の上でも 1 回に動くのはその数倍までである。狭い車線から小さく
                // 曲がる車も、1 回に回るのは 30 度より小さい。
                assert!(
                    step < 100.0,
                    "{inn:?} -> {out:?} の {k} 番目で位置が飛んだ: {step}"
                );
                let t = turn(&p[0], &p[1]);
                assert!(
                    t.abs() < 0.5,
                    "{inn:?} -> {out:?} の {k} 番目で向きが飛んだ: {t}"
                );
            }
            // 曲がり始めは、停止線の手前で待っていた所と同じ。窓を抜けた所では、向きが出ていく車線に戻っている。
            // 斜めのリンクの長さは 1 マス 11,314mm に丸めてあるので、位置は数 mm までずれる。
            let (wait, first) = (&drawn[0], &drawn[1]);
            let gap = (wait.x - first.x).hypot(wait.y - first.y);
            assert!(
                gap < 5.0 && turn(wait, first).abs() < 1e-9,
                "{inn:?} -> {out:?}: {wait:?} {first:?}"
            );
            let [.., end, beyond] = drawn else {
                unreachable!()
            };
            assert!(
                turn(end, beyond).abs() < 1e-9,
                "{inn:?} -> {out:?}: {end:?} {beyond:?}"
            );
        });
        assert!(moves > 0);
    }

    #[test]
    fn turning_cars_stay_on_the_carriageway_at_a_crossroads() {
        stays_on_the_carriageway(&crossroads(), (10, 10));
    }

    #[test]
    fn turning_cars_stay_on_the_carriageway_at_a_t_junction_of_two_kinds() {
        // #209 の交差点と同じ形。南北の中くらいの道路に、東から小さい道路がつながる。
        let w = town_of(&[
            ((10, 0), (10, 20), RoadKind::Medium),
            ((10, 10), (20, 10), RoadKind::Small),
        ]);
        stays_on_the_carriageway(&w, (10, 10));
    }

    #[test]
    fn turning_cars_stay_on_the_carriageway_where_kinds_differ() {
        // 大きい道路と小さい道路の十字路。車線の数も幅も違う。
        let w = town_of(&[
            ((0, 10), (20, 10), RoadKind::Large),
            ((10, 0), (10, 20), RoadKind::Small),
        ]);
        stays_on_the_carriageway(&w, (10, 10));
        // 大きい道路に、中くらいの道路が北からつながる T 字路。
        let w = town_of(&[
            ((0, 10), (20, 10), RoadKind::Large),
            ((10, 0), (10, 10), RoadKind::Medium),
        ]);
        stays_on_the_carriageway(&w, (10, 10));
    }

    #[test]
    fn turning_cars_stay_on_the_carriageway_with_a_diagonal_arm() {
        // 中くらいの道路の十字路に、北西から斜めの小さい道路がつながる。
        let w = town_of(&[
            ((0, 10), (20, 10), RoadKind::Medium),
            ((10, 0), (10, 20), RoadKind::Medium),
            ((2, 2), (10, 10), RoadKind::Small),
        ]);
        stays_on_the_carriageway(&w, (10, 10));
    }

    /// 道路の始まりと終わりのマスと、道路の種類。
    type KindedRoad = ((i64, i64), (i64, i64), RoadKind);

    /// `roads` の順に、種類を指定して道路を引いた街。
    fn town_of(roads: &[KindedRoad]) -> World {
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
