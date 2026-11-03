//! ゴールデンのハッシュ。
//!
//! 決まった記録を再生し、途中の決まった時点の状態ハッシュの列が `tests/golden/<名前>.hash` と
//! 一致するかを見る。各行は「tick、その時点の状態ハッシュ、tick 0 からその時点までの
//! 毎 tick の状態ハッシュをまとめた値」である。まとめた値があるので、時点と時点のあいだで
//! 起きた違いも見逃さない。最後の行は再生の終わりの時点である。
//! 記録そのものも `tests/golden/<名前>.rec` に置き、書き出しの形式が変わっていないかも見る。
//!
//! シミュレーションや記録の形式を意図して変えたときは、
//! `CHIZU_UPDATE_GOLDEN=1 cargo test -p chizu-core --test golden` でファイルを書き直し、同じ PR に含める。

use std::path::PathBuf;

use chizu_core::movement::signal;

use chizu_core::{
    Activity, BuildingId, ChunkCoord, Command, CommandLog, DenyReason, Heading, LaneMode,
    LaneOffset, Money, RecordedCommand, ReplayStats, RngPurpose, RoadKind, ServiceKind,
    SidewalkPos, SimSpeed, StateHasher, TICKS_PER_DAY, Tick, TilePos, TileRect, World, ZoneKind,
    replay, rng,
};

/// 却下した件数の内訳。挙げなかった理由は 0 件とみなす。
fn denied_by(counts: &[(DenyReason, u64)]) -> [u64; DenyReason::ALL.len()] {
    let mut by = [0; DenyReason::ALL.len()];
    for (reason, n) in counts {
        by[reason.index()] = *n;
    }
    by
}

fn assert_denied_by(stats: &ReplayStats, counts: &[(DenyReason, u64)]) {
    assert_eq!(stats.denied_by, denied_by(counts));
}

/// 状態ハッシュの列に書き出す間隔（tick）。
const CHECKPOINT_INTERVAL: u64 = 1_000;

struct Scenario {
    name: &'static str,
    seed: u64,
    ticks: u64,
    log: CommandLog,
    /// 始めたときの所持金。`None` なら決まった初期値である。
    money: Option<Money>,
}

impl Scenario {
    fn start(&self) -> World {
        match self.money {
            Some(m) => World::new_with_money(self.seed, m),
            None => World::new(self.seed),
        }
    }
}

fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "clock_and_commands",
            seed: 42,
            ticks: 2 * chizu_core::TICKS_PER_DAY,
            log: clock_and_commands(),
            money: None,
        },
        Scenario {
            name: "land_purchase",
            seed: 42,
            ticks: 600,
            log: land_purchase(),
            money: None,
        },
        Scenario {
            name: "road_network",
            seed: 42,
            ticks: 400,
            log: road_network(42),
            money: None,
        },
        Scenario {
            name: "zoning",
            seed: 42,
            ticks: 300,
            log: zoning(),
            money: None,
        },
        Scenario {
            name: "growth",
            seed: 42,
            ticks: 3_600,
            log: growth(),
            money: None,
        },
        Scenario {
            name: "utilities",
            seed: 42,
            ticks: 7_200 + 1_200,
            log: utilities(),
            money: None,
        },
        Scenario {
            name: "economy",
            seed: 42,
            ticks: 2 * chizu_core::TICKS_PER_DAY,
            log: economy(),
            money: None,
        },
        Scenario {
            name: "commute",
            seed: 42,
            ticks: COMMUTE_TICKS,
            log: commute(),
            money: None,
        },
        Scenario {
            name: "signals",
            seed: 42,
            ticks: COMMUTE_TICKS,
            log: signals(),
            money: None,
        },
        Scenario {
            name: "crosswalks",
            seed: 42,
            ticks: COMMUTE_TICKS,
            log: crosswalks(),
            money: None,
        },
        Scenario {
            name: "road_upgrade",
            seed: 42,
            ticks: UPGRADE_TICK + 900,
            log: road_upgrade(),
            money: None,
        },
        Scenario {
            name: "errand",
            seed: 42,
            ticks: ERRAND_TICKS,
            log: errand(),
            money: None,
        },
        Scenario {
            name: "long_run",
            seed: 42,
            ticks: LONG_RUN_TICKS,
            log: long_run(),
            money: None,
        },
        Scenario {
            name: "land_slots",
            seed: 42,
            ticks: 2 * TICKS_PER_DAY + 600,
            log: land_slots(),
            money: Some(Money(10_000_000)),
        },
    ]
}

/// 道路を引き、交差させ、高速道路につなぎ、消して、また引く記録。重なり、角での交わり、
/// 買っていない土地、何もない場所を消す操作で却下される場合も混ぜる。
fn road_network(seed: u64) -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let road = |from: (i64, i64), to: (i64, i64), kind| Command::BuildRoad {
        from: TilePos::new(from.0, from.1),
        to: TilePos::new(to.0, to.1),
        kind,
    };
    let bulldoze = |a: (i64, i64), b: (i64, i64)| Command::Bulldoze {
        area: TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1)),
    };
    rec(10, road((0, 0), (40, 0), RoadKind::Small));
    rec(10, road((40, 0), (40, 20), RoadKind::Medium));
    // 既存の区間の途中を横切り、両方を分ける。
    rec(20, road((20, -20), (20, 20), RoadKind::Small));
    rec(20, road((40, -20), (40, 0), RoadKind::Large));
    rec(20, road((0, 0), (30, 30), RoadKind::Large));
    rec(20, road((10, 0), (30, 0), RoadKind::Small));
    // 斜めの区間とマスの角で交わる。
    rec(20, road((1, 0), (0, 1), RoadKind::Small));
    // 既存のノードを途中で通り、そこで分ける。
    rec(30, road((35, 5), (45, -5), RoadKind::Small));
    rec(30, road((100, 0), (140, 0), RoadKind::Small));
    // 外部接続の高速道路の、買ってある土地の端のノードから内へ引く。
    let world = World::new(seed);
    let (highway, _) = world.network().segments().next().unwrap();
    let line = world.network().segment_line(highway).unwrap();
    let (inner, (sx, sy)) = (line.start, line.step);
    let inward = (inner.x - 10 * sx, inner.y - 10 * sy);
    rec(40, road((inner.x, inner.y), inward, RoadKind::Medium));
    rec(100, bulldoze((39, 10), (41, 12)));
    rec(100, bulldoze((-50, -50), (-40, -40)));
    rec(200, road((40, 0), (40, 30), RoadKind::Small));
    log
}

/// 道路沿いに区画を塗り、消し、道路を消して区画が消える記録。塗れるマスがない場合と、
/// 変わるマスがない場合の却下も混ぜる。
fn zoning() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let road = |from: (i64, i64), to: (i64, i64)| Command::BuildRoad {
        from: TilePos::new(from.0, from.1),
        to: TilePos::new(to.0, to.1),
        kind: RoadKind::Small,
    };
    let rect = |a: (i64, i64), b: (i64, i64)| {
        TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1))
    };
    let paint = |a, b, zone| Command::PaintZone {
        area: rect(a, b),
        zone,
    };
    rec(10, road((0, 0), (30, 0)));
    rec(10, road((0, 12), (30, 12)));
    rec(20, paint((0, -6), (30, 18), Some(ZoneKind::Residential)));
    rec(20, paint((0, -6), (30, -1), Some(ZoneKind::Office)));
    rec(
        30,
        paint((100, 100), (110, 110), Some(ZoneKind::Commercial)),
    );
    rec(30, paint((0, -6), (30, -1), Some(ZoneKind::Office)));
    rec(40, paint((0, 1), (5, 5), None));
    rec(
        100,
        Command::Bulldoze {
            area: rect((15, 12), (15, 12)),
        },
    );
    rec(110, paint((0, 1), (5, 5), None));
    log
}

/// 高速道路の内側のノードから、y = `y` の通りの `x_min..=x_max` のどこかまで道路を引くコマンド。
fn connect_to_outside(seed: u64, y: i64, x_min: i64, x_max: i64) -> Vec<Command> {
    let inner = chizu_core::world::highway_line(seed).start;
    let corner = TilePos::new(inner.x, y);
    let end = TilePos::new(inner.x.clamp(x_min, x_max), y);
    [(inner, corner), (corner, end)]
        .into_iter()
        .filter(|(a, b)| a != b)
        .map(|(from, to)| Command::BuildRoad {
            from,
            to,
            kind: RoadKind::Small,
        })
        .collect()
}

