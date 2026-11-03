//! 画面に描く移動体を、中核の `World::movers_in` から三重バッファの形に直す。
//! 開発用に、記録に入れずに歩く人を出す入口もここに置く。

use chizu_core::network::{DIAGONAL_TILE_MM, TILE_MM};
use chizu_core::{
    CHUNK_SIDE, ChunkCoord, Dir, LaneMode, LaneOffset, LinkId, Mover as CoreMover, SidewalkPos,
    TilePos, TileRect, World,
};
use std::collections::BTreeMap;

use crate::car_path;
use crate::snapshot::{MAX_MOVERS, Mover};
use crate::walk_path::{self, Walk};

/// 移動体の種類の番号。画面の `web/src/map/movers.ts` と同じ。
pub const KIND_WALK: u16 = 1;
pub const KIND_CAR: u16 = 2;

/// 移動体の手段を種類の番号に直す。
fn kind_of(m: &CoreMover) -> u16 {
    match m.mode {
        LaneMode::Sidewalk => KIND_WALK,
        LaneMode::Car => KIND_CAR,
    }
}

/// 8 方向の向きを、x 軸から y 軸（画面の下）へ回る角度に直す。1 周を 65536 とする。
fn heading_code((x, y): (i8, i8)) -> u16 {
    const EIGHTH: u16 = 8192;
    let octant = match (x.signum(), y.signum()) {
        (1, 0) | (0, 0) => 0,
        (1, 1) => 1,
        (0, 1) => 2,
        (-1, 1) => 3,
        (-1, 0) => 4,
        (-1, -1) => 5,
        (0, -1) => 6,
        _ => 7,
    };
    octant * EIGHTH
}

/// `found` が `limit` 体を超えたら、`view` の真ん中に近い `limit` 体だけを残す。同じ近さなら ID の小さいほうを残す。
fn keep_nearest(found: &mut Vec<CoreMover>, view: TileRect, limit: usize) {
    if found.len() <= limit {
        return;
    }
    let center = |a: i64, b: i64| (i128::from(a) + i128::from(b) + 1) * i128::from(TILE_MM) / 2;
    let (cx, cy) = (
        center(view.min.x, view.max.x),
        center(view.min.y, view.max.y),
    );
    let key = |m: &CoreMover| {
        let dx = i128::from(m.pos.x.0) - cx;
        let dy = i128::from(m.pos.y.0) - cy;
        (dx * dx + dy * dy, m.id)
    };
    if limit > 0 {
        found.select_nth_unstable_by_key(limit - 1, key);
    }
    found.truncate(limit);
}

/// 車ごとに、次の回に描くために覚えておくもの。描画のためだけに覚える。
#[derive(Clone, Copy, Debug)]
pub struct Memo {
    /// 描いた tick と、そのとき描いた道筋。
    tick: u64,
    last: Option<car_path::Last>,
    /// `tick` の前の tick に描いた道筋。
    before: Option<car_path::Last>,
}

impl Memo {
    /// 何も覚えていない覚え書き。
    const NONE: Memo = Memo {
        tick: u64::MAX,
        last: None,
        before: None,
    };
}

/// 移動体を描くために、回をまたいで覚えておくもの。描画のためだけに覚え、状態には入れない。
#[derive(Default)]
pub struct Memory {
    /// 車ごとの覚え書き。移動の添え字と世代で引く。並びは描く結果に効かない。
    arcs: car_path::FastMap<(u32, u16), Memo>,
    /// 曲線の上に描いた車の、曲線の始まり。曲線の上にいる車は少ないので、覚え書きとは分けて持つ。
    entries: car_path::FastMap<(u32, u16), (u64, car_path::Entry)>,
    /// 前の回に描いた tick。
    drawn_tick: Option<u64>,
    /// 求めた道筋。
    pub paths: car_path::Paths,
}

/// 前の回に描いた移動体。`out` は前の回の `visible_movers` の結果で、`base` はそのときの基準のチャンクである。
pub struct Before<'a> {
    pub out: &'a [Mover],
    pub base: ChunkCoord,
}

impl Before<'_> {
    /// 前の回に描いた移動体の、ID ごとの位置（基準のチャンク `base` の原点からのマスの座標）と向き。
    fn positions(&self, base: ChunkCoord) -> BTreeMap<(u32, u32), (f32, f32, u16)> {
        let side = CHUNK_SIDE as i64;
        let shift = |from: i32, to: i32| ((i64::from(from) - i64::from(to)) * side) as f32;
        let (dx, dy) = (shift(self.base.cx, base.cx), shift(self.base.cy, base.cy));
        self.out
            .iter()
            .map(|m| ((m.id, m.generation), (m.x + dx, m.y + dy, m.heading)))
            .collect()
    }
}

