use super::*;
use crate::chunk::TileUse;
use crate::command::{Command, RoadKind};
use crate::record::{CommandLog, replay};
use crate::units::Tick;
use crate::world::{ApplyOutcome, World};

fn at(x: i64, y: i64) -> TilePos {
    TilePos::new(x, y)
}

fn line(fx: i64, fy: i64, tx: i64, ty: i64) -> Line {
    Line::between(at(fx, fy), at(tx, ty)).unwrap()
}

fn build(r: &mut Rail, fx: i64, fy: i64, tx: i64, ty: i64) -> Result<(), DenyReason> {
    r.build_rail(&line(fx, fy, tx, ty), &BTreeSet::new())
}

fn degree(r: &Rail, x: i64, y: i64) -> usize {
    let n = r.node_at(at(x, y)).expect("ノードがない");
    r.node(n).unwrap().tracks.len()
}

/// 網の決まりと、索引の食い違いがないことを確かめる。曲がりの決まり（`arms_ok`）は、引いた網だけが満たす
/// （取り壊しで分岐の根元を消すと、V 字のノードが残る）ので、`turns` が真のときだけ確かめる。
fn check_invariants(r: &Rail) {
    check_network(r, true);
}

fn check_network(r: &Rail, turns: bool) {
    let mut tiles = BTreeMap::<TilePos, Vec<TrackId>>::new();
    let mut steps = BTreeSet::new();
    for (id, tr) in r.tracks() {
        let l = r.track_line(id).unwrap();
        let max = block_steps(l.is_diagonal());
        assert!(l.len <= max, "ノードのあいだが長すぎる {l:?}");
        for t in l.tiles() {
            tiles.entry(t).or_default().push(id);
        }
        for k in 0..l.len {
            let (p, q) = (l.tile(k), l.tile(k + 1));
            assert!(steps.insert((p.min(q), p.max(q))), "1 歩の重なり {p:?}");
        }
        for n in [tr.a, tr.b] {
            assert!(r.node(n).unwrap().tracks.contains(&id));
        }
    }
    for (p, q) in &steps {
        if p.x != q.x && p.y != q.y {
            let (a, b) = (at(q.x, p.y), at(p.x, q.y));
            assert!(!steps.contains(&(a.min(b), a.max(b))), "角での交わり {p:?}");
        }
    }
    for (t, ids) in &tiles {
        assert_eq!(r.tracks_through(*t), ids.as_slice());
        // 2 本以上が通るマスは、どの線路にとっても端のノードである。
        if ids.len() > 1 {
            assert!(r.node_at(*t).is_some(), "途中のマスで交わる {t:?}");
        }
    }
    assert_eq!(r.rail_tile_count(), tiles.len());
    let mut seen = BTreeSet::new();
    for (id, n) in r.nodes() {
        assert!(!n.tracks.is_empty(), "つながりのないノード {id:?}");
        assert!(n.tracks.is_sorted());
        assert!(seen.insert(n.tile), "同じマスにノードが 2 つある");
        let arms: Vec<Heading> = n
            .tracks
            .iter()
            .map(|tid| {
                let tr = r.track(*tid).unwrap();
                let other = if tr.a == id { tr.b } else { tr.a };
                let o = r.node(other).unwrap().tile;
                heading_of(((o.x - n.tile.x).signum(), (o.y - n.tile.y).signum()))
            })
            .collect();
        assert!(arms.len() <= MAX_TRACKS_PER_NODE);
        assert!(!turns || arms_ok(&arms), "{id:?} の向き {arms:?}");
    }
}

#[test]
fn arms_allow_up_to_45_degrees_and_one_branch() {
    use Heading::*;
    assert!(arms_ok(&[]));
    assert!(arms_ok(&[N]));
    assert!(arms_ok(&[E, W]));
    assert!(arms_ok(&[E, SW]));
    assert!(arms_ok(&[E, NW]));
    assert!(!arms_ok(&[E, S]));
    assert!(!arms_ok(&[E, SE]));
    // 分岐は、根元から両方へ 45 度までで抜けられる形だけである。
    assert!(arms_ok(&[W, E, SE]));
    assert!(arms_ok(&[W, NE, SE]));
    assert!(!arms_ok(&[W, E, S]));
    assert!(!arms_ok(&[N, E, S]));
    assert!(!arms_ok(&[W, E, NE, SE]));
}

