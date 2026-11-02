//! 職場の割り当て。
//!
//! 割り当て方は [`assign_jobs`] の 1 つに閉じ込める。職のない住民は ID の順に、家から出入口の
//! 直線距離が最も近い、空きのある職場を選ぶ。道のりは測らない。

use std::collections::BTreeMap;

use crate::building::{Building, Buildings};
use crate::citizens::{Population, WORKING_AGES};
use crate::id::{BuildingId, CitizenId};
use crate::network::TILE_MM;
use crate::tile::TilePos;

/// 建物ごとの、そこで働く住民の数。消えた建物を指す住民は数えない。
pub fn workers_by_building(
    population: &Population,
    buildings: &Buildings,
) -> BTreeMap<BuildingId, u64> {
    let mut out = BTreeMap::new();
    for id in population.citizen_ids() {
        if let Some(w) = population.citizen(id).and_then(|r| r.workplace)
            && buildings.get(w).is_some()
        {
            *out.entry(w).or_insert(0) += 1;
        }
    }
    out
}

/// 空きを探す格子の 1 辺のマス数。
const CELL: i64 = 16;

/// 家から職場までの出入口の直線距離（m）。小数点以下は切り捨てる。
pub fn commute_meters(home: &Building, work: &Building) -> u64 {
    let d2 = tile_distance_sq(home.front, work.front);
    let tiles = u128::from(d2).isqrt();
    let mm = tiles * u128::from(TILE_MM.unsigned_abs());
    u64::try_from(mm / 1_000).unwrap_or(u64::MAX)
}

/// 2 つのマスの距離の 2 乗（マス単位）。
fn tile_distance_sq(a: TilePos, b: TilePos) -> u64 {
    let dx = a.x.abs_diff(b.x);
    let dy = a.y.abs_diff(b.y);
    dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
}

fn cell_of(p: TilePos) -> (i64, i64) {
    (p.x.div_euclid(CELL), p.y.div_euclid(CELL))
}

/// 空きのある職場を、格子に分けて持つ。近い順に探すために使う。
struct OpenJobs {
    /// 格子の左上の角（セル座標）。
    origin: (i64, i64),
    /// 格子の幅と高さ（セルの数）。
    size: (i64, i64),
    /// セルごとの、職場の ID、出入口のマス、残りの空き。
    cells: Vec<Vec<(BuildingId, TilePos, u64)>>,
    /// 空きのある職場の数。
    open: usize,
}

impl OpenJobs {
    fn new(open: &BTreeMap<BuildingId, (TilePos, u64)>) -> OpenJobs {
        if open.is_empty() {
            return OpenJobs {
                origin: (0, 0),
                size: (0, 0),
                cells: Vec::new(),
                open: 0,
            };
        }
        let mut lo = (i64::MAX, i64::MAX);
        let mut hi = (i64::MIN, i64::MIN);
        for (at, _) in open.values() {
            let c = cell_of(*at);
            lo = (lo.0.min(c.0), lo.1.min(c.1));
            hi = (hi.0.max(c.0), hi.1.max(c.1));
        }
        let size = (hi.0 - lo.0 + 1, hi.1 - lo.1 + 1);
        let mut cells = vec![Vec::new(); (size.0 * size.1) as usize];
        for (id, (at, left)) in open {
            let c = cell_of(*at);
            cells[((c.1 - lo.1) * size.0 + (c.0 - lo.0)) as usize].push((*id, *at, *left));
        }
        OpenJobs {
            origin: lo,
            size,
            cells,
            open: open.len(),
        }
    }

    /// 1 つのセルを見て、`best` より近い職場があれば置き換える。
    fn scan(&self, cx: i64, cy: i64, from: TilePos, best: &mut Option<(u64, BuildingId, usize)>) {
        let idx = ((cy - self.origin.1) * self.size.0 + (cx - self.origin.0)) as usize;
        for (id, at, _) in &self.cells[idx] {
            let key = (tile_distance_sq(from, *at), *id);
            if best.is_none_or(|(d, b, _)| key < (d, b)) {
                *best = Some((key.0, key.1, idx));
            }
        }
    }