/// `view` の移動体を、基準のチャンク（`view.min` のチャンク）の原点からのマスの座標に直して `out` に入れ、
/// 基準のチャンクを返す。上限を超えたら、`view` の真ん中に近いものを残す。
/// `memory` は前の回までに覚えた車ごとの覚え書きと道筋で、この回のものに入れ替える。
/// `before` は前の回に描いたもので、網が変わってから次の `step` までのあいだ、道筋を求められない歩く人を
/// そこに描き続ける。
pub fn visible_movers(
    world: &World,
    view: TileRect,
    out: &mut Vec<Mover>,
    memory: &mut Memory,
    before: Before<'_>,
) -> ChunkCoord {
    let base = view.min.chunk().unwrap_or_default();
    let mut found = world.movers_in(view);
    keep_nearest(&mut found, view, MAX_MOVERS);
    let net = world.network();
    let walks = |id| world.walk_path(id);
    let scene = Scene {
        net,
        tick: world.tick().0,
        walks: &walks,
    };
    draw_movers(&scene, &found, base, out, memory, before);
    base
}

/// 移動体を描くのに使う、街の側のもの。
struct Scene<'a> {
    net: &'a chizu_core::Network,
    tick: u64,
    walks: &'a dyn Fn(chizu_core::TripId) -> Option<chizu_core::WalkPath>,
}

/// `found` の移動体を、基準のチャンク `base` の原点からのマスの座標に直して `out` に入れる。
fn draw_movers(
    scene: &Scene<'_>,
    found: &[CoreMover],
    base: ChunkCoord,
    out: &mut Vec<Mover>,
    memory: &mut Memory,
    before: Before<'_>,
) {
    let side = CHUNK_SIDE as i64;
    let origin = |c: i32| i64::from(c) * side * TILE_MM;
    let (ox, oy) = (origin(base.cx), origin(base.cy));
    // 基準の原点からの差は画面の広さに収まるので、f64 を経て f32 に直しても桁は足りる。
    let tiles = |v: i64, o: i64| (v.saturating_sub(o) as f64 / TILE_MM as f64) as f32;
    let net = scene.net;
    let tick = scene.tick;
    let from_mm = |v: f64, o: i64| ((v - o as f64) / TILE_MM as f64) as f32;
    memory.paths.sync(net, tick);
    // 覚え書きは、前の回か、この回と同じ tick に描いたものだけを使う。見えなくなった車の分は、
    // 溜まったら捨てる。
    let valid = |m: &Memo| m.tick == tick || Some(m.tick) == memory.drawn_tick;
    if memory.arcs.len() > 2 * found.len() + 4_096 {
        memory.arcs.retain(|_, m| valid(m));
    }
    let drawn_tick = memory.drawn_tick;
    memory
        .entries
        .retain(|_, (t, _)| *t == tick || Some(*t) == drawn_tick);
    let arcs = &mut memory.arcs;
    let entries = &mut memory.entries;
    let paths = &mut memory.paths;
    // 前の回の位置は、道筋を求められない人がいたときにだけ引く。
    let mut prev = None;
    out.clear();
    let drawn = out;
    for m in found {
        let mover = |x: f32, y: f32, heading: u16| Mover {
            x,
            y,
            heading,
            kind: kind_of(m),
            id: m.id.index,
            generation: u32::from(m.id.generation),
            path: 0,
            via: 0,
            frac: 0.0,
        };
        // 車は、ノードの前後を曲線でつないだ道筋の上に描く（`car_path`）。
        let car = m.path.and_then(|p| {
            let id = (m.id.index, m.id.generation);
            let slot = arcs.entry(id).or_insert(Memo::NONE);
            let memo = Some(*slot).filter(valid);
            // 同じ tick に描き直すときは、前の tick に描いた道筋を引き継ぐ。
            let last = memo.and_then(|k| if k.tick == tick { k.before } else { k.last });
            let entry = entries.get(&id).map(|(_, e)| *e);
            let placed = car_path::draw(net, paths, &p, m.shift, entry.as_ref(), last.as_ref());
            match placed.and_then(|p| p.entry) {
                Some(e) => {
                    entries.insert(id, (tick, e));
                }
                None if entry.is_some() => {
                    entries.remove(&id);
                }
                None => {}
            }
            *slot = Memo {
                tick,
                last: placed.and_then(|p| p.route.map(|r| r.last)),
                before: last,
            };
            placed
        });
        if let Some(placed) = car {
            let d = placed.drawn;
            let mut out = mover(
                from_mm(d.x, ox),
                from_mm(d.y, oy),
                car_path::angle_code(d.angle),
            );
            if let Some(r) = placed.route {
                (out.path, out.via, out.frac) = (r.id, r.via, r.frac);
            }
            drawn.push(out);
            continue;
        }
        // 歩く人は、交差点の前後で歩道の角を回る道筋の上に描く（`walk_path`）。道筋は、間引いたあとに
        // 残った人の分だけ求める。
        let (walk, route) = match m.mode {
            LaneMode::Sidewalk => (scene.walks)(m.id).map_or((Walk::Core, None), |w| {
                (walk_path::draw(net, &w), walk_path::route(net, paths, &w))
            }),
            LaneMode::Car => (Walk::Core, None),
        };
        let with_route = |mut out: Mover| {
            if let Some(r) = route {
                (out.path, out.frac) = (r.id, r.frac);
            }
            out
        };
        if let Walk::Lost(_) = walk {
            let prev = prev.get_or_insert_with(|| before.positions(base));
            if let Some(&(x, y, heading)) = prev.get(&(m.id.index, u32::from(m.id.generation))) {
                drawn.push(mover(x, y, heading));
                continue;
            }
        }
        if let Some(d) = walk.position() {
            drawn.push(with_route(mover(
                from_mm(d.x, ox),
                from_mm(d.y, oy),
                car_path::angle_code(d.angle),
            )));
            continue;
        }
        // 中心線の上の位置に、車線や歩道に寄せるずれを足して描く。
        drawn.push(with_route(mover(
            tiles(m.pos.x.0.saturating_add(m.shift.0.0), ox),
            tiles(m.pos.y.0.saturating_add(m.shift.1.0), oy),
            heading_code(m.heading),
        )));
    }
    memory.drawn_tick = Some(tick);
}