#[test]
fn a_straight_rail_makes_two_nodes_and_one_track() {
    let mut r = Rail::new();
    assert!(r.is_unused());
    build(&mut r, 0, 0, 10, 0).unwrap();
    assert!(!r.is_unused());
    assert_eq!(r.tracks().count(), 1);
    assert_eq!(r.nodes().count(), 2);
    assert_eq!(r.rail_tile_count(), 11);
    assert!(r.is_rail_tile(at(5, 0)));
    assert!(!r.is_rail_tile(at(5, 1)));
    check_invariants(&r);
}

#[test]
fn long_rails_get_block_nodes_at_equal_spacing() {
    let mut r = Rail::new();
    build(&mut r, 0, 0, 120, 0).unwrap();
    let mut tiles: Vec<i64> = r.nodes().map(|(_, n)| n.tile.x).collect();
    tiles.sort();
    assert_eq!(tiles, [0, 40, 80, 120]);
    check_invariants(&r);

    // 斜めは 400m を超えない 35 歩ごとに区切る。
    assert_eq!(block_steps(true), 35);
    let mut r = Rail::new();
    build(&mut r, 0, 0, 100, 100).unwrap();
    let mut xs: Vec<i64> = r.nodes().map(|(_, n)| n.tile.x).collect();
    xs.sort();
    assert_eq!(xs, [0, 33, 66, 100]);
    check_invariants(&r);

    // ちょうど 50 マスなら区切らない。
    let mut r = Rail::new();
    build(&mut r, 0, 0, 0, 50).unwrap();
    assert_eq!(r.nodes().count(), 2);

    // いちばん長い 1,024 マスは、50 マス以下の 21 本になる。
    let mut r = Rail::new();
    build(&mut r, 0, 0, 1_024, 0).unwrap();
    assert_eq!(r.tracks().count(), 21);
    check_invariants(&r);
}

#[test]
fn block_nodes_are_placed_between_the_cuts() {
    // 既存の線路に当たるマスで切ってから、それぞれを区切る。
    let mut r = Rail::new();
    build(&mut r, 0, 0, 30, 0).unwrap();
    build(&mut r, 30, 0, 100, 0).unwrap();
    let mut xs: Vec<i64> = r.nodes().map(|(_, n)| n.tile.x).collect();
    xs.sort();
    assert_eq!(xs, [0, 30, 65, 100]);
    check_invariants(&r);
}

#[test]
fn rails_join_at_ends_and_turn_up_to_45_degrees() {
    let mut r = Rail::new();
    build(&mut r, 0, 0, 10, 0).unwrap();
    build(&mut r, 10, 0, 20, 0).unwrap();
    assert_eq!(degree(&r, 10, 0), 2);
    build(&mut r, 20, 0, 30, 10).unwrap();
    assert_eq!(degree(&r, 20, 0), 2);
    // 南東へ来た線路は、南か東へ 45 度曲げられるが、北東（90 度）と西（135 度）へは曲げられない。
    let before = r.clone();
    assert_eq!(build(&mut r, 30, 10, 40, 0), Err(DenyReason::RailTooSharp));
    assert_eq!(build(&mut r, 30, 10, 20, 10), Err(DenyReason::RailTooSharp));
    assert_eq!(r, before);
    assert_eq!(build(&mut r, 30, 10, 30, 20), Ok(()));
    check_invariants(&r);
    let mut r = before;
    assert_eq!(build(&mut r, 30, 10, 40, 10), Ok(()));
    check_invariants(&r);
}

