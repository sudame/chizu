//! 車を、ノードの前後で曲線につないだ道筋の上に描く。
//!
//! 中核は、車の位置をリンクの上の距離で持ち、リンクはノードの中心からノードの中心までである。
//! 交差点の手前で待つ車はノードの中心に止まり、ノードを越えると次のリンクの向きへ一度に変わる。
//! そのまま描くと、待つ車が交差点の真ん中に重なり、曲がる車が直角に向きを変える。
//!
//! 画面では、車の位置を、リンクごとの道筋（[`Path`]）の上に写す。道筋は、入ってきた車線の停止線の
//! 手前から、ノードを曲がり、いまのリンクの車線を、終わりのノードの停止線の手前まで進む折れ線である。
//! 状態は変えない。
//!
//! - 始まりの窓（`TURN_MM`）：入ってきた車線の停止線の手前から、窓の終わりの出ていく車線までの
//!   曲線（[`Turn`]）に写す。入る車線をまっすぐ進み、2 本の車線の交わる所を制御点にした曲線で
//!   向きを変え、出ていく車線をまっすぐ進む。曲線の両端の離し方と車の向きの決め方は、描く車の絵が
//!   画面の車道（ノードの車道の四角と腕の帯、[`Carriageway`]）からはみ出さないものを選ぶ。
//!   まっすぐ進んで車線を移る車は、両端で車線に沿う S 字を通る。
//! - 窓の外は、中核の位置のまま描く。区間の途中を横切る人や、乗り降りする建物の前と位置がそろう。
//! - 終わりの窓（`QUEUE_MM`）：ノードの中心（距離がリンクの長さ）を、車の前の端が停止線に
//!   かかる所に写し、窓の中を一様に縮める。止まった列は、停止線の手前に詰めて並ぶ。
//! - 窓が重なる短いリンクでは、停止線の位置を保ったまま、あいだを一様に縮める。
//!
//! 2 本だけがつながるノードで向きが変わる角も、同じ道筋でつなぐ。
//!
//! 道筋は、網の版ごとに、入ってきた車線と、いまのリンクの車線の組ごとに 1 回だけ求めて
//! [`Paths`] に覚える。画面には、道筋の形を網が変わったときだけ送り、車ごとには道筋の番号と、道筋の
//! 上で進んだ割合を送る。画面は、tick と tick のあいだを道筋の上で補間する。1 つの道筋の終わりは、
//! 次のリンクの道筋の始まりと同じ所なので、画面は 2 つの道筋をつないで補間できる。
//!
//! 経路を探し直すと、中核は抜けてきたリンクを忘れる。曲線の途中の車が飛ばないよう、
//! 描いた曲線の始まりを [`Entry`] として呼ぶ側に覚えさせ、次の回に渡してもらう。

use chizu_core::movement::{CAR_LENGTH, MIN_GAP};
use chizu_core::network::TILE_MM;
use chizu_core::{CarPath, Dir, Heading, LinkId, Mm, Network, NodeId};
use std::collections::{BTreeMap, HashMap};

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
/// 描く車の幅の半分。web/src/map/movers.ts の車の半分の大きさ（0.065 マス）と同じにする。
const CAR_SIDE: f64 = 0.065 * TILE_MM as f64;
/// 高速道路の車道の半分の幅。web/src/map/roads.ts の HIGHWAY_ASPHALT_HALF と同じにする。
const HIGHWAY_ASPHALT: f64 = 0.45 * TILE_MM as f64;
/// 斜めの区間に描く車道の半分の幅の上限。web/src/map/roads.ts の DIAGONAL_ASPHALT_HALF と同じにする。
const DIAGONAL_ASPHALT: f64 = 0.2 * TILE_MM as f64;

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

fn dot(a: V, b: V) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

pub(crate) fn dist(a: V, b: V) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// 長さ 1 にした向き。長さがほとんどなければ `None`。
fn unit(v: V) -> Option<V> {
    let n = v.0.hypot(v.1);
    (n > 1e-6).then(|| scale(v, 1.0 / n))
}

pub(crate) fn mm((x, y): (Mm, Mm)) -> V {
    (x.0 as f64, y.0 as f64)
}