/// マス `t` の近くの歩道の位置を、反対側の歩道も含めて挙げる。
/// 道路のマスならその道路の両側の歩道、建物のマスならその建物の入口の歩道である。
fn sidewalks_at(world: &World, t: TilePos) -> Vec<(LinkId, LaneOffset)> {
    let net = world.network();
    let building = world.buildings().iter().find(|(_, b)| {
        (b.area.min.x..=b.area.max.x).contains(&t.x) && (b.area.min.y..=b.area.max.y).contains(&t.y)
    });
    let (road, links): (TilePos, Vec<LinkId>) = match building {
        Some((_, b)) => (
            TilePos::new(b.front.x + b.toward_road.0, b.front.y + b.toward_road.1),
            vec![b.entrance],
        ),
        None => (
            t,
            net.segments_through(t)
                .into_iter()
                .flat_map(|segment| {
                    [Dir::Forward, Dir::Backward].map(|dir| LinkId {
                        segment,
                        dir,
                        mode: LaneMode::Sidewalk,
                    })
                })
                .collect(),
        ),
    };
    links
        .into_iter()
        .filter_map(|link| {
            if !net.segment(link.segment)?.kind.has_sidewalks() {
                return None;
            }
            let line = net.segment_line(link.segment)?;
            let k = line.position_of(road)?;
            let steps = match link.dir {
                Dir::Forward => k,
                Dir::Backward => line.len - k,
            };
            let per = if line.is_diagonal() {
                DIAGONAL_TILE_MM
            } else {
                TILE_MM
            };
            let offset = u32::try_from(steps.checked_mul(per as u64)?).ok()?;
            Some((link, LaneOffset(offset)))
        })
        .collect()
}