/// 道路に面した住宅・商業・工業・オフィスの区画に建物が建ち、途中で道路の一部を消す記録。
/// 下の通りは外の世界とつながる。どちらの通りにも発電所と浄水場を 1 つずつ置く。
fn growth() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let rect = |a: (i64, i64), b: (i64, i64)| {
        TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1))
    };
    for (y, kind) in [(0, RoadKind::Medium), (20, RoadKind::Small)] {
        rec(
            1,
            Command::BuildRoad {
                from: TilePos::new(-40, y),
                to: TilePos::new(40, y),
                kind,
            },
        );
    }
    let zones = [
        (rect((-40, -6), (40, -1)), ZoneKind::Residential),
        (rect((-40, 1), (0, 6)), ZoneKind::Industrial),
        (rect((1, 1), (40, 6)), ZoneKind::Commercial),
        (rect((-40, 14), (40, 19)), ZoneKind::Office),
        (rect((-40, 21), (40, 26)), ZoneKind::Residential),
    ];
    for (area, zone) in zones {
        rec(
            2,
            Command::PaintZone {
                area,
                zone: Some(zone),
            },
        );
    }
    for c in connect_to_outside(42, 0, -40, 40) {
        rec(3, c);
    }
    // 新しい建物は電気と水道の余りのある網にしか建たないので、どちらの通りにも施設を置く。
    for (kind, x, y) in [
        (ServiceKind::PowerPlant, -37, -1),
        (ServiceKind::WaterPump, -33, -1),
        (ServiceKind::PowerPlant, -37, 19),
        (ServiceKind::WaterPump, -33, 19),
    ] {
        rec(
            3,
            Command::PlaceBuilding {
                kind,
                at: TilePos::new(x, y),
                rotation: 2,
            },
        );
    }
    rec(
        1_800,
        Command::Bulldoze {
            area: rect((30, 20), (30, 20)),
        },
    );
    log
}

/// 長く回すシナリオの長さ。7 日目の始まりである。
const LONG_RUN_TICKS: u64 = 6 * TICKS_PER_DAY;

/// `growth` の街を何日も回す記録。3 日目の始まりに北の通りの東半分の住宅の区画を消して家を壊し、
/// 家のない世帯を作る。空き家が足りないので、1 日たっても入れなかった世帯は街を出ていく。
/// 4 日目に同じ区画を塗り直して、また育てる。通勤、段階の上がり、電気と水道も入る。
/// test:wasm は、この記録をブラウザの中で読み込んで再生し、chizu-cli と同じ状態ハッシュになるかを見る。
fn long_run() -> CommandLog {
    let mut log = growth();
    let area = TileRect::from_corners(TilePos::new(0, -6), TilePos::new(40, -1));
    for (tick, zone) in [
        (2 * TICKS_PER_DAY, None),
        (3 * TICKS_PER_DAY + 600, Some(ZoneKind::Residential)),
    ] {
        log.record(Tick(tick), Command::PaintZone { area, zone })
            .unwrap();
    }
    log
}

#[test]
fn long_run_scenario_levels_up_and_sends_homeless_away() {
    let log = long_run();
    let mut world = World::new(42);
    let mut homeless_before = Vec::new();
    let mut left = 0;
    let mut leveled = 0;
    while world.tick().0 < LONG_RUN_TICKS {
        replay(&mut world, &log, 60);
        let p = world.population();
        left += homeless_before
            .iter()
            .filter(|h| p.home(**h).is_none())
            .count();
        homeless_before = p
            .household_ids()
            .filter(|h| p.home(*h) == Some(None))
            .collect();
        leveled = leveled.max(
            world
                .buildings()
                .iter()
                .filter(|(_, b)| b.level >= 2)
                .count(),
        );
    }
    assert!(leveled > 0, "段階の上がった建物がない");
    assert!(left > 0, "家のないまま街を出た世帯がない");
    let yesterday = world.traffic().yesterday.expect("前の日の移動がない");
    assert!(yesterday.arrived > 0, "通勤で着いた移動がない");
}

/// 2 本の通りに区画を塗り、片方にだけ発電所と浄水場を置く記録。道路に面さない場所と、
/// 道路の上に置こうとして却下される場合も混ぜる。
fn utilities() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let rect = |a: (i64, i64), b: (i64, i64)| {
        TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1))
    };
    for (i, y) in [10, 50].into_iter().enumerate() {
        let t = 1 + 2 * i as u64;
        rec(
            t,
            Command::BuildRoad {
                from: TilePos::new(0, y),
                to: TilePos::new(40, y),
                kind: RoadKind::Small,
            },
        );
        for (area, zone) in [
            (rect((0, y - 6), (40, y - 1)), ZoneKind::Residential),
            (rect((0, y + 1), (40, y + 6)), ZoneKind::Industrial),
        ] {
            rec(
                t + 1,
                Command::PaintZone {
                    area,
                    zone: Some(zone),
                },
            );
        }
    }
    let place = |kind, x, y, rotation| Command::PlaceBuilding {
        kind,
        at: TilePos::new(x, y),
        rotation,
    };
    rec(5, place(ServiceKind::PowerPlant, 3, 9, 2));
    rec(5, place(ServiceKind::WaterPump, 7, 9, 2));
    rec(5, place(ServiceKind::WaterPump, 11, 9, 2));
    rec(5, place(ServiceKind::PowerPlant, 30, 9, 2));
    rec(5, place(ServiceKind::WaterPump, 20, 30, 2));
    rec(5, place(ServiceKind::WaterPump, 20, 10, 2));
    for c in connect_to_outside(42, 10, 0, 40) {
        rec(5, c);
    }
    log
}

/// 住宅と工業の通りに人が住み、途中で工業の税率を上げる記録。
fn economy() -> CommandLog {
    let mut log = street_city();
    log.record(
        Tick(4_000),
        Command::SetTaxRate {
            zone: ZoneKind::Industrial,
            permille: 150,
        },
    )
    .unwrap();
    log
}

/// 通勤のシナリオを回す長さ。2 日目の朝の通勤が終わる 11 時までである。
const COMMUTE_TICKS: u64 = TICKS_PER_DAY + TICKS_PER_DAY * 11 / 24;

/// 住宅の帯の近くと遠くに工業の帯を塗った通りの記録。近くの職場へは歩き、遠くの職場へは車で通う。
fn commute() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let rect = |a: (i64, i64), b: (i64, i64)| {
        TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1))
    };
    // 通りの両端を、北側の通りでつないで輪にする。
    let corners = [(-40, 0), (100, 0), (100, 20), (-40, 20), (-40, 0)];
    for w in corners.windows(2) {
        rec(
            1,
            Command::BuildRoad {
                from: TilePos::new(w[0].0, w[0].1),
                to: TilePos::new(w[1].0, w[1].1),
                kind: RoadKind::Small,
            },
        );
    }
    for (area, zone) in [
        (rect((-20, -6), (20, -1)), ZoneKind::Residential),
        (rect((-40, -6), (-22, -1)), ZoneKind::Industrial),
        (rect((60, -6), (100, -1)), ZoneKind::Industrial),
    ] {
        rec(
            2,
            Command::PaintZone {
                area,
                zone: Some(zone),
            },
        );
    }
    for c in connect_to_outside(42, 0, -40, 100) {
        rec(3, c);
    }
    for (kind, x) in [
        (ServiceKind::PowerPlant, -37),
        (ServiceKind::WaterPump, -33),
    ] {
        rec(
            3,
            Command::PlaceBuilding {
                kind,
                at: TilePos::new(x, 1),
                rotation: 0,
            },
        );
    }
    log
}

/// 通勤のシナリオの街に、2 本の通りを横切る南北の道路を足し、できた 2 つの十字路に信号を置く記録。
/// 住宅から東の工業へ向かう車は、どちらかの信号を通る。
fn signals() -> CommandLog {
    let mut log = commute();
    log.record(
        Tick(4),
        Command::BuildRoad {
            from: TilePos::new(40, -10),
            to: TilePos::new(40, 30),
            kind: RoadKind::Small,
        },
    )
    .unwrap();
    // ノードの ID は網の作り方で決まるので、ここまでを再生して引く。
    let mut world = World::new(42);
    replay(&mut world, &log, 5);
    for y in [0, 20] {
        let node = world.network().node_at(TilePos::new(40, y)).unwrap();
        log.record(Tick(5), Command::SetSignal { node, on: true })
            .unwrap();
    }
    log
}

