//! 画面に描く移動体の、車線や歩道に合わせた横のずれ。
//!
//! 移動体の位置は区間の中心線の上にある。画面ではそこに足して、車は走っている車線の中心に、
//! 歩く人は歩道の上に描く。描画のためだけの値で、状態にも状態ハッシュにも入れない。
//!
//! 左側通行なので、車線も歩道も進む向きの左に寄せる。車線の番号は `Network::link_lanes` の
//! 並びの順で、0 が道路の端にいちばん近い。

use super::pos::link_geometry;
use crate::network::{LANE_WIDTH_MM, LaneMode, LinkId, Network, SIDEWALK_WIDTH_MM};
use crate::units::Mm;

/// 新しいリンクに入ってから、前のリンクでのずれから今のずれへ移りきるまでの距離（mm）。
/// 交差点で曲がる車や車線を移る車が、横に飛ばないようにする。
pub const BLEND_MM: i64 = 3_000;

/// リンクの上で、進む向きの左へ中心線から離れる距離（mm）。
/// 車は `index` 番目の車線の中心、歩く人は歩道の中心までである。
pub fn lateral_mm(net: &Network, link: LinkId, index: usize) -> i64 {
    let Some(segment) = net.segment(link.segment) else {
        return 0;
    };
    let per_side = i64::from(segment.kind.car_lanes_per_side());
    match link.mode {
        LaneMode::Car => {
            let index = i64::try_from(index)
                .unwrap_or(i64::MAX)
                .min((per_side - 1).max(0));
            (per_side - index) * LANE_WIDTH_MM - LANE_WIDTH_MM / 2
        }
        LaneMode::Sidewalk => per_side * LANE_WIDTH_MM + SIDEWALK_WIDTH_MM / 2,
    }
}

/// 進む向き `step` の左へ `d` だけ離れた点の、縦横の成分。斜めは √2 で割る。
pub fn left_of(step: (i8, i8), d: i64) -> (i64, i64) {
    let (sx, sy) = (i64::from(step.0), i64::from(step.1));
    if sx != 0 && sy != 0 {
        // 1 / √2 = 0.70711 を整数で掛ける。
        let c = d * 70_711 / 100_000;
        (c * sy, -c * sx)
    } else {
        (d * sy, -d * sx)
    }
}

/// リンク `link` を `step` の向きに `offset` だけ進んだ移動体の、中心線からのずれ。
/// リンクの始まりの近くでは、直前のリンク `prev` でのずれから少しずつ移す。
pub fn shift(
    net: &Network,
    link: LinkId,
    step: (i8, i8),
    index: usize,
    prev: Option<LinkId>,
    offset: u32,
) -> (Mm, Mm) {
    let cur = left_of(step, lateral_mm(net, link, index));
    let at = i64::from(offset);
    let blended = match prev {
        Some(prev) if at < BLEND_MM => link_geometry(net, prev).map_or(cur, |(_, prev_step, _)| {
            let before = left_of(prev_step, lateral_mm(net, prev, index));
            let mix = |b: i64, c: i64| (b * (BLEND_MM - at) + c * at) / BLEND_MM;
            (mix(before.0, cur.0), mix(before.1, cur.1))
        }),
        _ => cur,
    };
    (Mm(blended.0), Mm(blended.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::RoadKind;
    use crate::network::{Dir, Line};
    use crate::tile::TilePos;

    fn build(kind: RoadKind, from: (i64, i64), to: (i64, i64)) -> (Network, LinkId, LinkId) {
        let mut n = Network::new();
        let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
        n.build_road(&line, kind).unwrap();
        let (segment, _) = n.segments().next().unwrap();
        let link = |dir, mode| LinkId { segment, dir, mode };
        (
            n,
            link(Dir::Forward, LaneMode::Car),
            link(Dir::Forward, LaneMode::Sidewalk),
        )
    }

    #[test]
    fn lanes_sit_left_of_the_center_line_from_the_edge_inward() {
        // 片側 1 車線は、中心線から車線の半分だけ左にある。
        let (n, car, walk) = build(RoadKind::Small, (0, 0), (5, 0));
        assert_eq!(lateral_mm(&n, car, 0), 800);
        assert_eq!(lateral_mm(&n, walk, 0), 1_600 + 400);
        // 片側 3 車線は、端の車線が中心線からいちばん遠い。
        let (n, car, walk) = build(RoadKind::Large, (0, 0), (5, 0));
        let lanes: Vec<i64> = (0..3).map(|i| lateral_mm(&n, car, i)).collect();
        assert_eq!(lanes, [4_000, 2_400, 800]);
        assert_eq!(lateral_mm(&n, walk, 0), 3 * 1_600 + 400);
        // 範囲を越えた番号は、端の車線に丸める。
        assert_eq!(lateral_mm(&n, car, 9), 800);
    }

    #[test]
    fn left_hand_traffic_keeps_left_on_the_screen() {
        // 画面は y が下向きなので、右（+x）へ進む車の左は上（-y）である。
        assert_eq!(left_of((1, 0), 800), (0, -800));
        assert_eq!(left_of((-1, 0), 800), (0, 800));
        assert_eq!(left_of((0, 1), 800), (800, 0));
        assert_eq!(left_of((0, -1), 800), (-800, 0));
        // 斜めは長さが √2 で割られる。
        assert_eq!(left_of((1, 1), 1_000), (707, -707));
    }

    #[test]
    fn up_and_down_lanes_are_on_opposite_sides() {
        let (n, car, _) = build(RoadKind::Medium, (0, 0), (5, 0));
        let back = LinkId {
            dir: Dir::Backward,
            ..car
        };
        let (_, fwd_step, _) = link_geometry(&n, car).unwrap();
        let (_, back_step, _) = link_geometry(&n, back).unwrap();
        let f = shift(&n, car, fwd_step, 1, None, 5_000);
        let b = shift(&n, back, back_step, 1, None, 5_000);
        assert_eq!((f.0, b.0), (Mm(0), Mm(0)));
        assert_eq!(f.1.0, -b.1.0);
        assert_ne!(f.1, Mm(0));
    }

    #[test]
    fn shift_moves_smoothly_into_a_new_link_after_a_turn() {
        let mut n = Network::new();
        for (a, b) in [((0, 0), (5, 0)), ((5, 0), (5, 5))] {
            let line = Line::between(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1)).unwrap();
            n.build_road(&line, RoadKind::Small).unwrap();
        }
        let links: Vec<LinkId> = n
            .links()
            .filter(|l| l.dir == Dir::Forward && l.mode == LaneMode::Car)
            .collect();
        let (first, second) = (links[0], links[1]);
        let step = link_geometry(&n, second).unwrap().1;
        assert_eq!(step, (0, 1));
        // 入った直後は、前のリンクでのずれ（右へ進む車の左 = 上）と同じで、横に飛ばない。
        let enter = shift(&n, second, step, 0, Some(first), 0);
        assert_eq!(enter, (Mm(0), Mm(-800)));
        // 半分進むと、前と今のずれの真ん中になる。
        let mid = shift(&n, second, step, 0, Some(first), BLEND_MM as u32 / 2);
        assert_eq!(mid, (Mm(400), Mm(-400)));
        // 移りきると、下へ進む車の左（+x）の車線に着く。
        let done = shift(&n, second, step, 0, Some(first), BLEND_MM as u32);
        assert_eq!(done, (Mm(800), Mm(0)));
    }
}
