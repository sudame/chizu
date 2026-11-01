//! プレイヤーの操作を表すコマンド。
//!
//! 画面からの操作はすべて [`Command`] に直して中核へ渡す。値が受け付ける範囲にあるかは
//! [`Command::validate`] で確かめる。受け渡しの層も記録もこの関数を呼ぶので、
//! 範囲の決まりはここにしか書かない。
//!
//! 種類を表す enum の値は記録ファイルに書く番号である。既存の値を変えると古い記録が
//! 読めなくなるので、足すときは新しい値を使う。

use core::fmt;

use crate::network::{Line, MAX_ROAD_TILES};
use crate::tile::{ChunkCoord, TilePos, TileRect, WORLD_LIMIT};
use crate::zone::{MAX_ZONE_SIDE, rect_size};

/// 道路の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum RoadKind {
    /// 片側 1 車線。
    Small = 1,
    /// 片側 2 車線。
    Medium = 2,
    /// 片側 3 車線。
    Large = 3,
}

impl RoadKind {
    pub fn from_code(code: u8) -> Option<RoadKind> {
        match code {
            1 => Some(RoadKind::Small),
            2 => Some(RoadKind::Medium),
            3 => Some(RoadKind::Large),
            _ => None,
        }
    }
}

/// 区画の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ZoneKind {
    Residential = 1,
    Commercial = 2,
    Industrial = 3,
    Office = 4,
}

impl ZoneKind {
    pub const ALL: [ZoneKind; 4] = [
        ZoneKind::Residential,
        ZoneKind::Commercial,
        ZoneKind::Industrial,
        ZoneKind::Office,
    ];

    pub fn from_code(code: u8) -> Option<ZoneKind> {
        match code {
            1 => Some(ZoneKind::Residential),
            2 => Some(ZoneKind::Commercial),
            3 => Some(ZoneKind::Industrial),
            4 => Some(ZoneKind::Office),
            _ => None,
        }
    }

    /// `ALL` の中での位置。区画ごとの表の添え字に使う。
    pub fn index(self) -> usize {
        self as usize - 1
    }
}

/// プレイヤーが置く公共の建物の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ServiceKind {
    PowerPlant = 1,
    WaterPump = 2,
}

impl ServiceKind {
    pub fn from_code(code: u8) -> Option<ServiceKind> {
        match code {
            1 => Some(ServiceKind::PowerPlant),
            2 => Some(ServiceKind::WaterPump),
            _ => None,
        }
    }
}

/// 道路網のノードの ID。添え字と、使い回したことを見分ける世代の組である。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId {
    pub index: u32,
    pub generation: u16,
}

/// シミュレーションの速度。等速の何倍で回すかを表す。
///
/// 一時停止は tick を進めないだけなので、速度には含めない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SimSpeed(u16);

impl SimSpeed {
    pub const NORMAL: SimSpeed = SimSpeed(1);
    /// 最高速。1 日を 5 秒で回す。
    pub const MAX: SimSpeed = SimSpeed(48);

    /// 等速の `multiplier` 倍。`1..=48` の外なら `None`。
    pub fn new(multiplier: u16) -> Option<SimSpeed> {
        (SimSpeed::NORMAL.0..=SimSpeed::MAX.0)
            .contains(&multiplier)
            .then_some(SimSpeed(multiplier))
    }

    pub fn multiplier(self) -> u16 {
        self.0
    }
}

impl Default for SimSpeed {
    fn default() -> Self {
        SimSpeed::NORMAL
    }
}

/// 税率の上限（‰）。
pub const MAX_TAX_PERMILLE: u16 = 300;

/// プレイヤーの操作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    BuildRoad {
        from: TilePos,
        to: TilePos,
        kind: RoadKind,
    },
    Bulldoze {
        area: TileRect,
    },
    /// 長方形の中の、塗れるマスに区画を塗る。`zone` が `None` なら区画を消す。
    PaintZone {
        area: TileRect,
        zone: Option<ZoneKind>,
    },
    PlaceBuilding {
        kind: ServiceKind,
        at: TilePos,
        rotation: u8,
    },
    SetSignal {
        node: NodeId,
        on: bool,
    },
    BuyChunk {
        coord: ChunkCoord,
    },
    /// 区画の種類ごとの税率を ‰ で決める。`0..=MAX_TAX_PERMILLE` の外は受け付けない。
    SetTaxRate {
        zone: ZoneKind,
        permille: u16,
    },
    SetSimSpeed {
        speed: SimSpeed,
    },
}