/// 横断歩道のシナリオで、外した横断歩道を朝の通勤のさなかに戻す tick。
const CROSSWALK_BACK_TICK: u64 = 2_100;

/// 通勤のシナリオの街に、信号のシナリオと同じ南北の道路を足し、できた 2 つの十字路の横断歩道を外す記録。
/// 南の十字路は北と南の腕、北の十字路は東の腕を外し、朝の通勤のさなかに南の十字路の南の腕を戻す。
fn crosswalks() -> CommandLog {
    let mut log = commute();
    log.record(
        Tick(4),
        Command::BuildRoad {
            from: TilePos::new(40, -10),
            to: TilePos::new(40, 30),
            kind: RoadKind::Small,
        },
    )
    .unwrap();
    let mut world = World::new(42);
    replay(&mut world, &log, 5);
    let node = |y| world.network().node_at(TilePos::new(40, y)).unwrap();
    let (south, north) = (node(0), node(20));
    for (node, arm) in [
        (south, Heading::N),
        (south, Heading::S),
        (north, Heading::E),
    ] {
        log.record(
            Tick(5),
            Command::SetCrosswalk {
                node,
                arm,
                on: false,
            },
        )
        .unwrap();
    }
    log.record(
        Tick(CROSSWALK_BACK_TICK),
        Command::SetCrosswalk {
            node: south,
            arm: Heading::S,
            on: true,
        },
    )
    .unwrap();
    log
}

/// 横断歩道のシナリオで、外すのと戻すのがどれも適用され、戻さなかった腕は外したまま残る。
#[test]
fn crosswalks_scenario_removes_and_restores() {
    let log = crosswalks();
    let mut world = World::new(42);
    let stats = replay(&mut world, &log, CROSSWALK_BACK_TICK + 1);
    assert_eq!(stats.denied, 0);
    let net = world.network();
    let mask = |y| {
        let n = net.node_at(TilePos::new(40, y)).unwrap();
        net.node(n).unwrap().no_crosswalk
    };
    assert_eq!(mask(0), 1 << Heading::N as u8);
    assert_eq!(mask(20), 1 << Heading::E as u8);
}

/// 信号のシナリオで、信号が 2 つとも置けていて、車が赤の信号の手前で止まることを確かめる。
#[test]
fn signals_scenario_stops_cars_on_red() {
    let log = signals();
    let mut world = World::new(42);
    let stats = replay(&mut world, &log, 6);
    assert_eq!(stats.denied, 0);
    let lit: Vec<_> = world
        .network()
        .nodes()
        .filter(|(_, n)| n.control == chizu_core::NodeControl::Signal)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(lit.len(), 2);
    let end_of = |net: &chizu_core::Network, l: chizu_core::LinkId| {
        let s = net.segment(l.segment).unwrap();
        match l.dir {
            chizu_core::Dir::Forward => s.b,
            chizu_core::Dir::Backward => s.a,
        }
    };
    let (mut waited, mut entered) = (0, 0);
    for _ in 6..COMMUTE_TICKS {
        // 信号の交差点へ向かうリンクの上の車。進めたあとにリンクが変わっていれば、交差点に入った。
        let now = world.tick();
        let before: Vec<_> = world
            .trips()
            .iter()
            .filter(|(_, t)| t.mode() == LaneMode::Car)
            .filter(|(_, t)| lit.contains(&end_of(world.network(), t.link())))
            .map(|(id, t)| (id, t.link()))
            .collect();
        replay(&mut world, &log, 1);
        let net = world.network();
        for (id, link) in before {
            let Some(t) = world.trips().get(id) else {
                continue;
            };
            if t.mode() != LaneMode::Car || t.link() == link {
                continue;
            }
            entered += 1;
            let axis = signal::axis(net.segment_line(link.segment).unwrap().step);
            let light = world
                .trips()
                .lights()
                .state(net, end_of(net, link))
                .unwrap()
                .light_for(axis);
            assert_ne!(light, signal::Light::Red, "赤で入った {now:?} {link:?}");
        }
        assert_eq!(world.trips().lights().now, now, "控えた灯火の tick が違う");
        for (_, t) in world.trips().iter() {
            if t.mode() != LaneMode::Car || t.speed().0 != 0 {
                continue;
            }
            let s = net.segment(t.link().segment).unwrap();
            let end = match t.link().dir {
                chizu_core::Dir::Forward => s.b,
                chizu_core::Dir::Backward => s.a,
            };
            // 向き合う腕は同じ軸なので、区間の向きから軸が決まる。
            let line = net.segment_line(t.link().segment).unwrap();
            let axis = signal::axis(line.step);
            // 停止線の手前（車 1 台ぶんと車間の内）で止まった車だけを数える。
            let at_line = i64::from(t.offset().0) + 7_000 >= line.length().0;
            let red = at_line
                && lit.contains(&end)
                && world
                    .trips()
                    .lights()
                    .state(net, end)
                    .is_some_and(|st| st.light_for(axis) != signal::Light::Green);
            waited += usize::from(red);
        }
    }
    assert!(waited > 0, "信号の手前で止まった車がない");
    assert!(entered > 0, "信号の交差点に入った車がない");
}

/// 通勤のシナリオで、朝に住民が歩きと車の両方で職場へ向かい、着いたら職場にいることを確かめる。
/// 移動している住民は、住民の表の `trip` と移動の主体の両方から互いを指す。
#[test]
fn commuters_walk_and_drive_to_work_in_the_morning() {
    let log = commute();
    let mut world = World::new(42);
    let (mut walking, mut driving) = (0, 0);
    for _ in 0..COMMUTE_TICKS {
        replay(&mut world, &log, 1);
        let trips = world.trips();
        for (id, t) in trips.iter() {
            let Some(c) = trips.commuter(id) else {
                continue;
            };
            let row = world
                .population()
                .citizen(c.citizen)
                .expect("移動している住民がいない");
            assert_eq!(row.trip, Some(id));
            match t.mode() {
                LaneMode::Car => driving += 1,
                LaneMode::Sidewalk => walking += 1,
            }
        }
        let p = world.population();
        for id in p.citizen_ids() {
            if let Some(t) = p.citizen(id).unwrap().trip {
                assert_eq!(world.trips().commuter(t).map(|c| c.citizen), Some(id));
            }
        }
    }
    assert!(walking > 0 && driving > 0, "歩き {walking}、車 {driving}");
    let p = world.population();
    // 朝のあとに職場が決まり、翌朝に出発する住民は数えない。
    let workers = p
        .citizen_ids()
        .map(|id| p.citizen(id).unwrap())
        .filter(|r| r.workplace.is_some())
        .filter(|r| r.schedule.next != Activity::Work || r.schedule.depart_at.is_none())
        .count();
    assert!(workers > 0, "誰も働いていない");
    assert!(
        world.citizens_at_work() * 10 >= workers * 9,
        "朝に職場へ着かない：{} / {workers}",
        world.citizens_at_work()
    );
}

/// 買い物のシナリオを回す長さ。2 日目の 19 時、仕事の帰りに店へ寄る住民が出るまでである。
const ERRAND_TICKS: u64 = TICKS_PER_DAY + TICKS_PER_DAY * 19 / 24;

/// 通勤のシナリオの通りの、住宅の向かいに商業の帯を塗った記録。
fn errand() -> CommandLog {
    let mut log = commute();
    let area = TileRect::from_corners(TilePos::new(-20, 1), TilePos::new(20, 6));
    log.record(
        Tick(3),
        Command::PaintZone {
            area,
            zone: Some(ZoneKind::Commercial),
        },
    )
    .unwrap();
    log
}

