//! 建物が建つ・育つ・消える処理。
//!
//! 建物は、道路に縦か横で面した区画のマスを間口にして、奥へ向かって建つ。間口と奥行きは
//! 1〜[`MAX_LOT_SIDE`] マスで、種から引いた大きさが収まらなければ小さくする。斜めの道路沿いには
//! まだ建たない。道路か区画が消えて建物の条件が崩れたら、建物も消える。

use std::collections::{BTreeMap, BTreeSet};

use super::World;
use crate::building::{Building, MAX_LEVEL, MAX_LOT_SIDE, demand};
use crate::chunk::{Layer, TileUse};
use crate::clock::TICKS_PER_DAY;
use crate::command::ZoneKind;
use crate::id::BuildingId;
use crate::network::{Dir, LaneMode, LinkId};
use crate::rng::{RngPurpose, rng};
use crate::tile::{CHUNK_SIDE, ChunkCoord, TilePos, TileRect, local_index};
use crate::zone::ZONE_DEPTH;

/// 建物が建つ・育つかを見る間隔（tick）。等速で 1 秒に 1 回である。
pub const GROWTH_INTERVAL: u64 = 30;

/// 建物の奥行きの上限（マス）。区画の奥行きいっぱいまで建つので、奥に空きのマスが残らない。
pub const MAX_LOT_DEPTH: u64 = ZONE_DEPTH as u64;

/// 建てる候補の間口のマス。道路に縦か横で面した、区画のある空きマスである。
///
/// 道路、区画、マスの使い道が変わるたびに、変わったマスのまわりだけを [`World::refresh_lot_fronts`] で直す。
/// 区画とマスの使い道と網から作り直せるので、状態ハッシュには入れない。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct LotFronts {
    /// 間口のマスと、その区画の種類と、道路へ向かう向き（`DIRS` の添え字）。
    fronts: BTreeMap<TilePos, (ZoneKind, u8)>,
    /// 区画の種類ごとの間口のマス。添え字は [`ZoneKind::index`]。
    by_zone: [BTreeSet<TilePos>; 4],
}

/// 1 回に、区画の種類ごとに建てる建物の数の上限。
pub const SPAWN_PER_ZONE: u32 = 2;

/// 段階が 1 つ上がるまでの時間（tick）。
pub const LEVEL_UP_TICKS: u64 = TICKS_PER_DAY;

