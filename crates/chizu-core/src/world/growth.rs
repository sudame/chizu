//! 建物が建つ・育つ・消える処理。
//!
//! 建物は、道路に縦か横で面した区画のマスを間口にして、奥へ向かって建つ。間口と奥行きは
//! 1〜[`MAX_LOT_SIDE`] マスで、種から引いた大きさが収まらなければ小さくする。
//!
//! 斜めの道路は階段状にマスを通るので、その脇のマスは縦と横の 2 辺で道路のマスに接する。
//! このマスを間口にして、道路から離れる向きへ 1 辺 1〜[`DIAGONAL_LOT_SIDE`] マスの正方形の建物を
//! 建てる。2×2 と 1×1 が道路に沿って階段状に並ぶ。出入口は、接する 2 マスのあいだの歩道に付ける。
//!
//! 道路か区画が消えて建物の条件が崩れたら、建物も消える。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::World;
use crate::building::{Building, DIAGONAL_LOT_SIDE, MAX_LEVEL, MAX_LOT_SIDE, demand};
use crate::chunk::{Layer, TileUse};
use crate::clock::TICKS_PER_DAY;
use crate::command::ServiceKind;
use crate::command::ZoneKind;
use crate::id::BuildingId;
use crate::network::{DIAGONAL_TILE_MM, Dir, LaneMode, LinkId, TILE_MM};
use crate::rng::{RngPurpose, rng};
use crate::round;
use crate::service::{Facility, Headroom, Supply, evaluate_supply};
use crate::tile::{CHUNK_SIDE, ChunkCoord, TilePos, TileRect, local_index};
use crate::units::LaneOffset;
use crate::world::{ApplyOutcome, DenyReason};
use crate::zone::ZONE_DEPTH;

/// 建物が建つ・育つかを見る間隔（tick）。等速で 1 秒に 1 回である。
pub const GROWTH_INTERVAL: u64 = 30;

/// 建物の奥行きの上限（マス）。区画の奥行きいっぱいまで建つので、奥に空きのマスが残らない。
pub const MAX_LOT_DEPTH: u64 = ZONE_DEPTH as u64;

/// 建てる候補の間口のマス。道路に面した、区画のある空きマスである。
///
/// 道路、区画、マスの使い道が変わるたびに、変わったマスのまわりだけを [`World::refresh_lot_fronts`] で直す。
/// 区画とマスの使い道と網から作り直せるので、状態ハッシュには入れない。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct LotFronts {
    /// 間口のマスと、その区画の種類と、道路へ向かう向き（`FRONT_DIRS` の添え字）。
    fronts: BTreeMap<TilePos, (ZoneKind, u8)>,
    /// 区画の種類ごとの間口のマス。添え字は [`ZoneKind::index`]。
    by_zone: [BTreeSet<TilePos>; 4],
}

impl LotFronts {
    /// 区画の種類 `zone` に、建てる候補の間口があるか。
    pub(super) fn has_zone(&self, zone: ZoneKind) -> bool {
        !self.by_zone[zone.index()].is_empty()
    }
}

/// 需要があるときに、1 回に区画の種類ごとに建てる建物の数の下限。
pub const SPAWN_PER_ZONE: u32 = 2;

/// 1 回に建てる数に、需要の何分の 1 を足すか。需要が強いほど速く建つ。
pub const SPAWN_PER_DEMAND: i64 = 8;

/// 1 回に、区画の種類ごとに建てる建物の数の上限。
pub const MAX_SPAWN_PER_ZONE: u32 = 16;

/// 段階が 1 つ上がるまでの時間（tick）。
pub const LEVEL_UP_TICKS: u64 = TICKS_PER_DAY;

/// 間口のマスから道路へ向かう 1 歩の候補。この順に調べる。`PlaceBuilding` の `rotation` もこの順である。
pub(super) const DIRS: [(i64, i64); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];

/// 建てる候補の間口から道路へ向かう向き。この順に調べるので、縦横の道路にも斜めの道路にも
/// 面したマスは、大きな建物の建つ縦横の道路に向く。
const FRONT_DIRS: [(i64, i64); 8] = [
    (0, -1),
    (1, 0),
    (0, 1),
    (-1, 0),
    (1, -1),
    (1, 1),
    (-1, 1),
    (-1, -1),
];

fn is_diagonal(d: (i64, i64)) -> bool {
    d.0 != 0 && d.1 != 0
}

fn step(t: TilePos, d: (i64, i64), k: i64) -> TilePos {
    let at = |v: i64, s: i64| {
        s.checked_mul(k)
            .and_then(|o| v.checked_add(o))
            .expect("マスの座標があふれた")
    };
    TilePos::new(at(t.x, d.0), at(t.y, d.1))
}

/// 間口 `w`、奥行き `k` の敷地のマス。`(i, j)` は間口に沿って `i`、奥へ `j` 進んだマスである。
/// 斜めの道路に向くときは、x の向きに道路から `i`、y の向きに道路から `j` 離れたマスである。
fn lot_tiles(
    front: TilePos,
    toward_road: (i64, i64),
    w: u64,
    k: u64,
) -> impl Iterator<Item = (u64, u64, TilePos)> {
    let (tx, ty) = toward_road;
    let (along, back) = if is_diagonal(toward_road) {
        ((-tx, 0), (0, -ty))
    } else {
        ((-ty, tx), (-tx, -ty))
    };
    (0..k).flat_map(move |j| {
        (0..w).map(move |i| {
            let t = step(step(front, along, i as i64), back, j as i64);
            (i, j, t)
        })
    })
}

/// 区画の種類ごとの、建てる候補の間口の数と、電気か水道の余裕がなくて建たない数。
///
/// 需要があるのに建物が増えないとき、どの区画がなぜ建たないかを知る手がかりにする。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GrowthBlock {
    /// 建てる候補の間口の数。
    pub fronts: u64,
    /// 面した道路網に、電気か水道の余裕がなくて建たない間口の数。
    /// 区画の需要が 0 以下なら、余裕があっても建たないので数えない。
    pub blocked: u64,
    /// そのうち、電気の余裕がない間口の数。
    pub no_power: u64,
    /// そのうち、水道の余裕がない間口の数。
    pub no_water: u64,
}

