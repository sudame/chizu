//! 職場の割り当て。
//!
//! 割り当て方は [`assign_jobs`] の 1 つに閉じ込める。職のない住民は ID の順に、家から出入口の
//! 直線距離が近い順に [`JOB_CANDIDATES`] 件の空きのある職場を候補にし、近いほど選ばれやすく 1 件を選ぶ。
//! 道のりは測らない。すでに勤めている住民の一部も、職場の更新のたびに、近い職場へ乗り換えるかを見直す。

use std::collections::BTreeMap;

use crate::building::{Building, Buildings};
use crate::citizens::{Activity, Population, WORKING_AGES};
use crate::id::{BuildingId, CitizenId};
use crate::network::TILE_MM;
use crate::rng::{Pcg32, RngPurpose, rng};
use crate::tile::TilePos;
use crate::units::Tick;

/// 職場を選ぶ候補の数。家から近い順にこの数だけ取り、近いほど重くする。
pub const JOB_CANDIDATES: usize = 24;

/// すでに勤めている住民が、職場の更新のたびに乗り換えを見直す割合（%）。
pub const RECONSIDER_PERCENT: u32 = 5;

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
/// 通勤の手段を決める距離（`movement::commute` の `far_enough`）と同じ式で測る。
pub fn commute_meters(home: &Building, work: &Building) -> u64 {
    let d2 = u128::from(tile_distance_sq(home.front, work.front));
    let tile_mm = u128::from(TILE_MM.unsigned_abs());
    let mm = d2.saturating_mul(tile_mm * tile_mm).isqrt();
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

/// 候補の 1 件。距離の 2 乗、職場の ID、格子の中の添え字。
type Candidate = (u64, BuildingId, usize);

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

    /// 1 つのセルの職場を、近い順の候補 `near`（長さ `k` まで）に入れる。
    fn scan(&self, cx: i64, cy: i64, from: TilePos, k: usize, near: &mut Vec<Candidate>) {
        let idx = ((cy - self.origin.1) * self.size.0 + (cx - self.origin.0)) as usize;
        for (id, at, _) in &self.cells[idx] {
            let key = (tile_distance_sq(from, *at), *id, idx);
            let pos = near.partition_point(|c| (c.0, c.1) < (key.0, key.1));
            if pos < k {
                near.insert(pos, key);
                near.truncate(k);
            }
        }
    }

    /// `from` に近い空きのある職場を、近い順（同じ距離なら ID の小さい順）に最大 `k` 件返す。
    fn nearest(&self, from: TilePos, k: usize) -> Vec<Candidate> {
        let mut near = Vec::with_capacity(k + 1);
        if self.open == 0 || k == 0 {
            return near;
        }
        let (hx, hy) = cell_of(from);
        let (x0, y0) = self.origin;
        let (x1, y1) = (x0 + self.size.0 - 1, y0 + self.size.1 - 1);
        let max_ring = (hx - x0)
            .abs()
            .max((hx - x1).abs())
            .max((hy - y0).abs())
            .max((hy - y1).abs());
        for r in 0..=max_ring {
            // r 番目の輪のセルは、家から (r - 1) * CELL マスより近くない。
            if near.len() == k {
                let far = near[k - 1].0;
                let lower = ((r - 1).max(0) * CELL) as u64;
                if far <= lower * lower {
                    break;
                }
            }
            for cy in (hy - r).max(y0)..=(hy + r).min(y1) {
                if (cy - hy).abs() == r {
                    for cx in (hx - r).max(x0)..=(hx + r).min(x1) {
                        self.scan(cx, cy, from, k, &mut near);
                    }
                } else {
                    for cx in [hx - r, hx + r] {
                        if (x0..=x1).contains(&cx) {
                            self.scan(cx, cy, from, k, &mut near);
                        }
                    }
                }
            }
        }
        near
    }

    /// 候補 `c` の職場の空きを 1 つ埋める。
    fn take(&mut self, c: Candidate) {
        let cell = &mut self.cells[c.2];
        let Some(pos) = cell.iter().position(|(b, _, _)| *b == c.1) else {
            return;
        };
        cell[pos].2 -= 1;
        if cell[pos].2 == 0 {
            cell.remove(pos);
            self.open -= 1;
        }
    }
}

