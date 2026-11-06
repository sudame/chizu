//! 駅の徒歩圏の読み取り。画面の主題図と検査の欄、駅を置く前の円が、駅まで歩く範囲の中の世帯の数を知るために使う。
//!
//! 範囲の決め方は、手段を選ぶときの候補の探し方（[`crate::rail::choice`]）と同じである。家の正面のマスから、
//! 駅の出入口のマス（施設と同じ `facility_spot`）までの直線が [`STATION_WALK_RADIUS`] マス以内なら、その駅の徒歩圏に入る。
//! 状態は変えず、状態ハッシュにも入れない。

use std::collections::BTreeMap;

use super::World;
use crate::movement::facility_spot;
use crate::rail::StationId;
use crate::rail::choice::{STATION_WALK_RADIUS, in_walk_range};
use crate::tile::TilePos;

/// 駅の徒歩圏の世帯の数。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Catchment {
    /// 家のある世帯の数。
    pub households: u32,
    /// そのうち、車を置く枠のない（車を持てない）世帯の数。
    pub carless: u32,
    /// そのうち、ほかの駅の徒歩圏にも入っている世帯の数。この駅がなくても、歩いて乗れる駅が残る世帯である。
    pub shared: u32,
}

impl World {
    /// 駅 `id` の出入口のマス。道路に面していない駅は `None`。
    pub fn station_door(&self, id: StationId) -> Option<TilePos> {
        let station = self.rail.station(id)?;
        facility_spot(&self.network, &station.site()).map(|p| p.tile)
    }

    /// 出入口のマス `doors` のそれぞれの徒歩圏の世帯の数を、同じ並びで返す。
    /// 家のある世帯は、家の建物の正面のマスで数える。車を置く枠の有無は [`World::carless_households`] と同じ決まりである。
    pub fn catchments(&self, doors: &[TilePos]) -> Vec<Catchment> {
        let mut out = vec![Catchment::default(); doors.len()];
        if doors.is_empty() {
            return out;
        }
        // 出入口を STATION_WALK_RADIUS マス四方の区切りに分け、世帯ごとに隣り合う区切りだけを調べる
        let cell = |t: TilePos| {
            (
                t.x.div_euclid(STATION_WALK_RADIUS),
                t.y.div_euclid(STATION_WALK_RADIUS),
            )
        };
        let mut cells: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
        for (i, d) in doors.iter().enumerate() {
            cells.entry(cell(*d)).or_default().push(i);
        }
        let p = &self.population;
        for h in p.household_ids() {
            let Some(front) = p
                .home(h)
                .flatten()
                .and_then(|id| self.buildings.get(id))
                .map(|b| b.front)
            else {
                continue;
            };
            let carless = self.car_places.get(h).is_none();
            let (cx, cy) = cell(front);
            let mut hits: Vec<usize> = Vec::new();
            for x in cx - 1..=cx + 1 {
                for y in cy - 1..=cy + 1 {
                    for i in cells.get(&(x, y)).into_iter().flatten() {
                        if in_walk_range(front, doors[*i]) {
                            out[*i].households += 1;
                            out[*i].carless += u32::from(carless);
                            hits.push(*i);
                        }
                    }
                }
            }
            if hits.len() > 1 {
                for i in hits {
                    out[i].shared += 1;
                }
            }
        }
        out
    }
}