impl World {
    /// 区画の種類（[`ZoneKind::index`] の順）ごとに、需要があるのに新築が電気と水道の余裕で止まっている間口を数える。状態は変えない。
    pub fn growth_blocks(&self) -> [GrowthBlock; 4] {
        let mut headroom = Headroom::of(&self.network, &self.facilities, &self.buildings);
        let mut out = [GrowthBlock::default(); 4];
        let demand = demand(&self.city_stats());
        for (front, (zone, d)) in &self.lots.fronts {
            let slot = &mut out[zone.index()];
            slot.fronts += 1;
            // 新築は需要が正の区画にしか建たない。需要のない区画は、施設を置いても建たない。
            if demand[zone.index()] <= 0 {
                continue;
            }
            // 新築と同じく、1 マスの建物（需要 1）が入らなければ建たない。
            let lacks = self
                .frontage(*front, FRONT_DIRS[usize::from(*d)])
                .and_then(|(link, _)| headroom.lacks(&self.network, link));
            if let Some([power, water]) = lacks.filter(|l| l.iter().any(|x| *x)) {
                slot.blocked += 1;
                slot.no_power += u64::from(power);
                slot.no_water += u64::from(water);
            }
        }
        out
    }

    /// マスの区画と使い道。メモリにないチャンクのマスは `None` である。
    pub(super) fn tile_state(&self, t: TilePos) -> Option<(Option<ZoneKind>, TileUse)> {
        let tiles = self.chunks.get(&t.chunk()?)?.layer(Layer::Ground)?;
        let (lx, ly) = t.local();
        let i = local_index(lx, ly);
        Some((tiles.zone[i], tiles.occupancy[i]))
    }

