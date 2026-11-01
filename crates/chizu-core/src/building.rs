//! 区画の上に建つ建物の表。
//!
//! 建物は区画の上に、需要に応じて自然に建ち、時間がたつと段階が上がる。建つ・育つ処理は
//! `world::growth` にある。ここには表と、大きさと段階から決まる値だけを置く。

use std::collections::BTreeSet;

use crate::command::ZoneKind;
use crate::id::{BuildingId, Slots};
use crate::network::LinkId;
use crate::service::Supply;
use crate::state_hash::{StateHash, StateHasher};
use crate::tile::{TilePos, TileRect};
use crate::units::{LaneOffset, Tick};

/// 建物の段階の上限。
pub const MAX_LEVEL: u8 = 5;

/// 区画に建つ建物の、間口と奥行きの上限（マス）。
pub const MAX_LOT_SIDE: u64 = 4;

/// 斜めの道路に面して建つ建物の 1 辺の上限（マス）。斜めの道路沿いには正方形の小さな建物を
/// 階段状に並べる。2 マスまでにすると、2×2 と 1×1 が交互に並んで隙間なく埋まる。
pub const DIAGONAL_LOT_SIDE: u64 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Building {
    pub zone: ZoneKind,
    /// 建物が占めるマス。
    pub area: TileRect,
    /// 道路に面したマスの 1 つ。
    pub front: TilePos,
    /// `front` から道路へ向かう向き。縦か横なら、1 歩先が縦か横の道路のマスである。
    /// 斜め `(dx, dy)` なら、`(dx, 0)` と `(0, dy)` の 1 歩先がどちらも同じ斜めの区間のマスで、
    /// 敷地は `(-dx, -dy)` の側へ延びる。
    pub toward_road: (i64, i64),
    /// 出入口。面した道路の、建物の側の歩道のリンク。道路が分かれて ID が変わると付け直す。
    pub entrance: LinkId,
    /// 出入口の、`entrance` の始まりからの距離。`front` に最も近い歩道の上の点である。
    pub entrance_at: LaneOffset,
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

    /// 住める世帯の数の上限。4 マスあたり段階ぶんの世帯で、端数は切り上げる。住宅でなければ 0 である。
    pub fn households(&self) -> u64 {
        match self.zone {
            ZoneKind::Residential => (self.tile_count() * u64::from(self.level)).div_ceil(4),
            _ => 0,
        }
    }

    /// 職場の数の上限。住宅なら 0 である。
    pub fn workplaces(&self) -> u64 {
        let per_four_tiles = match self.zone {
            ZoneKind::Residential => return 0,
            ZoneKind::Commercial => 2,
            ZoneKind::Industrial => 3,
            ZoneKind::Office => 4,
        };
        (self.tile_count() * per_four_tiles * u64::from(self.level)).div_ceil(4)
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
        h.write(&self.entrance_at);
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
    /// `front` の (y, x) で引く索引。行から作り直せるので状態ハッシュには入れない。
    by_front: BTreeSet<(i64, i64, BuildingId)>,
}

impl Buildings {
    /// `front` が `area` の中にある建物を、`front` の (y, x) の順に返す。
    pub fn fronts_in(&self, area: TileRect) -> impl Iterator<Item = BuildingId> + '_ {
        let (lo, hi) = (
            BuildingId {
                index: 0,
                generation: 0,
            },
            BuildingId {
                index: u32::MAX,
                generation: u16::MAX,
            },
        );
        (area.min.y..=area.max.y).flat_map(move |y| {
            self.by_front
                .range((y, area.min.x, lo)..=(y, area.max.x, hi))
                .map(|k| k.2)
        })
    }

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
        self.by_front
            .insert((b.front.y, b.front.x, BuildingId { index, generation }));
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
        let b = self.rows[id.slot()].take()?;
        self.by_front.remove(&(b.front.y, b.front.x, id));
        Some(b)
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

/// 街の外から住みたい人が来る、いつもある需要。空き家が埋まっていれば、街はこの需要で育ち続ける。
pub const BASE_RESIDENTIAL_DEMAND: i64 = 30;

/// 住宅の需要に幸福度を効かせる強さ。平均の幸福度と中立の値との差をこの数で割って足す。
const HAPPINESS_DEMAND_DIVISOR: i64 = 4;

/// 住宅の需要のうち、幸福度から来る分。平均の幸福度が中立より高ければ正、低ければ負である。
pub fn happiness_pull(happiness: u8) -> i64 {
    let neutral = i64::from(crate::happiness::total(
        crate::citizens::HappinessParts::NEUTRAL,
    ));
    crate::round::div_floor(i64::from(happiness) - neutral, HAPPINESS_DEMAND_DIVISOR)
}

/// 働き手がいないうちから、工業が建ち始めるための需要。
const BASE_INDUSTRIAL_DEMAND: i64 = 10;

/// 商業の職場 1 つが、何人ぶんの住民の買い物をまかなうか。
const CITIZENS_PER_SHOP_JOB: u64 = 4;

/// 商業を除く働き口のうち、工業の割合（‰）。残りがオフィスになる。
const INDUSTRIAL_SHARE_PERMILLE: u64 = 600;

/// 需要の式に使う、街の人口と働き口の数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CityStats {
    pub citizens: u64,
    pub households: u64,
    /// 働く年齢の住民の数。
    pub working_age: u64,
    /// 職場のある住民の数。
    pub employed: u64,
    /// 住宅に住める世帯の数の合計。
    pub homes: u64,
    /// 区画の種類ごとの職場の数。添え字は [`ZoneKind::index`]。
    pub workplaces: [u64; 4],
    /// 区画の種類ごとの、埋まっている職場の数。
    pub filled: [u64; 4],
    /// 区画の種類ごとの、塗られていて建てる空きがあるか、建物があるか。
    pub zoned: [bool; 4],
    /// 家のある住民の幸福度（[`crate::happiness::total`]）の平均。家のある住民がいなければ中立の値である。
    pub happiness: u8,
}