/// 角度の差 `b - a` を、-π から π のあいだに直したもの。
fn turn_between(a: f64, b: f64) -> f64 {
    (b - a + core::f64::consts::PI).rem_euclid(core::f64::consts::TAU) - core::f64::consts::PI
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

    /// 曲線の上にいない車の、リンクの上の距離 `o` を描く、始まりのノードからの距離。
    fn straight(&self, o: f64) -> f64 {
        if o >= self.turn {
            self.along(o)
        } else {
            self.settle * o / self.turn
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

    /// 点 `p` の、車道の縁からの距離。外なら正、中なら負（中では、いちばん深く入っている四角か帯の
    /// 縁までの距離）。
    fn signed(&self, p: V) -> f64 {
        let r = sub(p, self.center);
        let box_sd = |a: f64, b: f64| a.max(0.0).hypot(b.max(0.0)) + a.max(b).min(0.0);
        let mut best = box_sd(r.0.abs() - self.square, r.1.abs() - self.square);
        for &(dir, half) in &self.arms {
            best = best.min(box_sd(-dot(r, dir), cross(dir, r).abs() - half));
        }
        best
    }

    /// 中心 `c`、向き `h`（長さ 1）に描く車の絵が、車道からはみ出す量（mm）。車の絵の 4 隅が車道の外に
    /// 出る距離と、車道の縁の角が車の絵に食い込む深さの大きいほうである。`margin` だけ内側に
    /// 収まらない分もはみ出しとみなす（負なら、その分の外は許す）。
    fn overshoot(&self, c: V, h: V, margin: f64) -> f64 {
        let l = (-h.1, h.0);
        let mut worst = f64::NEG_INFINITY;
        for (u, v) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            let p = add(add(c, scale(h, u * CAR_HALF)), scale(l, v * CAR_SIDE));
            worst = worst.max(self.signed(p));
        }
        for &p in &self.corners {
            let r = sub(p, c);
            let depth = (CAR_HALF - dot(r, h).abs()).min(CAR_SIDE - dot(r, l).abs());
            worst = worst.max(depth);
        }
        (worst + margin).max(0.0)
    }
}

/// 曲線の両端を、2 本の車線の交わる所からどれだけ離すか。交わる所から `p0` まで、`q` までの距離に掛ける割合で、
/// この中から入る側と出る側の組を選ぶ。車線が交わらないときは、`p0` から `q` までの距離の半分に掛ける。
const HANDLE_STEPS: [f64; 7] = [1.0, 0.75, 0.55, 0.4, 0.28, 0.18, 0.1];
/// 曲線を選ぶときに、道筋の上で車の絵を確かめる所の数。
const FIT_SAMPLES: usize = 48;
/// 曲線を選ぶときは、車の絵の縁が車道の境目からこれだけ内側に収まるものを探す（mm）。確かめる所の
/// あいだで縁がふくらむ分を見込む。
const FIT_MARGIN: f64 = 30.0;
/// 描く車の向きが変わる速さの上限（ラジアン / mm）。半径 0.5m の円を回る速さで、25mm 進むあいだに
/// 3 度ほど回る。これより急に向きを変える形は、はみ出す量が小さくても選ばない。
///
/// 中くらいの道路のいちばん外の車線は、車の絵の縁から車道の縁まで 120mm しか空かない。そこから内回りで
/// 曲がる車は、角を小さく回らないと車道の角が車の絵に食い込むので、半径 0.8m ほどで回る。
const MAX_CURVATURE: f64 = 1.0 / 500.0;
/// はみ出す量を比べるときの刻み（mm）。この差の中なら、緩いほうを選ぶ。
const MISFIT_STEP: f64 = 5.0;
/// 画面へ送る道筋で、曲線を折れ線に分ける数。
const PATH_SAMPLES: usize = 16;

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
/// まっすぐ進んで車線を移るときは、両端で向きが車線に沿う S 字にする。車線の交わる所が使えない
/// （`q` が交わる所の手前にある、など）ときは、両端の接線を車線の向きにした 3 次の曲線でつなぐ。
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

/// 曲線の形を比べる値。はみ出す量のいちばん大きい所（`MISFIT_STEP` で丸める）と、はみ出す量の和である。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Misfit {
    worst: f64,
    sum: f64,
}

impl Misfit {
    fn better_than(&self, other: &Misfit) -> bool {
        self.worst < other.worst || (self.worst == other.worst && self.sum < other.sum)
    }
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

