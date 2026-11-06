//! 環状の線路に駅を並べ、環状の路線と往復の路線がホームと分岐を共有する形のテスト。

use std::collections::BTreeMap;

use super::{Xorshift, at, check_blocks, create, ok, rail, road, run, set_trains, station};
use crate::command::Command;
use crate::rail::{LineId, StationId, TrainId, TrainState};
use crate::tile::{ChunkCoord, TileRect};
use crate::units::Money;
use crate::world::{ApplyOutcome, World};

/// 外を回る環状の線路に駅を 7 つ（y = 8 の下の段に 4 つ、y = -22 の上の段に 3 つ）並べた街。駅の ID は、下の段の
/// 西から東、上の段の東から西の順である。`chord` なら、下の段の x 52 から上の段の x 72 へ渡り線を足す。
/// `crossing` なら、y -7 の横の道路で、環状の線路の西と東の縦の区間に踏切を 1 つずつ置く。
fn ring_world(chord: bool, crossing: bool) -> (World, Vec<StationId>) {
    let mut w = World::new_with_money(6, Money(1_000_000_000_000)).with_unlimited_land();
    w.lift_turn_caps(true);
    for cx in -2..=4 {
        for cy in -2..=1 {
            let _ = w.apply(&Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            });
        }
    }
    ok(&mut w, road((0, 10), (130, 10)));
    ok(&mut w, road((0, -24), (130, -24)));
    let mut st = Vec::new();
    for x in [20, 45, 70, 95] {
        st.push(station(&mut w, (x, 8), 2));
    }
    for x in [95, 60, 30] {
        st.push(station(&mut w, (x, -22), 0));
    }
    for (from, to) in [
        ((20, 8), (36, 8)),
        ((45, 8), (61, 8)),
        ((70, 8), (86, 8)),
        ((95, 8), (110, 8)),
        ((110, 8), (120, -2)),
        ((120, -2), (120, -12)),
        ((120, -12), (110, -22)),
        ((110, -22), (95, -22)),
        ((86, -22), (60, -22)),
        ((51, -22), (30, -22)),
        ((21, -22), (5, -22)),
        ((5, -22), (-5, -12)),
        ((-5, -12), (-5, -2)),
        ((-5, -2), (5, 8)),
        ((5, 8), (11, 8)),
    ] {
        ok(&mut w, rail(from, to));
    }
    if chord {
        ok(&mut w, rail((52, 8), (62, -2)));
        ok(&mut w, rail((62, -2), (62, -12)));
        ok(&mut w, rail((62, -12), (72, -22)));
    }
    if crossing {
        ok(&mut w, road((-10, -7), (0, -7)));
        ok(&mut w, road((115, -7), (125, -7)));
        assert!(w.rail().is_crossing(at(-5, -7)) && w.rail().is_crossing(at(120, -7)));
    }
    (w, st)
}

/// 環状の路線 1（下の段の西の 2 駅と上の段の西の駅を回る）と、同じ下の段の 2 駅と上の段の東の駅を往復する
/// 路線 2 の街。`spur` なら、西の駅の東 7 マスに分岐を作る。西の駅は路線 2 の終点で、そこへ入る列車は、分岐、
/// 終点のホーム、折り返した先のホームをまとめて押さえる。折り返した先のホームは、路線 1 の列車が止まって
/// 待てる所でもある。
fn terminal_on_ring(spur: bool, n1: u8, n2: u8) -> World {
    let (mut w, st) = ring_world(true, false);
    if spur {
        ok(&mut w, rail((27, 8), (30, 5)));
    }
    let one = create(&mut w, st[0], st[1]);
    for s in [st[6], st[0]] {
        ok(
            &mut w,
            Command::ExtendLine {
                line: one,
                station: s,
            },
        );
    }
    let two = create(&mut w, st[0], st[1]);
    ok(
        &mut w,
        Command::ExtendLine {
            line: two,
            station: st[4],
        },
    );
    set_trains(&mut w, one, n1);
    set_trains(&mut w, two, n2);
    w
}

/// ほかの路線が止まって待つホームへ、分岐のすぐ先の短い線路を通って入る終点の列車が、閉塞が同時に空くのを
/// 待ち続けない。先に待った列車が要る閉塞を、同じ tick の後の列車に渡さないので、どの列車も 1,000 tick 以内に
/// 駅を出る。直す前は、分岐のある形で路線 2 の列車が tick 530 ごろから動かなくなった。
#[test]
fn a_terminal_on_a_ring_line_does_not_starve() {
    for spur in [true, false] {
        for (n1, n2) in [(8, 8), (8, 1), (4, 1), (2, 1)] {
            let mut w = terminal_on_ring(spur, n1, n2);
            run(&mut w, 8_000, 1_000);
        }
    }
}