#[test]
fn a_rail_from_the_middle_of_a_track_makes_one_branch() {
    let mut r = Rail::new();
    build(&mut r, 0, 0, 20, 0).unwrap();
    // 途中のマスから 45 度で分かれる線路は、既存の線路を分けて分岐にする。
    build(&mut r, 10, 0, 20, 10).unwrap();
    assert_eq!(degree(&r, 10, 0), 3);
    assert_eq!(r.tracks().count(), 3);
    check_invariants(&r);
    // 2 つ目の分岐は置けない。
    let before = r.clone();
    assert_eq!(
        build(&mut r, 10, 0, 20, -10),
        Err(DenyReason::RailTooManyBranches)
    );
    assert_eq!(r, before);
    // 途中のマスに直角に当たる線路と、十字に交わる線路は断る。
    let mut r = Rail::new();
    build(&mut r, 0, 0, 20, 0).unwrap();
    let before = r.clone();
    assert_eq!(build(&mut r, 10, 5, 10, 0), Err(DenyReason::RailTooSharp));
    assert_eq!(
        build(&mut r, 10, 5, 10, -5),
        Err(DenyReason::RailTooManyBranches)
    );
    // 途中のマスを 45 度で通り抜ける線路も、十字になるので断る。
    assert_eq!(
        build(&mut r, 5, 5, 15, -5),
        Err(DenyReason::RailTooManyBranches)
    );
    assert_eq!(r, before);
    // 端のノードから 2 本に分かれる形も、根元から両方へ 45 度までで抜けられればよい。
    build(&mut r, 20, 0, 30, -10).unwrap();
    assert_eq!(
        build(&mut r, 20, 0, 30, 10),
        Ok(()),
        "根元（西）から北東と南東へ抜けられる"
    );
    check_invariants(&r);
}

#[test]
fn overlapping_and_corner_crossing_rails_are_denied() {
    let mut r = Rail::new();
    build(&mut r, 0, 0, 20, 0).unwrap();
    build(&mut r, 0, 10, 3, 13).unwrap();
    let before = r.clone();
    assert_eq!(build(&mut r, 5, 0, 15, 0), Err(DenyReason::Overlap));
    assert_eq!(build(&mut r, 25, 0, 15, 0), Err(DenyReason::Overlap));
    // (1, 11) と (2, 12) を結ぶ線路と、(2, 11) と (1, 12) の角で交わる。
    assert_eq!(build(&mut r, 3, 10, 0, 13), Err(DenyReason::CornerCrossing));
    assert_eq!(r, before);
    // 道路の側から角の交わりを調べる。
    assert!(r.crosses_corner(&line(3, 10, 0, 13)));
    assert!(!r.crosses_corner(&line(4, 10, 1, 13)));
    assert!(!r.crosses_corner(&line(0, 5, 10, 5)));
}

#[test]
fn bulldozing_removes_whole_tracks_and_lonely_nodes() {
    let mut r = Rail::new();
    build(&mut r, 0, 0, 20, 0).unwrap();
    build(&mut r, 10, 0, 20, 10).unwrap();
    // 範囲にかかった区間（ノードからノードまで）ごと消える。
    let removed = r.bulldoze(TileRect::from_corners(at(3, -1), at(3, 1)));
    assert_eq!(removed.tracks, vec![line(0, 0, 10, 0)]);
    assert!(removed.stations.is_empty());
    assert!(r.node_at(at(0, 0)).is_none());
    // 分岐の根元を消すと、45 度で開いた V 字のノードが残る。列車はここで折り返せない。
    assert_eq!(degree(&r, 10, 0), 2);
    check_network(&r, false);
    // V 字のノードからは、どちらへ引いても曲がりの決まりに合わない。根元を引き直せば分岐に戻る。
    let before = r.clone();
    assert_eq!(build(&mut r, 10, 0, 10, -10), Err(DenyReason::RailTooSharp));
    assert_eq!(r, before);
    assert_eq!(build(&mut r, 10, 0, 0, 0), Ok(()));
    check_invariants(&r);
    assert_eq!(build(&mut r, 0, 0, 10, 0), Err(DenyReason::Overlap));
    let mut r = before;
    let removed = r.bulldoze(TileRect::from_corners(at(30, 30), at(40, 40)));
    assert!(removed.is_empty());
    let removed = r.bulldoze(TileRect::from_corners(at(-100, -100), at(100, 100)));
    assert_eq!(removed.tracks.len(), 2);
    assert_eq!(r.nodes().count(), 0);
    assert_eq!(r.rail_tile_count(), 0);
    // 消したあとも、ID の空き番号が残るので使ったことのある網である。
    assert!(!r.is_unused());
}

