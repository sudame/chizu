//! 建物の駐車場の台数と、敷地を切り欠く形。
//!
//! どちらも建物の種類、マスの数、段階、道路に面した向きから決まる純粋な関数で、状態には持たない。
//! 決め方は `docs/design.md` の 3.6「台数の決め方」と「敷地の切り欠き方」にある。
//!
//! 1. 要る台数を、住宅なら世帯の数、それ以外なら職場の数（税で減らす前）から決める。
//! 2. 要る台数を、敷地を切り欠いた平面の駐車場と、建物の中（地下か立体）の駐車場に分ける。
//!    集合住宅（住宅の段階 3〜5）は、平面には世帯の数の 0.7 倍までしか持たず、残りを建物の中に持つ。

use super::Building;
use crate::command::ZoneKind;

/// 平面の駐車場の、1 マスに並ぶ枠の数。枠は 2.5m × 5m で、8m の幅に 3 つ並ぶ。
pub const SLOTS_PER_TILE: u64 = 3;

/// 平面の駐車場の、1 つの枠の幅（mm）。前面の帯は、この幅の枠を道路に沿って並べる。
pub const SLOT_WIDTH_MM: u64 = 2_500;

/// 平面の駐車場に使えるのは、建物のマスのこの数分の 1（切り捨て）までである。建物の本体を、
/// 敷地の 2/3 より小さくしない。
const SURFACE_SHARE: u64 = 3;

/// これ以下のマスの建物は、道路に面した側の帯を平面の駐車場にする。
const STRIP_MAX_TILES: u64 = 2;

/// 敷地を切り欠いて作る、平面の駐車場の形。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParkingCut {
    /// 切り欠かない。斜めの道路に面した建物はこれになり、台数はすべて建物の中に持つ。
    None,
    /// 道路に面した辺に沿った、奥行き 5m の帯。マスが 1〜2 の建物に使う。枠を道路に直角に
    /// `slots` 個並べ、帯の長さは枠の数だけにする。
    FrontStrip { slots: u64 },
    /// 間口の端の列を、道路の側から奥へ切り欠いた駐車場。マスが 3 以上の建物に使う。
    /// `tiles` マスを、端の列から道路の側に詰めて切り欠き、1 列の奥行きで足りなければ隣の列も使う。
    /// 列は `columns` 本で、最後の列だけ浅いことがある。どちらの端の列にするかは、面した道路の
    /// 交差点の位置で決まるので、ここでは決めない（`movement::lot_spot`）。
    Side { columns: u64, tiles: u64 },
}

/// 建物の駐車場の配置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParkingLayout {
    /// 種類と大きさから決まる、要る台数。
    pub needed: u64,
    /// 切り欠いた平面の駐車場の台数。
    pub surface: u64,
    /// 建物の中の駐車場の台数。
    pub indoor: u64,
    pub cut: ParkingCut,
}

impl ParkingLayout {
    /// 停められる台数。戸建ては建物の中に駐車場を持たず、平面に入りきらない分は切り捨てるので、
    /// 要る台数より少なくなりうる（いまの大きさの上限では、戸建ての要る台数はいつも平面に収まる）。
    pub fn total(&self) -> u64 {
        self.surface + self.indoor
    }
}