    /// `from` に最も近い空きのある職場（同じ距離なら ID の小さいほう）を 1 人ぶん埋めて返す。
    fn take_nearest(&mut self, from: TilePos) -> Option<BuildingId> {
        if self.open == 0 {
            return None;
        }
        let (hx, hy) = cell_of(from);
        let (x0, y0) = self.origin;
        let (x1, y1) = (x0 + self.size.0 - 1, y0 + self.size.1 - 1);
        let max_ring = (hx - x0)
            .abs()
            .max((hx - x1).abs())
            .max((hy - y0).abs())
            .max((hy - y1).abs());
        let mut best: Option<(u64, BuildingId, usize)> = None;
        for r in 0..=max_ring {
            // r 番目の輪のセルは、家から (r - 1) * CELL マスより近くない。
            if let Some((d, _, _)) = best {
                let near = ((r - 1).max(0) * CELL) as u64;
                if d <= near * near {
                    break;
                }
            }
            for cy in (hy - r).max(y0)..=(hy + r).min(y1) {
                if (cy - hy).abs() == r {
                    for cx in (hx - r).max(x0)..=(hx + r).min(x1) {
                        self.scan(cx, cy, from, &mut best);
                    }
                } else {
                    for cx in [hx - r, hx + r] {
                        if (x0..=x1).contains(&cx) {
                            self.scan(cx, cy, from, &mut best);
                        }
                    }
                }
            }
        }
        let (_, id, idx) = best?;
        let cell = &mut self.cells[idx];
        let pos = cell.iter().position(|(b, _, _)| *b == id)?;
        cell[pos].2 -= 1;
        if cell[pos].2 == 0 {
            cell.remove(pos);
            self.open -= 1;
        }
        Some(id)
    }
}

/// 職場を変える住民と、その新しい職場を、住民の ID の順に返す。
///
/// 職場の上限は、区画の種類ごとの税率のもとで雇える数（[`crate::building::Building::workplaces_at`]）である。
///
/// - 消えた建物の職場や、上限を超えた職場は外す。上限を超えたときは、ID の大きい住民から外す。
/// - 職のない働く年齢の住民に、家から近い順に職場の空きを割り当てる。住民は ID の順に選び、
///   家のない住民には割り当てない。近さは出入口のマスの直線距離で測り、同じ距離なら建物の ID が小さいほうにする。
pub fn assign_jobs(
    population: &Population,
    buildings: &Buildings,
    tax_permille: [u16; 4],
) -> Vec<(CitizenId, Option<BuildingId>)> {
    let mut vacancy: BTreeMap<BuildingId, u64> = buildings
        .iter()
        .map(|(id, b)| (id, b.workplaces_at(tax_permille[b.zone.index()])))
        .filter(|(_, n)| *n > 0)
        .collect();
    let mut changes = BTreeMap::new();
    let mut seekers = Vec::new();
    for id in population.citizen_ids() {
        let row = population.citizen(id).expect("いる住民の行がない");
        match row.workplace {
            Some(w) => match vacancy.get_mut(&w) {
                Some(left) if *left > 0 => *left -= 1,
                _ => {
                    changes.insert(id, None);
                    seekers.push(id);
                }
            },
            None => seekers.push(id),
        }
    }
    let open: BTreeMap<BuildingId, (TilePos, u64)> = vacancy
        .into_iter()
        .filter(|(_, left)| *left > 0)
        .map(|(id, left)| (id, (buildings.get(id).expect("いる建物").front, left)))
        .collect();
    let mut jobs = OpenJobs::new(&open);
    for id in seekers {
        let row = population.citizen(id).expect("いる住民の行がない");
        let home = population
            .home(row.household)
            .flatten()
            .and_then(|h| buildings.get(h));
        let Some(home) = home else {
            continue;
        };
        if !WORKING_AGES.contains(&row.age) {
            continue;
        }
        let Some(w) = jobs.take_nearest(home.front) else {
            break;
        };
        changes.insert(id, Some(w));
    }
    changes.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> BuildingId {
        BuildingId {
            index,
            generation: 0,
        }
    }

    /// 格子で探した結果は、全部を見比べて選んだ結果と同じになる。空きが尽きたら `None` を返す。
    #[test]
    fn nearest_matches_brute_force() {
        let mut state = 12345u64;
        let mut next = |m: i64| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as i64) % m
        };
        let mut open = BTreeMap::new();
        for i in 0..60 {
            open.insert(
                id(i),
                (
                    TilePos::new(next(300) - 100, next(200) - 50),
                    1 + next(3) as u64,
                ),
            );
        }
        let mut brute: Vec<(BuildingId, TilePos, u64)> =
            open.iter().map(|(i, (p, n))| (*i, *p, *n)).collect();
        let mut jobs = OpenJobs::new(&open);
        for _ in 0..200 {
            let from = TilePos::new(next(500) - 200, next(400) - 150);
            let want = brute
                .iter()
                .filter(|(_, _, n)| *n > 0)
                .min_by_key(|(i, p, _)| (tile_distance_sq(from, *p), *i))
                .map(|(i, _, _)| *i);
            assert_eq!(jobs.take_nearest(from), want);
            if let Some(w) = want {
                brute.iter_mut().find(|(i, _, _)| *i == w).unwrap().2 -= 1;
            }
        }
        assert_eq!(jobs.take_nearest(TilePos::new(0, 0)), None);
    }
}
