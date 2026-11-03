//! 車のリンクの上で、交差点の中に入る所と出る所。
//!
//! リンクの上の距離は、ノードの中心からノードの中心までで測る。車は、終わりの交差点の停止線
//! （[`stop_line`]）で待ち、そこを越えると交差点の中にいる。ノードの中心で次のリンクへ移り、
//! 始まりの交差点の出口（[`exit_line`]）までは、まだ交差点の中にいる。停止線から次のリンクの
//! 出口までが、交差点の中の通り抜けの区間である。CS2 と同じく、待つ車は交差点の中で止まらない。
//!
//! 2 本の区間が向きを変えてつながる角も、道路の端から端までを通り抜けの区間にする。角では譲り合わないので、
//! 停止線では止まらず、画面が曲線で描くのに使うだけである。
//!
//! 道路の端、横断歩道、停止線の寸法は、画面の `web/src/map/roads.ts` と `detail.ts` と同じにする。
//! 画面は、停止線と出口の位置をここから読んで車を描く。

use super::CAR_LENGTH;
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

/// 車の位置（リンクの上の距離）から、車の前の端までの距離。
const CAR_HALF_MM: i64 = CAR_LENGTH as i64 / 2;

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

/// ノード `n` から、区間 `link.segment` の腕に沿って道路の端までの距離。斜めの腕は √2 倍にする。
fn edge(net: &Network, n: NodeId, link: LinkId) -> i64 {
    let half = node_half(net, n);
    let diagonal = net
        .segment_line(link.segment)
        .is_some_and(|l| l.is_diagonal());
    if diagonal {
        half * DIAGONAL_TILE_MM / TILE_MM
    } else {
        half
    }
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

/// リンク `link` の始まりの交差点の出口の、始まりからの距離。通り抜けの区間がなければ 0。
pub fn exit_line(net: &Network, link: LinkId) -> LaneOffset {
    let Some((from, _)) = ends(net, link) else {
        return LaneOffset::START;
    };
    if !has_interior(net, from) {
        return LaneOffset::START;
    }
    offset(edge(net, from, link).min(link_len(net, link)))
}

/// リンク `link` の終わりの交差点の手前で、待つ車の位置（始まりからの距離）。車の前の端が停止線にかかる。
/// 角では道路の端、通り抜けの区間がなければリンクの終わりである。
///
/// 1 マスのリンクのように、停止線が始まりの交差点の出口より手前になるときは、出口で待つ。
pub fn stop_line(net: &Network, link: LinkId) -> LaneOffset {
    let len = link_len(net, link);
    let Some((_, to)) = ends(net, link) else {
        return offset(len);
    };
    let back = if junction::is_junction(net, to) {
        let walks = net
            .segment(link.segment)
            .is_some_and(|s| s.kind.has_sidewalks());
        let zebra = walks
            && net
                .arm_heading(to, link.segment)
                .zip(net.node(to))
                .is_some_and(|(h, n)| !n.crosswalk_removed(h));
        edge(net, to, link)
            + if zebra { ZEBRA_MM } else { 0 }
            + STOP_GAP_MM
            + STOP_WIDTH_MM
            + CAR_HALF_MM
    } else if is_bend(net, to) {
        edge(net, to, link)
    } else {
        0
    };
    let exit = i64::from(exit_line(net, link).0);
    offset((len - back).max(exit).min(len))
}

/// リンク `link` の始まりの交差点から出ていく車が、交差点の中にいるとみなされるあいだの距離。
/// 停止線から出口までが交差点の中なので、出口までである。
pub(super) fn clear_line(net: &Network, link: LinkId) -> u32 {
    let exit = i64::from(exit_line(net, link).0);
    let clear = exit.max(1);
    u32::try_from(clear).expect("距離が u32 に収まらない")
}