#[test]
fn rail_cost_is_per_tile_and_longer_on_diagonals() {
    assert_eq!(rail_cost(&line(0, 0, 10, 0)), Money(15_000));
    // 斜め 1 歩は 11,314 mm なので、10 歩で 21,214 を切り上げる。
    assert_eq!(rail_cost(&line(0, 0, 10, 10)), Money(21_214));
}

// ここから下は、街の中で線路と駅を置く決まりを確かめる。

fn apply(w: &mut World, c: Command) -> ApplyOutcome {
    w.apply(&c)
}

fn road(w: &mut World, from: (i64, i64), to: (i64, i64)) -> ApplyOutcome {
    apply(
        w,
        Command::BuildRoad {
            from: at(from.0, from.1),
            to: at(to.0, to.1),
            kind: RoadKind::Small,
        },
    )
}

fn rail(w: &mut World, from: (i64, i64), to: (i64, i64)) -> ApplyOutcome {
    apply(
        w,
        Command::BuildRail {
            from: at(from.0, from.1),
            to: at(to.0, to.1),
        },
    )
}

/// 駅を (12, 9) に、y = 10 の道路を向いて置く。敷地は x 3〜12、y 7〜9 で、駅の線路は y = 8 の x 3〜12 である。
fn station(w: &mut World) -> ApplyOutcome {
    apply(
        w,
        Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: at(12, 9),
            rotation: 2,
        },
    )
}

fn tile_use(w: &World, x: i64, y: i64) -> TileUse {
    let t = at(x, y);
    let chunk = w.chunk(t.chunk().unwrap()).unwrap();
    let (lx, ly) = t.local();
    chunk.layer(Layer::Ground).unwrap().occupancy[crate::tile::local_index(lx, ly)]
}

/// 原点のまわりが陸で、y = 10 に道路を引いた街。
fn street() -> World {
    let mut w = World::new(0);
    for y in -20..=30 {
        for x in -20..=60 {
            assert!(w.is_buildable(at(x, y)), "({x}, {y}) が水辺");
        }
    }
    assert_eq!(road(&mut w, (0, 10), (40, 10)), ApplyOutcome::Applied);
    w
}

#[test]
fn building_a_rail_charges_and_marks_the_tiles() {
    let mut w = street();
    let money = w.money();
    let hash = w.state_hash();
    assert_eq!(rail(&mut w, (0, 0), (40, 0)), ApplyOutcome::Applied);
    assert_eq!(w.money(), money - Money(60_000));
    assert_ne!(w.state_hash(), hash);
    for x in 0..=40 {
        assert_eq!(tile_use(&w, x, 0), TileUse::Rail);
    }
    assert_eq!(tile_use(&w, 0, 1), TileUse::Empty);
    // 維持費は道路と同じく建設費の 1%（1 日）。
    assert_eq!(w.finances().rail_upkeep, Money(600));
    let broke = World::new_with_money(0, Money(1_000));
    let mut broke = broke;
    let before = broke.clone();
    assert_eq!(
        rail(&mut broke, (0, 0), (40, 0)),
        ApplyOutcome::Denied(DenyReason::NotEnoughMoney)
    );
    assert_eq!(broke, before);
}

#[test]
fn rails_and_roads_share_a_tile_only_at_a_crossing() {
    let mut w = street();
    let before = w.clone();
    // 道路に沿う線路と、道路の端（ノード）で始まる線路は、踏切にならないので断る。
    assert_eq!(
        rail(&mut w, (5, 10), (15, 10)),
        ApplyOutcome::Denied(DenyReason::Occupied)
    );
    assert_eq!(
        rail(&mut w, (40, 5), (40, 15)),
        ApplyOutcome::Denied(DenyReason::NodeOnCrossing)
    );
    assert_eq!(w, before);
    // 道路の区間の途中を横切る線路は、踏切になる。
    assert_eq!(rail(&mut w, (20, 5), (20, 15)), ApplyOutcome::Applied);
    assert_eq!(tile_use(&w, 20, 10), TileUse::Crossing);
    assert_eq!(rail(&mut w, (0, 0), (40, 0)), ApplyOutcome::Applied);
    let before = w.clone();
    // 線路のマスで終わる道路は、そこに道路のノードを置くので断る。
    assert_eq!(
        road(&mut w, (30, -5), (30, 0)),
        ApplyOutcome::Denied(DenyReason::NodeOnCrossing)
    );
    assert_eq!(w, before);
    // 斜めどうしがマスの角で交わる置き方も、どちらの順でも断る。
    assert_eq!(road(&mut w, (50, 20), (55, 25)), ApplyOutcome::Applied);
    let before = w.clone();
    assert_eq!(
        rail(&mut w, (53, 20), (50, 23)),
        ApplyOutcome::Denied(DenyReason::CornerCrossing)
    );
    assert_eq!(w, before);
    assert_eq!(rail(&mut w, (50, -20), (55, -15)), ApplyOutcome::Applied);
    let before = w.clone();
    assert_eq!(
        road(&mut w, (53, -20), (50, -17)),
        ApplyOutcome::Denied(DenyReason::CornerCrossing)
    );
    assert_eq!(w, before);
}

