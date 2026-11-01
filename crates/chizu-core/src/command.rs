//! プレイヤーの操作を表すコマンド。
//!
//! 画面からの操作はすべて [`Command`] に直して中核へ渡す。値が受け付ける範囲にあるかは
//! [`Command::validate`] で確かめる。受け渡しの層も記録もこの関数を呼ぶので、
//! 範囲の決まりはここにしか書かない。
//!
//! 種類を表す enum の値は記録ファイルに書く番号である。既存の値を変えると古い記録が
//! 読めなくなるので、足すときは新しい値を使う。

use core::fmt;

use crate::tile::{ChunkCoord, TilePos, TileRect};

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
    PaintZone {
        area: TileRect,
        zone: ZoneKind,
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
    /// 長方形の `min` が `max` を超えている。
    UnorderedRect(TileRect),
    /// 税率が `MAX_TAX_PERMILLE` を超えている。
    TaxRateTooHigh(u16),
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
        }
    }
}

impl std::error::Error for InvalidCommand {}

impl Command {
    /// 値が受け付ける範囲にあるか。範囲の外のコマンドは記録せず、適用もしない。
    pub fn validate(&self) -> Result<(), InvalidCommand> {
        match *self {
            Command::Bulldoze { area } | Command::PaintZone { area, .. } => {
                if area.is_ordered() {
                    Ok(())
                } else {
                    Err(InvalidCommand::UnorderedRect(area))
                }
            }
            Command::SetTaxRate { permille, .. } => {
                if permille <= MAX_TAX_PERMILLE {
                    Ok(())
                } else {
                    Err(InvalidCommand::TaxRateTooHigh(permille))
                }
            }
            // 種類の enum と `SimSpeed` は、型そのものが範囲の外の値を持てない。
            Command::BuildRoad { .. }
            | Command::PlaceBuilding { .. }
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
        assert_eq!(
            Command::PaintZone { area: rect, zone }.validate(),
            Err(InvalidCommand::UnorderedRect(rect))
        );
        let ok = TileRect::from_corners(rect.min, rect.max);
        assert_eq!(Command::PaintZone { area: ok, zone }.validate(), Ok(()));
        let tax = |permille| Command::SetTaxRate { zone, permille }.validate();
        assert_eq!(tax(MAX_TAX_PERMILLE), Ok(()));
        assert_eq!(
            tax(MAX_TAX_PERMILLE + 1),
            Err(InvalidCommand::TaxRateTooHigh(MAX_TAX_PERMILLE + 1))
        );
    }
}