    /// `front` が `toward_road` の向きで面する、歩道のある区間の、`front` の側の歩道のリンクと、
    /// その始まりから出入口までの距離。
    ///
    /// 縦か横なら、1 歩先のマスを横切る区間に面する。出入口はそのマスの中心である。
    /// 斜め `(dx, dy)` なら、`(dx, 0)` と `(0, dy)` の 1 歩先の 2 マスを続けて通る斜めの区間に面する。
    /// 出入口は 2 マスの中心のあいだの点である。
    fn frontage(&self, front: TilePos, toward_road: (i64, i64)) -> Option<(LinkId, LaneOffset)> {
        let (tx, ty) = toward_road;
        let diagonal = is_diagonal(toward_road);
        let (p, q) = if diagonal {
            (step(front, (tx, 0), 1), step(front, (0, ty), 1))
        } else {
            let road = step(front, toward_road, 1);
            (road, road)
        };
        self.network.segments_through(p).into_iter().find_map(|s| {
            if !self.network.segment(s)?.kind.has_sidewalks() {
                return None;
            }
            let line = self.network.segment_line(s)?;
            let (kp, kq) = (line.position_of(p)?, line.position_of(q)?);
            let (sx, sy) = line.step;
            if !diagonal && sx * tx + sy * ty != 0 {
                return None;
            }
            // y が下向きの座標なので、進む向きの右は (-sy, sx) である。建物は道路から
            // (-tx, -ty) の側にあり、右との内積の符号で側が決まる。斜めでは内積は ±2 になる。
            let dir = if sy * tx - sx * ty > 0 {
                Dir::Forward
            } else {
                Dir::Backward
            };
            let per = u64::try_from(if diagonal { DIAGONAL_TILE_MM } else { TILE_MM })
                .expect("1 歩の長さが負");
            let steps = match dir {
                Dir::Forward => kp.min(kq),
                Dir::Backward => line.len.checked_sub(kp.max(kq))?,
            };
            let half = if diagonal { per / 2 } else { 0 };
            let at = steps.checked_mul(per)?.checked_add(half)?;
            let link = LinkId {
                segment: s,
                dir,
                mode: LaneMode::Sidewalk,
            };
            Some((link, LaneOffset(u32::try_from(at).ok()?)))
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
        // 縦横なら間口の列のどのマスも、斜めなら角のマスだけが道路に面する。
        let facing = |i: u64, j: u64| j == 0 && (i == 0 || !is_diagonal(toward_road));
        lot_tiles(front, toward_road, w, k).all(|(i, j, t)| {
            self.tile_state(t) == Some((Some(zone), TileUse::Empty))
                && (!facing(i, j) || self.frontage(t, toward_road).is_some())
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
    ///
    /// 間口かどうかは、そのマスと、縦横に隣り合うマスを通る区間だけで決まる。斜めの道路に面する
    /// ときも、接する 2 マスはどちらも縦横の隣である。区間が分かれても通るマスは変わらないので、
    /// 道路を引いたり消したりしたマスのまわりを見直せば足りる。
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
            if let Some(d) = self.front_dir(t) {
                self.lots.fronts.insert(t, (zone, d));
                self.lots.by_zone[zone.index()].insert(t);
            }
        }
    }

    /// マス `t` が道路に面するなら、その向きの `FRONT_DIRS` の添え字。
    fn front_dir(&self, t: TilePos) -> Option<u8> {
        (0..FRONT_DIRS.len())
            .find(|d| self.frontage(t, FRONT_DIRS[*d]).is_some())
            .map(|d| d as u8)
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
        // 電気と水道が届き、住む世帯か働く住民で埋まり、その区画に需要があり、網に余りのある建物だけが、段階を上げる。
        let occupants = self.occupants();
        let mut stats = self.city_stats();
        let ids: Vec<BuildingId> = self.buildings.iter().map(|(id, _)| id).collect();
        // 段階を上げると需要が増えるので、その網の電気と水道に増分の余りがあるときだけ上げる。
        let mut level_room = Headroom::of(&self.network, &self.facilities, &self.buildings);
        for id in ids {
            let b = *self.buildings.get(id).expect("建物がない");
            let supplied = b.supply.power && b.supply.water;
            let full = occupants.get(&id).copied().unwrap_or(0) >= b.households() + b.workplaces();
            let wanted = demand(&stats)[b.zone.index()] > 0;
            if supplied
                && full
                && wanted
                && b.level < MAX_LEVEL
                && tick.since(b.leveled_at) >= LEVEL_UP_TICKS
                && level_room.take(&self.network, b.entrance, b.tile_count())
            {
                let up = Building {
                    level: b.level + 1,
                    leveled_at: tick,
                    ..b
                };
                stats = stats.without(&b).with(&up);
                *self.buildings.get_mut(id).expect("建物がない") = up;
                // 需要が増えたので、次の tick の頭で供給を判定し直す。
                self.supply_dirty = true;
            }
        }
        // 新しい建物は、その道路網の電気と水道に余りがあるときだけ建つ。施設のない網には数軒しか建たない。
        let mut headroom = Headroom::of(&self.network, &self.facilities, &self.buildings);
        for zone in ZoneKind::ALL {
            let pull = round::div_floor(demand(&stats)[zone.index()].max(0), SPAWN_PER_DEMAND);
            let spawns = u32::try_from(pull)
                .unwrap_or(u32::MAX)
                .saturating_add(SPAWN_PER_ZONE)
                .min(MAX_SPAWN_PER_ZONE);
            for attempt in 0..spawns {
                let list = &self.lots.by_zone[zone.index()];
                if list.is_empty() || demand(&stats)[zone.index()] <= 0 {
                    break;
                }
                let id = u64::from(zone as u8) * u64::from(MAX_SPAWN_PER_ZONE) + u64::from(attempt);
                let mut r = rng(self.seed, RngPurpose::BuildingGrowth, id, tick);
                let len = u32::try_from(list.len()).expect("候補の数が u32 に収まらない");
                let front = *list.iter().nth(r.below(len) as usize).expect("候補がない");
                let toward_road = FRONT_DIRS[usize::from(self.lots.fronts[&front].1)];
                let sizes: Vec<(u64, u64)> = if is_diagonal(toward_road) {
                    // 斜めの道路沿いは、引いた 1 辺の正方形から、収まるまで小さくする。
                    let side = 1 + u64::from(r.below(DIAGONAL_LOT_SIDE as u32));
                    (1..=side).rev().map(|s| (s, s)).collect()
                } else {
                    // 間口は引いた幅から、奥行きは区画の奥行きいっぱいから、収まるまで小さくする。
                    let wt = 1 + u64::from(r.below(MAX_LOT_SIDE as u32));
                    (1..=wt)
                        .rev()
                        .flat_map(|w| (1..=MAX_LOT_DEPTH).rev().map(move |k| (w, k)))
                        .collect()
                };
                // 建ったときは段階 1 なので、需要は占めるマスの数である。
                // 余裕に収まらない大きさは飛ばし、収まる中でいちばん大きな敷地に建てる。
                let link = self.frontage(front, toward_road).map(|(l, _)| l);
                let room = link.and_then(|l| headroom.room(&self.network, l));
                let fit = room.and_then(|room| {
                    sizes
                        .into_iter()
                        .filter(|(w, k)| w * k <= room)
                        .find(|(w, k)| self.lot_fits(zone, front, toward_road, *w, *k))
                });
                let (Some(link), Some((w, k))) = (link, fit) else {
                    continue;
                };
                assert!(headroom.take(&self.network, link, w * k));
                let id = self.place_building(zone, front, toward_road, w, k);
                stats = stats.with(self.buildings.get(id).expect("建てた建物がない"));
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
        let (entrance, entrance_at) = self
            .frontage(front, toward_road)
            .expect("間口が道路に面していない");
        let id = self.buildings.insert(Building {
            zone,
            area: TileRect::from_corners(front, far),
            front,
            toward_road,
            entrance,
            entrance_at,
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

    /// 施設を `at` に `rotation` の向きで置けるか。置けるなら間口の歩道のリンクを返す。状態は変えない。
    /// `rotation` は `0..=3` で、`at` から `DIRS[rotation]` へ 1 歩のマスが道路になる。
    pub fn check_service(
        &self,
        kind: ServiceKind,
        at: TilePos,
        rotation: u8,
    ) -> Result<LinkId, DenyReason> {
        let toward_road = DIRS[usize::from(rotation)];
        let (w, k) = kind.size();
        for (_, _, t) in lot_tiles(at, toward_road, w, k) {
            self.check_buildable(t)?;
        }
        let free = |t: TilePos| {
            matches!(
                self.tile_state(t),
                Some((_, TileUse::Empty | TileUse::Building))
            )
        };
        if !lot_tiles(at, toward_road, w, k).all(|(_, _, t)| free(t)) {
            return Err(DenyReason::Occupied);
        }
        let (entrance, _) = self
            .frontage(at, toward_road)
            .ok_or(DenyReason::NoRoadAccess)?;
        if lot_tiles(at, toward_road, w, 1).any(|(_, _, t)| self.frontage(t, toward_road).is_none())
        {
            return Err(DenyReason::NoRoadAccess);
        }
        if self.money < kind.cost() {
            return Err(DenyReason::NotEnoughMoney);
        }
        Ok(entrance)
    }

    /// 公共の施設を置く。`at` から `DIRS[rotation]` へ 1 歩のマスが道路で、敷地は反対側へ延びる。
    pub(super) fn place_service(
        &mut self,
        kind: ServiceKind,
        at: TilePos,
        rotation: u8,
    ) -> ApplyOutcome {
        let entrance = match self.check_service(kind, at, rotation) {
            Ok(entrance) => entrance,
            Err(reason) => return ApplyOutcome::Denied(reason),
        };
        let toward_road = DIRS[usize::from(rotation)];
        let (w, k) = kind.size();
        let tiles: Vec<TilePos> = lot_tiles(at, toward_road, w, k)
            .map(|(_, _, t)| t)
            .collect();
        let cost = kind.cost();
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
            .map(|f| self.frontage(f.front, f.toward_road).map(|(link, _)| link))
            .collect();
        for (f, entrance) in self.facilities.iter_mut().zip(facilities) {
            f.entrance = entrance;
        }
        let all: Vec<(BuildingId, Building)> =
            self.buildings.iter().map(|(id, b)| (id, *b)).collect();
        let mut removed = Vec::new();
        for (id, b) in all {
            let rect_tiles = crate::zone::tiles_of(b.area).collect::<Vec<_>>();
            let intact = rect_tiles
                .iter()
                .all(|t| self.tile_state(*t) == Some((Some(b.zone), TileUse::Building)));
            match self.frontage(b.front, b.toward_road).filter(|_| intact) {
                Some((entrance, entrance_at)) => {
                    let b = self.buildings.get_mut(id).expect("建物がない");
                    b.entrance = entrance;
                    b.entrance_at = entrance_at;
                }
                None => {
                    self.buildings.remove(id);
                    removed.push(id);
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
        self.vacate(&removed);
        self.vacate_shops(&removed);
        // 道路と施設の変化は、その場で供給に映す。
        self.update_supply();
    }

    /// 長方形にかかる施設を取り除き、取り除いた数を返す。払い戻しはしない。
    pub(super) fn bulldoze_facilities(&mut self, area: TileRect) -> usize {
        let (gone, kept): (Vec<Facility>, Vec<Facility>) =
            self.facilities.iter().partition(|f| overlaps(f.area, area));
        self.facilities = kept;
        let freed: Vec<_> = gone
            .iter()
            .flat_map(|f| crate::zone::tiles_of(f.area))
            .map(|t| (t, TileUse::Empty))
            .collect();
        self.write_occupancy(freed);
        gone.len()
    }

    /// 長方形にかかる建物のマスを空け、空けた建物の数を返す。区画は残す。払い戻しはしない。
    /// 建物は、続く [`World::refresh_buildings`] で、ほかの理由で消える建物と同じく後始末して消える。
    pub(super) fn bulldoze_buildings(&mut self, area: TileRect) -> usize {
        let gone: Vec<TileRect> = self
            .buildings
            .iter()
            .map(|(_, b)| b.area)
            .filter(|r| overlaps(*r, area))
            .collect();
        let freed: Vec<_> = gone
            .iter()
            .flat_map(|r| crate::zone::tiles_of(*r))
            .map(|t| (t, TileUse::Empty))
            .collect();
        self.write_occupancy(freed);
        gone.len()
    }
}

/// 2 つの長方形が 1 マスでも重なるか。
fn overlaps(a: TileRect, b: TileRect) -> bool {
    a.min.x <= b.max.x && b.min.x <= a.max.x && a.min.y <= b.max.y && b.min.y <= a.max.y
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, RoadKind};
    use crate::movement::SidewalkPos;
    use crate::service::{UNSUPPLIED_ALLOWANCE, building_demand};
    use crate::units::{Mm, Money, Tick, WorldPos};
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
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (9, 9), 2),
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
            assert_eq!(
                w.frontage(b.front, b.toward_road),
                Some((b.entrance, b.entrance_at))
            );
            // 出入口は、間口のマスの中心から道路へ向かって、縦横なら 1 マス、斜めなら
            // x と y にそれぞれ半マス進んだ点にある。
            let reach = if is_diagonal(b.toward_road) {
                TILE_MM / 2
            } else {
                TILE_MM
            };
            let at = |v: i64, d: i64| Mm(v * TILE_MM + TILE_MM / 2 + d * reach);
            let door = SidewalkPos::on_link(w.network(), b.entrance, b.entrance_at).unwrap();
            assert_eq!(
                door.world(),
                WorldPos::new(
                    at(b.front.x, b.toward_road.0),
                    at(b.front.y, b.toward_road.1)
                ),
                "{b:?}"
            );
            // 出入口の歩道を歩く人は、画面でも建物の側の歩道に描かれる。
            let net = w.network();
            let line = net.segment_line(b.entrance.segment).unwrap();
            let sign = if b.entrance.dir == Dir::Forward {
                1
            } else {
                -1
            };
            let step = (line.step.0 as i8 * sign, line.step.1 as i8 * sign);
            let lateral = crate::movement::lane_shift::lateral_mm(net, b.entrance, 0);
            let (dx, dy) = crate::movement::lane_shift::left_of(step, lateral);
            // 建物は道路から `toward_road` の反対にある。
            assert!(
                dx * b.toward_road.0 + dy * b.toward_road.1 < 0,
                "歩道が建物と反対側に描かれる {b:?}"
            );
            if is_diagonal(b.toward_road) {
                let (w, h) = crate::zone::rect_size(b.area).unwrap();
                assert!(w == h && w <= DIAGONAL_LOT_SIDE, "{b:?}");
            }
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
                if let Some(d) = w.front_dir(t) {
                    rebuilt.fronts.insert(t, (zone, d));
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
    fn services_supply_only_connected_buildings() {
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
        // 発電所は (0..=3, 6..=9) と (10..=13, 6..=9)、浄水場は (6..=7, 8..=9) と (8..=9, 8..=9) を占め、
        // y = 10 の道路に面する。
        for x in [3, 13] {
            assert_eq!(
                place(&mut w, ServiceKind::PowerPlant, (x, 9), 2),
                ApplyOutcome::Applied
            );
        }
        for x in [7, 9] {
            assert_eq!(
                place(&mut w, ServiceKind::WaterPump, (x, 9), 2),
                ApplyOutcome::Applied
            );
        }
        assert_eq!(
            w.money(),
            money
                - ServiceKind::PowerPlant.cost()
                - ServiceKind::PowerPlant.cost()
                - ServiceKind::WaterPump.cost()
                - ServiceKind::WaterPump.cost()
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

    /// 建物の 1 マスにかかる長方形でも、建物ごと取り壊す。区画と道路は残り、住む世帯は家なしに、
    /// 働く住民は職場を失う。空いた区画には、需要があればまた建つ。
    #[test]
    fn bulldoze_demolishes_buildings_and_keeps_their_zone() {
        let mut w = served_street();
        run(&mut w, 30 * 60);
        let p = w.population();
        let home = p
            .household_ids()
            .find_map(|h| p.home(h).flatten())
            .expect("住む世帯がない");
        let work = p
            .citizen_ids()
            .find_map(|id| p.citizen(id).unwrap().workplace)
            .expect("働く住民がない");
        let segments = w.network().segments().count();
        let mut freed = Vec::new();
        for id in [home, work] {
            let b = *w.buildings().get(id).unwrap();
            let coord = b.area.max.chunk().unwrap();
            let version = w.chunk(coord).unwrap().version();
            let area = rect((b.area.max.x, b.area.max.y), (b.area.max.x, b.area.max.y));
            assert_eq!(w.apply(&Command::Bulldoze { area }), ApplyOutcome::Applied);
            assert!(w.buildings().get(id).is_none());
            assert!(w.chunk(coord).unwrap().version() > version);
            for t in crate::zone::tiles_of(b.area) {
                assert_eq!(w.tile_state(t), Some((Some(b.zone), TileUse::Empty)));
            }
            check_buildings(&w);
            freed.push(b.area);
        }
        assert_eq!(w.network().segments().count(), segments);
        // 住む世帯と働く住民の表は、残った建物だけを指す。
        let p = w.population();
        let exists = |b: Option<BuildingId>| b.is_none_or(|b| w.buildings().get(b).is_some());
        assert!(p.household_ids().all(|h| exists(p.home(h).flatten())));
        assert!(p.household_ids().any(|h| p.home(h) == Some(None)));
        assert!(
            p.citizen_ids()
                .all(|id| exists(p.citizen(id).unwrap().workplace))
        );
        run(&mut w, 30 * 60);
        check_buildings(&w);
        let regrown = |r: TileRect| w.buildings().iter().any(|(_, b)| overlaps(b.area, r));
        assert!(freed.into_iter().any(regrown), "空いた区画にまた建たない");
    }

    /// 64 マスで段階 5 の建物（需要 320）を、`front` の x ごとに、道路の `toward_road` と反対の側に置く。
    fn insert_large(w: &mut World, xs: impl Iterator<Item = i64>, y: i64, toward_road: (i64, i64)) {
        for x in xs {
            let front = TilePos::new(x, y);
            let (entrance, entrance_at) = w.frontage(front, toward_road).unwrap();
            let far = (x + 7, y - 7 * toward_road.1);
            w.buildings.insert(Building {
                zone: ZoneKind::Residential,
                area: rect((x, y), far),
                front,
                toward_road,
                entrance,
                entrance_at,
                level: MAX_LEVEL,
                leveled_at: Tick::ZERO,
                supply: Supply::default(),
            });
        }
    }

    /// 需要が供給量を超えると、施設から遠い建物にだけ届かなくなる。
    #[test]
    fn supply_fails_only_for_buildings_beyond_capacity() {
        let mut w = World::new(0);
        let road = Command::BuildRoad {
            from: TilePos::new(0, 10),
            to: TilePos::new(110, 10),
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
        // 需要 320 の建物を 11 棟、施設から東へ並べると、需要は 3,520 になる。
        // 電気（3,600）は足り、水道（3,000）は近い 9 棟の分しかない。
        insert_large(&mut w, (0..11).map(|i| 10 + 8 * i), 9, (0, 1));
        w.update_supply();
        for (_, b) in w.buildings().iter() {
            let near = b.front.x < 10 + 8 * 9;
            assert_eq!(
                b.supply,
                Supply {
                    power: true,
                    water: near
                },
                "{:?}",
                b.front
            );
        }
    }

    /// 施設が道路の途中にあれば、両側の近い建物から順に届く。
    #[test]
    fn supply_reaches_the_nearest_buildings_first() {
        let mut w = World::new(0);
        let road = Command::BuildRoad {
            from: TilePos::new(0, 10),
            to: TilePos::new(125, 10),
            kind: RoadKind::Small,
        };
        assert_eq!(w.apply(&road), ApplyOutcome::Applied);
        // 北へ枝道を出して、東の端の区間を分ける。
        build(&mut w, (62, 0), (62, 10));
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (57, 9), 2),
            ApplyOutcome::Applied
        );
        // 道路の南側に 16 棟並べる。水道（3,000）は 9 棟の分しかない。
        insert_large(&mut w, (0..16).map(|i| 8 * i), 11, (0, -1));
        w.update_supply();
        let gap = |b: &Building| b.front.x.abs_diff(57);
        let (on, off): (Vec<Building>, Vec<Building>) = w
            .buildings()
            .iter()
            .map(|(_, b)| *b)
            .partition(|b| b.supply.water);
        assert_eq!(on.len(), 9);
        let farthest_on = on.iter().map(gap).max().unwrap();
        assert!(off.iter().all(|b| gap(b) > farthest_on), "{on:?}");
        // 枝道の交差点の先の区間にも、近い順に届く。
        assert!(on.iter().any(|b| b.front.x < 57) && on.iter().any(|b| b.front.x > 62));
        // 発電所がないので、電気はどこにも届かない。
        assert!(w.buildings().iter().all(|(_, b)| !b.supply.power));
    }

    /// `side` マス四方で段階 `level` の住宅を、道路の `toward_road` と反対の側に 1 棟置く。
    fn insert_square(
        w: &mut World,
        front: (i64, i64),
        toward_road: (i64, i64),
        side: i64,
        level: u8,
    ) -> BuildingId {
        let (x, y) = front;
        let front = TilePos::new(x, y);
        let (entrance, entrance_at) = w.frontage(front, toward_road).unwrap();
        w.buildings.insert(Building {
            zone: ZoneKind::Residential,
            area: rect((x, y), (x + side - 1, y - (side - 1) * toward_road.1)),
            front,
            toward_road,
            entrance,
            entrance_at,
            level,
            leveled_at: Tick::ZERO,
            supply: Supply::default(),
        })
    }

    fn street(w: &mut World, from: i64, to: i64) {
        let road = Command::BuildRoad {
            from: TilePos::new(from, 10),
            to: TilePos::new(to, 10),
            kind: RoadKind::Small,
        };
        assert_eq!(w.apply(&road), ApplyOutcome::Applied);
    }

    /// 収まらない建物が出たら、残りに収まる小さな建物がそれより遠くにあっても届けない。
    #[test]
    fn supply_stops_at_the_first_building_that_does_not_fit() {
        let mut w = World::new(0);
        street(&mut w, 0, 115);
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (7, 9), 2),
            ApplyOutcome::Applied
        );
        // 需要 320 の 9 棟で 2,880 を使い、残りは 120 になる。
        insert_large(&mut w, (0..9).map(|i| 10 + 8 * i), 9, (0, 1));
        let big = insert_square(&mut w, (90, 9), (0, 1), 6, MAX_LEVEL);
        let small = insert_square(&mut w, (100, 9), (0, 1), 1, 1);
        w.update_supply();
        let water = |id: BuildingId| w.buildings().get(id).unwrap().supply.water;
        assert!(!water(big));
        assert!(!water(small), "収まらない建物より遠い建物に届いた");
        let on = w.buildings().iter().filter(|(_, b)| b.supply.water).count();
        assert_eq!(on, 9);
    }

    /// 足りない網の代表のマスは、需要が最も大きい建物のマスで、同点なら ID の小さい建物のマスである。
    #[test]
    fn balance_spot_is_the_biggest_building_then_the_smallest_id() {
        let spot = |w: &World| {
            let b = crate::service::supply_balance(w.network(), w.facilities(), w.buildings());
            // 建物のある網は 1 つだけである。
            let spots: Vec<TilePos> = b.spots.into_iter().flatten().collect();
            assert!(spots.len() <= 1, "{spots:?}");
            spots.first().copied()
        };
        let mut w = World::new(0);
        street(&mut w, 0, 80);
        // 同じ需要の 2 棟なら、先に置いた（ID の小さい）ほう。
        insert_square(&mut w, (10, 9), (0, 1), 4, 2);
        insert_square(&mut w, (30, 9), (0, 1), 4, 2);
        assert_eq!(spot(&w), Some(TilePos::new(10, 9)));
        // 需要の大きい棟が後から来ても、そちらを選ぶ。
        insert_square(&mut w, (50, 9), (0, 1), 4, 3);
        assert_eq!(spot(&w), Some(TilePos::new(50, 9)));
        // 同じ需要の棟がもう 1 棟あっても、ID の小さい棟のままである。
        insert_square(&mut w, (70, 9), (0, 1), 4, 3);
        assert_eq!(spot(&w), Some(TilePos::new(50, 9)));
    }

    /// 道のりが同じ建物は、建物の ID の小さい順に届く。
    #[test]
    fn supply_breaks_ties_by_building_id() {
        let mut w = World::new(0);
        street(&mut w, 0, 100);
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (3, 9), 2),
            ApplyOutcome::Applied
        );
        // 2 棟を近くに置き、その先に道路の両側で同じ x の組を並べる。水道（3,000）は需要 80 の 37 棟の分なので、
        // 18 組めの 2 棟のうち 1 棟だけに届く。南の側を先に置くので、南の側の ID が小さい。
        insert_square(&mut w, (6, 9), (0, 1), 4, MAX_LEVEL);
        insert_square(&mut w, (6, 11), (0, -1), 4, MAX_LEVEL);
        let mut pairs = Vec::new();
        for i in 0..20 {
            let x = 10 + 4 * i;
            let south = insert_square(&mut w, (x, 11), (0, -1), 4, MAX_LEVEL);
            let north = insert_square(&mut w, (x, 9), (0, 1), 4, MAX_LEVEL);
            pairs.push((south, north));
        }
        w.update_supply();
        let water = |id: BuildingId| w.buildings().get(id).unwrap().supply.water;
        for (i, (south, north)) in pairs.into_iter().enumerate() {
            let expected = match i {
                0..17 => (true, true),
                17 => (true, false),
                _ => (false, false),
            };
            assert!(south < north);
            assert_eq!((water(south), water(north)), expected, "{i} 組め");
        }
    }

    /// 同じ種類の施設が 2 つあれば、建物はどちらか近いほうから数えた道のりの順に届く。
    #[test]
    fn supply_counts_the_distance_from_the_nearest_facility() {
        let mut w = World::new(0);
        street(&mut w, -60, 120);
        // 北へ枝道を出して、通りを x = 30 で 2 つの区間に分ける。
        build(&mut w, (30, 0), (30, 10));
        for x in [-55, 115] {
            assert_eq!(
                place(&mut w, ServiceKind::WaterPump, (x, 9), 2),
                ApplyOutcome::Applied
            );
        }
        // 南の側に 22 棟（需要 7,040）を並べる。水道（6,000）は 18 棟の分しかない。
        insert_large(&mut w, (0..22).map(|i| -56 + 8 * i), 11, (0, -1));
        w.update_supply();
        for (_, b) in w.buildings().iter() {
            let middle = (16..=40).contains(&b.front.x);
            assert_eq!(b.supply.water, !middle, "{:?}", b.front);
        }
    }

    /// 新しい建物は、電気と水道の両方に余りがある網にしか建たない。施設のない網にも、
    /// [`UNSUPPLIED_ALLOWANCE`] までは建つ。施設を置けば、直後の成長の回から建ちはじめる。
    #[test]
    fn new_buildings_need_room_for_both_power_and_water() {
        let mut w = zoned_street();
        // 区画のない西の延長に施設を置く。
        street(&mut w, -20, 0);
        let demand =
            |w: &World| -> u64 { w.buildings().iter().map(|(_, b)| building_demand(b)).sum() };
        run(&mut w, 20 * GROWTH_INTERVAL);
        let bare = w.buildings().len();
        assert!(bare > 0, "施設を置く前に 1 軒も建たない");
        assert!(demand(&w) <= UNSUPPLIED_ALLOWANCE, "{}", demand(&w));
        // 発電所だけでは、水道に余りがないので建たない。
        assert_eq!(
            place(&mut w, ServiceKind::PowerPlant, (-10, 9), 2),
            ApplyOutcome::Applied
        );
        run(&mut w, 20 * GROWTH_INTERVAL);
        assert_eq!(w.buildings().len(), bare);
        assert_eq!(
            place(&mut w, ServiceKind::WaterPump, (-5, 9), 2),
            ApplyOutcome::Applied
        );
        run(&mut w, GROWTH_INTERVAL);
        assert!(w.buildings().len() > bare, "浄水場を置いても建たない");
    }

    /// 道路を切って施設のない側に分かれた網には、[`UNSUPPLIED_ALLOWANCE`] までしか建たない。
    #[test]
    fn new_buildings_stop_on_the_side_cut_off_from_facilities() {
        let mut w = served_street();
        // 西へ 10 マスのつなぎの道路を出し、その先の通りに住宅を塗ってから、つなぎを消す。
        street(&mut w, -10, 0);
        street(&mut w, -50, -10);
        paint(&mut w, (-50, 4), (-10, 9), ZoneKind::Residential);
        let cut = Command::Bulldoze {
            area: rect((-5, 10), (-5, 10)),
        };
        assert_eq!(w.apply(&cut), ApplyOutcome::Applied);
        run(&mut w, 30 * GROWTH_INTERVAL);
        let cut_off: Vec<Building> = w
            .buildings()
            .iter()
            .map(|(_, b)| *b)
            .filter(|b| b.front.x <= -10)
            .collect();
        let served = w.buildings().len() - cut_off.len();
        let cut_demand: u64 = cut_off.iter().map(building_demand).sum();
        assert!(!cut_off.is_empty());
        assert!(cut_demand <= UNSUPPLIED_ALLOWANCE, "{cut_demand}");
        assert!(cut_off.iter().all(|b| b.supply == Supply::default()));
        assert!(served > cut_off.len(), "{served} 棟");
    }

    /// 離れた網に片方の施設だけを置くと、足りないほうだけが `growth_blocks` に数えられる。
    #[test]
    fn growth_blocks_tell_power_from_water() {
        for (kind, power_short, water_short) in [
            (ServiceKind::PowerPlant, false, true),
            (ServiceKind::WaterPump, true, false),
        ] {
            let mut w = served_street();
            let end = crate::world::highway_line(w.seed()).start.x.max(40);
            let (x0, x1) = (end + 1, end + 40);
            street(&mut w, x0, x1);
            paint(&mut w, (x0, 4), (x1, 9), ZoneKind::Residential);
            assert_eq!(place(&mut w, kind, (x0 + 5, 9), 2), ApplyOutcome::Applied);
            run(&mut w, 40 * GROWTH_INTERVAL);
            let res = w.growth_blocks()[0];
            assert!(res.blocked > 0, "{kind:?} {res:?}");
            assert_eq!(res.no_power > 0, power_short, "{kind:?} {res:?}");
            assert_eq!(res.no_water > 0, water_short, "{kind:?} {res:?}");
            assert_eq!(res.no_power + res.no_water, res.blocked, "{kind:?} {res:?}");
        }
    }

    /// 需要のない区画は、余裕がなくても施設を置いて直る話ではないので、止まっている間口に数えない。
    #[test]
    fn growth_blocks_ignore_zones_without_demand() {
        let mut w = served_street();
        let end = crate::world::highway_line(w.seed()).start.x.max(40);
        let (x0, x1) = (end + 1, end + 40);
        street(&mut w, x0, x1);
        paint(&mut w, (x0, 4), (x1, 9), ZoneKind::Office);
        run(&mut w, 5 * GROWTH_INTERVAL);
        let off = w.growth_blocks()[ZoneKind::Office.index()];
        assert!(demand(&w.city_stats())[ZoneKind::Office.index()] <= 0);
        assert!(off.fronts > 0, "{off:?}");
        assert_eq!(off.blocked, 0, "{off:?}");
    }

    /// 道路を 1 マス離して引いた先は施設のない別の網になり、[`UNSUPPLIED_ALLOWANCE`] までしか建たない。
    /// 建たない間口は [`World::growth_blocks`] に数えられ、道路をつなげば建ちはじめる。
    #[test]
    fn a_road_one_tile_apart_is_a_separate_network_where_growth_stalls() {
        let mut w = served_street();
        // 通りは外とつなぐ道路の端までで、その次のマスから引いた道路との間に 1 マスの隙間がある。
        let end = crate::world::highway_line(w.seed()).start.x.max(40);
        let (x0, x1) = (end + 1, end + 40);
        street(&mut w, x0, x1);
        paint(&mut w, (x0, 4), (x1, 9), ZoneKind::Residential);
        run(&mut w, 40 * GROWTH_INTERVAL);
        let apart = |w: &World| -> Vec<Building> {
            w.buildings()
                .iter()
                .map(|(_, b)| *b)
                .filter(|b| b.front.x >= x0)
                .collect()
        };
        let before = apart(&w);
        let demand: u64 = before.iter().map(building_demand).sum();
        assert!(demand <= UNSUPPLIED_ALLOWANCE, "{demand}");
        let [res, ..] = w.growth_blocks();
        assert!(res.blocked > 0, "{res:?}");
        assert!(
            res.no_power == res.blocked && res.no_water == res.blocked,
            "{res:?}"
        );
        // 施設のある網の区画は建ち尽くしていて、残る間口は施設のない網のものだけである。
        assert_eq!(res.blocked, res.fronts, "{res:?}");
        // 道路を引き直して隙間をなくせば、同じ網になって建ちはじめる。
        let clear = Command::Bulldoze {
            area: rect((x0, 10), (x1, 10)),
        };
        assert_eq!(w.apply(&clear), ApplyOutcome::Applied);
        street(&mut w, end, x1);
        paint(&mut w, (x0, 4), (x1, 9), ZoneKind::Residential);
        // 手前の施設の余裕は建ち尽くしているので、足りない分の施設も置く。
        for (kind, dx) in [
            (ServiceKind::PowerPlant, 5),
            (ServiceKind::WaterPump, 12),
            (ServiceKind::WaterPump, 15),
        ] {
            assert_eq!(place(&mut w, kind, (end + dx, 9), 2), ApplyOutcome::Applied);
        }
        run(&mut w, 40 * GROWTH_INTERVAL);
        assert!(apart(&w).len() > before.len() + 2, "つないでも建たない");
        assert_eq!(w.growth_blocks()[0].blocked, 0);
    }

    /// 余裕が最小の敷地より少し大きいだけでも、収まる小さな敷地が建ち、余裕を使い切る。
    /// 余裕が尽きれば、残る間口は `growth_blocks` に数えられる。
    #[test]
    fn growth_shrinks_the_lot_to_the_remaining_room() {
        let mut w = World::new(0);
        street(&mut w, 0, 40);
        paint(&mut w, (0, 4), (40, 9), ZoneKind::Residential);
        // 反対側の需要 45 の建物で、施設のない網の余裕 48 のうち 3 が残る。
        insert_square(&mut w, (10, 11), (0, -1), 3, 5);
        w.update_supply();
        let before = w.growth_blocks()[0];
        assert!(before.fronts > 0);
        assert_eq!(before.blocked, 0, "余裕が 3 あるのに数えられた");
        run(&mut w, 40 * GROWTH_INTERVAL);
        let demand: u64 = w.buildings().iter().map(|(_, b)| building_demand(b)).sum();
        assert_eq!(demand, UNSUPPLIED_ALLOWANCE, "余裕を使い切っていない");
        let after = w.growth_blocks()[0];
        assert!(after.fronts > 0);
        assert_eq!(after.blocked, after.fronts, "{after:?}");
    }

    /// 施設のない網に建てられる需要の合計は、ちょうど [`UNSUPPLIED_ALLOWANCE`] までである。
    #[test]
    fn unsupplied_allowance_is_the_exact_limit() {
        let mut w = World::new(0);
        street(&mut w, 0, 40);
        let id = insert_square(&mut w, (10, 9), (0, 1), 4, 1);
        let link = w.buildings().get(id).unwrap().entrance;
        let used = building_demand(w.buildings().get(id).unwrap());
        let headroom = || Headroom::of(&w.network, &w.facilities, &w.buildings);
        let rest = UNSUPPLIED_ALLOWANCE - used;
        assert!(!headroom().take(&w.network, link, rest + 1));
        let mut h = headroom();
        assert!(h.take(&w.network, link, rest));
        assert!(!h.take(&w.network, link, 1));
    }

    fn paint(w: &mut World, a: (i64, i64), b: (i64, i64), zone: ZoneKind) {
        let c = Command::PaintZone {
            area: rect(a, b),
            zone: Some(zone),
        };
        assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{c:?}");
    }

    #[test]
    fn houses_grow_along_a_diagonal_street_and_vanish_with_it() {
        let mut w = World::new(7);
        build(&mut w, (0, 0), (30, 30));
        paint(&mut w, (-6, -6), (36, 36), ZoneKind::Residential);
        check_buildings(&w);
        run(&mut w, 30 * 60);
        check_buildings(&w);
        assert!(
            w.buildings().len() >= 8,
            "{} 棟しか建たない",
            w.buildings().len()
        );
        // 道路の両側に建ち、どれも斜めの道路に向く。
        let side = |b: &Building| b.front.x > b.front.y;
        assert!(w.buildings().iter().any(|(_, b)| side(b)));
        assert!(w.buildings().iter().any(|(_, b)| !side(b)));
        assert!(
            w.buildings()
                .iter()
                .all(|(_, b)| is_diagonal(b.toward_road))
        );
        // 区間を途中で分けても、出入口を付け直して残る。
        let before = w.buildings().len();
        build(&mut w, (15, 15), (15, 25));
        check_buildings(&w);
        assert!(w.buildings().len() >= before - 2);
        // 斜めの道路を消すと、建物も消える。
        let bulldoze = Command::Bulldoze {
            area: rect((0, 0), (30, 30)),
        };
        assert_eq!(w.apply(&bulldoze), ApplyOutcome::Applied);
        assert!(w.buildings().is_empty());
        check_buildings(&w);
    }

    #[test]
    fn buildings_agree_where_straight_and_diagonal_roads_cross() {
        let mut w = World::new(3);
        build(&mut w, (0, 15), (40, 15));
        build(&mut w, (5, 0), (35, 30));
        build(&mut w, (30, 0), (30, 30));
        paint(&mut w, (-6, -6), (46, 36), ZoneKind::Residential);
        paint(&mut w, (21, 16), (46, 36), ZoneKind::Industrial);
        // 施設のない網では余裕が小さく、建物の数が決まらないので、施設を置く。
        for (kind, x) in [
            (ServiceKind::PowerPlant, 5),
            (ServiceKind::WaterPump, 8),
            (ServiceKind::WaterPump, 11),
        ] {
            assert_eq!(place(&mut w, kind, (x, 14), 2), ApplyOutcome::Applied);
        }
        check_buildings(&w);
        run(&mut w, 30 * 60);
        check_buildings(&w);
        let diagonal = |w: &World| {
            w.buildings()
                .iter()
                .filter(|(_, b)| is_diagonal(b.toward_road))
                .count()
        };
        assert!(diagonal(&w) > 0 && diagonal(&w) < w.buildings().len());
        // 交わる 2 か所のあいだの斜めの区間だけを消すと、そこに面した建物だけが消える。
        let cut = Command::Bulldoze {
            area: rect((25, 20), (25, 20)),
        };
        assert_eq!(w.apply(&cut), ApplyOutcome::Applied);
        check_buildings(&w);
        assert!(w.buildings().iter().all(|(_, b)| {
            let (x, y) = (b.front.x, b.front.y);
            !(is_diagonal(b.toward_road) && 20 < x && x < 31 && 15 < y && y < 26)
        }));
        // 引き直すと、また建つ。
        build(&mut w, (20, 15), (30, 25));
        check_buildings(&w);
        run(&mut w, 30 * 60);
        check_buildings(&w);
        assert!(diagonal(&w) > 0);
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// チャンクの境目のまわりで、斜めを含む道路を引く・消す・分ける、区画を塗る、時間を進めるを
        /// 任意に並べても、毎回、増分の候補が全体から作り直したものと一致し、建物が網と食い違わない。
        #[test]
        fn lot_fronts_match_a_full_rebuild_across_chunk_borders(
            edits in proptest::collection::vec(
                (0_u8..6, 0_usize..4, -8_i64..8, -8_i64..8, -1_i64..=1, -1_i64..=1, 1_i64..10),
                1..24,
            )
        ) {
            let mut w = World::new(0);
            w.money = Money(1_000_000_000);
            // 間口のマスと、道路の側の 2 マスが別のチャンクに分かれるよう、境目の角のまわりを使う。
            let corners = [(0, 0), (64, 0), (0, 64), (64, 64)];
            for (op, c, dx, dy, sx, sy, len) in edits {
                let (x, y) = (corners[c].0 + dx, corners[c].1 + dy);
                match op {
                    0 => {
                        let area = rect((x, y), (x + len / 3, y + len / 3));
                        w.apply(&Command::Bulldoze { area });
                    }
                    1 => {
                        let zone = [None, Some(ZoneKind::Residential), Some(ZoneKind::Industrial)]
                            [(sx + 1) as usize];
                        w.apply(&Command::PaintZone {
                            area: rect((x - len, y - len), (x + len, y + len)),
                            zone,
                        });
                    }
                    2 => run(&mut w, GROWTH_INTERVAL),
                    _ if (sx, sy) != (0, 0) => {
                        // 斜めを多めに引く。縦横の道路を引くと、斜めの区間を分けることもある。
                        let (sx, sy) = if op == 3 && sx * sy == 0 { (sx | 1, sy | 1) } else { (sx, sy) };
                        w.apply(&Command::BuildRoad {
                            from: TilePos::new(x, y),
                            to: TilePos::new(x + sx * len, y + sy * len),
                            kind: RoadKind::Small,
                        });
                    }
                    _ => {}
                }
                check_buildings(&w);
            }
        }
    }
}