/// 買い物のシナリオで、2 日目の昼に働いていない住民が店へ向かい、夕方には仕事の帰りに店へ寄る住民がいる。
/// 店から家へ帰る移動もある。
#[test]
fn citizens_go_shopping_by_day_and_after_work() {
    let log = errand();
    let mut world = World::new(42);
    let (mut daytime, mut evening, mut home_by_day) = (0, 0, 0);
    let mut noon_movers = 0;
    let hour = |h: u64| TICKS_PER_DAY + TICKS_PER_DAY * h / 24;
    for t in 0..ERRAND_TICKS {
        replay(&mut world, &log, 1);
        let trips = world.trips();
        if (hour(12)..hour(13)).contains(&t) {
            noon_movers = trips.iter().count().max(noon_movers);
        }
        for (id, _) in trips.iter() {
            match trips.commuter(id).map(|c| c.to) {
                Some(Activity::Shop) if (hour(9)..hour(16)).contains(&t) => daytime += 1,
                Some(Activity::Shop) if t >= hour(17) => evening += 1,
                Some(Activity::Home) if (hour(10)..hour(16)).contains(&t) => home_by_day += 1,
                _ => {}
            }
        }
    }
    assert!(daytime > 0, "昼に誰も店へ向かわない");
    assert!(evening > 0, "仕事の帰りに誰も店へ寄らない");
    assert!(home_by_day > 0, "昼に店から家へ帰らない");
    assert!(noon_movers > 0, "正午からの 1 時間に街を誰も動いていない");
}

/// 住民の表、移動、出発の待ち行列、買い物の行き先が互いに食い違わないことを確かめる。
fn assert_errands_consistent(world: &World, when: &str) {
    let p = world.population();
    let trips = world.trips();
    let mut scheduled = 0;
    for id in p.citizen_ids() {
        let row = p.citizen(id).unwrap();
        if let Some(t) = row.trip {
            assert_eq!(trips.commuter(t).map(|c| c.citizen), Some(id), "{when}");
        }
        if row.schedule.depart_at.is_some() {
            scheduled += 1;
        }
        if row.schedule.next == Activity::Shop {
            assert!(
                world.errands().any(|(c, _)| c == id),
                "{when}：店の予定に店がない"
            );
        }
    }
    for (t, _) in trips.iter() {
        if let Some(c) = trips.commuter(t) {
            assert_eq!(p.citizen(c.citizen).and_then(|r| r.trip), Some(t), "{when}");
        }
    }
    for (at, id) in p.departures().iter() {
        assert_eq!(
            p.citizen(id).unwrap().schedule.depart_at,
            Some(at),
            "{when}"
        );
    }
    assert_eq!(p.departures().len(), scheduled, "{when}");
    for (id, shop) in world.errands() {
        assert!(p.citizen(id).is_some(), "{when}：いない住民の買い物");
        assert!(
            world.buildings().get(shop).is_some(),
            "{when}：消えた店への買い物"
        );
    }
}

/// 店へ向かう移動の数。
fn shop_trips(world: &World) -> usize {
    let trips = world.trips();
    trips
        .iter()
        .filter(|(id, _)| trips.commuter(*id).is_some_and(|c| c.to == Activity::Shop))
        .count()
}

/// 2 日目の昼、店へ向かう住民がいる時刻に商業の区画を消して店を取り壊しても、表どうしが食い違わず、
/// 消えた店への買い物も残らない。
#[test]
fn removing_shops_mid_errand_keeps_tables_consistent() {
    let noon = TICKS_PER_DAY + TICKS_PER_DAY / 2;
    let mut probe = World::new(42);
    let base = errand();
    replay(&mut probe, &base, noon);
    let mut at = noon;
    while shop_trips(&probe) == 0 {
        replay(&mut probe, &base, 1);
        at += 1;
        assert!(at < noon + TICKS_PER_DAY / 6, "昼に店へ向かう住民がいない");
    }
    let mut log = errand();
    let area = TileRect::from_corners(TilePos::new(-20, 1), TilePos::new(20, 6));
    log.record(Tick(at), Command::PaintZone { area, zone: None })
        .unwrap();
    let mut world = World::new(42);
    replay(&mut world, &log, at);
    replay(&mut world, &log, 1);
    assert_eq!(shop_trips(&world), 0, "消えた店へ向かい続ける");
    assert_eq!(world.errands().count(), 0, "消えた店への買い物が残る");
    for _ in 0..TICKS_PER_DAY / 3 {
        replay(&mut world, &log, 1);
        assert_errands_consistent(&world, "店を壊したあと");
    }
}

/// 仕事の帰りに店へ寄っているあいだに職場が消え、職場の割り当てが変わっても、表どうしが食い違わない。
#[test]
fn losing_the_job_while_at_the_shop_keeps_tables_consistent() {
    let far = TileRect::from_corners(TilePos::new(60, -6), TilePos::new(100, -1));
    // 店で帰りを待っている、遠い工業の帯で働く住民。
    let at_far_shop = |w: &World| {
        let p = w.population();
        w.errands().find(|(id, _)| {
            let r = p.citizen(*id).unwrap();
            r.schedule.next == Activity::Home
                && r.trip.is_none()
                && r.schedule.depart_at.is_some()
                && r.workplace
                    .and_then(|b| w.buildings().get(b))
                    .is_some_and(|b| b.front.x >= far.min.x)
        })
    };
    let base = errand();
    let mut probe = World::new(42);
    let evening = TICKS_PER_DAY + TICKS_PER_DAY * 17 / 24;
    replay(&mut probe, &base, evening);
    let mut at = evening;
    let who = loop {
        if let Some((id, _)) = at_far_shop(&probe) {
            break id;
        }
        replay(&mut probe, &base, 1);
        at += 1;
        assert!(
            at < evening + TICKS_PER_DAY / 6,
            "帰りに店にいる住民がいない"
        );
    };
    let mut log = errand();
    log.record(
        Tick(at),
        Command::PaintZone {
            area: far,
            zone: None,
        },
    )
    .unwrap();
    let mut world = World::new(42);
    replay(&mut world, &log, at + 1);
    assert!(
        world.errands().all(|(id, _)| id != who),
        "職場が消えても店にいる"
    );
    for _ in 0..TICKS_PER_DAY {
        replay(&mut world, &log, 1);
        assert_errands_consistent(&world, "職場を壊したあと");
    }
}

/// 通勤のシナリオを 20 日回しても、駐車場に停めてある車が増え続けない。停めてある車はどれも、
/// 住民が自分の車として覚えている 1 台で、停めてある台数は住民の数を超えない。
#[test]
fn parked_cars_do_not_pile_up_over_days() {
    let log = commute();
    let mut world = World::new(42);
    let mut by_day = Vec::new();
    for day in 1..=20 {
        replay(&mut world, &log, TICKS_PER_DAY);
        let parked: u64 = world.trips().lots().map(|(_, l)| u64::from(l.parked)).sum();
        let p = world.population();
        let owned = p
            .citizen_ids()
            .filter_map(|id| world.trips().parked_car(id))
            .filter(|b| world.buildings().get(*b).is_some())
            .count() as u64;
        assert_eq!(parked, owned, "{day} 日目：持ち主のいない車が停めてある");
        assert!(
            parked <= p.citizen_count() as u64,
            "{day} 日目：{parked} 台"
        );
        by_day.push(parked);
    }
    let early = by_day[4..10].iter().max().copied().unwrap();
    let late = by_day[14..].iter().max().copied().unwrap();
    assert!(early > 0, "車が停められていない {by_day:?}");
    assert!(
        late * 4 <= early * 5,
        "停めてある台数が増え続ける {by_day:?}"
    );
}

/// 始めたときの枠のぶんだけチャンクを買い、次の 1 つは枠で断られ、
/// 人口がマイルストーンに届いたあとに同じチャンクを買える記録。所持金は多めにして始める。
fn land_slots() -> CommandLog {
    let mut log = street_city();
    // 住宅が電気と水道で頭打ちにならないよう、工業の側に発電所と浄水場を足す。
    let place = |kind, x| Command::PlaceBuilding {
        kind,
        at: TilePos::new(x, 1),
        rotation: 0,
    };
    for x in [-40, -36] {
        log.record(Tick(4), place(ServiceKind::PowerPlant, x))
            .unwrap();
    }
    for x in [-32, -30, -28] {
        log.record(Tick(4), place(ServiceKind::WaterPump, x))
            .unwrap();
    }
    let buy = |cx| Command::BuyChunk {
        coord: ChunkCoord::new(cx, -1),
    };
    for cx in 2..=8 {
        log.record(Tick(10), buy(cx)).unwrap();
    }
    log.record(Tick(2 * TICKS_PER_DAY), buy(8)).unwrap();
    log
}

