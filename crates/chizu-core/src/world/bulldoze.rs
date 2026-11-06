//! 取り壊しで何が消えるかを、状態を変えずに求める。

use std::collections::BTreeSet;

use super::World;
use crate::network::Line;
use crate::tile::{TilePos, TileRect};

/// 長方形を [`crate::command::Command::Bulldoze`] で取り壊したときに消えるもの。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BulldozePreview {
    /// 消える道路の区間（交差点から交差点まで）の直線。ID の順に並ぶ。
    pub segments: Vec<Line>,
    /// 消える道路のマスの数。ほかの区間が残って通るマスは数えない。
    pub road_tiles: u64,
    /// 消える施設の敷地。
    pub facilities: Vec<TileRect>,
    /// 消える建物の敷地。長方形にかかる建物と、面した道路をなくす建物である。
    pub buildings: Vec<TileRect>,
    /// 消える建物に住んでいて、家をなくす世帯の数。
    pub displaced_households: u64,
    /// 消える線路（ノードからノードまで）の直線。駅の中の線路も含み、ID の順に並ぶ。
    pub tracks: Vec<Line>,
    /// 消える線路のマスの数。駅の中を含み、ほかの線路が残って通るマスは数えない。
    pub rail_tiles: u64,
    /// 消える駅の敷地。
    pub stations: Vec<TileRect>,
}

impl BulldozePreview {
    /// 消えるものがないか。このときの取り壊しは `NothingToBulldoze` で断られる。
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
            && self.facilities.is_empty()
            && self.buildings.is_empty()
            && self.tracks.is_empty()
            && self.stations.is_empty()
    }
}

fn overlaps(a: TileRect, b: TileRect) -> bool {
    a.min.x <= b.max.x && b.min.x <= a.max.x && a.min.y <= b.max.y && b.min.y <= a.max.y
}

impl World {
    /// `area` を取り壊したときに消えるものを求める。状態は変えない。
    ///
    /// 数は [`crate::command::Command::Bulldoze`] を適用した前後の差と同じになる（テストで確かめる）。
    /// 消える建物は、長方形にかかる建物と、面した歩道つきの道路が消える区間だけだった建物である。
    /// 線路と駅は [`crate::rail::Rail::bulldoze_targets`] と同じ選び方で求める。
    pub fn bulldoze_preview(&self, area: TileRect) -> BulldozePreview {
        let targets = self.network().bulldoze_targets(area);
        let segments: Vec<Line> = targets
            .iter()
            .filter_map(|s| self.network().segment_line(*s))
            .collect();
        let tiles: BTreeSet<TilePos> = segments
            .iter()
            .flat_map(Line::tiles)
            .filter(|t| {
                self.network()
                    .segments_through(*t)
                    .iter()
                    .all(|s| targets.contains(s))
            })
            .collect();
        let facilities = self
            .facilities()
            .iter()
            .map(|f| f.area)
            .filter(|a| overlaps(*a, area))
            .collect();
        let mut buildings = Vec::new();
        let mut gone = BTreeSet::new();
        for (id, b) in self.buildings().iter() {
            let hit = overlaps(b.area, area)
                || (!targets.is_empty()
                    && self
                        .frontage_without(b.front, b.toward_road, &targets)
                        .is_none());
            if hit {
                buildings.push(b.area);
                gone.insert(id);
            }
        }
        let people = self.population();
        let displaced_households = people
            .household_ids()
            .filter(|h| {
                people
                    .home(*h)
                    .flatten()
                    .is_some_and(|home| gone.contains(&home))
            })
            .count() as u64;
        let rail = self.rail();
        let (stations, track_ids) = rail.bulldoze_targets(area);
        let tracks: Vec<Line> = track_ids
            .iter()
            .filter_map(|id| rail.track_line(*id))
            .collect();
        let rail_tiles: BTreeSet<TilePos> = tracks
            .iter()
            .flat_map(Line::tiles)
            .filter(|t| {
                rail.tracks_through(*t)
                    .iter()
                    .all(|id| track_ids.contains(id))
            })
            .collect();
        let stations = stations
            .iter()
            .filter_map(|id| rail.station(*id))
            .map(|s| s.area)
            .collect();
        BulldozePreview {
            segments,
            road_tiles: tiles.len() as u64,
            facilities,
            buildings,
            displaced_households,
            tracks,
            rail_tiles: rail_tiles.len() as u64,
            stations,
        }
    }
}