impl Building {
    /// 駐車場の配置。
    pub fn parking_layout(&self) -> ParkingLayout {
        let needed = self.parking_needed();
        let surface_wanted = self.surface_wanted(needed);
        let (dx, dy) = self.toward_road;
        let tiles = self.tile_count();
        let w = self.area.max.x.abs_diff(self.area.min.x) + 1;
        let h = self.area.max.y.abs_diff(self.area.min.y) + 1;
        // 道路が南北の側にあれば、間口は東西の幅で、奥行きは南北の長さである。
        let (frontage, depth) = if dx == 0 { (w, h) } else { (h, w) };
        let diagonal = dx != 0 && dy != 0;
        let (surface, cut) = if diagonal {
            (0, ParkingCut::None)
        } else if tiles <= STRIP_MAX_TILES {
            let slots = surface_wanted.min(frontage * SLOTS_PER_TILE);
            (slots, ParkingCut::FrontStrip { slots })
        } else {
            let slots = surface_wanted.min(tiles / SURFACE_SHARE * SLOTS_PER_TILE);
            let cut_tiles = slots.div_ceil(SLOTS_PER_TILE);
            let cut = ParkingCut::Side {
                columns: cut_tiles.div_ceil(depth),
                tiles: cut_tiles,
            };
            (slots, cut)
        };
        let (surface, cut) = if surface == 0 {
            (0, ParkingCut::None)
        } else {
            (surface, cut)
        };
        // 戸建ては建物の中に駐車場を持たず、平面に入りきらない分は切り捨てる。ただし斜めの道路に面した
        // 戸建ては切り欠けないので、建物の中の車庫に持つ。
        let detached = self.zone == ZoneKind::Residential && self.level <= 2 && !diagonal;
        let indoor = if detached { 0 } else { needed - surface };
        ParkingLayout {
            needed,
            surface,
            indoor,
            cut,
        }
    }

    /// 要る台数。住宅は世帯の数、商業と工業は職場の数、オフィスは職場の数の 0.5 倍で、端数は切り上げる。
    fn parking_needed(&self) -> u64 {
        let (base, num, den) = match self.zone {
            ZoneKind::Residential => (self.households(), 1, 1),
            ZoneKind::Commercial | ZoneKind::Industrial => (self.workplaces(), 1, 1),
            ZoneKind::Office => (self.workplaces(), 1, 2),
        };
        (base * num).div_ceil(den)
    }