/// 住宅と工業の通りに、電気と水道を引き、外の世界とつないだ記録。
fn street_city() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let rect = |a: (i64, i64), b: (i64, i64)| {
        TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1))
    };
    rec(
        1,
        Command::BuildRoad {
            from: TilePos::new(-40, 0),
            to: TilePos::new(40, 0),
            kind: RoadKind::Small,
        },
    );
    for (area, zone) in [
        (rect((-30, -6), (40, -1)), ZoneKind::Residential),
        (rect((-40, 1), (40, 6)), ZoneKind::Industrial),
    ] {
        rec(
            2,
            Command::PaintZone {
                area,
                zone: Some(zone),
            },
        );
    }
    for c in connect_to_outside(42, 0, -40, 40) {
        rec(3, c);
    }
    for (kind, x) in [
        (ServiceKind::PowerPlant, -37),
        (ServiceKind::WaterPump, -33),
    ] {
        rec(
            3,
            Command::PlaceBuilding {
                kind,
                at: TilePos::new(x, -1),
                rotation: 2,
            },
        );
    }
    log
}

/// 土地を買い進める記録。買える場合と、買った土地・離れた土地・角だけで接する土地・
/// 所持金の不足で却下される場合を混ぜる。
fn land_purchase() -> CommandLog {
    let mut log = CommandLog::new();
    let mut buy = |tick: u64, cx: i32, cy: i32| {
        log.record(
            Tick(tick),
            Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            },
        )
        .unwrap();
    };
    buy(10, 2, 0);
    buy(10, 2, 0);
    buy(20, 4, 4);
    buy(20, 1, 1);
    buy(30, 2, 2);
    buy(100, 0, -2);
    buy(100, 3, 0);
    buy(200, -2, 0);
    // 次の 1 件で所持金をちょうど使い切り、その次は却下される。
    buy(300, 0, 2);
    buy(300, 0, 3);
    buy(400, 0, -3);
    log
}

/// 速度と税率を何度か変え、中身のまだないコマンドも混ぜた 2 日ぶんの記録。
/// 終わりの直前にもコマンドを置き、速度は等速に戻さずに終える。
fn clock_and_commands() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let speed = |m| Command::SetSimSpeed {
        speed: SimSpeed::new(m).unwrap(),
    };
    let tax = |zone, permille| Command::SetTaxRate { zone, permille };

    rec(
        0,
        Command::BuildRoad {
            from: TilePos::new(0, 0),
            to: TilePos::new(20, 0),
            kind: RoadKind::Small,
        },
    );
    rec(
        0,
        Command::PaintZone {
            area: TileRect::from_corners(TilePos::new(0, 1), TilePos::new(20, 4)),
            zone: Some(ZoneKind::Residential),
        },
    );
    rec(30, tax(ZoneKind::Residential, 120));
    rec(30, tax(ZoneKind::Residential, 90));
    rec(600, speed(3));
    rec(2_400, speed(8));
    rec(2_400, tax(ZoneKind::Industrial, 150));
    rec(
        5_000,
        Command::BuyChunk {
            coord: ChunkCoord::new(1, 0),
        },
    );
    rec(7_200, speed(48));
    rec(9_000, tax(ZoneKind::Office, 0));
    rec(12_000, speed(1));
    rec(13_500, speed(12));
    rec(14_398, tax(ZoneKind::Commercial, 200));
    rec(14_399, speed(6));
    // 再生する範囲より後なので適用されない。
    rec(20_000, speed(2));
    log
}

/// 再生して、状態ハッシュの列を `.hash` の形式の文字列で返す。
fn checkpoints(seed: u64, log: &CommandLog, ticks: u64) -> String {
    checkpoints_from(World::new(seed), log, ticks).0
}

fn checkpoints_and_world(seed: u64, log: &CommandLog, ticks: u64) -> (String, World) {
    checkpoints_from(World::new(seed), log, ticks)
}

/// `world` から再生する [`checkpoints`]。
fn checkpoints_from(mut world: World, log: &CommandLog, ticks: u64) -> (String, World) {
    let mut digest = StateHasher::new();
    digest.write(&world.state_hash());
    let mut out = String::new();
    for _ in 0..ticks {
        replay(&mut world, log, 1);
        let h = world.state_hash();
        digest.write(&h);
        let t = world.tick().0;
        if t.is_multiple_of(CHECKPOINT_INTERVAL) || t == ticks {
            out.push_str(&format!("{t} {h:016x} {:016x}\n", digest.finish()));
        }
    }
    (out, world)
}

/// 買い進める記録の結果が、水辺の配置と購入の規則を通っていることを確かめる。
/// ここが崩れると、ゴールデンのハッシュがそれらを見ていないことになる。
#[test]
fn land_purchase_covers_water_and_the_rules() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "land_purchase")
        .unwrap();
    let mut world = World::new(s.seed);
    let stats = replay(&mut world, &s.log, s.ticks);
    assert_eq!((stats.applied, stats.denied), (5, 6));
    assert_denied_by(
        &stats,
        &[
            (DenyReason::NotEnoughMoney, 2),
            (DenyReason::NotAdjacent, 2),
            (DenyReason::AlreadyOwned, 2),
        ],
    );
    assert_eq!(world.money(), chizu_core::Money::ZERO);
    assert_eq!(world.chunks().filter(|c| c.owned()).count(), 9 + 5);
    let water: u32 = world.chunks().map(|c| c.terrain().water_count()).sum();
    assert!(water > 0, "買った土地に水辺が 1 マスもない");
    assert_eq!(checkpoints_and_world(s.seed, &s.log, s.ticks).1, world);
}

/// 道路の記録が、交差での分割、高速道路へのつなぎ、却下、消したあとの引き直しを通っていることを確かめる。
#[test]
fn road_network_covers_the_rules() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "road_network")
        .unwrap();
    let mut world = World::new(s.seed);
    let stats = replay(&mut world, &s.log, s.ticks);
    assert_eq!((stats.applied, stats.denied), (9, 4));
    assert_denied_by(
        &stats,
        &[
            (DenyReason::NotOwned, 1),
            (DenyReason::Overlap, 1),
            (DenyReason::CornerCrossing, 1),
            (DenyReason::NothingToBulldoze, 1),
        ],
    );
    let net = world.network();
    let degree = |x, y| {
        let node = net.node_at(TilePos::new(x, y)).unwrap();
        net.node(node).unwrap().segments.len()
    };
    assert_eq!((degree(20, 0), degree(20, 20), degree(40, 0)), (4, 3, 5));
    // 高速道路と、つないだ道路の 2 本が端のノードに集まる。
    let (highway, _) = net.segments().next().unwrap();
    let inner = net.segment_line(highway).unwrap().start;
    assert_eq!(degree(inner.x, inner.y), 2);
    assert!(world.money() < chizu_core::world::INITIAL_MONEY);
}

/// 区画の記録が、塗る、消す、道路を消したときの見直し、却下を通っていることを確かめる。
#[test]
fn zoning_covers_the_rules() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "zoning")
        .unwrap();
    let mut world = World::new(s.seed);
    let stats = replay(&mut world, &s.log, s.ticks);
    assert_eq!((stats.applied, stats.denied), (6, 3));
    assert_denied_by(
        &stats,
        &[
            (DenyReason::NoZonableTile, 1),
            (DenyReason::ZoneUnchanged, 2),
        ],
    );
    let zone = |x, y| world.zone_at(TilePos::new(x, y));
    assert_eq!(zone(10, -3), Some(ZoneKind::Office));
    assert_eq!(zone(10, 3), Some(ZoneKind::Residential));
    assert_eq!(zone(3, 3), None);
    // 消した道路にだけ面していたマスは消え、残った下の道路に面するマスは残る。
    assert_eq!(zone(10, 15), None);
    assert_eq!(zone(10, 6), Some(ZoneKind::Residential));
}

/// 建物の記録が、4 種類の区画に建ち、道路を消すと消えることを確かめる。
#[test]
fn growth_builds_every_zone_and_removes_with_the_road() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "growth")
        .unwrap();
    let mut world = World::new(s.seed);
    replay(&mut world, &s.log, 1_799);
    let count =
        |w: &World, zone: ZoneKind| w.buildings().iter().filter(|(_, b)| b.zone == zone).count();
    for zone in ZoneKind::ALL {
        assert!(count(&world, zone) > 0, "{zone:?} の建物が建たない");
    }
    let offices = count(&world, ZoneKind::Office);
    replay(&mut world, &s.log, 1);
    assert!(count(&world, ZoneKind::Office) < offices);
    assert!(
        world
            .buildings()
            .iter()
            .all(|(_, b)| b.area.min.y < 14 || b.area.min.y > 26)
    );
}

