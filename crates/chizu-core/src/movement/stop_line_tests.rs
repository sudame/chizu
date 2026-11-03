use super::stop_line::{clear_line, exit_line, stop_line};
use super::*;
use crate::command::{Heading, RoadKind};
use crate::network::Line;

fn road(n: &mut Network, from: (i64, i64), to: (i64, i64), kind: RoadKind) {
    let line = Line::between(TilePos::new(from.0, from.1), TilePos::new(to.0, to.1)).unwrap();
    n.build_road(&line, kind).unwrap();
}

fn at(x: i64, y: i64, step: (i8, i8), along: u32) -> SidewalkPos {
    SidewalkPos {
        tile: TilePos::new(x, y),
        step,
        along: LaneOffset(along),
    }
}

/// マス `a` から隣のマス `b` へ向かう車のリンク。
fn link(n: &Network, a: (i64, i64), b: (i64, i64)) -> LinkId {
    let step = ((b.0 - a.0).signum() as i8, (b.1 - a.1).signum() as i8);
    at(a.0, a.1, step, 1)
        .resolve_on(n, LaneMode::Car)
        .unwrap()
        .0
}

fn len(n: &Network, l: LinkId) -> u32 {
    pos::link_geometry(n, l).unwrap().2.0
}

#[test]
fn the_stop_line_sits_behind_the_crosswalk() {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 20), RoadKind::Medium);
    let east = link(&n, (9, 10), (10, 10));
    // 中くらいの道路の端 3.2m、横断歩道 1.28m、すき間 0.4m、停止線 0.32m、車の長さの半分 2.25m。
    let back = 3_200 + 1_280 + 400 + 320 + CAR_LENGTH / 2;
    assert_eq!(stop_line(&n, east).0, len(&n, east) - back);
    // 行き止まりから始まるリンクに出口はなく、交差点から出るリンクの出口は道路の端である。
    assert_eq!(exit_line(&n, east).0, 0);
    let out = link(&n, (10, 10), (11, 10));
    assert_eq!(exit_line(&n, out).0, 3_200);
    assert_eq!(stop_line(&n, out).0, len(&n, out));
    assert_eq!(clear_line(&n, out), 3_200);
    // 横断歩道を外した腕では、縞の分だけ前で待つ。
    let node = n.node_at(TilePos::new(10, 10)).unwrap();
    n.set_crosswalk(node, Heading::W, false).unwrap();
    assert_eq!(stop_line(&n, east).0, len(&n, east) - back + 1_280);
}

#[test]
fn a_short_link_waits_at_the_exit_of_the_junction_behind() {
    let mut n = Network::new();
    road(&mut n, (0, 10), (20, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 20), RoadKind::Medium);
    road(&mut n, (11, 0), (11, 20), RoadKind::Medium);
    let short = link(&n, (10, 10), (11, 10));
    assert_eq!(len(&n, short), 8_000);
    // 停止線は手前の交差点の出口より手前になるので、出口で待つ。手前の交差点の中では待たない。
    assert_eq!(stop_line(&n, short).0, 3_200);
    assert_eq!(exit_line(&n, short).0, 3_200);
}

#[test]
fn diagonal_arms_and_bends_measure_along_the_arm() {
    let mut n = Network::new();
    road(&mut n, (0, 0), (10, 10), RoadKind::Medium);
    road(&mut n, (10, 10), (20, 10), RoadKind::Medium);
    // (10, 10) は、斜めの区間から東西の区間へ向きを変える角である。
    let diag = link(&n, (9, 9), (10, 10));
    let edge = 3_200 * crate::network::DIAGONAL_TILE_MM / crate::network::TILE_MM;
    assert_eq!(
        stop_line(&n, diag).0,
        len(&n, diag) - u32::try_from(edge).unwrap()
    );
    let out = link(&n, (10, 10), (11, 10));
    assert_eq!(exit_line(&n, out).0, 3_200);
}

#[test]
fn the_lane_chosen_at_the_stop_line_leads_to_the_next_turn() {
    // 東西の中くらいの道路に、x=10 と x=20 で南北の道路が交わる。西から来た車が x=10 をまっすぐ抜け、
    // x=20 で北か南へ曲がる。x=10 の停止線で決めた車線は、x=20 で曲がる先へつながる。
    let mut n = Network::new();
    road(&mut n, (0, 10), (30, 10), RoadKind::Medium);
    road(&mut n, (10, 0), (10, 20), RoadKind::Medium);
    road(&mut n, (20, 0), (20, 20), RoadKind::Medium);
    let mut router = AstarRouter::new(&n);
    let mut trips = Trips::default();
    let ids: Vec<TripId> = [at(20, 4, (0, -1), 0), at(20, 16, (0, 1), 0)]
        .into_iter()
        .map(|to| {
            trips.spawn_drives(&n, &mut router, &[(at(2, 10, (1, 0), 0), to)], Tick::ZERO)[0]
                .unwrap()
        })
        .collect();
    let mid = link(&n, (10, 10), (11, 10));
    let mut chosen: Vec<Option<LaneId>> = vec![None; ids.len()];
    for _ in 0..200 {
        trips.step(&n, &mut router, Tick::ZERO);
        for (k, id) in ids.iter().enumerate() {
            let Some(t) = trips.get(*id) else {
                continue;
            };
            // 停止線を越えたまま x=10 の交差点の中にいれば、決めた車線を見る。越えたあとは、入った車線を見る。
            let next = t.route.links.get(t.leg as usize + 1).copied();
            let lane = if next == Some(mid) {
                car::bound_lane(&n, t)
            } else {
                (t.link() == mid).then_some(t.lane)
            };
            let Some(lane) = lane else {
                continue;
            };
            let after = *t
                .route
                .links
                .iter()
                .skip_while(|l| **l != mid)
                .nth(1)
                .unwrap();
            let node = car::end_node(&n, mid);
            let reaches = car::from_lane(router.connectors(), node, lane)
                .iter()
                .any(|c| {
                    n.lane(c.to)
                        .is_some_and(|l| (l.segment, l.dir) == (after.segment, after.dir))
                });
            assert!(reaches, "選んだ車線 {lane:?} が曲がる先へつながらない");
            // 停止線で決めた車線と、ノードを越えて入った車線は同じである。
            assert!(chosen[k].is_none_or(|c| c == lane), "車線が変わった");
            chosen[k] = Some(lane);
        }
    }
    assert!(chosen.iter().all(Option::is_some));
    // 北へ曲がる車と南へ曲がる車は、別の車線を選ぶ。
    assert_ne!(chosen[0], chosen[1]);
    assert!(trips.is_empty(), "着かなかった");
}