#[test]
fn rails_off_the_owned_land_or_on_water_are_denied() {
    let mut w = World::new(0);
    assert_eq!(
        rail(&mut w, (120, 0), (130, 0)),
        ApplyOutcome::Denied(DenyReason::NotOwned)
    );
    let water = (-64..128)
        .flat_map(|y| (-64..128).map(move |x| at(x, y)))
        .find(|t| w.check_buildable(*t) == Err(DenyReason::Water));
    if let Some(t) = water {
        assert_eq!(
            rail(&mut w, (t.x, t.y), (t.x, t.y + 1)),
            ApplyOutcome::Denied(DenyReason::Water)
        );
    }
}

#[test]
fn a_rail_clears_the_zone_and_the_buildings_on_its_tiles() {
    let mut w = street();
    let paint = Command::PaintZone {
        area: TileRect::from_corners(at(0, 4), at(40, 9)),
        zone: Some(crate::command::Zone::Residential),
    };
    assert_eq!(apply(&mut w, paint), ApplyOutcome::Applied);
    assert!(w.zone_at(at(5, 6)).is_some());
    assert_eq!(rail(&mut w, (0, 6), (40, 6)), ApplyOutcome::Applied);
    assert_eq!(w.zone_at(at(5, 6)), None);
    assert!(w.zone_at(at(5, 7)).is_some());
}

#[test]
fn a_station_sits_in_the_rail_network_not_in_the_facilities() {
    let mut w = street();
    let money = w.money();
    assert_eq!(station(&mut w), ApplyOutcome::Applied);
    assert_eq!(w.money(), money - Money(40_000));
    assert!(w.facilities().is_empty());
    let (id, s) = w.rail().stations().next().unwrap();
    assert_eq!(s.area, TileRect::from_corners(at(3, 7), at(12, 9)));
    assert!(s.entrance.is_some());
    assert_eq!(w.rail().station_at(at(5, 7)), Some(id));
    // 駅の線路は真ん中の列の両端を結ぶ。
    let line = w.rail().track_line(s.track).unwrap();
    let ends = BTreeSet::from([line.start, line.end()]);
    assert_eq!(ends, BTreeSet::from([at(3, 8), at(12, 8)]));
    assert_eq!(w.rail().track(s.track).unwrap().station, Some(id));
    for y in 7..=9 {
        for x in 3..=12 {
            assert_eq!(tile_use(&w, x, y), TileUse::Station, "({x}, {y})");
        }
    }
    // 出入口は施設と同じ決まりで求める。
    assert!(crate::movement::facility_spot(w.network(), &s.site()).is_some());
    assert_eq!(
        w.finances().rail_upkeep,
        ServiceKind::Station.upkeep_per_day()
    );
    // 駅の上には、施設も駅も道路も置けない。
    let before = w.clone();
    let plant = Command::PlaceBuilding {
        kind: ServiceKind::WaterPump,
        at: at(6, 9),
        rotation: 2,
    };
    assert_eq!(
        apply(&mut w, plant),
        ApplyOutcome::Denied(DenyReason::Occupied)
    );
    assert_eq!(station(&mut w), ApplyOutcome::Denied(DenyReason::Occupied));
    assert_eq!(
        road(&mut w, (5, 0), (5, 8)),
        ApplyOutcome::Denied(DenyReason::Occupied)
    );
    assert_eq!(w, before);
}