impl CityStats {
    /// 建物を 1 つ除いたときの値。
    pub fn without(mut self, b: &Building) -> CityStats {
        self.homes -= b.households();
        self.workplaces[b.zone.index()] -= b.workplaces();
        self
    }

    /// 建物を 1 つ足したときの値。
    pub fn with(mut self, b: &Building) -> CityStats {
        self.homes += b.households();
        self.workplaces[b.zone.index()] += b.workplaces();
        self
    }
}

/// 区画の種類ごとの需要。正なら建物が建つ。添え字は [`ZoneKind::index`]。
///
/// CS2 にならい、次のように決める。どれも差の式なので、街が釣り合えば 0 のまわりに収まる。
/// - 住宅：いつもある需要に、働き口の空きと幸福度の分（[`happiness_pull`]）を足し、空き家の 2 倍と
///   職のない住民の 1/4 を引く。
/// - 商業：住民の数の 1/[`CITIZENS_PER_SHOP_JOB`] が買い物に要る職場で、いまの商業の職場との差。
/// - 工業とオフィス：働く年齢の住民のうち、商業で働かない人に要る職場を、塗られている区画の間で分けたものと、
///   いまの職場との差。両方が塗られていれば決まった割合で、片方だけならその区画にすべてを割り当てる。
///   商業が塗られていなければ、買い物の職場の分も工業とオフィスに回す。
pub fn demand(s: &CityStats) -> [i64; 4] {
    let n = |v: u64| i64::try_from(v).expect("街の数があふれた");
    let zoned = |z: ZoneKind| s.zoned[z.index()];
    let jobs: u64 = s.workplaces.iter().sum();
    let filled: u64 = s.filled.iter().sum();
    let vacant_jobs = n(jobs.saturating_sub(filled));
    let vacant_homes = n(s.homes.saturating_sub(s.households));
    let jobless_quarter = n(s.working_age.saturating_sub(s.employed) / 4);
    let shop_jobs = s.citizens / CITIZENS_PER_SHOP_JOB;
    let shop_workers = if zoned(ZoneKind::Commercial) {
        shop_jobs
    } else {
        0
    };
    let other_jobs = s.working_age.saturating_sub(shop_workers);
    let share = match (zoned(ZoneKind::Industrial), zoned(ZoneKind::Office)) {
        (true, false) => 1_000,
        (false, true) => 0,
        _ => INDUSTRIAL_SHARE_PERMILLE,
    };
    let industry = other_jobs * share / 1_000;
    let office = other_jobs - industry;
    let w = |z: ZoneKind| n(s.workplaces[z.index()]);
    [
        BASE_RESIDENTIAL_DEMAND + vacant_jobs + happiness_pull(s.happiness)
            - 2 * vacant_homes
            - jobless_quarter,
        n(shop_jobs) - w(ZoneKind::Commercial),
        BASE_INDUSTRIAL_DEMAND + n(industry) - w(ZoneKind::Industrial),
        n(office) - w(ZoneKind::Office),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(zoned: [bool; 4]) -> CityStats {
        CityStats {
            citizens: 1_000,
            working_age: 600,
            zoned,
            happiness: 50,
            ..CityStats::default()
        }
    }

    #[test]
    fn workers_go_to_the_zones_that_are_painted() {
        let all = demand(&stats([true; 4]));
        assert_eq!(all[1..], [250, 10 + 210, 140]);
        // 工業だけなら、商業の分も含めて工業がすべて受ける。
        let industry = demand(&stats([true, false, true, false]));
        assert_eq!(industry[2..], [10 + 600, 0]);
        // オフィスだけでも同じである。
        let office = demand(&stats([true, true, false, true]));
        assert_eq!(office[2..], [10, 350]);
    }
}