    /// 2 本の車線が同じ向きで、`q` が `p0` の先にあるか。
    fn parallel(p0: V, d0: V, q: V, d2: V) -> bool {
        dot(d0, d2) > 1.0 - 1e-9 && dot(sub(q, p0), d0) > 0.0
    }

    /// 曲線の両端を離す長さの基準。交わる所までの距離か、交わる所が使えなければ、`p0` から `q` までの
    /// 距離の半分である。
    fn handles(p0: V, d0: V, q: V, d2: V) -> (f64, f64) {
        match Turn::corner(p0, d0, q, d2) {
            Some((_, a, b)) => (a, b),
            None => {
                let half = dist(p0, q) / 2.0;
                (half, half)
            }
        }
    }

    /// 形 `shape` の道筋。2 本の車線が同じ向きなら、形によらず [`Aim::Slide`] の S 字にする。
    /// 作れなければ `None`。
    fn new(p0: V, d0: V, q: V, d2: V, shape: Shape) -> Option<Turn> {
        let aim = if shape.slide {
            Aim::Slide
        } else {
            Aim::Tangent
        };
        let (c, before, after, aim) = if let Some((p, a, b)) = Turn::corner(p0, d0, q, d2) {
            let (ta, tb) = (a * shape.fa, b * shape.fb);
            let ca = sub(p, scale(d0, ta));
            let cb = add(p, scale(d2, tb));
            let third = |e: V| add(e, scale(sub(p, e), 2.0 / 3.0));
            ([ca, third(ca), third(cb), cb], a - ta, b - tb, aim)
        } else if Turn::parallel(p0, d0, q, d2) {
            let h = scale(d0, dot(sub(q, p0), d0) / 3.0);
            ([p0, add(p0, h), sub(q, h), q], 0.0, 0.0, Aim::Slide)
        } else {
            let half = dist(p0, q) / 2.0;
            if half < 1.0 {
                return None;
            }
            let ca = add(p0, scale(d0, half * shape.fa));
            let cb = sub(q, scale(d2, half * shape.fb));
            ([p0, ca, cb, q], 0.0, 0.0, aim)
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

    /// 曲線の前後の車の長さの半分を含めた、確かめる範囲の始まりと長さ。そのほかのまっすぐな所は
    /// 車線の上なので見ない。
    fn span(&self) -> (f64, f64) {
        let from = (self.before - CAR_HALF).max(0.0);
        (
            from,
            (self.before + self.arc() + CAR_HALF).min(self.length()) - from,
        )
    }

    /// 描く車の向きが変わる速さのいちばん大きい値（ラジアン / mm）。
    fn steepness(&self) -> f64 {
        let (from, span) = self.span();
        let n = 4 * FIT_SAMPLES;
        let mut steep: f64 = 0.0;
        let mut last = self.at(from).1;
        for k in 1..=n {
            let h = self.at(from + span * k as f64 / n as f64).1;
            steep = steep.max(turn_between(last.1.atan2(last.0), h.1.atan2(h.0)).abs());
            last = h;
        }
        steep * n as f64 / span.max(1.0)
    }

    /// 確かめる範囲の `FIT_SAMPLES + 1` か所と道筋の両端で、車の絵が車道からはみ出す量。
    /// はみ出す量のいちばん大きい所が `cap` を超えたら、そこで数えるのをやめる。
    fn misfit(&self, cw: &Carriageway, cap: f64) -> Misfit {
        let (from, span) = self.span();
        let mut out = Misfit {
            worst: 0.0,
            sum: 0.0,
        };
        for s in (0..=FIT_SAMPLES)
            .map(|k| from + span * k as f64 / FIT_SAMPLES as f64)
            .chain([0.0, self.length()])
        {
            let (c, h) = self.at(s);
            let over = cw.overshoot(c, h, FIT_MARGIN);
            out.worst = out.worst.max((over / MISFIT_STEP).ceil() * MISFIT_STEP);
            out.sum += over;
            if out.worst > cap {
                break;
            }
        }
        out
    }

    /// 車の絵が車道 `cw` の外に出ない道筋の形を選ぶ。曲線の短いほうの端が交わる所から遠いもの
    /// （緩く曲がるもの）を先に試し、同じ割合なら [`Aim::Slide`] を先に試す。向きを変える速さが
    /// [`MAX_CURVATURE`] を超える形は選ばない。どれでも出るなら、はみ出す量がいちばん小さいものにする。
    /// どの形も急すぎるなら、いちばん緩く向きを変えるものにする。道筋を作れなければ `None`。
    fn fit(cw: &Carriageway, p0: V, d0: V, q: V, d2: V) -> Option<Shape> {
        let straight = Shape {
            fa: 1.0,
            fb: 1.0,
            slide: true,
        };
        if Turn::parallel(p0, d0, q, d2) && Turn::corner(p0, d0, q, d2).is_none() {
            return Turn::new(p0, d0, q, d2, straight).map(|_| straight);
        }
        let (a, b) = Turn::handles(p0, d0, q, d2);
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
        let mut best: Option<(Misfit, Shape)> = None;
        let mut gentlest: Option<(f64, Shape)> = None;
        for shape in shapes {
            let Some(turn) = Turn::new(p0, d0, q, d2, shape) else {
                continue;
            };
            let steep = turn.steepness();
            if steep > MAX_CURVATURE {
                if gentlest.is_none_or(|(g, _)| steep < g) {
                    gentlest = Some((steep, shape));
                }
                continue;
            }
            let cap = best.map_or(f64::INFINITY, |(m, _)| m.worst);
            let misfit = turn.misfit(cw, cap);
            if misfit.worst > cap {
                continue;
            }
            if best.is_none_or(|(m, _)| misfit.better_than(&m)) {
                best = Some((misfit, shape));
            }
            if misfit.worst == 0.0 {
                break;
            }
        }
        best.or(gentlest.map(|(_, s)| {
            (
                Misfit {
                    worst: 0.0,
                    sum: 0.0,
                },
                s,
            )
        }))
        .map(|(_, s)| s)
    }
}

/// 道筋の上の 1 点。位置（mm）、道筋の始まりからの長さ（mm）、描く車の向き（ラジアン）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    p: V,
    s: f64,
    angle: f64,
}

/// 画面に送る道筋。リンクの上の車を描く折れ線で、点のあいだは位置も向きもまっすぐ移す。
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    points: Vec<Point>,
    /// 始まりの窓の終わり（[`Layout::settle`] の所）までの長さ。
    settled: f64,
}