/// コマンドの値が受け付ける範囲の外にある。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvalidCommand {
    /// マスかチャンクが、コマンドで指せる範囲（`WORLD_LIMIT`）の外にある。
    OutOfWorld,
    /// 長方形の `min` が `max` を超えている。
    UnorderedRect(TileRect),
    /// 税率が `MAX_TAX_PERMILLE` を超えている。
    TaxRateTooHigh(u16),
    /// 区画を塗る長方形の辺が `MAX_ZONE_SIDE` マスより長い。
    ZoneAreaTooLarge(TileRect),
    /// 道路の始点と終点が同じマスである。
    ZeroLengthRoad(TilePos),
    /// 道路が縦横と 45 度の斜めのどれでもない。
    RoadNotStraight { from: TilePos, to: TilePos },
    /// 道路が `MAX_ROAD_TILES` マスより長い。
    RoadTooLong { from: TilePos, to: TilePos },
}

impl fmt::Display for InvalidCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvalidCommand::UnorderedRect(r) => write!(
                f,
                "長方形の角の順番が逆になっている：min ({}, {})、max ({}, {})",
                r.min.x, r.min.y, r.max.x, r.max.y
            ),
            InvalidCommand::TaxRateTooHigh(p) => {
                write!(f, "税率 {p}‰ は上限の {MAX_TAX_PERMILLE}‰ を超えている")
            }
            InvalidCommand::OutOfWorld => {
                write!(f, "マスの座標の絶対値が上限の {WORLD_LIMIT} を超えている")
            }
            InvalidCommand::ZoneAreaTooLarge(r) => write!(
                f,
                "区画を塗る長方形 ({}, {})〜({}, {}) の辺が上限の {MAX_ZONE_SIDE} マスより長い",
                r.min.x, r.min.y, r.max.x, r.max.y
            ),
            InvalidCommand::ZeroLengthRoad(t) => {
                write!(f, "道路の始点と終点が同じマス ({}, {}) である", t.x, t.y)
            }
            InvalidCommand::RoadNotStraight { from, to } => write!(
                f,
                "道路 ({}, {}) → ({}, {}) が縦横と 45 度の斜めのどれでもない",
                from.x, from.y, to.x, to.y
            ),
            InvalidCommand::RoadTooLong { from, to } => write!(
                f,
                "道路 ({}, {}) → ({}, {}) が上限の {MAX_ROAD_TILES} マスより長い",
                from.x, from.y, to.x, to.y
            ),
        }
    }
}

impl std::error::Error for InvalidCommand {}

impl Command {
    /// コマンドが指すマスとチャンクが、すべて [`WORLD_LIMIT`] の範囲の中か。
    fn in_world(&self) -> bool {
        match *self {
            Command::BuildRoad { from, to, .. } => from.in_world() && to.in_world(),
            Command::Bulldoze { area } | Command::PaintZone { area, .. } => area.in_world(),
            Command::PlaceBuilding { at, .. } => at.in_world(),
            Command::BuyChunk { coord } => coord.in_world(),
            Command::SetSignal { .. }
            | Command::SetTaxRate { .. }
            | Command::SetSimSpeed { .. } => true,
        }
    }