/// 近い順の候補 `near` から、近いほど選ばれやすく 1 件を選ぶ。
/// 近い順に n、…、1 の重みを付ける（n は候補の数）。
fn pick(near: &[Candidate], r: &mut Pcg32) -> Option<Candidate> {
    let n = u32::try_from(near.len()).ok()?;
    if n == 0 {
        return None;
    }
    let mut roll = r.below(n.checked_mul(n.checked_add(1)?)? / 2);
    for (rank, c) in (0_u32..).zip(near) {
        let weight = n - rank;
        if roll < weight {
            return Some(*c);
        }
        roll -= weight;
    }
    None
}

/// 職場を変える住民と、その新しい職場を、住民の ID の順に返す。
///
/// 職場の上限は、区画の種類ごとの税率のもとで雇える数（[`crate::building::Building::workplaces_at`]）である。
///
/// - 消えた建物の職場や、上限を超えた職場は外す。上限を超えたときは、ID の大きい住民から外す。
/// - 職のない働く年齢の住民に、職場の空きを割り当てる。住民は ID の順に選び、家のない住民には割り当てない。
///   家から出入口の直線距離が近い順に [`JOB_CANDIDATES`] 件の空きを候補にし、近いほど重く、
///   種と住民の ID と `tick` から作った乱数で 1 件選ぶ。同じ距離なら建物の ID が小さいほうを近いとする。
/// - 家で出発を待っている住民のうち、ID と `tick` で決まる [`RECONSIDER_PERCENT`]% は、同じ選び方で候補を選び、
///   今の職場までの距離の半分以下なら乗り換える。外した職場の空きは、次の更新から使える。
pub fn assign_jobs(
    population: &Population,
    buildings: &Buildings,
    tax_permille: [u16; 4],
    seed: u64,
    tick: Tick,
) -> Vec<(CitizenId, Option<BuildingId>)> {
    let mut vacancy: BTreeMap<BuildingId, u64> = buildings
        .iter()
        .map(|(id, b)| (id, b.workplaces_at(tax_permille[b.zone.index()])))
        .filter(|(_, n)| *n > 0)
        .collect();
    let mut changes = BTreeMap::new();
    let mut seekers = Vec::new();
    let mut employed = Vec::new();
    for id in population.citizen_ids() {
        let row = population.citizen(id).expect("いる住民の行がない");
        match row.workplace {
            Some(w) => match vacancy.get_mut(&w) {
                Some(left) if *left > 0 => {
                    *left -= 1;
                    employed.push((id, w));
                }
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
    let home_of = |id: CitizenId| {
        let row = population.citizen(id)?;
        population
            .home(row.household)
            .flatten()
            .and_then(|h| buildings.get(h))
    };
    let r_of = |id: CitizenId| rng(seed, RngPurpose::JobAssignment, u64::from(id.index), tick);
    for id in seekers {
        let row = population.citizen(id).expect("いる住民の行がない");
        let Some(home) = home_of(id) else {
            continue;
        };
        if !WORKING_AGES.contains(&row.age) {
            continue;
        }
        let near = jobs.nearest(home.front, JOB_CANDIDATES);
        let Some(c) = pick(&near, &mut r_of(id)) else {
            break;
        };
        jobs.take(c);
        changes.insert(id, Some(c.1));
    }
    for (id, work) in employed {
        let row = population.citizen(id).expect("いる住民の行がない");
        // 通勤の途中や、職場や店にいる住民の予定は動かさない。
        if row.trip.is_some() || row.schedule.next != Activity::Work {
            continue;
        }
        let mut r = r_of(id);
        if r.below(100) >= RECONSIDER_PERCENT {
            continue;
        }
        let (Some(home), Some(now)) = (home_of(id), buildings.get(work)) else {
            continue;
        };
        let near = jobs.nearest(home.front, JOB_CANDIDATES);
        let Some(c) = pick(&near, &mut r) else {
            continue;
        };
        if c.0.saturating_mul(4) <= tile_distance_sq(home.front, now.front) {
            jobs.take(c);
            changes.insert(id, Some(c.1));
        }
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

    fn random_open(seed: u64) -> BTreeMap<BuildingId, (TilePos, u64)> {
        let mut state = seed;
        let mut next = |m: i64| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((state >> 33) as i64) % m
        };
        (0..60)
            .map(|i| {
                let at = TilePos::new(next(300) - 100, next(200) - 50);
                (id(i), (at, 1 + next(3) as u64))
            })
            .collect()
    }

    /// 格子で探した近い順の候補は、全部を見比べて並べた先頭と同じになる。空きが尽きれば短くなる。
    #[test]
    fn nearest_matches_brute_force() {
        let open = random_open(12345);
        let mut brute: Vec<(BuildingId, TilePos, u64)> =
            open.iter().map(|(i, (p, n))| (*i, *p, *n)).collect();
        let mut jobs = OpenJobs::new(&open);
        let mut state = 7u64;
        for _ in 0..200 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let from = TilePos::new(
                ((state >> 33) % 500) as i64 - 200,
                ((state >> 20) % 400) as i64 - 150,
            );
            let mut want: Vec<(u64, BuildingId)> = brute
                .iter()
                .filter(|(_, _, n)| *n > 0)
                .map(|(i, p, _)| (tile_distance_sq(from, *p), *i))
                .collect();
            want.sort();
            want.truncate(JOB_CANDIDATES);
            let got = jobs.nearest(from, JOB_CANDIDATES);
            assert_eq!(got.iter().map(|c| (c.0, c.1)).collect::<Vec<_>>(), want);
            if let Some(c) = got.first().copied() {
                jobs.take(c);
                brute.iter_mut().find(|(i, _, _)| *i == c.1).unwrap().2 -= 1;
            }
        }
        assert!(jobs.nearest(TilePos::new(0, 0), JOB_CANDIDATES).is_empty());
    }

    /// 同じ距離の職場は、建物の ID の小さい順に並ぶ。
    #[test]
    fn equal_distances_go_by_building_id() {
        let at = |x, y| (TilePos::new(x, y), 1);
        let open: BTreeMap<_, _> = [
            (id(5), at(10, 0)),
            (id(2), at(-10, 0)),
            (id(9), at(0, 10)),
            (id(1), at(0, -10)),
            (id(3), at(1, 0)),
        ]
        .into_iter()
        .collect();
        let jobs = OpenJobs::new(&open);
        let got: Vec<u32> = jobs
            .nearest(TilePos::new(0, 0), 5)
            .iter()
            .map(|c| c.1.index)
            .collect();
        assert_eq!(got, [3, 1, 2, 5, 9]);
        let got: Vec<u32> = jobs
            .nearest(TilePos::new(0, 0), 3)
            .iter()
            .map(|c| c.1.index)
            .collect();
        assert_eq!(got, [3, 1, 2]);
    }

    /// 近い候補ほど選ばれやすく、遠い候補も選ばれる。同じ乱数なら同じ結果になる。
    #[test]
    fn nearer_candidates_are_picked_more_often() {
        let near: Vec<Candidate> = (0..4).map(|i| (u64::from(i), id(i), 0)).collect();
        let mut count = [0u32; 4];
        for k in 0..4000 {
            let mut r = rng(1, RngPurpose::JobAssignment, k, Tick(0));
            let c = pick(&near, &mut r).unwrap();
            count[c.1.index as usize] += 1;
            let mut r2 = rng(1, RngPurpose::JobAssignment, k, Tick(0));
            assert_eq!(pick(&near, &mut r2).unwrap().1, c.1);
        }
        assert!(count.windows(2).all(|w| w[0] > w[1]), "{count:?}");
        assert!(count[3] > 0);
        assert!(pick(&[], &mut rng(1, RngPurpose::JobAssignment, 0, Tick(0))).is_none());
    }
}
