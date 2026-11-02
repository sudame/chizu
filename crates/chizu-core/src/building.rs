//! 区画の上に建つ建物の表。
//!
//! 建物は区画の上に、需要に応じて自然に建ち、時間がたつと段階が上がる。建つ・育つ処理は
//! `world::growth` にある。ここには表と、大きさと段階から決まる値だけを置く。

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::command::ZoneKind;
use crate::happiness::REFERENCE_TAX_PERMILLE;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

    /// 税率 `tax_permille` のもとで実際に働き手を雇える職場の数。基準の税率までは [`Building::workplaces`] のままで、
    /// それより高いと [`jobs_kept_permille`] の割合まで減る。端数は四捨五入する。
    pub fn workplaces_at(&self, tax_permille: u16) -> u64 {
        let kept = jobs_kept_permille(tax_permille);
        (self.workplaces() * kept + 500) / 1_000
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
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

    /// 区画の種類ごとの、税率のもとで雇える職場の数の合計。添え字は [`ZoneKind::index`]。
    pub fn job_slots(&self, tax_permille: [u16; 4]) -> [u64; 4] {
        let mut slots = [0; 4];
        for (_, b) in self.iter() {
            slots[b.zone.index()] += b.workplaces_at(tax_permille[b.zone.index()]);
        }
        slots
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

/// 税率が基準（[`REFERENCE_TAX_PERMILLE`]）を 1‰ 上回るごとに、雇える職場の割合を減らす値（‰）。
/// 高い税率では企業が人を雇えなくなり、職のない住民が増えて、幸福度と税収が下がる。
const JOBS_LOST_PERMILLE_PER_TAX_PERMILLE: u64 = 7;

/// 住宅の需要に、税率の基準との差を効かせる強さ。基準より 2‰ 高いごとに 1 引き、2‰ 低いごとに 1 足す。
const TAX_DEMAND_DIVISOR: i64 = 2;

/// 税率 `tax_permille` のもとで、職場のうち雇える割合（‰）。基準までは 1000 で、それより高いと下がり、0 で止まる。
pub fn jobs_kept_permille(tax_permille: u16) -> u64 {
    let over = u64::from(tax_permille).saturating_sub(REFERENCE_TAX_PERMILLE as u64);
    1_000u64.saturating_sub(over * JOBS_LOST_PERMILLE_PER_TAX_PERMILLE)
}

/// 住宅の需要のうち、住宅の税率から来る分。基準より高ければ負、低ければ正である。
pub fn residential_tax_pull(tax_permille: u16) -> i64 {
    crate::round::div_floor(
        REFERENCE_TAX_PERMILLE - i64::from(tax_permille),
        TAX_DEMAND_DIVISOR,
    )
}

/// 働き手がいないうちから、工業が建ち始めるための需要。
const BASE_INDUSTRIAL_DEMAND: i64 = 10;

/// 商業の職場 1 つが、何人ぶんの住民の買い物をまかなうか。
const CITIZENS_PER_SHOP_JOB: u64 = 4;

/// 商業を除く働き口のうち、工業の割合（‰）。残りがオフィスになる。
const INDUSTRIAL_SHARE_PERMILLE: u64 = 600;

/// 需要の式に使う、街の人口と働き口の数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// 区画の種類ごとの、税率のもとで雇える職場の数。税率が基準以下なら `workplaces` と同じである。
    pub job_slots: [u64; 4],
    /// 区画の種類ごとの税率（‰）。
    pub tax_permille: [u16; 4],
    /// 区画の種類ごとの、埋まっている職場の数。
    pub filled: [u64; 4],
    /// 区画の種類ごとの、塗られていて建てる空きがあるか、建物があるか。
    pub zoned: [bool; 4],
    /// 家のある住民の幸福度（[`crate::happiness::total`]）の平均。家のある住民がいなければ中立の値である。
    pub happiness: u8,
}

impl Default for CityStats {
    fn default() -> CityStats {
        CityStats {
            citizens: 0,
            households: 0,
            working_age: 0,
            employed: 0,
            homes: 0,
            workplaces: [0; 4],
            job_slots: [0; 4],
            tax_permille: [REFERENCE_TAX_PERMILLE as u16; 4],
            filled: [0; 4],
            zoned: [false; 4],
            happiness: 0,
        }
    }
}

impl CityStats {
    /// 建物を 1 つ除いたときの値。
    pub fn without(mut self, b: &Building) -> CityStats {
        let i = b.zone.index();
        self.homes -= b.households();
        self.workplaces[i] -= b.workplaces();
        self.job_slots[i] -= b.workplaces_at(self.tax_permille[i]);
        self
    }

    /// 建物を 1 つ足したときの値。
    pub fn with(mut self, b: &Building) -> CityStats {
        let i = b.zone.index();
        self.homes += b.households();
        self.workplaces[i] += b.workplaces();
        self.job_slots[i] += b.workplaces_at(self.tax_permille[i]);
        self
    }
}

/// 区画の種類ごとの需要。正なら建物が建つ。添え字は [`ZoneKind::index`]。
///
/// CS2 にならい、次のように決める。どれも差の式なので、街が釣り合えば 0 のまわりに収まる。
/// - 住宅：いつもある需要に、働き口の空きと幸福度の分（[`happiness_pull`]）、税率の分
///   （[`residential_tax_pull`]）を足し、空き家の 2 倍と職のない住民の 1/4 を引く。
/// - 商業：住民の数の 1/[`CITIZENS_PER_SHOP_JOB`] が買い物に要る職場で、いまの商業の職場との差。
/// - 商業・工業・オフィスは、必要な職場の数に税率の分（[`jobs_kept_permille`]）をかける。
/// - 工業とオフィス：働く年齢の住民のうち、商業で働かない人に要る職場を、塗られている区画の間で分けたものと、
///   いまの職場との差。両方が塗られていれば決まった割合で、片方だけならその区画にすべてを割り当てる。
///   商業が塗られていなければ、買い物の職場の分も工業とオフィスに回す。
pub fn demand(s: &CityStats) -> [i64; 4] {
    let n = |v: u64| i64::try_from(v).expect("街の数があふれた");
    let zoned = |z: ZoneKind| s.zoned[z.index()];
    let jobs: u64 = s.job_slots.iter().sum();
    let filled: u64 = s.filled.iter().sum();
    let vacant_jobs = n(jobs.saturating_sub(filled));
    // 税率が高い区画は、必要な職場の数が減る。企業が増やしたがらないためである。
    let kept = |z: ZoneKind| jobs_kept_permille(s.tax_permille[z.index()]);
    let scaled = |v: u64, z: ZoneKind| n(v * kept(z) / 1_000);
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
        BASE_RESIDENTIAL_DEMAND
            + vacant_jobs
            + happiness_pull(s.happiness)
            + residential_tax_pull(s.tax_permille[ZoneKind::Residential.index()])
            - 2 * vacant_homes
            - jobless_quarter,
        scaled(shop_jobs, ZoneKind::Commercial) - w(ZoneKind::Commercial),
        scaled(
            BASE_INDUSTRIAL_DEMAND as u64 + industry,
            ZoneKind::Industrial,
        ) - w(ZoneKind::Industrial),
        scaled(office, ZoneKind::Office) - w(ZoneKind::Office),
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
    fn a_high_tax_cuts_jobs_and_demand() {
        assert_eq!(jobs_kept_permille(0), 1_000);
        assert_eq!(jobs_kept_permille(100), 1_000);
        assert_eq!(jobs_kept_permille(150), 650);
        assert_eq!(jobs_kept_permille(300), 0);
        assert_eq!(residential_tax_pull(100), 0);
        assert_eq!(residential_tax_pull(150), -25);
        assert_eq!(residential_tax_pull(80), 10);
        let mut taxed = stats([true; 4]);
        taxed.tax_permille = [150; 4];
        let base = demand(&stats([true; 4]));
        let high = demand(&taxed);
        // 住宅は税率の分だけ下がる。職場は、必要な数が 65% に減る。
        assert_eq!(high[0], base[0] - 25);
        assert_eq!(
            high[1..],
            [250 * 65 / 100, (10 + 210) * 65 / 100, 140 * 65 / 100]
        );
        // 基準以下の税率では、職場の需要は変わらない。
        taxed.tax_permille = [50; 4];
        assert_eq!(demand(&taxed)[1..], base[1..]);
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