    /// 値が受け付ける範囲にあるか。範囲の外のコマンドは記録せず、適用もしない。
    pub fn validate(&self) -> Result<(), InvalidCommand> {
        if !self.in_world() {
            return Err(InvalidCommand::OutOfWorld);
        }
        match *self {
            Command::Bulldoze { area } => {
                if area.is_ordered() {
                    Ok(())
                } else {
                    Err(InvalidCommand::UnorderedRect(area))
                }
            }
            Command::PaintZone { area, .. } => {
                if !area.is_ordered() {
                    return Err(InvalidCommand::UnorderedRect(area));
                }
                match rect_size(area) {
                    Some((w, h)) if w <= MAX_ZONE_SIDE && h <= MAX_ZONE_SIDE => Ok(()),
                    _ => Err(InvalidCommand::ZoneAreaTooLarge(area)),
                }
            }
            Command::SetTaxRate { permille, .. } => {
                if permille <= MAX_TAX_PERMILLE {
                    Ok(())
                } else {
                    Err(InvalidCommand::TaxRateTooHigh(permille))
                }
            }
            Command::BuildRoad { from, to, .. } => Line::between(from, to).map(|_| ()),
            // 種類の enum と `SimSpeed` は、型そのものが範囲の外の値を持てない。
            Command::PlaceBuilding { .. }
            | Command::SetSignal { .. }
            | Command::BuyChunk { .. }
            | Command::SetSimSpeed { .. } => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_kind_all_matches_the_index_and_every_variant() {
        for (i, zone) in ZoneKind::ALL.iter().enumerate() {
            assert_eq!(zone.index(), i);
            assert_eq!(ZoneKind::from_code(*zone as u8), Some(*zone));
        }
        // 種類を足したら、ここの match がコンパイルできなくなり、ALL にも足すことを思い出させる。
        let count = |z: ZoneKind| match z {
            ZoneKind::Residential
            | ZoneKind::Commercial
            | ZoneKind::Industrial
            | ZoneKind::Office => 4,
        };
        assert_eq!(ZoneKind::ALL.len(), count(ZoneKind::Office));
    }

    #[test]
    fn validate_checks_ranges() {
        let rect = TileRect {
            min: TilePos::new(1, 0),
            max: TilePos::new(0, 0),
        };
        assert_eq!(
            Command::Bulldoze { area: rect }.validate(),
            Err(InvalidCommand::UnorderedRect(rect))
        );
        let zone = ZoneKind::Office;
        let paint = |area| {
            Command::PaintZone {
                area,
                zone: Some(zone),
            }
            .validate()
        };
        assert_eq!(paint(rect), Err(InvalidCommand::UnorderedRect(rect)));
        let ok = TileRect::from_corners(rect.min, rect.max);
        assert_eq!(paint(ok), Ok(()));
        let side = MAX_ZONE_SIDE as i64;
        let max = TileRect::from_corners(TilePos::new(0, 0), TilePos::new(side - 1, side - 1));
        assert_eq!(paint(max), Ok(()));
        let wide = TileRect::from_corners(TilePos::new(0, 0), TilePos::new(side, 0));
        assert_eq!(paint(wide), Err(InvalidCommand::ZoneAreaTooLarge(wide)));
        let huge = TileRect::from_corners(TilePos::new(i64::MIN, 0), TilePos::new(i64::MAX, 0));
        assert_eq!(paint(huge), Err(InvalidCommand::OutOfWorld));
        let clear = Command::PaintZone {
            area: ok,
            zone: None,
        };
        assert_eq!(clear.validate(), Ok(()));
        let tax = |permille| Command::SetTaxRate { zone, permille }.validate();
        assert_eq!(tax(MAX_TAX_PERMILLE), Ok(()));
        assert_eq!(
            tax(MAX_TAX_PERMILLE + 1),
            Err(InvalidCommand::TaxRateTooHigh(MAX_TAX_PERMILLE + 1))
        );
    }

    #[test]
    fn validate_checks_road_shape() {
        let road = |fx, fy, tx, ty| {
            let (from, to) = (TilePos::new(fx, fy), TilePos::new(tx, ty));
            let kind = RoadKind::Small;
            Command::BuildRoad { from, to, kind }.validate()
        };
        let max = MAX_ROAD_TILES as i64;
        for ok in [
            (0, 0, 5, 0),
            (0, 0, 0, -5),
            (3, 3, -2, -2),
            (0, 0, max, -max),
        ] {
            assert_eq!(road(ok.0, ok.1, ok.2, ok.3), Ok(()), "{ok:?}");
        }
        let at = |x, y| TilePos::new(x, y);
        assert_eq!(
            road(1, 2, 1, 2),
            Err(InvalidCommand::ZeroLengthRoad(at(1, 2)))
        );
        assert_eq!(
            road(0, 0, 2, 1),
            Err(InvalidCommand::RoadNotStraight {
                from: at(0, 0),
                to: at(2, 1)
            })
        );
        assert!(matches!(
            road(0, 0, max + 1, 0),
            Err(InvalidCommand::RoadTooLong { .. })
        ));
        for (fx, tx) in [(-3, i64::MAX), (i64::MIN, i64::MAX)] {
            assert_eq!(
                road(fx, 0, tx, 0),
                Err(InvalidCommand::OutOfWorld),
                "{fx} → {tx}"
            );
        }
    }

    #[test]
    fn validate_keeps_commands_inside_the_world() {
        let lim = WORLD_LIMIT;
        let at = |x, y| TilePos::new(x, y);
        let rect = |a, b| TileRect::from_corners(a, b);
        let edge = rect(at(lim - 5, -lim), at(lim, -lim + 5));
        let over = rect(at(lim - 5, 0), at(lim + 1, 0));
        let road = |from, to| Command::BuildRoad {
            from,
            to,
            kind: RoadKind::Small,
        };
        let chunk = |cx| Command::BuyChunk {
            coord: ChunkCoord::new(cx, 0),
        };
        let paint = |area| Command::PaintZone { area, zone: None };
        let place = |at| Command::PlaceBuilding {
            kind: ServiceKind::PowerPlant,
            at,
            rotation: 0,
        };
        let chunk_lim = (lim / 64) as i32;
        for ok in [
            road(at(lim - 3, 0), at(lim, 0)),
            Command::Bulldoze { area: edge },
            paint(edge),
            chunk(chunk_lim - 1),
            chunk(-chunk_lim),
            place(at(-lim, lim)),
        ] {
            assert_eq!(ok.validate(), Ok(()), "{ok:?}");
        }
        for bad in [
            road(at(lim - 3, 0), at(lim + 1, 0)),
            Command::Bulldoze { area: over },
            paint(over),
            chunk(chunk_lim),
            chunk(-chunk_lim - 1),
            place(at(-lim - 1, 0)),
        ] {
            assert_eq!(bad.validate(), Err(InvalidCommand::OutOfWorld), "{bad:?}");
        }
    }
}