/// 列車の動きを tick ごとに確かめる。どの列車も `limit` tick 以内に駅を出て、続けて先を待つのは `waited` tick
/// までで、上限が 1 以上の路線には `limit` tick 以内に列車が現れる。閉塞の重なりは [`check_blocks`] が panic で
/// 知らせる。駅を出るまでの tick には、間を空ける決まりで待つ時間（環状の長い路線で 1 本なら 1,000 tick を
/// 超える）も入るので、閉塞を待ち続ける列車は `waited` で見る。続けて待った長さの最大と、駅を出た数も数える。
#[derive(Default)]
struct Watch {
    last: BTreeMap<TrainId, (u64, TrainState)>,
    empty_since: BTreeMap<LineId, u64>,
    most_waited: u32,
    departures: u64,
}

impl Watch {
    fn check(&mut self, w: &World, limit: u64, waited: u32) -> Result<(), String> {
        check_blocks(w);
        let rail = w.rail();
        let now = w.tick().0;
        if rail.tally().deadlock != 0 {
            return Err(format!("tick {now}: 備えが働いた"));
        }
        let mut next = BTreeMap::new();
        for (id, t) in rail.trains() {
            let since = match self.last.get(&id) {
                Some((_, before))
                    if *before != TrainState::Running && t.state == TrainState::Running =>
                {
                    self.departures += 1;
                    now
                }
                Some((since, _)) => *since,
                None => now,
            };
            self.most_waited = self.most_waited.max(t.waited);
            if now - since > limit || t.waited > waited {
                return Err(format!(
                    "tick {now}: {id:?} が {} tick 駅を出ていない {t:?}",
                    now - since
                ));
            }
            next.insert(id, (since, t.state));
        }
        self.last = next;
        for (lid, l) in rail.lines() {
            if l.allowed() == 0 || rail.trains().any(|(_, t)| t.line == lid) {
                self.empty_since.remove(&lid);
                continue;
            }
            let s = *self.empty_since.entry(lid).or_insert(now);
            if now - s > limit {
                return Err(format!(
                    "tick {now}: {lid:?} は上限 {} なのに {} tick 列車がいない",
                    l.allowed(),
                    now - s
                ));
            }
        }
        Ok(())
    }
}

fn pick(r: &mut Xorshift, n: usize) -> usize {
    r.below(n as u64) as usize
}

/// [`ring_world`] に路線を 2〜5 本（駅 2〜4 つ、3 分の 1 は環状、本数 1〜16）置き、1,000 tick ごとに線路を分ける、
/// 枝を取り壊す、本数を変える、路線を延ばす、作る、取り除くのどれかをして、12,000 tick 回す。`zero` なら、始めに
/// 置く路線の 3 分の 1 を本数 0（使っていない路線）にする。
fn ring_fuzz(
    seed: u64,
    crossing: bool,
    zero: bool,
    limit: u64,
    waited: u32,
    watch: &mut Watch,
) -> Result<(), String> {
    let mut r = Xorshift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let chord = r.below(2) == 0;
    let (mut w, st) = ring_world(chord, crossing);
    let mut ids = Vec::new();
    for _ in 0..2 + r.below(4) {
        let mut pool = st.clone();
        let mut chosen = Vec::new();
        for _ in 0..2 + r.below(3) {
            chosen.push(pool.remove(pick(&mut r, pool.len())));
        }
        if w.apply(&Command::CreateLine {
            from: chosen[0],
            to: chosen[1],
        }) != ApplyOutcome::Applied
        {
            continue;
        }
        let id = w.rail().lines().last().unwrap().0;
        for s in &chosen[2..] {
            let _ = w.apply(&Command::ExtendLine {
                line: id,
                station: *s,
            });
        }
        if r.below(3) == 0 {
            let _ = w.apply(&Command::ExtendLine {
                line: id,
                station: chosen[0],
            });
        }
        let trains = if zero && r.below(3) == 0 {
            0
        } else {
            1 + r.below(16) as u8
        };
        let _ = w.apply(&Command::SetLineTrains { line: id, trains });
        ids.push(id);
    }
    for tick in 0..12_000u64 {
        if tick % 1_000 == 500 {
            let kind = r.below(6);
            let x = 1 + r.below(118) as i64;
            let row = if r.below(2) == 0 { 8 } else { -22 };
            let dy = if row == 8 { -3 } else { 3 };
            let c = match kind {
                1 => Command::Bulldoze {
                    area: TileRect::from_corners(
                        at(x, row.min(row + dy)),
                        at(x + 4, row.max(row + dy)),
                    ),
                },
                2 if !ids.is_empty() => Command::SetLineTrains {
                    line: ids[pick(&mut r, ids.len())],
                    trains: r.below(17) as u8,
                },
                3 if !ids.is_empty() => Command::ExtendLine {
                    line: ids[pick(&mut r, ids.len())],
                    station: st[pick(&mut r, st.len())],
                },
                4 => Command::CreateLine {
                    from: st[pick(&mut r, st.len())],
                    to: st[pick(&mut r, st.len())],
                },
                5 if !ids.is_empty() => Command::RemoveLine {
                    line: ids[pick(&mut r, ids.len())],
                },
                _ => rail((x, row), (x + 3, row + dy)),
            };
            if w.apply(&c) == ApplyOutcome::Applied
                && let Command::CreateLine { .. } = c
            {
                let id = w.rail().lines().last().unwrap().0;
                let _ = w.apply(&Command::SetLineTrains {
                    line: id,
                    trains: 1 + r.below(16) as u8,
                });
                ids.push(id);
            }
            ids.retain(|id| w.rail().line(*id).is_some());
            check_blocks(&w);
        }
        w.step();
        watch
            .check(&w, limit, waited)
            .map_err(|e| format!("種 {seed}（渡り線 {chord}）{e}"))?;
    }
    Ok(())
}