impl Path {
    /// 位置と向きを順に並べた点から作る。長さは点のあいだの距離を足していく。
    fn from_points(at: impl IntoIterator<Item = (V, f64)>, settle_index: usize) -> Path {
        let mut points: Vec<Point> = Vec::new();
        let mut settled = 0.0;
        for (k, (p, angle)) in at.into_iter().enumerate() {
            let s = points.last().map_or(0.0, |l| l.s + dist(l.p, p));
            if let Some(l) = points.last_mut()
                && dist(l.p, p) < 0.5
            {
                l.angle = angle;
            } else {
                points.push(Point { p, s, angle });
            }
            if k == settle_index {
                settled = points.last().map_or(0.0, |l| l.s);
            }
        }
        Path { points, settled }
    }

    /// 道筋の長さ。
    pub fn length(&self) -> f64 {
        self.points.last().map_or(0.0, |p| p.s)
    }

    /// 道筋の始まりから `s` だけ進んだ所の位置と向き。
    pub fn at(&self, s: f64) -> (V, f64) {
        let k = self.points.partition_point(|p| p.s <= s);
        let (Some(a), Some(b)) = (self.points.get(k.wrapping_sub(1)), self.points.get(k)) else {
            let edge = if k == 0 {
                self.points.first()
            } else {
                self.points.last()
            };
            return edge.map_or(((0.0, 0.0), 0.0), |p| (p.p, p.angle));
        };
        let t = (s - a.s) / (b.s - a.s);
        (
            add(a.p, scale(sub(b.p, a.p), t)),
            a.angle + turn_between(a.angle, b.angle) * t,
        )
    }

    /// 点の位置（マス）、長さ（マス）、向きを、1 点 4 つの数で並べたもの。
    fn encode(&self, out: &mut Vec<f64>) {
        let tile = TILE_MM as f64;
        for p in &self.points {
            out.extend([p.p.0 / tile, p.p.1 / tile, p.s / tile, p.angle]);
        }
    }
}

/// 道筋を見分ける鍵。いまのリンク、その車線のずれ（mm）、入ってきた車線の停止線の手前の点と向き
/// （曲線の上を通らない道筋なら `None`）である。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    link: LinkId,
    lane: (i64, i64),
    entry: Option<[i64; 4]>,
}