/// 間口のマスから道路へ向かう 1 歩の候補。この順に調べる。
const DIRS: [(i64, i64); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

fn step(t: TilePos, d: (i64, i64), k: i64) -> TilePos {
    let at = |v: i64, s: i64| {
        s.checked_mul(k)
            .and_then(|o| v.checked_add(o))
            .expect("マスの座標があふれた")
    };
    TilePos::new(at(t.x, d.0), at(t.y, d.1))
}

/// 間口 `w`、奥行き `k` の敷地のマス。`(i, j)` は間口に沿って `i`、奥へ `j` 進んだマスである。
fn lot_tiles(
    front: TilePos,
    toward_road: (i64, i64),
    w: u64,
    k: u64,
) -> impl Iterator<Item = (u64, u64, TilePos)> {
    let along = (-toward_road.1, toward_road.0);
    let back = (-toward_road.0, -toward_road.1);
    (0..k).flat_map(move |j| {
        (0..w).map(move |i| {
            let t = step(step(front, along, i as i64), back, j as i64);
            (i, j, t)
        })
    })
}

impl World {
    /// マスの区画と使い道。メモリにないチャンクのマスは `None` である。
    fn tile_state(&self, t: TilePos) -> Option<(Option<ZoneKind>, TileUse)> {
        let tiles = self.chunks.get(&t.chunk()?)?.layer(Layer::Ground)?;
        let (lx, ly) = t.local();
        let i = local_index(lx, ly);
        Some((tiles.zone[i], tiles.occupancy[i]))
    }

    /// `front` から `toward_road` へ 1 歩のマスを通る、歩道のある縦か横の区間の、`front` の側の歩道のリンク。
    fn frontage(&self, front: TilePos, toward_road: (i64, i64)) -> Option<LinkId> {
        let road = step(front, toward_road, 1);
        let inward = (-toward_road.0, -toward_road.1);
        self.network
            .segments_through(road)
            .into_iter()
            .find_map(|s| {
                if !self.network.segment(s)?.kind.has_sidewalks() {
                    return None;
                }
                let (sx, sy) = self.network.segment_line(s)?.step;
                if sx * toward_road.0 + sy * toward_road.1 != 0 {
                    return None;
                }
                // y が下向きの座標なので、進む向きの右は (-sy, sx) である。
                let dir = if (-sy, sx) == inward {
                    Dir::Forward
                } else {
                    Dir::Backward
                };
                Some(LinkId {
                    segment: s,
                    dir,
                    mode: LaneMode::Sidewalk,
                })
            })
    }

    /// 敷地に `zone` の建物を建てられるか。
    fn lot_fits(
        &self,
        zone: ZoneKind,
        front: TilePos,
        toward_road: (i64, i64),
        w: u64,
        k: u64,
    ) -> bool {
        lot_tiles(front, toward_road, w, k).all(|(_, j, t)| {
            self.tile_state(t) == Some((Some(zone), TileUse::Empty))
                && (j > 0 || self.frontage(t, toward_road).is_some())
        })
    }

    /// マスの使い道をまとめて書き換え、変えたチャンクの版を上げる。
    fn write_occupancy(&mut self, tiles: impl IntoIterator<Item = (TilePos, TileUse)>) {
        let mut edits = BTreeMap::<ChunkCoord, Vec<(usize, TileUse)>>::new();
        for (t, u) in tiles {
            let coord = t.chunk().expect("建物のマスのチャンクがない");
            let (lx, ly) = t.local();
            edits
                .entry(coord)
                .or_default()
                .push((local_index(lx, ly), u));
        }
        let touched = World::edited_tiles(&edits);
        for (coord, list) in edits {
            let chunk = self
                .chunks
                .get_mut(&coord)
                .expect("建物のマスのチャンクがない");
            chunk.edit_layer(Layer::Ground, |tiles| {
                for (i, u) in list {
                    tiles.occupancy[i] = u;
                }
            });
        }
        self.refresh_lot_fronts(touched);
    }

    /// マスの位置。チャンクの中の添え字から求める。
    fn tile_of(coord: ChunkCoord, i: usize) -> TilePos {
        let o = coord.origin();
        TilePos::new(o.x + (i % CHUNK_SIDE) as i64, o.y + (i / CHUNK_SIDE) as i64)
    }

    /// `tiles` とそのまわり 4 マスについて、建てる候補の間口かどうかを見直す。
    /// 道路、区画、マスの使い道を変えたら、変えたマスを渡して呼ぶ。
    pub(super) fn refresh_lot_fronts(&mut self, tiles: impl IntoIterator<Item = TilePos>) {
        let mut around = BTreeSet::new();
        for t in tiles {
            around.insert(t);
            around.extend(DIRS.iter().map(|d| step(t, *d, 1)));
        }
        for t in around {
            if let Some((zone, _)) = self.lots.fronts.remove(&t) {
                self.lots.by_zone[zone.index()].remove(&t);
            }
            let Some((Some(zone), TileUse::Empty)) = self.tile_state(t) else {
                continue;
            };
            if let Some(d) = (0..DIRS.len()).find(|d| self.frontage(t, DIRS[*d]).is_some()) {
                self.lots.fronts.insert(t, (zone, d as u8));
                self.lots.by_zone[zone.index()].insert(t);
            }
        }
    }

    /// チャンクの中で書き換えたマスの位置。
    pub(super) fn edited_tiles<T>(edits: &BTreeMap<ChunkCoord, Vec<(usize, T)>>) -> Vec<TilePos> {
        edits
            .iter()
            .flat_map(|(c, list)| list.iter().map(|(i, _)| World::tile_of(*c, *i)))
            .collect()
    }

    /// 段階を上げ、需要のある区画に建物を建てる。`GROWTH_INTERVAL` ごとに呼ばれる。
    pub(super) fn grow_buildings(&mut self) {
        let tick = self.tick;
        if !tick.0.is_multiple_of(GROWTH_INTERVAL) {
            return;
        }
        let ids: Vec<BuildingId> = self.buildings.iter().map(|(id, _)| id).collect();
        for id in ids {
            let b = self.buildings.get_mut(id).expect("建物がない");
            if b.level < MAX_LEVEL && tick.since(b.leveled_at) >= LEVEL_UP_TICKS {
                b.level += 1;
                b.leveled_at = tick;
            }
        }
        for zone in ZoneKind::ALL {
            for attempt in 0..SPAWN_PER_ZONE {
                let list = &self.lots.by_zone[zone.index()];
                if list.is_empty() || demand(self.buildings.capacity())[zone.index()] <= 0 {
                    break;
                }
                let id = u64::from(zone as u8) * 16 + u64::from(attempt);
                let mut r = rng(self.seed, RngPurpose::BuildingGrowth, id, tick);
                let len = u32::try_from(list.len()).expect("候補の数が u32 に収まらない");
                let front = *list.iter().nth(r.below(len) as usize).expect("候補がない");
                let toward_road = DIRS[usize::from(self.lots.fronts[&front].1)];
                // 間口は引いた幅から、奥行きは区画の奥行きいっぱいから、収まるまで小さくする。
                let wt = 1 + u64::from(r.below(MAX_LOT_SIDE as u32));
                let fit = (1..=wt)
                    .rev()
                    .flat_map(|w| (1..=MAX_LOT_DEPTH).rev().map(move |k| (w, k)))
                    .find(|(w, k)| self.lot_fits(zone, front, toward_road, *w, *k));
                if let Some((w, k)) = fit {
                    self.place_building(zone, front, toward_road, w, k);
                }
            }
        }
    }

    fn place_building(
        &mut self,
        zone: ZoneKind,
        front: TilePos,
        toward_road: (i64, i64),
        w: u64,
        k: u64,
    ) {
        let tiles: Vec<TilePos> = lot_tiles(front, toward_road, w, k)
            .map(|(_, _, t)| t)
            .collect();
        let far = *tiles.last().expect("敷地が空");
        let entrance = self
            .frontage(front, toward_road)
            .expect("間口が道路に面していない");
        self.buildings.insert(Building {
            zone,
            area: TileRect::from_corners(front, far),
            front,
            toward_road,
            entrance,
            level: 1,
            leveled_at: self.tick,
        });
        self.write_occupancy(tiles.into_iter().map(|t| (t, TileUse::Building)));
    }

    /// 道路や区画が変わったあとに呼ぶ。条件の崩れた建物を消し、残った建物の出入口を付け直す。
    pub(super) fn refresh_buildings(&mut self) {
        let all: Vec<(BuildingId, Building)> =
            self.buildings.iter().map(|(id, b)| (id, *b)).collect();
        for (id, b) in all {
            let rect_tiles = crate::zone::tiles_of(b.area).collect::<Vec<_>>();
            let intact = rect_tiles
                .iter()
                .all(|t| self.tile_state(*t) == Some((Some(b.zone), TileUse::Building)));
            match self.frontage(b.front, b.toward_road).filter(|_| intact) {
                Some(entrance) => {
                    self.buildings.get_mut(id).expect("建物がない").entrance = entrance;
                }
                None => {
                    self.buildings.remove(id);
                    let freed: Vec<_> = rect_tiles
                        .into_iter()
                        .filter(|t| {
                            self.tile_state(*t)
                                .is_some_and(|(_, u)| u == TileUse::Building)
                        })
                        .map(|t| (t, TileUse::Empty))
                        .collect();
                    self.write_occupancy(freed);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, RoadKind};
    use crate::world::ApplyOutcome;

    fn rect(a: (i64, i64), b: (i64, i64)) -> TileRect {
        TileRect::from_corners(TilePos::new(a.0, a.1), TilePos::new(b.0, b.1))
    }

    /// 横の道路を引き、北に住宅、南に工業を塗った街。
    fn zoned_street() -> World {
        let mut w = World::new(7);
        let road = Command::BuildRoad {
            from: TilePos::new(0, 10),
            to: TilePos::new(40, 10),
            kind: RoadKind::Small,
        };
        assert_eq!(w.apply(&road), ApplyOutcome::Applied);
        for (area, zone) in [
            (rect((0, 4), (40, 9)), ZoneKind::Residential),
            (rect((0, 11), (40, 16)), ZoneKind::Industrial),
        ] {
            let paint = Command::PaintZone {
                area,
                zone: Some(zone),
            };
            assert_eq!(w.apply(&paint), ApplyOutcome::Applied);
        }
        w
    }

    fn run(w: &mut World, ticks: u64) {
        for _ in 0..ticks {
            w.step();
        }
    }

    /// どの建物も、区画の上で道路に面し、マスの使い道と出入口が網と食い違わない。
    fn check_buildings(w: &World) {
        let mut covered = 0;
        for (_, b) in w.buildings().iter() {
            assert!(b.tile_count() <= MAX_LOT_SIDE * MAX_LOT_DEPTH);
            for t in crate::zone::tiles_of(b.area) {
                assert_eq!(w.tile_state(t), Some((Some(b.zone), TileUse::Building)));
            }
            assert_eq!(w.frontage(b.front, b.toward_road), Some(b.entrance));
            covered += b.tile_count();
        }
        let mut building_tiles = 0;
        for c in w.chunks() {
            let tiles = c.layer(Layer::Ground).unwrap();
            building_tiles += tiles
                .occupancy
                .iter()
                .filter(|u| **u == TileUse::Building)
                .count() as u64;
        }
        assert_eq!(building_tiles, covered, "建物のないマスが建物になっている");
        // 建てる候補は、全チャンクをなめて作り直したものと一致する。
        let mut rebuilt = LotFronts::default();
        for c in w.chunks() {
            let tiles = c.layer(Layer::Ground).unwrap();
            for i in 0..crate::tile::CHUNK_AREA {
                let (Some(zone), TileUse::Empty) = (tiles.zone[i], tiles.occupancy[i]) else {
                    continue;
                };
                let t = World::tile_of(c.coord(), i);
                if let Some(d) = (0..DIRS.len()).find(|d| w.frontage(t, DIRS[*d]).is_some()) {
                    rebuilt.fronts.insert(t, (zone, d as u8));
                    rebuilt.by_zone[zone.index()].insert(t);
                }
            }
        }
        assert_eq!(w.lots, rebuilt);
    }

    #[test]
    fn houses_grow_along_a_zoned_street_within_minutes() {
        let mut w = zoned_street();
        // 等速で 1 分。
        run(&mut w, 30 * 60);
        assert!(
            w.buildings().len() >= 8,
            "{} 棟しか建たない",
            w.buildings().len()
        );
        assert!(w.buildings().capacity().households > 0);
        check_buildings(&w);
        // 同じ種と操作からは同じ街になる。
        let mut again = zoned_street();
        run(&mut again, 30 * 60);
        assert_eq!(again, w);
    }

    #[test]
    fn buildings_level_up_once_a_day() {
        let mut w = zoned_street();
        run(&mut w, GROWTH_INTERVAL + 1);
        let (id, b) = w.buildings().iter().next().map(|(id, b)| (id, *b)).unwrap();
        assert_eq!(b.level, 1);
        run(&mut w, LEVEL_UP_TICKS);
        let after = w.buildings().get(id).unwrap();
        assert_eq!(after.level, 2);
        assert!(after.households() > b.households());
    }

    #[test]
    fn buildings_vanish_with_their_road_or_zone() {
        let mut w = zoned_street();
        run(&mut w, 30 * 60);
        let before = w.buildings().len();
        // 道路を横切る道路を引くと区間が分かれるが、建物は出入口を付け直して残る。
        let cross = Command::BuildRoad {
            from: TilePos::new(20, 10),
            to: TilePos::new(20, 30),
            kind: RoadKind::Small,
        };
        if w.apply(&cross) == ApplyOutcome::Applied {
            check_buildings(&w);
            assert!(!w.buildings().is_empty());
        }
        // 区画を消すと、その上の建物は消える。
        let clear = Command::PaintZone {
            area: rect((0, 4), (40, 9)),
            zone: None,
        };
        assert_eq!(w.apply(&clear), ApplyOutcome::Applied);
        check_buildings(&w);
        assert!(w.buildings().iter().all(|(_, b)| b.area.min.y > 9));
        assert!(w.buildings().len() < before);
        // 道路を消すと、残りの建物も消える。
        let bulldoze = Command::Bulldoze {
            area: rect((0, 0), (40, 40)),
        };
        assert_eq!(w.apply(&bulldoze), ApplyOutcome::Applied);
        assert!(w.buildings().is_empty());
        check_buildings(&w);
    }
}