    /// 要る台数 `needed` のうち、平面の駐車場に持ちたい台数。集合住宅（住宅の段階 3〜5）は世帯の数の
    /// 0.7 倍（切り上げ）までで、残りは地下の駐車場として建物の中に持つ。ほかの建物は `needed` のまま。
    fn surface_wanted(&self, needed: u64) -> u64 {
        if self.zone == ZoneKind::Residential && self.level > 2 {
            (self.households() * 7).div_ceil(10)
        } else {
            needed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{Dir, LaneMode, LinkId, SegmentId};
    use crate::service::Supply;
    use crate::tile::{TilePos, TileRect};
    use crate::units::{LaneOffset, Tick};

    /// 間口 `w` マス、奥行き `h` マスの建物。道路は南（y の小さい側）にある。
    fn building(zone: ZoneKind, (w, h): (i64, i64), level: u8) -> Building {
        Building {
            zone,
            area: TileRect::from_corners(TilePos::new(0, 0), TilePos::new(w - 1, h - 1)),
            front: TilePos::new(0, 0),
            toward_road: (0, -1),
            entrance: LinkId {
                segment: SegmentId {
                    index: 0,
                    generation: 0,
                },
                dir: Dir::Forward,
                mode: LaneMode::Sidewalk,
            },
            entrance_at: LaneOffset(0),
            level,
            leveled_at: Tick(0),
            supply: Supply::default(),
        }
    }

    fn layout(zone: ZoneKind, size: (i64, i64), level: u8) -> (u64, u64, u64, ParkingCut) {
        let l = building(zone, size, level).parking_layout();
        (l.needed, l.surface, l.indoor, l.cut)
    }

    use ParkingCut::{FrontStrip, Side};
    use ZoneKind::{Commercial, Industrial, Office, Residential};

    #[test]
    fn small_buildings_park_on_a_strip_along_the_road() {
        // 1 マスの戸建ては 1 世帯で 1 台。
        assert_eq!(
            layout(Residential, (1, 1), 1),
            (1, 1, 0, FrontStrip { slots: 1 })
        );
        // 1 マスの工業の段階 5 は職場 4 で 4 台。帯には 3 台しか並ばず、残りは建物の中に持つ。
        assert_eq!(
            layout(Industrial, (1, 1), 5),
            (4, 3, 1, FrontStrip { slots: 3 })
        );
        // 間口 2 マスなら帯に 6 台並ぶ。
        assert_eq!(
            layout(Industrial, (2, 1), 2),
            (3, 3, 0, FrontStrip { slots: 3 })
        );
        assert_eq!(
            layout(Industrial, (2, 1), 4),
            (6, 6, 0, FrontStrip { slots: 6 })
        );
        // 奥行き 2 マスでも、間口が 1 マスなら帯は 3 台まで。
        assert_eq!(
            layout(Industrial, (1, 2), 4),
            (6, 3, 3, FrontStrip { slots: 3 })
        );
    }

    #[test]
    fn larger_buildings_cut_a_side_lot_up_to_a_third_of_the_tiles() {
        // 4×4 の戸建ては 8 世帯。平面に使えるのは 5 マス（15 台）までで、8 台なら 3 マスを 1 列で切り欠く。
        assert_eq!(
            layout(Residential, (4, 4), 2),
            (
                8,
                8,
                0,
                Side {
                    columns: 1,
                    tiles: 3
                }
            )
        );
        // 4×4 の集合住宅の段階 5 は 20 世帯で 20 台。平面は世帯の 0.7 倍の 14 台で、5 マスで、奥行き 4 の列に
        // 入りきらず 2 列にまたがる。残りの 6 台は地下に持つ。
        assert_eq!(
            layout(Residential, (4, 4), 5),
            (
                20,
                14,
                6,
                Side {
                    columns: 2,
                    tiles: 5
                }
            )
        );
        // 4×4 のオフィスの段階 5 は職場 80 で 40 台。平面は 15 台までで、残りの 25 台は建物の中。
        assert_eq!(
            layout(Office, (4, 4), 5),
            (
                40,
                15,
                25,
                Side {
                    columns: 2,
                    tiles: 5
                }
            )
        );
        // 3×1 は 1 マスまで切り欠ける。
        assert_eq!(
            layout(Commercial, (3, 1), 1),
            (
                2,
                2,
                0,
                Side {
                    columns: 1,
                    tiles: 1
                }
            )
        );
        // 工業の 4×3 の段階 3 は職場 27 で 27 台。平面は 4 マス 12 台、奥行き 3 の列 2 本。
        assert_eq!(
            layout(Industrial, (4, 3), 3),
            (
                27,
                12,
                15,
                Side {
                    columns: 2,
                    tiles: 4
                }
            )
        );
    }

    #[test]
    fn the_depth_follows_the_side_facing_the_road() {
        // 4×3 のオフィスは 4 マスを切り欠く。道路が南なら奥行き 3 で 2 列、東なら奥行き 4 で 1 列になる。
        let mut wide = building(Office, (4, 3), 5);
        assert_eq!(
            wide.parking_layout().cut,
            Side {
                columns: 2,
                tiles: 4
            }
        );
        wide.toward_road = (1, 0);
        assert_eq!(
            wide.parking_layout().cut,
            Side {
                columns: 1,
                tiles: 4
            }
        );
    }

    #[test]
    fn buildings_on_a_diagonal_road_keep_every_space_indoors() {
        let mut shop = building(Commercial, (2, 2), 3);
        shop.toward_road = (1, -1);
        let l = shop.parking_layout();
        assert_eq!((l.needed, l.surface, l.indoor), (6, 0, 6));
        assert_eq!(l.cut, ParkingCut::None);
        // 戸建ても、斜めの道路に面すると建物の中の車庫に持つ。台数はほかの戸建てと同じ 1 世帯 1 台。
        for level in [1, 2] {
            let mut house = building(Residential, (2, 2), level);
            house.toward_road = (-1, 1);
            let l = house.parking_layout();
            let households = house.households();
            assert_eq!(
                (l.needed, l.surface, l.indoor, l.cut),
                (households, 0, households, ParkingCut::None)
            );
        }
    }

    #[test]
    fn low_level_houses_have_no_indoor_parking_unless_on_a_diagonal_road() {
        for size in [(1, 1), (1, 2), (2, 1), (2, 2), (3, 4), (4, 4)] {
            for level in [1, 2] {
                let l = building(Residential, size, level).parking_layout();
                assert_eq!(l.indoor, 0, "{size:?} の段階 {level}");
                assert_eq!(l.surface, l.needed, "{size:?} の段階 {level}");
            }
        }
        // 段階 3 からは集合住宅で、平面に入りきらない分を建物の中に持つ。
        let l = building(Residential, (1, 1), 5).parking_layout();
        assert_eq!((l.needed, l.surface, l.indoor), (2, 2, 0));
        let l = building(Residential, (1, 2), 5).parking_layout();
        assert_eq!((l.needed, l.surface, l.indoor), (3, 3, 0));
        let mut l = building(Residential, (3, 3), 5);
        l.toward_road = (1, 1);
        let l = l.parking_layout();
        assert_eq!((l.needed, l.surface, l.indoor), (12, 0, 12));
    }

    #[test]
    fn apartments_have_a_space_per_household_and_keep_the_surface_lot() {
        for w in 1..=6_i64 {
            for h in 1..=6_i64 {
                for road in [(0, -1), (0, 1), (1, 0), (-1, 0), (1, 1)] {
                    for level in 3..=crate::building::MAX_LEVEL {
                        let mut b = building(Residential, (w, h), level);
                        b.toward_road = road;
                        let at = format!("{w}×{h} {road:?} の段階 {level}");
                        let l = b.parking_layout();
                        let households = b.households();
                        assert_eq!(l.needed, households, "{at}");
                        assert_eq!(l.total(), households, "{at}");
                        // 平面は世帯の 0.7 倍（切り上げ）までで、足りない分はすべて建物の中に持つ。
                        assert!(l.surface <= (households * 7).div_ceil(10), "{at}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_layout_holds_together_for_every_lot() {
        // 区画に建つのは 1 辺 4 マスまでだが、式が大きさによらず崩れないことを 6 マスまで確かめる。
        for zone in ZoneKind::ALL {
            for w in 1..=6_i64 {
                for h in 1..=6_i64 {
                    for road in [(0, -1), (0, 1), (1, 0), (-1, 0)] {
                        for level in 1..=crate::building::MAX_LEVEL {
                            let mut b = building(zone, (w, h), level);
                            b.toward_road = road;
                            let at = format!("{zone:?} {w}×{h} {road:?} の段階 {level}");
                            let (w, h) = (w.unsigned_abs(), h.unsigned_abs());
                            let (frontage, depth) = if road.0 == 0 { (w, h) } else { (h, w) };
                            let l = b.parking_layout();
                            assert_eq!(b.parking_spaces(), l.total(), "{at}");
                            assert_eq!(l.total(), l.needed, "{at}");
                            assert!(l.total() > 0, "{at}");
                            match l.cut {
                                ParkingCut::None => panic!("縦か横の道路なのに切り欠かない：{at}"),
                                FrontStrip { slots } => {
                                    assert!(b.tile_count() <= 2, "{at}");
                                    assert_eq!(slots, l.surface, "{at}");
                                    assert!(slots <= frontage * SLOTS_PER_TILE, "{at}");
                                }
                                Side { columns, tiles } => {
                                    assert!(b.tile_count() >= 3, "{at}");
                                    assert!(tiles * 3 <= b.tile_count(), "{at}");
                                    assert!(l.surface <= tiles * SLOTS_PER_TILE, "{at}");
                                    assert!(l.surface > (tiles - 1) * SLOTS_PER_TILE, "{at}");
                                    // 列は間口に沿って並び、1 列は奥行きぶんのマスを持つ。
                                    assert!(columns <= frontage, "{at}");
                                    assert!(tiles <= columns * depth, "{at}");
                                    assert!(tiles > (columns - 1) * depth, "{at}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn spaces_do_not_shrink_when_a_building_levels_up() {
        for zone in ZoneKind::ALL {
            for w in 1..=4 {
                for h in 1..=4 {
                    for road in [(0, -1), (1, 0), (1, 1)] {
                        let mut prev = 0;
                        for level in 1..=crate::building::MAX_LEVEL {
                            let mut b = building(zone, (w, h), level);
                            b.toward_road = road;
                            let n = b.parking_spaces();
                            assert!(n >= prev, "{zone:?} {w}×{h} {road:?} の段階 {level}");
                            prev = n;
                        }
                    }
                }
            }
        }
    }
}