/// マス `from` と `to` のあいだに、歩く人を `count` 人出し、出せた人数を返す。
/// 記録に入れないので、これで出した人はフィードバックの記録を再生しても現れない。
pub fn spawn_dev_walks(world: &mut World, from: TilePos, to: TilePos, count: u32) -> u32 {
    let starts = sidewalks_at(world, from);
    let ends = sidewalks_at(world, to);
    if starts.is_empty() || ends.is_empty() {
        return 0;
    }
    // 同じ所に重ならないよう、1 人ずつ 1 マスの中で少しずらす。
    let spread = |(link, offset): (LinkId, LaneOffset), i: u32| {
        let net = world.network();
        let len = u32::try_from(net.segment_line(link.segment)?.length().0).ok()?;
        let at = offset.0.saturating_add(i % 8 * 900).min(len);
        SidewalkPos::on_link(net, link, LaneOffset(at))
    };
    let walks: Vec<(SidewalkPos, SidewalkPos)> = (0..count)
        .filter_map(|i| {
            let a = starts[i as usize % starts.len()];
            let b = ends[i as usize / starts.len() % ends.len()];
            Some((spread(a, i)?, spread(b, i)?))
        })
        .collect();
    let spawned = walks
        .into_iter()
        .filter(|&(a, b)| world.spawn_walk(a, b).is_some())
        .count();
    u32::try_from(spawned).expect("出した人数が u32 に収まらない")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::{Command, RoadKind};

    fn world_with_road() -> World {
        let mut w = World::new(1);
        let road = Command::BuildRoad {
            from: TilePos::new(0, 10),
            to: TilePos::new(30, 10),
            kind: RoadKind::Small,
        };
        w.apply_recorded(0, &road);
        w
    }

    fn none() -> Before<'static> {
        Before {
            out: &[],
            base: ChunkCoord::default(),
        }
    }

    fn all() -> TileRect {
        TileRect {
            min: TilePos::new(-64, -64),
            max: TilePos::new(127, 127),
        }
    }

    #[test]
    fn spawned_walkers_move_and_arrive() {
        let mut w = world_with_road();
        let n = spawn_dev_walks(&mut w, TilePos::new(2, 10), TilePos::new(25, 10), 30);
        assert_eq!(n, 30);
        let mut first = Vec::new();
        let base = visible_movers(&w, all(), &mut first, &mut Memory::default(), none());
        assert_eq!(base, ChunkCoord::new(-1, -1));
        assert_eq!(first.len(), 30);
        assert!(first.iter().all(|m| m.kind == KIND_WALK));
        // 基準のチャンクの原点は (-64, -64) なので、道路の y = 10 のマスは 74 の辺りにある。
        assert!(first.iter().all(|m| (74.0..75.0).contains(&m.y)));
        // 歩く人は歩道の上にいる。小さい道路の歩道は中心線から 0.26 マス離れる。出発の位置の歩道は
        // 両側にあるので、どちらの側を歩く人もいる。
        assert!(
            first
                .iter()
                .all(|m| (m.y - 74.76).abs() < 0.001 || (m.y - 74.24).abs() < 0.001)
        );
        for _ in 0..20 {
            w.step();
        }
        let mut later = Vec::new();
        visible_movers(&w, all(), &mut later, &mut Memory::default(), none());
        assert_ne!(first, later);
        for _ in 0..2_000 {
            w.step();
        }
        visible_movers(&w, all(), &mut later, &mut Memory::default(), none());
        assert!(later.is_empty());
    }

    #[test]
    fn nothing_spawns_off_the_road() {
        let mut w = world_with_road();
        assert_eq!(
            spawn_dev_walks(&mut w, TilePos::new(2, 20), TilePos::new(25, 10), 5),
            0
        );
    }

    #[test]
    fn over_the_limit_keeps_the_nearest_and_breaks_ties_by_id() {
        use chizu_core::{Mm, TripId, WorldPos};
        // 範囲の真ん中はマス (10, 10) の中心、ワールド座標で (84_000, 84_000) mm にある。
        let view = TileRect {
            min: TilePos::new(0, 0),
            max: TilePos::new(20, 20),
        };
        let at = |index: u32, dx: i64| CoreMover {
            id: TripId {
                index,
                generation: 0,
            },
            pos: WorldPos::new(Mm(84_000 + dx), Mm(84_000)),
            heading: (1, 0),
            mode: LaneMode::Sidewalk,
            shift: (Mm(0), Mm(0)),
            path: None,
        };
        let mut found = vec![
            at(0, 9_000),
            at(1, -100),
            at(2, 5_000),
            at(3, 100),
            at(4, 50),
        ];
        keep_nearest(&mut found, view, 3);
        let mut kept: Vec<u32> = found.iter().map(|m| m.id.index).collect();
        kept.sort_unstable();
        assert_eq!(kept, [1, 3, 4]);

        let mut tied = vec![at(5, 100), at(2, -100), at(7, 100)];
        keep_nearest(&mut tied, view, 1);
        assert_eq!(tied[0].id.index, 2);

        let mut few = vec![at(0, 9_000)];
        keep_nearest(&mut few, view, 3);
        assert_eq!(few.len(), 1);
    }

    #[test]
    fn cars_and_walkers_get_their_own_kinds() {
        use chizu_core::{Mm, TripId, WorldPos};
        let m = |mode| CoreMover {
            id: TripId {
                index: 0,
                generation: 0,
            },
            pos: WorldPos::new(Mm(0), Mm(0)),
            heading: (0, 1),
            mode,
            shift: (Mm(0), Mm(0)),
            path: None,
        };
        assert_eq!(kind_of(&m(LaneMode::Sidewalk)), KIND_WALK);
        assert_eq!(kind_of(&m(LaneMode::Car)), KIND_CAR);
    }

    #[test]
    fn headings_turn_toward_screen_down() {
        assert_eq!(heading_code((1, 0)), 0);
        assert_eq!(heading_code((0, 1)), 16_384);
        assert_eq!(heading_code((-1, 0)), 32_768);
        assert_eq!(heading_code((1, -1)), 57_344);
    }
}