impl Key {
    fn of(link: LinkId, lane: (Mm, Mm), entry: Option<&Entry>) -> Key {
        // 点は 1/16 mm、向きは 10 億分の 1 に丸める。同じ式で求めた値は同じ鍵になる。
        let q = |v: f64, k: f64| (v * k).round() as i64;
        Key {
            link,
            lane: (lane.0.0, lane.1.0),
            entry: entry.map(|e| {
                [
                    q(e.p0.0, 16.0),
                    q(e.p0.1, 16.0),
                    q(e.d0.0, 1e9),
                    q(e.d0.1, 1e9),
                ]
            }),
        }
    }
}

/// 速く引くための、リンクの ID を数に直したもの。
type LinkBits = (u32, u16, u8);

fn link_bits(l: LinkId) -> LinkBits {
    let dir = match l.dir {
        Dir::Forward => 0,
        Dir::Backward => 1,
    };
    (l.segment.index, l.segment.generation, dir)
}

/// 中核が渡す道筋（[`CarPath`]）のうち、描く道筋を決めるもの。いまのリンクと車線のずれ、抜けてきた
/// リンクとノードを越える直前のずれである。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Raw {
    link: LinkBits,
    lane: (i64, i64),
    prev: Option<(LinkBits, (i64, i64))>,
}

impl Raw {
    fn of(path: &CarPath) -> Raw {
        Raw {
            link: link_bits(path.link),
            lane: (path.lane_shift.0.0, path.lane_shift.1.0),
            prev: path.prev.map(|(l, (x, y))| (link_bits(l), (x.0, y.0))),
        }
    }
}

/// `Raw` から引いた道筋。番号と、曲線の上を通る道筋か。覚えた曲線の始まりを使ったときは覚えない。
type RawHit = Option<(u32, bool)>;

/// 描画のためだけの表を引く、速いハッシュ。表は反復しないので、並びは描く結果に効かない。
#[derive(Default, Clone, Copy)]
pub struct FastHasher(u64);