/// 環状の線路のファズで、直す前に列車が先を待ち続けた種。11 と 58 は分岐のすぐ先にある、環状の路線と
/// 共有する終点の形で、377 と 390 は往復の路線が環状の路線の流れに合流する所で約 2,800 tick 止まった。
/// 直したあとは、どの列車も続けて 1,000 tick は待たない。
#[test]
fn trains_on_a_ring_with_shared_terminals_keep_departing() {
    for seed in [11, 58, 377, 390] {
        ring_fuzz(seed, false, false, 3_000, 1_000, &mut Watch::default()).unwrap();
    }
}

/// 踏切を置いた環状の線路でも、閉塞が重ならず、備えが働かず、列車が駅を出続ける。
#[test]
fn trains_on_a_ring_with_crossings_keep_departing() {
    for seed in [0, 1, 2] {
        ring_fuzz(seed, true, false, 3_000, 3_000, &mut Watch::default()).unwrap();
    }
}

/// 本数 0 の路線（区画の枠の分が 0）がまざっても、閉塞が重ならず、備えが働かず、列車が駅を出続ける（#344）。
#[test]
fn trains_on_a_ring_with_unused_lines_keep_departing() {
    for seed in [0, 1, 2] {
        ring_fuzz(seed, false, true, 3_000, 3_000, &mut Watch::default()).unwrap();
    }
}

/// 環状の線路のファズを `RING_FROM` から `RING_N` 種回し、閉塞の重なりか panic、備え、`RING_LIMIT` tick 駅を
/// 出ないか `RING_WAITED`（既定は `RING_LIMIT`）tick 続けて先を待つ列車か、上限が 1 以上なのに `RING_LIMIT` tick
/// 列車がいない路線があった種と、続けて待った長さが 1,000 tick 以上になった種の数、駅を出た数の和を出す。
/// `RING_CROSSING` があれば踏切を置き、`RING_ZERO` があれば始めに置く路線の 3 分の 1 を本数 0 にする。手で回す。
#[test]
#[ignore = "長いファズは手で回す"]
fn ring_fuzz_many() {
    let env = |name: &str, d: u64| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(d)
    };
    let from = env("RING_FROM", 0);
    let n = env("RING_N", 50);
    let limit = env("RING_LIMIT", 3_000);
    let crossing = std::env::var("RING_CROSSING").is_ok();
    let zero = std::env::var("RING_ZERO").is_ok();
    // `RING_SEEDS`（コンマ区切り）があれば、その種だけを回す。
    let seeds: Vec<u64> = match std::env::var("RING_SEEDS") {
        Ok(v) => v.split(',').map(|s| s.trim().parse().unwrap()).collect(),
        Err(_) => (from..from + n).collect(),
    };
    let waited = u32::try_from(env("RING_WAITED", limit)).unwrap();
    let mut fails = Vec::new();
    let mut long_waits = 0;
    let mut most = 0;
    let mut departures = 0;
    for seed in seeds {
        let mut watch = Watch::default();
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ring_fuzz(seed, crossing, zero, limit, waited, &mut watch)
        }));
        long_waits += u32::from(watch.most_waited >= 1_000);
        most = most.max(watch.most_waited);
        departures += watch.departures;
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                eprintln!("FAIL {e}");
                fails.push(seed);
            }
            Err(_) => {
                eprintln!("PANIC 種 {seed}");
                fails.push(seed);
            }
        }
    }
    eprintln!(
        "RESULT fails {fails:?} long_waits {long_waits} most_waited {most} departures {departures}"
    );
    assert!(fails.is_empty());
}
