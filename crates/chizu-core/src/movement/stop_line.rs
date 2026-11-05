//! 車のリンクの上で、ノードの中の通り抜けの区間に入る所と出る所。
//!
//! リンクの上の距離は、ノードの中心からノードの中心までで測る。車の位置は車の真ん中である。
//! 交差点では、車は終わりの交差点の停止線（[`stop_line`]）で待ち、そこを越えると交差点の中にいる。
//! ノードの中心で次のリンクへ移り、出ていく腕の横断歩道を車の後ろの端が抜ける所（[`clear_line`]）までは、
//! まだ交差点の中にいる。停止線から次のリンクのその所までが、交差点の中の通り抜けの区間である。
//! CS2 と同じく、待つ車は交差点の中で止まらない。
//!
//! 2 本の区間が向きを変えてつながる角も、道路の端から端までを通り抜けの区間にする。角では譲り合わないので、
//! 角の手前では止まらず、画面が曲線で描くのに使うだけである。
//!
//! 道路の端、横断歩道、停止線、描く車の寸法は、画面の `web/src/map/roads.ts`、`detail.ts`、`movers.ts` と
//! 同じにする。画面は、停止線と抜ける所の位置をここから読んで車を描く。

use super::junction;
use crate::command::NodeId;
use crate::network::{DIAGONAL_TILE_MM, Dir, LinkId, Network, TILE_MM};
use crate::units::LaneOffset;

/// 横断歩道の縞の長さ（mm）。roads.ts の ZEBRA_LENGTH（0.16 マス）と同じ。
pub const ZEBRA_MM: i64 = 1_280;

/// 横断歩道と停止線のすき間（mm）。detail.ts の STOP_LINE_GAP（0.05 マス）と同じ。
pub const STOP_GAP_MM: i64 = 400;

/// 停止線の幅（mm）。detail.ts の STOP_LINE_WIDTH（0.04 マス）と同じ。
pub const STOP_WIDTH_MM: i64 = 320;

/// 画面に描く車の長さの半分（mm）。movers.ts の車の半分の大きさ（0.2 マス）と同じ。
/// 待つ車は、描く車の前の端が停止線にかかる所で止まる。
pub const DRAWN_CAR_HALF_MM: i64 = 1_600;

/// リンクの始まりと終わりのノード。
fn ends(net: &Network, link: LinkId) -> Option<(NodeId, NodeId)> {
    let s = net.segment(link.segment)?;
    Some(match link.dir {
        Dir::Forward => (s.a, s.b),
        Dir::Backward => (s.b, s.a),
    })
}

/// ノード `n` につながる区間の、道路の端（歩道を含む）までのいちばん広い半分の幅。roads.ts の RoadNode の half と同じ。
fn node_half(net: &Network, n: NodeId) -> i64 {
    let Some(node) = net.node(n) else {
        return 0;
    };
    node.segments
        .iter()
        .filter_map(|s| net.segment(*s))
        .map(|s| {
            let k = s.kind;
            i64::from(k.car_lanes_per_side()) * k.lane_width_mm() + k.sidewalk_width_mm()
        })
        .max()
        .unwrap_or(0)
}

/// 区間 `link.segment` が斜めなら、マスの辺に沿った長さ `d` を斜めの腕に沿った長さに直す。
fn along_arm(net: &Network, link: LinkId, d: i64) -> i64 {
    let diagonal = net
        .segment_line(link.segment)
        .is_some_and(|l| l.is_diagonal());
    if diagonal {
        d * DIAGONAL_TILE_MM / TILE_MM
    } else {
        d
    }
}

/// ノード `n` から、区間 `link.segment` の腕に沿って横断歩道の外の端までの距離。横断歩道がなければ道路の端まで。
fn crosswalk_end(net: &Network, n: NodeId, link: LinkId) -> i64 {
    let walks = net
        .segment(link.segment)
        .is_some_and(|s| s.kind.has_sidewalks());
    let zebra = walks
        && net
            .arm_heading(n, link.segment)
            .zip(net.node(n))
            .is_some_and(|(h, node)| !node.crosswalk_removed(h));
    along_arm(net, link, node_half(net, n)) + if zebra { ZEBRA_MM } else { 0 }
}

