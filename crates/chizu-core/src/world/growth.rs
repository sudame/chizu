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
use crate::command::ServiceKind;
use crate::command::ZoneKind;
use crate::id::BuildingId;
use crate::network::{Dir, LaneMode, LinkId};
use crate::rng::{RngPurpose, rng};
use crate::service::{Facility, Supply, evaluate_supply};
use crate::tile::{CHUNK_SIDE, ChunkCoord, TilePos, TileRect, local_index};
use crate::world::{ApplyOutcome, DenyReason};
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

/// 間口のマスから道路へ向かう 1 歩の候補。この順に調べる。`PlaceBuilding` の `rotation` もこの順である。
pub(super) const DIRS: [(i64, i64); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

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
    pub(super) fn tile_state(&self, t: TilePos) -> Option<(Option<ZoneKind>, TileUse)> {
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
        if self.supply_dirty {
            self.update_supply();
        }
        if !tick.0.is_multiple_of(GROWTH_INTERVAL) {
            return;
        }
        // 電気と水道が届き、住む世帯か働く住民で埋まった建物だけが、段階を上げる。
        let occupants = self.occupants();
        let ids: Vec<BuildingId> = self.buildings.iter().map(|(id, _)| id).collect();
        for id in ids {
            let b = self.buildings.get_mut(id).expect("建物がない");
            let supplied = b.supply.power && b.supply.water;
            let full = occupants.get(&id).copied().unwrap_or(0) >= b.households() + b.workplaces();
            if supplied && full && b.level < MAX_LEVEL && tick.since(b.leveled_at) >= LEVEL_UP_TICKS
            {
                b.level += 1;
                b.leveled_at = tick;
                // 需要が増えたので、次の tick の頭で供給を判定し直す。
                self.supply_dirty = true;
            }
        }
        let mut stats = self.city_stats();
        for zone in ZoneKind::ALL {
            for attempt in 0..SPAWN_PER_ZONE {
                let list = &self.lots.by_zone[zone.index()];
                if list.is_empty() || demand(&stats)[zone.index()] <= 0 {
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
                    let id = self.place_building(zone, front, toward_road, w, k);
                    stats = stats.with(self.buildings.get(id).expect("建てた建物がない"));
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
    ) -> BuildingId {
        let tiles: Vec<TilePos> = lot_tiles(front, toward_road, w, k)
            .map(|(_, _, t)| t)
            .collect();
        let far = *tiles.last().expect("敷地が空");
        let entrance = self
            .frontage(front, toward_road)
            .expect("間口が道路に面していない");
        let id = self.buildings.insert(Building {
            zone,
            area: TileRect::from_corners(front, far),
            front,
            toward_road,
            entrance,
            level: 1,
            leveled_at: self.tick,
            supply: Supply::default(),
        });
        self.write_occupancy(tiles.into_iter().map(|t| (t, TileUse::Building)));
        self.supply_dirty = true;
        id
    }

    /// 道路や区画が変わったあとに呼ぶ。条件の崩れた建物を消し、残った建物の出入口を付け直す。
    /// 建物ごとの電気と水道の届き方を見直す。
    pub(super) fn update_supply(&mut self) {
        self.supply_dirty = false;
        let result = evaluate_supply(&self.network, &self.facilities, &self.buildings);
        for (id, supply) in result {
            self.buildings.get_mut(id).expect("建物がない").supply = supply;
        }
    }

    /// 公共の施設を置く。`at` から `DIRS[rotation]` へ 1 歩のマスが道路で、敷地は反対側へ延びる。
    pub(super) fn place_service(
        &mut self,
        kind: ServiceKind,
        at: TilePos,
        rotation: u8,
    ) -> ApplyOutcome {
        let toward_road = DIRS[usize::from(rotation)];
        let (w, k) = kind.size();
        let tiles: Vec<TilePos> = lot_tiles(at, toward_road, w, k)
            .map(|(_, _, t)| t)
            .collect();
        if let Err(reason) = tiles.iter().try_for_each(|t| self.check_buildable(*t)) {
            return ApplyOutcome::Denied(reason);
        }
        if tiles.iter().any(|t| {
            !matches!(
                self.tile_state(*t),
                Some((_, TileUse::Empty | TileUse::Building))
            )
        }) {
            return ApplyOutcome::Denied(DenyReason::Occupied);
        }
        let front_row = lot_tiles(at, toward_road, w, 1).map(|(_, _, t)| t);
        let Some(entrance) = self.frontage(at, toward_road) else {
            return ApplyOutcome::Denied(DenyReason::NoRoadAccess);
        };
        let mut front_row = front_row;
        if front_row.any(|t| self.frontage(t, toward_road).is_none()) {
            return ApplyOutcome::Denied(DenyReason::NoRoadAccess);
        }
        let cost = kind.cost();
        if self.money < cost {
            return ApplyOutcome::Denied(DenyReason::NotEnoughMoney);
        }
        self.money = self.money - cost;
        let far = *tiles.last().expect("敷地が空");
        self.facilities.push(Facility {
            kind,
            area: TileRect::from_corners(at, far),
            front: at,
            toward_road,
            entrance: Some(entrance),
        });
        // CS2 と同じく、施設の下の区画は消し、区画の建物は取り壊す。
        let mut zones = BTreeMap::<ChunkCoord, Vec<(usize, Option<ZoneKind>)>>::new();
        for t in &tiles {
            let (lx, ly) = t.local();
            zones
                .entry(t.chunk().expect("チャンクがない"))
                .or_default()
                .push((local_index(lx, ly), None));
        }
        self.write_zones(zones);
        self.refresh_buildings();
        self.write_occupancy(tiles.into_iter().map(|t| (t, TileUse::Facility)));
        ApplyOutcome::Applied
    }

    pub(super) fn refresh_buildings(&mut self) {
        let facilities: Vec<Option<LinkId>> = self
            .facilities
            .iter()
            .map(|f| self.frontage(f.front, f.toward_road))
            .collect();
        for (f, entrance) in self.facilities.iter_mut().zip(facilities) {
            f.entrance = entrance;
        }
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
        // 道路と施設の変化は、その場で供給に映す。
        self.update_supply();
    }

    /// 長方形にかかる施設を取り除き、取り除いた数を返す。払い戻しはしない。
    pub(super) fn bulldoze_facilities(&mut self, area: TileRect) -> usize {
        let overlaps = |r: TileRect| {
            r.min.x <= area.max.x
                && area.min.x <= r.max.x
                && r.min.y <= area.max.y
                && area.min.y <= r.max.y
        };
        let (gone, kept): (Vec<Facility>, Vec<Facility>) =
            self.facilities.iter().partition(|f| overlaps(f.area));
        self.facilities = kept;
        let freed: Vec<_> = gone
            .iter()
            .flat_map(|f| crate::zone::tiles_of(f.area))
            .map(|t| (t, TileUse::Empty))
            .collect();
        self.write_occupancy(freed);
        gone.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, RoadKind};
    use crate::units::{Money, Tick};
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
        connect_to_outside(&mut w);
        w
    }

    /// 高速道路の内側のノードから、y = 10 の通りまで道路を引く。
    fn connect_to_outside(w: &mut World) {
        let inner = crate::world::highway_line(w.seed()).start;
        let corner = TilePos::new(inner.x, 10);
        let mut road = |from: TilePos, to: TilePos| {
            if from != to {
                let c = Command::BuildRoad {
                    from,
                    to,
                    kind: RoadKind::Small,
                };
                assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{from:?} → {to:?}");
            }
        };
        road(inner, corner);
        road(corner, TilePos::new(corner.x.clamp(0, 40), 10));
    }

    /// 通りに発電所と浄水場を置いた街。
    fn served_street() -> World {
        let mut w = zoned_street();
        assert_eq!(
            place(&mut w, ServiceKind::PowerPlant, (3, 9), 2),
            ApplyOutcome::Applied
        );
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (7, 9), 2),
            ApplyOutcome::Applied
        );
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
        let mut w = served_street();
        // 等速で 1 分。
        run(&mut w, 30 * 60);
        assert!(
            w.buildings().len() >= 5,
            "{} 棟しか建たない",
            w.buildings().len()
        );
        assert!(w.buildings().capacity().households > 0);
        assert!(w.population().citizen_count() > 0, "誰も移り住まない");
        check_buildings(&w);
        // 同じ種と操作からは同じ街になる。
        let mut again = served_street();
        run(&mut again, 30 * 60);
        assert_eq!(again, w);
    }

    #[test]
    fn full_supplied_buildings_level_up_after_a_day() {
        let mut w = served_street();
        run(&mut w, 2 * LEVEL_UP_TICKS);
        let occupants = w.occupants();
        let leveled: Vec<_> = w.buildings().iter().filter(|(_, b)| b.level >= 2).collect();
        assert!(!leveled.is_empty(), "段階の上がった建物がない");
        // 段階が上がるのは、住む世帯で埋まった住宅か、働く住民で埋まった職場だけである。
        for (id, b) in &leveled {
            assert!(occupants.get(id).copied().unwrap_or(0) > 0, "{b:?}");
        }
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

    fn place(w: &mut World, kind: ServiceKind, at: (i64, i64), rotation: u8) -> ApplyOutcome {
        w.apply(&Command::PlaceBuilding {
            kind,
            at: TilePos::new(at.0, at.1),
            rotation,
        })
    }

    #[test]
    fn services_supply_connected_buildings_so_they_fill_and_level_up() {
        let mut w = zoned_street();
        // 同じ種類の区画を、つながっていない別の通りにも塗る。
        let far = Command::BuildRoad {
            from: TilePos::new(0, 40),
            to: TilePos::new(40, 40),
            kind: RoadKind::Small,
        };
        assert_eq!(w.apply(&far), ApplyOutcome::Applied);
        for (area, zone) in [
            (rect((0, 34), (40, 39)), ZoneKind::Residential),
            (rect((0, 41), (40, 46)), ZoneKind::Industrial),
        ] {
            let paint = Command::PaintZone {
                area,
                zone: Some(zone),
            };
            assert_eq!(w.apply(&paint), ApplyOutcome::Applied);
        }
        let money = w.money();
        // 発電所は (0..=3, 6..=9)、浄水場は (6..=7, 8..=9) を占め、y = 10 の道路に面する。
        assert_eq!(
            place(&mut w, ServiceKind::PowerPlant, (3, 9), 2),
            ApplyOutcome::Applied
        );
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (7, 9), 2),
            ApplyOutcome::Applied
        );
        assert_eq!(
            w.money(),
            money - ServiceKind::PowerPlant.cost() - ServiceKind::WaterPump.cost()
        );
        assert_eq!(
            w.tile_state(TilePos::new(0, 6)),
            Some((None, TileUse::Facility))
        );
        run(&mut w, 2 * LEVEL_UP_TICKS);
        check_buildings(&w);
        let mut near = 0;
        for (_, b) in w.buildings().iter() {
            if b.area.min.y < 20 {
                assert!(b.supply.power && b.supply.water, "{b:?}");
                near += 1;
            } else {
                assert_eq!((b.supply, b.level), (Supply::default(), 1), "{b:?}");
            }
        }
        assert!(near > 0);
        assert!(w.buildings().iter().any(|(_, b)| b.level >= 2));
        // 道路を施設の上に引くことはできない。
        let over = Command::BuildRoad {
            from: TilePos::new(1, 0),
            to: TilePos::new(1, 9),
            kind: RoadKind::Small,
        };
        assert_eq!(w.apply(&over), ApplyOutcome::Denied(DenyReason::Occupied));
    }

    #[test]
    fn services_need_owned_free_land_facing_a_road_and_money() {
        let mut w = zoned_street();
        let pump = ServiceKind::WaterPump;
        assert_eq!(
            place(&mut w, pump, (5, 10), 2),
            ApplyOutcome::Denied(DenyReason::Occupied)
        );
        assert_eq!(
            place(&mut w, pump, (5, 30), 2),
            ApplyOutcome::Denied(DenyReason::NoRoadAccess)
        );
        // 敷地の一部が道路の端を越えて、間口の 1 マスが道路に面さない。
        assert_eq!(
            place(&mut w, pump, (0, 9), 2),
            ApplyOutcome::Denied(DenyReason::NoRoadAccess)
        );
        assert_eq!(
            place(&mut w, pump, (200, 9), 2),
            ApplyOutcome::Denied(DenyReason::NotOwned)
        );
        w.money = Money(10);
        assert_eq!(
            place(&mut w, pump, (7, 9), 2),
            ApplyOutcome::Denied(DenyReason::NotEnoughMoney)
        );
        let bad = Command::PlaceBuilding {
            kind: pump,
            at: TilePos::new(7, 9),
            rotation: 4,
        };
        assert_eq!(w.apply(&bad), ApplyOutcome::Rejected);
        assert!(w.facilities().is_empty());
    }

    fn build(w: &mut World, a: (i64, i64), b: (i64, i64)) {
        let c = Command::BuildRoad {
            from: TilePos::new(a.0, a.1),
            to: TilePos::new(b.0, b.1),
            kind: RoadKind::Small,
        };
        assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{c:?}");
    }

    #[test]
    fn supply_stops_the_moment_the_network_is_cut() {
        let mut w = zoned_street();
        // 施設は y = 30 の通りに置き、x = 40 の道路で y = 10 の通りとつなぐ。
        build(&mut w, (0, 30), (40, 30));
        build(&mut w, (40, 10), (40, 30));
        assert_eq!(
            place(&mut w, ServiceKind::PowerPlant, (3, 29), 2),
            ApplyOutcome::Applied
        );
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (7, 29), 2),
            ApplyOutcome::Applied
        );
        run(&mut w, 4 * GROWTH_INTERVAL);
        assert!(
            w.buildings()
                .iter()
                .all(|(_, b)| b.supply.power && b.supply.water)
        );
        let levels: Vec<_> = w.buildings().iter().map(|(id, b)| (id, b.level)).collect();
        let cut = Command::Bulldoze {
            area: rect((40, 20), (40, 20)),
        };
        assert_eq!(w.apply(&cut), ApplyOutcome::Applied);
        // 道路を消したその場で届かなくなり、そのあとも段階は上がらない。
        assert!(
            w.buildings()
                .iter()
                .all(|(_, b)| b.supply == Supply::default())
        );
        run(&mut w, LEVEL_UP_TICKS + 2 * GROWTH_INTERVAL);
        for (id, level) in levels {
            assert_eq!(w.buildings().get(id).map(|b| b.level), Some(level));
        }
    }

    #[test]
    fn bulldoze_removes_facilities_and_frees_their_tiles() {
        let mut w = zoned_street();
        let pump = ServiceKind::WaterPump;
        assert_eq!(
            place(&mut w, ServiceKind::PowerPlant, (3, 9), 2),
            ApplyOutcome::Applied
        );
        assert_eq!(place(&mut w, pump, (7, 9), 2), ApplyOutcome::Applied);
        let origin = ChunkCoord::new(0, 0);
        let version = w.chunk(origin).unwrap().version();
        // 浄水場の 1 マスだけにかかる長方形でも、浄水場ごと取り除く。
        let area = rect((6, 8), (6, 8));
        assert_eq!(w.apply(&Command::Bulldoze { area }), ApplyOutcome::Applied);
        assert_eq!(w.facilities().len(), 1);
        assert!(w.chunk(origin).unwrap().version() > version);
        for t in crate::zone::tiles_of(rect((6, 8), (7, 9))) {
            assert_eq!(w.tile_state(t), Some((None, TileUse::Empty)));
        }
        // 空いたマスには、また施設を置ける。
        assert_eq!(place(&mut w, pump, (7, 9), 2), ApplyOutcome::Applied);
        // 前の道路を消しても、施設は残るが供給はしない。
        let street = Command::Bulldoze {
            area: rect((20, 10), (20, 10)),
        };
        assert_eq!(w.apply(&street), ApplyOutcome::Applied);
        assert_eq!(w.facilities().len(), 2);
        assert!(w.facilities().iter().all(|f| f.entrance.is_none()));
        // 何もない長方形は、これまでどおり却下する。
        let empty = Command::Bulldoze {
            area: rect((100, 100), (101, 101)),
        };
        assert_eq!(
            w.apply(&empty),
            ApplyOutcome::Denied(DenyReason::NothingToBulldoze)
        );
    }

    #[test]
    fn supply_fails_when_demand_exceeds_capacity() {
        let mut w = World::new(0);
        let road = Command::BuildRoad {
            from: TilePos::new(0, 10),
            to: TilePos::new(40, 10),
            kind: RoadKind::Small,
        };
        assert_eq!(w.apply(&road), ApplyOutcome::Applied);
        assert_eq!(
            place(&mut w, ServiceKind::PowerPlant, (3, 9), 2),
            ApplyOutcome::Applied
        );
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (7, 9), 2),
            ApplyOutcome::Applied
        );
        let entrance = w.frontage(TilePos::new(20, 9), (0, 1)).unwrap();
        // 16 マスで段階 5 の建物を 4 棟置くと、需要は 320 になる。電気（400）は足り、水道（300）は足りない。
        for i in 0..4 {
            w.buildings.insert(Building {
                zone: ZoneKind::Residential,
                area: rect((10 + 4 * i, 6), (13 + 4 * i, 9)),
                front: TilePos::new(10 + 4 * i, 9),
                toward_road: (0, 1),
                entrance,
                level: MAX_LEVEL,
                leveled_at: Tick::ZERO,
                supply: Supply::default(),
            });
        }
        w.update_supply();
        for (_, b) in w.buildings().iter() {
            assert_eq!(
                b.supply,
                Supply {
                    power: true,
                    water: false
                }
            );
        }
    }
}