#[test]
fn a_station_needs_a_road_and_free_land() {
    let mut w = street();
    let away = Command::PlaceBuilding {
        kind: ServiceKind::Station,
        at: at(12, 0),
        rotation: 2,
    };
    assert_eq!(
        apply(&mut w, away),
        ApplyOutcome::Denied(DenyReason::NoRoadAccess)
    );
    // 線路の上には置けない。先に駅を置いてから線路をつなぐ。
    assert_eq!(rail(&mut w, (0, 7), (8, 7)), ApplyOutcome::Applied);
    let before = w.clone();
    assert_eq!(station(&mut w), ApplyOutcome::Denied(DenyReason::Occupied));
    assert_eq!(w, before);
}

#[test]
fn rails_start_or_end_only_at_the_station_ends() {
    let mut w = street();
    assert_eq!(station(&mut w), ApplyOutcome::Applied);
    let before = w.clone();
    // 駅の中のほかのマスは通れない。
    for (from, to) in [
        ((11, 8), (11, 0)),
        ((5, 2), (5, 8)),
        ((0, 7), (20, 7)),
        ((13, 7), (9, 7)),
        // 駅の端を途中で通り抜ける線路は、駅の中のマスも通るので断る。
        ((-5, 8), (4, 8)),
    ] {
        assert_eq!(
            rail(&mut w, from, to),
            ApplyOutcome::Denied(DenyReason::Occupied),
            "{from:?} → {to:?}"
        );
    }
    assert_eq!(
        rail(&mut w, (12, 8), (14, 8)),
        ApplyOutcome::Applied,
        "端から外へは引ける"
    );
    let mut w2 = before.clone();
    assert_eq!(
        rail(&mut w2, (3, 8), (-10, 8)),
        ApplyOutcome::Applied,
        "もう一方の端からも引ける"
    );
    // 終えるのもよい。45 度の曲がりでつなげる。
    let mut w3 = before.clone();
    assert_eq!(rail(&mut w3, (20, 0), (12, 8)), ApplyOutcome::Applied);
    let end = w3.rail().node_at(at(12, 8)).unwrap();
    assert_eq!(w3.rail().node(end).unwrap().tracks.len(), 2);
    assert_eq!(tile_use(&w3, 12, 8), TileUse::Station);
    assert_eq!(tile_use(&w3, 13, 7), TileUse::Rail);
}

#[test]
fn bulldozing_a_station_removes_its_track_and_keeps_the_rail_outside() {
    let mut w = street();
    assert_eq!(station(&mut w), ApplyOutcome::Applied);
    assert_eq!(rail(&mut w, (12, 8), (30, 8)), ApplyOutcome::Applied);
    let bulldoze = Command::Bulldoze {
        area: TileRect::from_corners(at(5, 9), at(5, 9)),
    };
    assert_eq!(apply(&mut w, bulldoze), ApplyOutcome::Applied);
    assert_eq!(w.rail().stations().count(), 0);
    assert_eq!(w.rail().tracks().count(), 1);
    // 駅の端のマスには、外へ延びる線路のノードが残る。
    assert_eq!(tile_use(&w, 12, 8), TileUse::Rail);
    assert_eq!(tile_use(&w, 11, 8), TileUse::Empty);
    assert_eq!(tile_use(&w, 5, 9), TileUse::Empty);
    // 道路は残る。
    assert!(w.network().is_road_tile(at(5, 10)));
    // 線路だけの範囲も取り壊せる。消すものがなければ断る。
    let bulldoze = Command::Bulldoze {
        area: TileRect::from_corners(at(20, 8), at(20, 8)),
    };
    assert_eq!(apply(&mut w, bulldoze), ApplyOutcome::Applied);
    assert_eq!(w.rail().tracks().count(), 0);
    assert_eq!(tile_use(&w, 12, 8), TileUse::Empty);
    assert_eq!(
        apply(&mut w, bulldoze),
        ApplyOutcome::Denied(DenyReason::NothingToBulldoze)
    );
    assert_eq!(w.finances().rail_upkeep, Money::ZERO);
}

