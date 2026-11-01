//! 職場の割り当て。
//!
//! 割り当て方は [`assign_jobs`] の 1 つに閉じ込める。いまは近さを使わず、住民も職場も ID の順に
//! 埋めていく。経路探索を使って近い職場を選ぶようにするときは、ここを差し替える。

use std::collections::BTreeMap;

use crate::building::Buildings;
use crate::citizens::{Population, WORKING_AGES};
use crate::id::{BuildingId, CitizenId};

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

/// 職場を変える住民と、その新しい職場を、住民の ID の順に返す。
///
/// - 消えた建物の職場や、上限を超えた職場は外す。上限を超えたときは、ID の大きい住民から外す。
/// - 職のない働く年齢の住民に、職場の空きを ID の順に割り当てる。家のない住民には割り当てない。
pub fn assign_jobs(
    population: &Population,
    buildings: &Buildings,
) -> Vec<(CitizenId, Option<BuildingId>)> {
    let mut vacancy: BTreeMap<BuildingId, u64> = buildings
        .iter()
        .filter(|(_, b)| b.workplaces() > 0)
        .map(|(id, b)| (id, b.workplaces()))
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
    let mut open = vacancy.iter_mut().filter(|(_, left)| **left > 0);
    let mut current = open.next();
    for id in seekers {
        let row = population.citizen(id).expect("いる住民の行がない");
        let housed = population.home(row.household).flatten().is_some();
        if !WORKING_AGES.contains(&row.age) || !housed {
            continue;
        }
        while let Some((_, left)) = &current
            && **left == 0
        {
            current = open.next();
        }
        let Some((w, left)) = current.as_mut() else {
            break;
        };
        **left -= 1;
        changes.insert(id, Some(**w));
    }
    changes.into_iter().collect()
}