/// ノード `n` の車道の四角の半分の大きさ。つながる区間の、車道の半分の幅のいちばん大きい値である。
fn asphalt_half(net: &Network, n: NodeId) -> i64 {
    let Some(node) = net.node(n) else {
        return 0;
    };
    node.segments
        .iter()
        .filter_map(|s| net.segment(*s))
        .map(|s| i64::from(s.kind.car_lanes_per_side()) * s.kind.lane_width_mm())
        .max()
        .unwrap_or(0)
}

/// 交差点 `n` の車道の四角に、区間 `link.segment` の腕から車が入る所と出る所の、車の真ん中のノードの中心からの距離。
/// 描く車の前の端が四角の縁にかかる所で、出ていくときは後ろの端が縁を抜ける所である。
/// 交差点の中で道筋が交わるのは、この四角の中だけである。
pub fn box_reach(net: &Network, n: NodeId, link: LinkId) -> i64 {
    along_arm(net, link, asphalt_half(net, n)) + DRAWN_CAR_HALF_MM
}

/// ノード `n` が、2 本の区間を向きを変えてつなぐ角か。
pub fn is_bend(net: &Network, n: NodeId) -> bool {
    let Some(node) = net.node(n) else {
        return false;
    };
    let [s, t] = node.segments.as_slice() else {
        return false;
    };
    match (net.arm_heading(n, *s), net.arm_heading(n, *t)) {
        (Some(a), Some(b)) => (a as u8 + 4) % 8 != b as u8,
        _ => false,
    }
}

/// ノード `n` の中に、通り抜けの区間があるか。交差点と角である。
pub fn has_interior(net: &Network, n: NodeId) -> bool {
    junction::is_junction(net, n) || is_bend(net, n)
}

fn link_len(net: &Network, link: LinkId) -> i64 {
    net.segment_line(link.segment)
        .and_then(|l| l.checked_length())
        .map_or(0, |m| m.0)
}

fn offset(v: i64) -> LaneOffset {
    LaneOffset(u32::try_from(v.max(0)).expect("リンクの上の距離が u32 に収まらない"))
}

/// リンク `link` の始まりのノードの通り抜けの区間を、車が抜けきる所の、始まりからの距離。
/// 交差点では、描く車の後ろの端が出ていく腕の横断歩道を抜ける所、角では道路の端である。
/// 通り抜けの区間がなければ 0。
pub fn clear_line(net: &Network, link: LinkId) -> LaneOffset {
    let Some((from, _)) = ends(net, link) else {
        return LaneOffset::START;
    };
    let clear = if junction::is_junction(net, from) {
        crosswalk_end(net, from, link) + DRAWN_CAR_HALF_MM
    } else if is_bend(net, from) {
        along_arm(net, link, node_half(net, from))
    } else {
        0
    };
    offset(clear.min(link_len(net, link)))
}

/// リンク `link` の終わりのノードの手前で、車が通り抜けの区間に入る所（始まりからの距離）。
/// 交差点では停止線で、待つ車はここで止まる。描く車の前の端が停止線にかかる。角では道路の端、
/// 通り抜けの区間がなければリンクの終わりである。
///
/// 1 マスのリンクのように、そこが始まりのノードの通り抜けの区間の中になるときは、その区間を抜けた所にする。
/// 待つ車が手前の交差点の中で止まらないようにするためである。ただし、その所が終わりの交差点の車道の四角に
/// かかるときは、四角の縁（[`box_reach`]）にする。交わる車の通る四角に、譲るかを決める前に入らないようにする
/// ためである。2 つの交差点の四角が重なるときは、待つ車は手前の交差点の四角にかかる。
pub fn stop_line(net: &Network, link: LinkId) -> LaneOffset {
    let len = link_len(net, link);
    let clear = i64::from(clear_line(net, link).0);
    match ends(net, link) {
        Some((_, to)) if junction::is_junction(net, to) => {
            let back =
                crosswalk_end(net, to, link) + STOP_GAP_MM + STOP_WIDTH_MM + DRAWN_CAR_HALF_MM;
            let edge = len - box_reach(net, to, link);
            offset((len - back).max(clear.min(edge)).min(len))
        }
        Some((_, to)) if is_bend(net, to) => {
            let back = along_arm(net, link, node_half(net, to));
            offset((len - back).max(clear).min(len))
        }
        _ => offset(len),
    }
}