impl core::hash::Hasher for FastHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.write_u64(u64::from(n));
    }

    fn write_u16(&mut self, n: u16) {
        self.write_u64(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_i64(&mut self, n: i64) {
        self.write_u64(n as u64);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn write_isize(&mut self, n: isize) {
        self.write_u64(n as u64);
    }
}

/// [`FastHasher`] で引く表。
pub type FastMap<K, T> = HashMap<K, T, core::hash::BuildHasherDefault<FastHasher>>;

/// 覚えておく道筋の数の上限。超えたら、すべて忘れて求め直す。
const PATHS_LIMIT: usize = 20_000;
/// 選んだ曲線の形を覚えておく数の上限。超えたら忘れて選び直す。
const SHAPES_LIMIT: usize = 65_536;

/// 道筋の覚え書き。網の版ごとに、鍵ごとの道筋を 1 回だけ求めて覚える。道筋の番号は網の版が変わっても
/// 使い回さないので、画面は前の網の道筋を新しい網の道筋と取り違えない。描画のためだけに覚え、状態には
/// 入れない。
pub struct Paths {
    version: Option<u64>,
    /// いまの網の版で最初に振った番号。番号 `first + k` の道筋は `paths[k]` にある。
    first: u32,
    paths: Vec<Path>,
    ids: BTreeMap<Key, Option<u32>>,
    raw: FastMap<Raw, Option<RawHit>>,
    /// まだ画面へ送っていない道筋の、`paths` の中での始まり。
    unsent: usize,
    /// 前に送ってから、覚えた道筋を忘れたか。
    reset: bool,
    /// 交差点の車道と、入る車線、出ていく車線の、ノードの中心からの位置と向きごとに選んだ曲線の形。
    /// 同じ形の交差点は多いので、網が変わっても選び直さずに済ませる。反復はしない。
    shapes: FastMap<Vec<u64>, Option<Shape>>,
}

impl Default for Paths {
    fn default() -> Self {
        Self::new()
    }
}

impl Paths {
    pub fn new() -> Paths {
        Paths {
            version: None,
            first: 1,
            paths: Vec::new(),
            ids: BTreeMap::new(),
            raw: FastMap::default(),
            unsent: 0,
            reset: true,
            shapes: FastMap::default(),
        }
    }

    fn forget(&mut self) {
        self.first = self.next_id();
        self.paths.clear();
        self.ids.clear();
        self.raw.clear();
        self.unsent = 0;
        self.reset = true;
    }

    fn next_id(&self) -> u32 {
        u32::try_from(self.paths.len())
            .ok()
            .and_then(|n| self.first.checked_add(n))
            .expect("道筋の番号が尽きた")
    }

    /// 網の版が変わっていれば、覚えた道筋を忘れる。
    pub fn sync(&mut self, net: &Network) {
        if self.version != Some(net.version()) {
            self.version = Some(net.version());
            self.forget();
        }
    }

    /// 番号 `id` の道筋。
    pub fn get(&self, id: u32) -> Option<&Path> {
        self.paths.get(id.checked_sub(self.first)? as usize)
    }

    /// 鍵 `key` の道筋の番号。まだなければ求めて覚える。求められなければ `None`。
    fn id(&mut self, net: &Network, key: &Key, entry: Option<&Entry>) -> Option<u32> {
        if let Some(&id) = self.ids.get(key) {
            return id;
        }
        if self.ids.len() >= PATHS_LIMIT {
            self.forget();
        }
        let id = self.build(net, key, entry).map(|path| {
            let id = self.next_id();
            self.paths.push(path);
            id
        });
        self.ids.insert(*key, id);
        id
    }

    fn build(&mut self, net: &Network, key: &Key, entry: Option<&Entry>) -> Option<Path> {
        let g = geom(net, key.link)?;
        let len = link_len(net, key.link);
        if len <= 0.0 {
            return None;
        }
        let lay = Layout::of(net, key.link, &g, len);
        let start = center(net, g.from)?;
        let lane = (key.lane.0 as f64, key.lane.1 as f64);
        let angle = g.dir.1.atan2(g.dir.0);
        let on_lane = |along: f64| add(add(start, scale(g.dir, along)), lane);
        let end = (on_lane(len - lay.stop), angle);
        let Some(e) = entry.filter(|_| lay.turn > 0.0) else {
            return Some(Path::from_points([(on_lane(0.0), angle), end], 0));
        };
        let q = on_lane(lay.settle);
        let cw = Carriageway::of(net, g.from)?;
        let shape = self.fitted(&cw, e.p0, e.d0, q, g.dir)?;
        let t = Turn::new(e.p0, e.d0, q, g.dir, shape)?;
        let mut at: Vec<(V, f64)> = Vec::with_capacity(PATH_SAMPLES + 4);
        let mut put = |s: f64| {
            let (p, h) = t.at(s);
            at.push((p, h.1.atan2(h.0)));
        };
        put(0.0);
        put(t.before);
        for k in 1..=PATH_SAMPLES {
            put(t.before + t.arc() * k as f64 / PATH_SAMPLES as f64);
        }
        put(t.length());
        let settle_index = at.len() - 1;
        at.push(end);
        Some(Path::from_points(at, settle_index))
    }

    /// [`Turn::fit`] の結果を、同じ車道と同じ車線の組について覚えておいて使う。
    fn fitted(&mut self, cw: &Carriageway, p0: V, d0: V, q: V, d2: V) -> Option<Shape> {
        let c = cw.center;
        let mut key: Vec<u64> = [
            p0.0 - c.0,
            p0.1 - c.1,
            d0.0,
            d0.1,
            q.0 - c.0,
            q.1 - c.1,
            d2.0,
            d2.1,
            cw.square,
        ]
        .iter()
        .map(|v| v.to_bits())
        .collect();
        for &(d, h) in &cw.arms {
            key.extend([d.0.to_bits(), d.1.to_bits(), h.to_bits()]);
        }
        if let Some(shape) = self.shapes.get(&key) {
            return *shape;
        }
        let shape = Turn::fit(cw, p0, d0, q, d2);
        if self.shapes.len() >= SHAPES_LIMIT {
            self.shapes.clear();
        }
        self.shapes.insert(key, shape);
        shape
    }

    /// まだ画面へ送っていない道筋を、送る形に並べて返す。先頭は、前に送ってから覚えた道筋を忘れたか
    /// （忘れたら 1）と、道筋の数である。道筋ごとに、番号、点の数、点ごとの x、y（マス）、始まりからの
    /// 長さ（マス）、向き（ラジアン）が続く。
    pub fn take(&mut self) -> Vec<f64> {
        let fresh = &self.paths[self.unsent..];
        let mut out = vec![f64::from(u8::from(self.reset)), fresh.len() as f64];
        for (k, path) in fresh.iter().enumerate() {
            let id = self.first as usize + self.unsent + k;
            out.extend([id as f64, path.points.len() as f64]);
            path.encode(&mut out);
        }
        self.unsent = self.paths.len();
        self.reset = false;
        out
    }
}

/// 前の tick に車を描いた道筋の、リンクと車線のずれ。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Last {
    link: LinkId,
    lane: (i64, i64),
}

/// 車を道筋の上に描いたときの、道筋の番号と、道筋の上で進んだ割合。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Route {
    pub id: u32,
    /// 前の回にいた道筋から、この道筋までのあいだに通り抜けた道筋の番号。なければ 0。
    pub via: u32,
    pub frac: f32,
    /// 次の tick に [`draw`] へ渡す、この道筋のリンクと車線。
    pub last: Last,
}

