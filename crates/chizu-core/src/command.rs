//! プレイヤーの操作を表すコマンド。
//!
//! 画面からの操作はすべて [`Command`] に直して中核へ渡す。値の検証は受け渡しの層で済ませ、
//! 中核に届くのは正しい値だけという前提にする。ただし記録ファイルから読むときは、
//! [`crate::record`] がここと同じ範囲を確かめる。
//!
//! 種類を表す enum の値は記録ファイルに書く番号である。既存の値を変えると古い記録が
//! 読めなくなるので、足すときは新しい値を使う。

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
