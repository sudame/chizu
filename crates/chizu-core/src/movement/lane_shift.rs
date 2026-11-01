//! 画面に描く移動体の、車線や歩道に合わせた横のずれ。
//!
//! 移動体の位置は区間の中心線の上にある。画面ではそこに足して、車は走っている車線の中心に、
//! 歩く人は歩道の上に描く。描画のためだけの値で、状態にも状態ハッシュにも入れない。
//!
//! 左側通行なので、車は進む向きの左の車線を走る。歩道は進む向きの右にある。建物の出入口の
//! 歩道（`world/growth.rs` の `frontage`）が、その向きに進む人の右側だからである。
//! 車線の番号は `Network::link_lanes` の並びの順で、0 が道路の端にいちばん近い。

use crate::network::{LaneMode, LinkId, Network};

/// 新しいリンクに入ってから、前のずれから今のずれへ移りきるまでの距離（mm）。
/// 交差点で曲がる車や車線を移る車が、横に飛ばないようにする。
pub const BLEND_MM: i64 = 3_000;

/// 車線や歩道が変わったときの、変わる前のずれと、変わった位置のリンクの中での距離。
/// 描画のためだけの値で、状態ハッシュには入れない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blend {
    pub from: (i64, i64),
    pub start: u32,
}

/// リンクの上で、進む向きの左へ中心線から離れる距離（mm）。右なら負になる。
/// 車は `index` 番目の車線の中心、歩く人は歩道の中心までである。歩道は右側にある。
pub fn lateral_mm(net: &Network, link: LinkId, index: usize) -> i64 {
    let Some(segment) = net.segment(link.segment) else {
        return 0;
    };
    let kind = segment.kind;
    let per_side = i64::from(kind.car_lanes_per_side());
    let lane = kind.lane_width_mm();
    match link.mode {
        LaneMode::Car => {
            let index = i64::try_from(index)
                .unwrap_or(i64::MAX)
                .min((per_side - 1).max(0));
            (per_side - index) * lane - lane / 2
        }
        LaneMode::Sidewalk => -(per_side * lane + kind.sidewalk_width_mm() / 2),
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

/// リンク `link` を `step` の向きに走る、車線 `index` のずれ。
pub fn target(net: &Network, link: LinkId, step: (i8, i8), index: usize) -> (i64, i64) {
    left_of(step, lateral_mm(net, link, index))
}

/// リンクの始まりから `offset` だけ進んだ所のずれ。`blend` があれば、そこから `target` へ少しずつ移す。
pub fn blended(target: (i64, i64), blend: Option<Blend>, offset: u32) -> (i64, i64) {
    let Some(b) = blend else {
        return target;
    };
    let at = (i64::from(offset) - i64::from(b.start)).clamp(0, BLEND_MM);
    let mix = |from: i64, to: i64| (from * (BLEND_MM - at) + to * at) / BLEND_MM;
    (mix(b.from.0, target.0), mix(b.from.1, target.1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::RoadKind;
    use crate::network::{Dir, Line};
    use crate::tile::TilePos;

    fn build(kind: RoadKind) -> (Network, LinkId, LinkId) {
        let mut n = Network::new();
        let line = Line::between(TilePos::new(0, 0), TilePos::new(5, 0)).unwrap();
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
    fn every_road_kind_fits_in_one_tile() {
        use crate::network::SegmentKind;
        // 半分の幅（mm）は小 0.3、中 0.4、大 0.5、高速 0.5 マス。道路 1 本は 1 マス（8,000mm）に収まる。
        for (kind, half) in [
            (SegmentKind::Road(RoadKind::Small), 2_400),
            (SegmentKind::Road(RoadKind::Medium), 3_200),
            (SegmentKind::Road(RoadKind::Large), 4_000),
            (SegmentKind::Highway, 4_000),
        ] {
            let lanes = i64::from(kind.car_lanes_per_side());
            assert_eq!(lanes * kind.lane_width_mm() + kind.sidewalk_width_mm(), half);
        }
    }

    #[test]
    fn lanes_run_from_the_edge_inward_and_sidewalks_are_on_the_right() {
        let (n, car, walk) = build(RoadKind::Large);
        let lanes: Vec<i64> = (0..3).map(|i| lateral_mm(&n, car, i)).collect();
        assert_eq!(lanes, [2_800, 1_680, 560]);
        assert_eq!(lateral_mm(&n, car, 9), 560);
        assert_eq!(lateral_mm(&n, walk, 0), -(3 * 1_120 + 320));
        let (n, car, walk) = build(RoadKind::Small);
        assert_eq!(lateral_mm(&n, car, 0), 880);
        assert_eq!(lateral_mm(&n, walk, 0), -(1_760 + 320));
    }

    #[test]
    fn left_hand_traffic_keeps_left_on_the_screen() {
        // 画面は y が下向きなので、右（+x）へ進む車の左は上（-y）である。
        assert_eq!(left_of((1, 0), 800), (0, -800));
        assert_eq!(left_of((-1, 0), 800), (0, 800));
        assert_eq!(left_of((0, 1), 800), (800, 0));
        assert_eq!(left_of((0, -1), 800), (-800, 0));
        assert_eq!(left_of((1, 1), 1_000), (707, -707));
        // 負なら右になる。
        assert_eq!(left_of((1, 0), -800), (0, 800));
    }

    #[test]
    fn up_and_down_lanes_and_sidewalks_are_on_opposite_sides() {
        let (n, car, walk) = build(RoadKind::Medium);
        let back = |l: LinkId| LinkId {
            dir: Dir::Backward,
            ..l
        };
        let east = (1, 0);
        let west = (-1, 0);
        let f = target(&n, car, east, 1);
        let b = target(&n, back(car), west, 1);
        assert_eq!(f.1, -b.1);
        assert_ne!(f.1, 0);
        let f = target(&n, walk, east, 0);
        let b = target(&n, back(walk), west, 0);
        assert_eq!(f.1, -b.1);
        // 車は左の（北の）車線、歩く人は右の（南の）歩道にいる。
        assert!(target(&n, car, east, 0).1 < 0);
        assert!(target(&n, walk, east, 0).1 > 0);
    }

    #[test]
    fn blending_starts_at_the_old_shift_and_ends_at_the_new_one() {
        let b = Blend {
            from: (0, -800),
            start: 100,
        };
        let to = (800, 0);
        assert_eq!(blended(to, Some(b), 100), (0, -800));
        assert_eq!(blended(to, Some(b), 100 + BLEND_MM as u32 / 2), (400, -400));
        assert_eq!(blended(to, Some(b), 100 + BLEND_MM as u32), to);
        assert_eq!(blended(to, Some(b), 50), (0, -800));
        assert_eq!(blended(to, None, 0), to);
    }
}