/// 線路と駅を置く記録は、分けて再生しても、書き出して読み戻しても同じ街になる。
#[test]
fn rail_replays_and_round_trips_through_the_snapshot() {
    let commands = [
        Command::BuildRoad {
            from: at(0, 10),
            to: at(40, 10),
            kind: RoadKind::Small,
        },
        Command::PlaceBuilding {
            kind: ServiceKind::Station,
            at: at(12, 9),
            rotation: 2,
        },
        Command::BuildRail {
            from: at(12, 8),
            to: at(100, 8),
        },
        Command::BuildRail {
            from: at(3, 8),
            to: at(-10, -5),
        },
        Command::BuildRail {
            from: at(60, 8),
            to: at(70, -2),
        },
        Command::Bulldoze {
            area: TileRect::from_corners(at(80, 8), at(80, 8)),
        },
    ];
    let mut log = CommandLog::new();
    for (i, c) in commands.into_iter().enumerate() {
        log.record(Tick(i as u64 * 5), c).unwrap();
    }
    let mut whole = World::new(0);
    let stats = replay(&mut whole, &log, 40);
    assert_eq!(stats.applied, 6, "{stats:?}");
    let mut pieces = World::new(0);
    for _ in 0..8 {
        replay(&mut pieces, &log, 5);
    }
    assert_eq!(whole, pieces);
    assert_eq!(whole.state_hash(), pieces.state_hash());

    let mut back = World::from_snapshot(&whole.to_snapshot()).unwrap();
    assert_eq!(back, whole);
    assert_eq!(back.state_hash(), whole.state_hash());
    // 読み戻した街も、続きを同じように進める。
    let more = Command::BuildRail {
        from: at(-10, -5),
        to: at(-10, -30),
    };
    for w in [&mut whole, &mut back] {
        assert_eq!(w.apply(&more), ApplyOutcome::Applied);
        for _ in 0..10 {
            w.step();
        }
    }
    assert_eq!(back, whole);
    assert_eq!(back.state_hash(), whole.state_hash());
}

/// 線路も駅もない街の状態ハッシュには、鉄道の欄を書かない。一度でも置けば、取り壊しても書く。
#[test]
fn the_state_hash_skips_an_unused_rail() {
    use crate::state_hash::StateHasher;
    let w = World::new(0);
    assert!(w.rail().is_unused());
    // 鉄道の欄を書かないハッシュは、鉄道を持たない形で書いたものと同じである。
    let mut h = StateHasher::new();
    w.write_state_without_rail(&mut h);
    assert_eq!(w.state_hash(), h.finish());

    let mut used = w.clone();
    assert_eq!(rail(&mut used, (0, 0), (5, 0)), ApplyOutcome::Applied);
    let bulldoze = Command::Bulldoze {
        area: TileRect::from_corners(at(0, 0), at(5, 0)),
    };
    assert_eq!(apply(&mut used, bulldoze), ApplyOutcome::Applied);
    assert_eq!(used.rail().tracks().count(), 0);
    let mut h = StateHasher::new();
    used.write_state_without_rail(&mut h);
    assert_ne!(used.state_hash(), h.finish());
}

/// 画面が引く前に使う `check_build_rail` は、状態を変えず、適用と同じ答えを返す。
#[test]
fn check_build_rail_agrees_with_applying() {
    let mut w = street();
    let hash = w.state_hash();
    let free = line(0, 3, 40, 3);
    assert_eq!(w.check_build_rail(&free), Ok(rail_cost(&free)));
    // 道路に沿う線路と、お金が足りない街の線路は、適用と同じ理由で断る
    let along = line(5, 10, 15, 10);
    assert_eq!(w.check_build_rail(&along), Err(DenyReason::Occupied));
    let broke = World::new_with_money(0, Money(1_000));
    assert_eq!(
        broke.check_build_rail(&free),
        Err(DenyReason::NotEnoughMoney)
    );
    assert_eq!(w.state_hash(), hash);
    assert_eq!(
        rail(&mut w, (5, 10), (15, 10)),
        ApplyOutcome::Denied(DenyReason::Occupied)
    );
    assert_eq!(rail(&mut w, (0, 3), (40, 3)), ApplyOutcome::Applied);
}