/// 車を描いた結果。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub drawn: Drawn,
    /// 曲線の上にいれば、次の回に渡す曲線の始まり。
    pub entry: Option<Entry>,
    /// 道筋の上に描いたなら、その道筋。
    pub route: Option<Route>,
}

/// 車を描く位置と向き。`shift` は中核が求めた、車線に寄せるずれである。`remembered` は、前の回に
/// この車について返した曲線の始まりで、中核が抜けてきたリンクを忘れたときに代わりに使う。`before` は、
/// 前の tick にこの車を描いた道筋で、そこからいまの道筋までのあいだに通り抜けた道筋を求めるのに使う。
/// 網から道筋を求められなければ `None`。
pub fn draw(
    net: &Network,
    paths: &mut Paths,
    path: &CarPath,
    shift: (Mm, Mm),
    remembered: Option<&Entry>,
    before: Option<&Last>,
) -> Option<Placed> {
    let g = geom(net, path.link)?;
    let len = link_len(net, path.link);
    if len <= 0.0 || f64::from(path.len.0) <= 0.0 {
        return None;
    }
    let o = f64::from(path.offset.0).min(len);
    let lay = Layout::of(net, path.link, &g, len);
    let start = center(net, g.from)?;
    let on_arc = o < lay.turn;
    // 曲線の始まりは、曲線の上にいる車か、道筋を初めて求める車のときだけ求める。
    let mut entry_cache: Option<Option<Entry>> = None;
    let mut entry = || {
        *entry_cache.get_or_insert_with(|| {
            let fresh = path
                .prev
                .and_then(|(prev, shown)| entry_from(net, prev, shown, start, &g));
            let same_place = |e: &&Entry| dist(e.node, start) < 1.0 && dist(e.dir, g.dir) < 1e-6;
            fresh
                .or_else(|| remembered.filter(same_place).copied())
                .filter(|_| lay.turn > 0.0)
        })
    };
    let kept = if on_arc { entry() } else { None };
    // 曲線の上の車は車線のずれによらず道筋の上にいる。ほかの車は、車線を移り終えていれば道筋の上にいる。
    // 中核は、ノードを越えた車を越えた先の所に置き、そこから前の車線のずれを今の車線のずれへ移す。
    // 曲線を通る道筋は前の車線から今の車線へつながっているので、その移り変わりのあいだも道筋の上に描く。
    let settling = shift != path.lane_shift
        && path
            .prev
            .is_some_and(|(_, shown)| between(mm(shift), mm(shown), mm(path.lane_shift)));
    if kept.is_some() || shift == path.lane_shift || settling {
        let raw = Raw::of(path);
        let hit = match paths.raw.get(&raw) {
            Some(&Some(hit)) => hit,
            _ => {
                let e = entry();
                let key = Key::of(path.link, path.lane_shift, e.as_ref());
                let hit = paths.id(net, &key, e.as_ref()).map(|id| (id, e.is_some()));
                // 覚えた曲線の始まりを使ったときは、抜けてきたリンクから決まらないので覚えない。
                let from_prev = path
                    .prev
                    .and_then(|(prev, shown)| entry_from(net, prev, shown, start, &g))
                    .filter(|_| lay.turn > 0.0);
                let cacheable = from_prev.is_some() == e.is_some();
                paths.raw.insert(raw, cacheable.then_some(hit));
                hit
            }
        };
        if let Some((id, curved)) = hit
            && (curved || !settling)
            && let Some(placed) = on_path(net, paths, path, &lay, id, curved, before)
        {
            return Some(Placed {
                entry: kept,
                ..placed
            });
        }
    }
    let side = mm(shift);
    let along = lay.straight(o);
    Some(Placed {
        drawn: Drawn {
            x: start.0 + g.dir.0 * along + side.0,
            y: start.1 + g.dir.1 * along + side.1,
            angle: g.dir.1.atan2(g.dir.0),
        },
        entry: kept,
        route: None,
    })
}