/// 施設のある通りの建物だけに電気と水道が届き、世帯が移り住むことを確かめる。
#[test]
fn utilities_supply_only_the_connected_street() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "utilities")
        .unwrap();
    let mut world = World::new(s.seed);
    let stats = replay(&mut world, &s.log, s.ticks);
    assert_denied_by(
        &stats,
        &[(DenyReason::NoRoadAccess, 1), (DenyReason::Occupied, 1)],
    );
    assert_eq!(world.facilities().len(), 4);
    let near = |b: &chizu_core::building::Building| b.area.min.y < 30;
    let buildings: Vec<_> = world.buildings().iter().map(|(_, b)| *b).collect();
    // 施設のある通りの住宅にだけ、外から世帯が移り住む。
    let p = world.population();
    assert!(p.household_count() > 0);
    for h in p.household_ids() {
        let home = world.buildings().get(p.home(h).flatten().unwrap()).unwrap();
        assert!(near(home), "{home:?}");
    }
    assert!(buildings.iter().any(|b| !near(b)));
    for b in &buildings {
        assert_eq!(b.supply.power && b.supply.water, near(b), "{b:?}");
        if !near(b) {
            assert_eq!(b.level, 1);
        }
    }
}

/// 枠の記録が、始めたときの枠を使い切って断られ、マイルストーンのあとに買えることを確かめる。
#[test]
fn land_slots_open_with_the_milestone() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "land_slots")
        .unwrap();
    let mut world = s.start();
    let stats = replay(&mut world, &s.log, 11);
    assert_denied_by(&stats, &[(DenyReason::NoChunkSlot, 1)]);
    assert_eq!(world.milestone(), 0);
    assert!(!world.is_owned(ChunkCoord::new(8, -1)));
    let stats = replay(&mut world, &s.log, s.ticks - 11);
    assert_eq!(stats.denied, 0, "{:?}", stats.denied_by);
    assert!(world.milestone() >= 1, "{} 人", world.housed_citizens());
    assert!(world.is_owned(ChunkCoord::new(8, -1)));
}

/// 財政の記録が、税収と維持費の精算を通っていることを確かめる。
#[test]
fn economy_settles_every_hour() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "economy")
        .unwrap();
    let mut world = World::new(s.seed);
    let stats = replay(&mut world, &s.log, 7_000);
    let f = world.finances();
    assert!(world.population().citizen_count() > 0);
    assert!(f.taxes[ZoneKind::Residential.index()].0 > 0, "{f:?}");
    assert!(f.taxes[ZoneKind::Industrial.index()].0 > 0, "{f:?}");
    assert!(f.road_upkeep.0 > 0 && f.facility_upkeep.iter().all(|m| m.0 > 0));
    assert_eq!(stats.denied, 0);
    // 精算の tick を進めると、その tick の終わりの 1 日の見込みのおよそ 1/24 だけ所持金が動く。
    replay(&mut world, &s.log, 200);
    let money = world.money().0;
    replay(&mut world, &s.log, 1);
    let (moved, net) = (world.money().0 - money, world.finances().net().0);
    assert!((moved * 24 - net).abs() <= 24, "{moved} × 24 と {net}");
}

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn updating() -> bool {
    std::env::var_os("CHIZU_UPDATE_GOLDEN").is_some_and(|v| v == "1")
}

