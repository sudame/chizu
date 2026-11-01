//! 区画の上に建つ建物の表。
//!
//! 建物は区画の上に、需要に応じて自然に建ち、時間がたつと段階が上がる。建つ・育つ処理は
//! `world::growth` にある。ここには表と、大きさと段階から決まる値だけを置く。

use crate::command::ZoneKind;
use crate::id::{BuildingId, Slots};
use crate::network::LinkId;
use crate::service::Supply;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::Tick;

/// 建物の段階の上限。
pub const MAX_LEVEL: u8 = 5;

/// 区画に建つ建物の、間口と奥行きの上限（マス）。
pub const MAX_LOT_SIDE: u64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Building {
    pub zone: ZoneKind,
    /// 建物が占めるマス。
    pub area: TileRect,
    /// 道路に面したマスの 1 つ。
    pub front: TilePos,
    /// `front` から道路のマスへ向かう 1 歩。縦か横である。
    pub toward_road: (i64, i64),
    /// 出入口。面した道路の、建物の側の歩道のリンク。道路が分かれて ID が変わると付け直す。
    pub entrance: LinkId,
    /// 1 から `MAX_LEVEL` まで。
    pub level: u8,
    /// いまの段階になった tick。
    pub leveled_at: Tick,
    /// 電気と水道が届いているか。供給の判定のたびに書き換える。
    pub supply: Supply,
}

impl Building {
    /// 占めるマスの数。
    pub fn tile_count(&self) -> u64 {
        let w = self.area.max.x.abs_diff(self.area.min.x) + 1;
        let h = self.area.max.y.abs_diff(self.area.min.y) + 1;
        w * h
    }

    /// 住める世帯の数の上限。住宅でなければ 0 である。
    pub fn households(&self) -> u64 {
        match self.zone {
            ZoneKind::Residential => self.tile_count() * u64::from(self.level),
            _ => 0,
        }
    }

    /// 職場の数の上限。住宅なら 0 である。
    pub fn workplaces(&self) -> u64 {
        let per_tile = match self.zone {
            ZoneKind::Residential => 0,
            ZoneKind::Commercial => 2,
            ZoneKind::Industrial => 3,
            ZoneKind::Office => 4,
        };
        self.tile_count() * per_tile * u64::from(self.level)
    }

    /// 駐車場の台数。1 マスに段階の数だけ停められる。
    pub fn parking_spaces(&self) -> u64 {
        self.tile_count() * u64::from(self.level)
    }
}

impl StateHash for Building {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&(self.zone as u8));
        h.write(&self.area.min.x);
        h.write(&self.area.min.y);
        h.write(&self.area.max.x);
        h.write(&self.area.max.y);
        h.write(&self.front.x);
        h.write(&self.front.y);
        h.write(&self.toward_road);
        h.write(&self.entrance.segment);
        h.write(&(self.entrance.dir as u8));
        h.write(&(self.entrance.mode as u8));
        h.write(&self.level);
        h.write(&self.leveled_at);
        h.write(&self.supply);
    }
}

/// 建物の表。行は ID の添え字の順に並ぶ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Buildings {
    slots: Slots,
    rows: Vec<Option<Building>>,
}

impl Buildings {
    pub fn get(&self, id: BuildingId) -> Option<&Building> {
        if !self.slots.contains(id.index, id.generation) {
            return None;
        }
        self.rows[id.slot()].as_ref()
    }

    pub(crate) fn get_mut(&mut self, id: BuildingId) -> Option<&mut Building> {
        if !self.slots.contains(id.index, id.generation) {
            return None;
        }
        self.rows[id.slot()].as_mut()
    }

    /// 建物を ID の順に返す。
    pub fn iter(&self) -> impl Iterator<Item = (BuildingId, &Building)> {
        self.rows.iter().enumerate().filter_map(|(slot, row)| {
            let (index, generation) = self.slots.current(slot)?;
            row.as_ref().map(|b| (BuildingId { index, generation }, b))
        })
    }

    pub fn len(&self) -> usize {
        self.slots.live_count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn insert(&mut self, b: Building) -> BuildingId {
        let ((index, generation), fresh) = self.slots.alloc();
        if fresh {
            self.rows.push(Some(b));
        } else {
            self.rows[index as usize] = Some(b);
        }
        BuildingId { index, generation }
    }

    pub(crate) fn remove(&mut self, id: BuildingId) -> Option<Building> {
        if !self.slots.release(id.index, id.generation) {
            return None;
        }
        self.rows[id.slot()].take()
    }

    /// 区画の種類ごとの、世帯の数と職場の数の合計。
    pub fn capacity(&self) -> ZoneTotals {
        let mut t = ZoneTotals::default();
        for (_, b) in self.iter() {
            t.households += b.households();
            t.workplaces[b.zone.index()] += b.workplaces();
        }
        t
    }
}

impl StateHash for Buildings {
    fn write_state(&self, h: &mut StateHasher) {
        h.write(&self.slots);
        h.write(&self.rows);
    }
}

/// 世帯の数と、区画の種類ごとの職場の数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoneTotals {
    pub households: u64,
    /// 添え字は [`ZoneKind::index`]。住宅の欄は 0 のままである。
    pub workplaces: [u64; 4],
}

/// 住民がまだいないあいだにも、住宅が建ち始めるための需要。
const BASE_RESIDENTIAL_DEMAND: i64 = 20;

/// 住民がまだいないあいだにも、工業が建ち始めるための需要。
const BASE_INDUSTRIAL_DEMAND: i64 = 10;

/// 区画の種類ごとの需要。正なら建物が建つ。添え字は [`ZoneKind::index`]。
///
/// 住宅は職場の数と世帯の数の差から、商業・工業・オフィスは世帯の数（人口の代わり）から決める。
pub fn demand(t: ZoneTotals) -> [i64; 4] {
    let n = |v: u64| i64::try_from(v).expect("建物の数があふれた");
    let jobs: u64 = t.workplaces.iter().sum();
    let homes = n(t.households);
    let w = |z: ZoneKind| n(t.workplaces[z.index()]);
    [
        BASE_RESIDENTIAL_DEMAND + n(jobs) - homes,
        n(t.households / 2) - w(ZoneKind::Commercial),
        BASE_INDUSTRIAL_DEMAND + n(t.households / 2) - w(ZoneKind::Industrial),
        n(t.households / 4) - w(ZoneKind::Office),
    ]
}