/// 点 `p` が、`a` から `b` への線分の上（1mm まで）にあるか。
fn between(p: V, a: V, b: V) -> bool {
    let ab = sub(b, a);
    let len2 = dot(ab, ab);
    if len2 < 1.0 {
        return dist(p, a) < 1.0;
    }
    let t = dot(sub(p, a), ab) / len2;
    (-1e-6..=1.0 + 1e-6).contains(&t) && dist(p, add(a, scale(ab, t))) < 1.0
}

/// リンク `prev` を抜けて、`start` から `g` の向きのリンクへ入った車の曲線の始まり。`shown` は、
/// ノードを越える直前に描いていたずれである。
fn entry_from(
    net: &Network,
    prev: LinkId,
    shown: (Mm, Mm),
    start: V,
    g: &LinkGeom,
) -> Option<Entry> {
    let pg = geom(net, prev)?;
    (pg.to == g.from).then_some(())?;
    let pstop = Layout::of(net, prev, &pg, link_len(net, prev)).stop;
    Some(Entry {
        node: start,
        dir: g.dir,
        p0: add(sub(start, scale(pg.dir, pstop)), mm(shown)),
        d0: pg.dir,
    })
}

/// 番号 `id` の道筋の上に描く。`curved` は、曲線の上を通る道筋か。
fn on_path(
    net: &Network,
    paths: &mut Paths,
    path: &CarPath,
    lay: &Layout,
    id: u32,
    curved: bool,
    before: Option<&Last>,
) -> Option<Placed> {
    let o = f64::from(path.offset.0).min(lay.len);
    let p = paths.get(id)?;
    let s = if !curved {
        lay.straight(o)
    } else if o < lay.turn {
        p.settled * o / lay.turn
    } else {
        p.settled + lay.along(o) - lay.settle
    };
    let s = s.clamp(0.0, p.length());
    let ((x, y), angle) = p.at(s);
    let frac = if p.length() > 0.0 {
        (s / p.length()) as f32
    } else {
        0.0
    };
    let via = before
        .filter(|b| b.link != path.link)
        .and_then(|b| passed(net, paths, b, path))
        .unwrap_or(0);
    Some(Placed {
        drawn: Drawn { x, y, angle },
        entry: None,
        route: Some(Route {
            id,
            via,
            frac,
            last: Last {
                link: path.link,
                lane: (path.lane_shift.0.0, path.lane_shift.1.0),
            },
        }),
    })
}

/// 前の tick に `before` の道筋にいた車が、いまのリンクの手前のリンクを丸ごと通り抜けたなら、
/// その通り抜けたリンクの道筋の番号。
fn passed(net: &Network, paths: &mut Paths, before: &Last, path: &CarPath) -> Option<u32> {
    let (mid, shown) = path.prev?;
    if before.link == mid || before.link == path.link {
        return None;
    }
    let bg = geom(net, before.link)?;
    let mg = geom(net, mid)?;
    if bg.to != mg.from {
        return None;
    }
    let start = center(net, mg.from)?;
    let lane = (Mm(before.lane.0), Mm(before.lane.1));
    let len = link_len(net, mid);
    let entry = entry_from(net, before.link, lane, start, &mg)
        .filter(|_| Layout::of(net, mid, &mg, len).turn > 0.0);
    let key = Key::of(mid, shown, entry.as_ref());
    paths.id(net, &key, entry.as_ref())
}

/// 角度（ラジアン）を、x 軸から y 軸へ回る 1 周を 65536 とした値に直す。
pub fn angle_code(angle: f64) -> u16 {
    let turns = angle / core::f64::consts::TAU;
    let code = (turns * 65536.0).round().rem_euclid(65536.0);
    code as u16
}

#[cfg(test)]
mod tests;