#[test]
fn golden_hashes_match() {
    let dir = golden_dir();
    let mut mismatches = Vec::new();
    for s in scenarios() {
        let rec_path = dir.join(format!("{}.rec", s.name));
        let hash_path = dir.join(format!("{}.hash", s.name));
        let encoded = s.log.encode();
        let got = checkpoints_from(s.start(), &s.log, s.ticks).0;

        if updating() {
            std::fs::write(&rec_path, &encoded).unwrap();
            std::fs::write(&hash_path, &got).unwrap();
            continue;
        }

        let stored = std::fs::read(&rec_path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", rec_path.display()));
        if stored != encoded {
            mismatches.push(format!("{}: 記録の書き出しが変わった", s.name));
        }
        // 保存した記録を読み戻して再生しても、同じ結果になる。
        let decoded = CommandLog::decode(&stored).unwrap();
        assert_eq!(
            checkpoints_from(s.start(), &decoded, s.ticks).0,
            got,
            "{}: 読み戻した記録で結果が変わった",
            s.name
        );

        let expected = std::fs::read_to_string(&hash_path)
            .unwrap_or_else(|e| panic!("{} を読めない: {e}", hash_path.display()));
        if let Some((e, g)) = expected.lines().zip(got.lines()).find(|(e, g)| e != g) {
            mismatches.push(format!("{}: {e} が {g} に変わった", s.name));
        } else if expected.lines().count() != got.lines().count() {
            mismatches.push(format!("{}: 時点の数が変わった", s.name));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{}\n意図した変更なら CHIZU_UPDATE_GOLDEN=1 cargo test -p chizu-core --test golden で書き直す",
        mismatches.join("\n")
    );
}

#[test]
fn replaying_twice_gives_the_same_hash() {
    for s in scenarios() {
        assert_eq!(
            checkpoints_from(s.start(), &s.log, s.ticks).0,
            checkpoints_from(s.start(), &s.log, s.ticks).0,
            "{}",
            s.name
        );
    }
}

/// 記録の各件を `f` で書き換えた記録。`None` を返した件は消し、番号は 0 から振り直す。
fn rewrite(log: &CommandLog, f: impl Fn(RecordedCommand) -> Option<RecordedCommand>) -> CommandLog {
    let mut out = CommandLog::new();
    for e in log.entries().iter().copied().filter_map(f) {
        out.record(e.tick, e.command).unwrap();
    }
    out
}

#[test]
fn broken_records_change_the_checkpoints() {
    let s = &scenarios()[0];
    let base = checkpoints(s.seed, &s.log, s.ticks);
    let shift = |by: u64| {
        rewrite(&s.log, move |e| {
            Some(RecordedCommand {
                tick: if e.tick.0 < s.ticks {
                    e.tick + by
                } else {
                    e.tick
                },
                ..e
            })
        })
    };
    let broken = [
        ("種を変える", s.seed + 1, s.log.clone()),
        ("記録を空にする", s.seed, CommandLog::new()),
        (
            "SetSimSpeed を消す",
            s.seed,
            rewrite(&s.log, |e| {
                (!matches!(e.command, Command::SetSimSpeed { .. })).then_some(e)
            }),
        ),
        ("範囲内の tick を 1,000 ずらす", s.seed, shift(1_000)),
        ("範囲内の tick を 1 ずらす", s.seed, shift(1)),
        (
            "最後の 1 件を消す",
            s.seed,
            rewrite(&s.log, |e| (e.tick != Tick(14_399)).then_some(e)),
        ),
    ];
    for (what, seed, log) in broken {
        assert_ne!(checkpoints(seed, &log, s.ticks), base, "{what}");
    }
}

/// 歩く人の記録。格子の道路を引き、決まった tick ごとに歩く移動を作る。途中で道路を消し、
/// 交わる道路で分けて、歩いている人に経路を探し直させる。
fn walking_log() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    let road = |from: (i64, i64), to: (i64, i64)| Command::BuildRoad {
        from: TilePos::new(from.0, from.1),
        to: TilePos::new(to.0, to.1),
        kind: RoadKind::Small,
    };
    for i in 0..=4 {
        rec(0, road((0, i * 12), (48, i * 12)));
        rec(0, road((i * 12, 0), (i * 12, 48)));
    }
    rec(
        300,
        Command::Bulldoze {
            area: TileRect::from_corners(TilePos::new(18, 24), TilePos::new(18, 24)),
        },
    );
    rec(400, road((6, -6), (6, 54)));
    log
}

/// 格子の道路の上の、種から決めた歩道の位置。
fn walk_end(seed: u64, k: u64, tick: u64) -> SidewalkPos {
    let mut r = rng(seed, RngPurpose::DailySchedule, k, Tick(tick));
    let line = i64::from(r.below(5)) * 12;
    let along = i64::from(r.below(47));
    let back = r.chance(1, 2);
    let (tile, step) = match (r.chance(1, 2), back) {
        (true, false) => (TilePos::new(along, line), (1, 0)),
        (true, true) => (TilePos::new(along + 1, line), (-1, 0)),
        (false, false) => (TilePos::new(line, along), (0, 1)),
        (false, true) => (TilePos::new(line, along + 1), (0, -1)),
    };
    SidewalkPos {
        tile,
        step,
        along: LaneOffset(r.below(8_000)),
    }
}

/// 歩く人の記録を再生し、状態ハッシュの列と、作った移動の数、終わりの世界を返す。
fn walking_checkpoints(seed: u64, ticks: u64) -> (String, u32, World) {
    let log = walking_log();
    let mut world = World::new(seed);
    let mut digest = StateHasher::new();
    let mut out = String::new();
    let mut spawned = 0;
    for _ in 0..ticks {
        let t = world.tick().0;
        if t < 600 && t.is_multiple_of(3) {
            for k in 0..4 {
                let (from, to) = (walk_end(seed, 2 * k, t), walk_end(seed, 2 * k + 1, t));
                spawned += u32::from(world.spawn_walk(from, to).is_some());
            }
        }
        replay(&mut world, &log, 1);
        let h = world.state_hash();
        digest.write(&h);
        let t = world.tick().0;
        if t.is_multiple_of(250) || t == ticks {
            out.push_str(&format!("{t} {h:016x} {:016x}\n", digest.finish()));
        }
    }
    (out, spawned, world)
}

#[test]
fn walking_golden() {
    const SEED: u64 = 42;
    const TICKS: u64 = 1_500;
    let mut world = World::new(SEED);
    let stats = replay(&mut world, &walking_log(), 1);
    assert_eq!((stats.applied, stats.denied), (10, 0));
    let (got, spawned, end) = walking_checkpoints(SEED, TICKS);
    assert!(spawned > 600, "作れた移動が少ない {spawned}");
    assert!(
        end.trips().len() < 100,
        "着かずに残った移動が多い {}",
        end.trips().len()
    );
    assert_eq!(walking_checkpoints(SEED, TICKS).0, got);
    compare_golden("walking", &got);
}

/// 街の状態の書き出しの形が変わっていないかを見る。`growth` の街を書き出したバイト列のハッシュを固定する。
#[test]
fn snapshot_bytes_match_the_golden() {
    let s = scenarios()
        .into_iter()
        .find(|s| s.name == "growth")
        .unwrap();
    let mut world = s.start();
    replay(&mut world, &s.log, s.ticks);
    let mut h = StateHasher::new();
    h.write_bytes(&world.to_snapshot());
    let got = format!("{:016x}\n", h.finish());
    let path = golden_dir().join("snapshot.hash");
    if updating() {
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    assert_eq!(
        expected, got,
        "街の状態を書き出したバイト列が変わった。World の欄を足したり、型や順番を変えたりしたなら、\
         chizu_core::SNAPSHOT_VERSION を上げる。そのうえで（シミュレーションの規則だけを変えたときも）\
         CHIZU_UPDATE_GOLDEN=1 cargo test -p chizu-core --test golden で書き直す"
    );
}

fn compare_golden(name: &str, got: &str) {
    let path = golden_dir().join(format!("{name}.hash"));
    if updating() {
        std::fs::write(&path, got).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    assert_eq!(
        expected, got,
        "意図した変更なら CHIZU_UPDATE_GOLDEN=1 cargo test -p chizu-core --test golden で書き直す"
    );
}

/// 車の記録を再生し、状態ハッシュの列と、作った移動の数、終わりの世界を返す。建物が建ったあと、
/// 決まった tick ごとに種から選んだ建物どうしを車で行き来させる。途中で道路を消し、建物も消える。
/// 毎 tick、停めてある台数と押さえた台数の和が駐車場の台数を超えず、押さえた台数がその駐車場へ
/// 向かう主体の数と同じで、駐車場が建っている建物にだけあることを確かめる。
fn driving_checkpoints(seed: u64, ticks: u64) -> (String, u32, World) {
    driving_checkpoints_on(&growth(), seed, ticks)
}

/// [`driving_checkpoints`] と同じく、記録 `log` の街で車を行き来させる。
fn driving_checkpoints_on(log: &CommandLog, seed: u64, ticks: u64) -> (String, u32, World) {
    let mut world = World::new(seed);
    let mut digest = StateHasher::new();
    let mut out = String::new();
    let mut spawned = 0;
    for _ in 0..ticks {
        let t = world.tick().0;
        if (1_200..2_000).contains(&t) && t.is_multiple_of(4) {
            let ids: Vec<BuildingId> = world.buildings().iter().map(|(id, _)| id).collect();
            let len = u32::try_from(ids.len()).unwrap();
            for k in 0..3 {
                let mut r = rng(seed, RngPurpose::DailySchedule, k, Tick(t));
                let (a, b) = (ids[r.below(len) as usize], ids[r.below(len) as usize]);
                spawned += u32::from(world.spawn_drive(a, b).is_some());
            }
        }
        replay(&mut world, log, 1);
        let mut heading = std::collections::BTreeMap::<BuildingId, u32>::new();
        for (_, t) in world.trips().iter() {
            if let Some(b) = t
                .parking_lot()
                .filter(|b| world.buildings().get(*b).is_some())
            {
                *heading.entry(b).or_default() += 1;
            }
        }
        for (id, lot) in world.trips().lots() {
            let b = world
                .buildings()
                .get(id)
                .expect("消えた建物に駐車場が残った");
            assert!(u64::from(lot.parked + lot.reserved) <= b.parking_spaces());
            assert_eq!(lot.reserved, heading.remove(&id).unwrap_or(0), "{id:?}");
        }
        assert!(
            heading.is_empty(),
            "押さえた空きが数えられていない {heading:?}"
        );
        let h = world.state_hash();
        digest.write(&h);
        let t = world.tick().0;
        if t.is_multiple_of(250) || t == ticks {
            out.push_str(&format!("{t} {h:016x} {:016x}\n", digest.finish()));
        }
    }
    (out, spawned, world)
}

#[test]
fn driving_golden() {
    const SEED: u64 = 42;
    const TICKS: u64 = 2_600;
    let (got, spawned, end) = driving_checkpoints(SEED, TICKS);
    assert!(spawned > 300, "作れた移動が少ない {spawned}");
    // 朝の通勤の移動は数えない。
    let left = end
        .trips()
        .iter()
        .filter(|(id, _)| end.trips().commuter(*id).is_none())
        .count();
    assert!(left < 30, "着かずに残った移動が多い {left}");
    assert!(end.trips().lots().any(|(_, l)| l.parked > 0));
    assert_eq!(driving_checkpoints(SEED, TICKS).0, got);
    compare_golden("driving", &got);
}

/// 同じ道路の碁盤の目に区画を塗った記録。西の 2 列の区画は住宅、東の 2 列はオフィスと商業にする。
/// 区画の角から向かいの角へは、費用の同じ経路がたくさんある。
fn grid_city() -> CommandLog {
    let mut log = CommandLog::new();
    let mut rec = |tick: u64, c: Command| {
        log.record(Tick(tick), c).unwrap();
    };
    for i in 0..=4 {
        for (from, to) in [((0, i * 12), (48, i * 12)), ((i * 12, 0), (i * 12, 48))] {
            rec(
                1,
                Command::BuildRoad {
                    from: TilePos::new(from.0, from.1),
                    to: TilePos::new(to.0, to.1),
                    kind: RoadKind::Small,
                },
            );
        }
    }
    for bx in 0..4 {
        for by in 0..4 {
            let zone = match (bx, by % 2) {
                (0 | 1, _) => ZoneKind::Residential,
                (_, 0) => ZoneKind::Office,
                _ => ZoneKind::Commercial,
            };
            let area = TileRect::from_corners(
                TilePos::new(bx * 12 + 1, by * 12 + 1),
                TilePos::new(bx * 12 + 11, by * 12 + 11),
            );
            rec(
                2,
                Command::PaintZone {
                    area,
                    zone: Some(zone),
                },
            );
        }
    }
    for c in connect_to_outside(42, 0, 0, 48) {
        rec(3, c);
    }
    for (kind, x) in [(ServiceKind::PowerPlant, 3), (ServiceKind::WaterPump, 7)] {
        rec(
            3,
            Command::PlaceBuilding {
                kind,
                at: TilePos::new(x, -1),
                rotation: 2,
            },
        );
    }
    log
}

/// 碁盤の目の街で車を行き来させる。費用の同じ経路をどう散らすかを変えると、ハッシュが変わる。
#[test]
fn grid_driving_golden() {
    const SEED: u64 = 42;
    const TICKS: u64 = 2_600;
    let log = grid_city();
    let mut world = World::new(SEED);
    let stats = replay(&mut world, &log, 4);
    assert_eq!(stats.denied, 0);
    let (got, spawned, end) = driving_checkpoints_on(&log, SEED, TICKS);
    assert!(spawned > 300, "作れた移動が少ない {spawned}");
    assert_eq!(driving_checkpoints_on(&log, SEED, TICKS).0, got);
    compare_golden("grid_driving", &got);
    // 建物どうしをまとめて車で結ぶと、同じ 2 つのリンクを結ぶ車のうち、別の経路を通る組がある。
    let mut world = World::new(SEED);
    replay(&mut world, &log, 1_200);
    let ids: Vec<BuildingId> = world.buildings().iter().map(|(id, _)| id).collect();
    assert!(ids.len() > 20, "建物が少ない {}", ids.len());
    for a in &ids {
        for b in ids.iter().rev().take(5) {
            world.spawn_drive(*a, *b);
        }
    }
    // 歩いて車に乗り、車の区切りの経路を探してから走るので、しばらく進めながら車の経路を集める。
    // 一度に出た車で、交差点の先の車線が詰まる。路肩の車を置く決まりを変えると、ハッシュが変わる。
    let mut by_ends = std::collections::BTreeMap::new();
    let mut digest = StateHasher::new();
    let mut burst = String::new();
    for _ in 0..200 {
        replay(&mut world, &log, 1);
        let h = world.state_hash();
        digest.write(&h);
        let t = world.tick().0;
        if t.is_multiple_of(50) {
            burst.push_str(&format!("{t} {h:016x} {:016x}\n", digest.finish()));
        }
        for (_, t) in world
            .trips()
            .iter()
            .filter(|(_, t)| t.mode() == LaneMode::Car)
        {
            let links = &t.route().links;
            by_ends
                .entry((links[0], *links.last().unwrap()))
                .or_insert_with(std::collections::BTreeSet::new)
                .insert(links.clone());
        }
    }
    assert!(
        by_ends.values().any(|r| r.len() > 1),
        "同じ 2 つのリンクを結ぶ車が、みな同じ経路を通る"
    );
    compare_golden("grid_driving_burst", &burst);
    assert!(
        end.trips().len() < 200,
        "着かずに残った移動が多い {}",
        end.trips().len()
    );
}

/// 通勤のシナリオで、朝の車が走っている通りを `UPGRADE_TICK` に大きい道路へ上げる tick。
const UPGRADE_TICK: u64 = 2_100;

/// 通勤のシナリオの朝に、車の走る通りを大きい道路に上げ、少しあとに中の道路へ下げる記録。
/// 下げるときは通りの一部だけをなぞるが、種類は区間ごと変わる。
fn road_upgrade() -> CommandLog {
    let mut log = commute();
    let street = |from: i64, to: i64, kind| Command::UpgradeRoad {
        from: TilePos::new(from, 0),
        to: TilePos::new(to, 0),
        kind,
    };
    log.record(Tick(UPGRADE_TICK), street(-40, 100, RoadKind::Large))
        .unwrap();
    log.record(Tick(UPGRADE_TICK + 150), street(20, 60, RoadKind::Medium))
        .unwrap();
    log
}

/// 道路の種類を変えるシナリオで、上げると下げるの両方が適用される。
#[test]
fn road_upgrade_scenario_applies_both_changes() {
    let mut world = World::new(42);
    let stats = replay(&mut world, &road_upgrade(), UPGRADE_TICK + 151);
    assert_eq!(stats.denied, 0);
    let net = world.network();
    let kind_at = |x: i64| {
        let s = net.segments_through(TilePos::new(x, 0))[0];
        net.segment(s).unwrap().kind
    };
    for x in [-30, 30, 90] {
        assert_eq!(
            kind_at(x),
            chizu_core::SegmentKind::Road(RoadKind::Medium),
            "{x}"
        );
    }
}

/// 朝の通勤の車が走る通りを大きい道路に上げても、建物と区画が残り、走っていた車が車線の上を進み続ける。
#[test]
fn upgrading_a_busy_street_keeps_buildings_and_moving_cars() {
    let mut world = World::new(42);
    replay(&mut world, &commute(), UPGRADE_TICK);
    let buildings: Vec<_> = world
        .buildings()
        .iter()
        .map(|(id, b)| (id, b.area, b.zone, b.level))
        .collect();
    assert!(buildings.len() >= 20, "建物が少ない {}", buildings.len());
    let band = TileRect::from_corners(TilePos::new(-40, -8), TilePos::new(100, 8));
    let zones = |w: &World| {
        (band.min.y..=band.max.y)
            .flat_map(|y| (band.min.x..=band.max.x).map(move |x| TilePos::new(x, y)))
            .map(|t| w.zone_at(t))
            .collect::<Vec<_>>()
    };
    let zones_before = zones(&world);
    let on_street = |w: &World| -> Vec<(chizu_core::TripId, SidewalkPos)> {
        w.trips()
            .iter()
            .filter(|(_, t)| t.mode() == LaneMode::Car && t.position().tile.y == 0)
            .map(|(id, t)| (id, t.position()))
            .collect()
    };
    let cars = on_street(&world);
    assert!(cars.len() >= 10, "通りを走る車が少ない {}", cars.len());

    let line = chizu_core::Line::between(TilePos::new(-40, 0), TilePos::new(100, 0)).unwrap();
    let cost = world
        .network()
        .plan_road_kind(&line, RoadKind::Large)
        .unwrap()
        .cost;
    let money = world.money();
    let up = Command::UpgradeRoad {
        from: line.start,
        to: line.end(),
        kind: RoadKind::Large,
    };
    let view = TileRect::from_corners(TilePos::new(-64, -64), TilePos::new(127, 127));
    let shifts = |w: &World| {
        w.movers_in(view)
            .into_iter()
            .map(|m| (m.id, (m.pos, m.shift)))
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let shown = shifts(&world);
    assert_eq!(world.apply(&up), chizu_core::ApplyOutcome::Applied);
    // 車も歩く人も、種類を変えた直後は直前と同じ所に描く。道路の真ん中へ飛ばない。
    let now = shifts(&world);
    let mut compared = 0;
    for (id, before) in &shown {
        if let Some(after) = now.get(id) {
            assert_eq!(after, before, "{id:?} が飛んで見える");
            compared += 1;
        }
    }
    assert!(compared >= 10, "比べた移動体が少ない {compared}");
    assert_eq!(world.money(), money - cost);
    // 小 250、大 750 の 1 マスあたりの差額を、通りの 140 マスぶん払う。
    assert_eq!(cost, Money(500 * 140));

    let after: Vec<_> = world
        .buildings()
        .iter()
        .map(|(id, b)| (id, b.area, b.zone, b.level))
        .collect();
    assert_eq!(after, buildings, "建物が変わった");
    assert_eq!(zones(&world), zones_before, "区画が変わった");
    // 通りに面した建物の出入口は、上げた道路の歩道に付け直してある。
    let net = world.network();
    for (_, b) in world.buildings().iter() {
        let seg = net
            .segment(b.entrance.segment)
            .expect("出入口が消えた区間を指す");
        if b.front.y.abs() <= 6 {
            assert_eq!(seg.kind, chizu_core::SegmentKind::Road(RoadKind::Large));
        }
    }

    // 上げたあとも、どの車もいまのリンクの車線の上にいて、通りにいた車はどれも動くか着く。
    let mut moved: std::collections::BTreeMap<_, _> =
        cars.iter().map(|(id, pos)| (*id, (*pos, false))).collect();
    for _ in 0..200 {
        replay(&mut world, &commute(), 1);
        let net = world.network();
        for (id, t) in world.trips().iter() {
            let lane = net.lane(t.lane()).expect("車が消えた車線にいる");
            let link = t.link();
            assert_eq!(
                (lane.segment, lane.dir, lane.mode),
                (link.segment, link.dir, t.mode()),
                "{id:?}"
            );
        }
        for (id, (pos, done)) in moved.iter_mut() {
            let now = world.trips().get(*id).map(|t| (t.mode(), t.position()));
            if now != Some((LaneMode::Car, *pos)) {
                *done = true;
            }
        }
    }
    let stuck: Vec<_> = moved
        .iter()
        .filter(|(_, (_, d))| !d)
        .map(|(id, _)| id)
        .collect();
    assert!(stuck.is_empty(), "止まったままの車 {stuck:?}");
}
