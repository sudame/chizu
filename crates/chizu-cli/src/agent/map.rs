//! `agent map`：範囲の地図を、1 マス 1 文字のマス目のテキストにする。
//!
//! 道路や区画をどこに置くかを、数値の observe だけでは決めにくいので、地図を文字で見せる。
//! 書き込みはせず、中核の公開の読み出し口だけを使う。

use chizu_core::chunk::Layer;
use chizu_core::movement::{lane_tiles, lot_tiles};
use chizu_core::network::{NodeControl, SegmentKind};
use chizu_core::rail::choice::{STATION_WALK_RADIUS, in_walk_range};
use chizu_core::tile::local_index;
use chizu_core::{
    CHUNK_SIDE, Congestion, ServiceKind, TilePos, TileRect, World, Zone, ZoneKind, is_water,
};

/// 1 回の出力に載せる文字（セル）の数の上限。始めに買ってある 192×192 マスが、縮尺 2 でちょうど収まる。
/// 行頭の座標などを足しても 1 万文字ほどになる。
pub const MAX_CELLS: u64 = 96 * 96;

/// 1 つのセルの水辺と買った土地を、何点で調べるか（1 辺あたり）。縮尺が大きくても、1 セルの計算を一定に抑える。
const SAMPLES_PER_SIDE: i64 = 8;

/// 高速道路を表す、道路の種類の番号。通常の道路の `RoadKind` は 1〜3 である。
const HIGHWAY: u8 = 4;

pub const LEGEND: &str = "\
凡例（重なったときは上のものを出す）
  P W L G  公共施設（P 発電所、W 浄水場、L 平面の共同の駐車場、G 駐車場ビル）
  E        駅（駅の中の線路のマスも E にする）
  F        踏切（道路と線路が交わるマス。交差点と道路より先に出す）
  S s      信号のある交差点（s は横断歩道を外した腕がある。--layer traffic では混み具合の文字を先に出す）
  Z Y      信号のない交差点。Z は高速道路でないどの腕にも横断歩道があり、Y は横断歩道を外した腕がある
           （--layer traffic では混み具合の文字を先に出す）
  H 1 2 3  道路（H 高速道路、1 2 3 は Small Medium Large）
  T        線路（複線。駅の外のマス）
  =        私道の通るマス（私道の敷地の住宅の、端の列。私道の脇の枠もこの列にある。駐車場と建物より先に出す）
  p        建物の駐車場に切り欠いたマス（建物より先に出す。前面の帯のように 1 マスの一部だけの切り欠きも p にする）
  * ^ %    電気か水が届いていない建物（* 電気だけ、^ 水だけ、% 両方。R D C I O の代わりに出す）
  R D C I O  建物（住宅（高密度）、住宅（低密度）、商業、工業、オフィスの区画に建っている。電気も水も届いている）
  r d c i o  建物のない区画（同じ並び）
  ~        水辺（海か川）
  ,        買っていない土地
  .        買った土地の空きマス（標高はいまどこも 0 の平らな地図なので出さない）";

/// 凡例の「P W L G」の行の代わりに出す、駐車場の層の凡例。
const PARKING_FACILITY_LEGEND_LINE: &str =
    "  P W      公共施設（P 発電所、W 浄水場）。共同の駐車場は埋まり具合の文字で出す";

/// 駐車場の層で、凡例の先頭に足す行。
const PARKING_LEGEND_LINES: &str = "\
  ! 車を持てない世帯のいる住宅（R と D の代わりに出す。電気か水が届いていない印より先に出す）
  - + # X  共同の駐車場の埋まり具合（- 借りられた枠と外来の車が台数の半分に満たない、+ 半分以上、# 8 割以上、X 満車。L G の代わりに出す）";

/// 凡例の「H 1 2 3」の行の代わりに出す、交通の層の道路の凡例。
const TRAFFIC_LEGEND_LINE: &str = "  - + # X  道路の混み具合（- 空いている、+ やや混んでいる、# 混んでいる、X 詰まっている。H 高速道路は混み具合を数えない）";

/// 乗りきれず人が残った駅とみなす、待っている人の数。画面の「公共交通」の主題図（`web/src/map/transit.ts` の
/// `WAITING_ALERT`）と同じである。
const CROWDED_WAITING: usize = 100;

/// 路線の色の番号 0〜7 に当てる文字。道路、施設、建物、区画の文字と重ならない文字を選んだ。
const LINE_GLYPHS: [char; 8] = ['A', 'B', 'J', 'K', 'M', 'N', 'Q', 'U'];

/// 路線の色の呼び名。画面の `LINE_COLOR_NAMES` と同じ並びである。
const LINE_COLOR_NAMES: [&str; 8] = ["青", "緑", "橙", "紫", "桃", "水", "茶", "黄緑"];

/// 凡例の「T」の行の代わりに出す、鉄道の層の線路と駅の凡例。
const RAIL_LEGEND_LINES: &str = "  A B J K M N Q U  路線の通る線路のマス（路線の色の番号 0〜7 の順。青 緑 橙 紫 桃 水 茶 黄緑。同じ線路を通る路線は、止まっていない路線の若い番号を出す）
  x        止まっている路線の線路のマス（線路がつながっていない）
  T        どの路線も通らない線路のマス
  !        乗りきれず人が残った駅か、待っている人が 100 人以上の駅（E の代わりに出す）
  :        駅の出入口から駅まで歩く範囲（64 マス）の中の、ほかに何も載らない空きマス";

/// 地図に重ねる層。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MapLayer {
    /// 道路を種類で出す。
    #[default]
    Terrain,
    /// 道路を混み具合の段階で出す。
    Traffic,
    /// 車を持てない世帯のいる住宅と、共同の駐車場の埋まり具合を出す。
    Parking,
    /// 線路を路線の色の番号で出し、駅まで歩く範囲と、乗りきれず人が残った駅を出す。
    Rail,
}

impl MapLayer {
    pub fn parse(name: &str) -> Result<MapLayer, String> {
        match name {
            "traffic" => Ok(MapLayer::Traffic),
            "parking" => Ok(MapLayer::Parking),
            "rail" => Ok(MapLayer::Rail),
            _ => Err(format!(
                "--layer {name:?} は知らない層。traffic、parking、rail のどれかを使う"
            )),
        }
    }
}

fn congestion_glyph(level: Congestion) -> char {
    match level {
        Congestion::Free => '-',
        Congestion::Busy => '+',
        Congestion::Heavy => '#',
        Congestion::Jammed => 'X',
    }
}

/// 共同の駐車場の埋まり具合の段階（0〜3）の文字。
fn fill_glyph(level: u8) -> char {
    match level {
        0 => '-',
        1 => '+',
        2 => '#',
        _ => 'X',
    }
}

/// セルにまとめたマスの数え上げ。
#[derive(Clone, Default)]
struct Cell {
    /// 通る区間の混み具合の最大。交通の層でだけ入れる。
    congestion: Option<Congestion>,
    facility: [u32; ServiceKind::ALL.len()],
    /// 駅のマスの数。
    station: u32,
    /// 線路が通るマスの数（駅の中を除く）。
    rail: u32,
    /// 踏切のマスの数。
    crossing: u32,
    /// 線路のマスを通る路線のうち、止まっていない路線の若い番号（なければ止まっている路線の若い番号）と、止まっているか。
    /// 鉄道の層でだけ入れる。
    line: Option<(bool, u32)>,
    /// 乗りきれず人が残った駅があるか。鉄道の層でだけ入れる。
    crowded: bool,
    /// セルの真ん中のマスが、駅の徒歩圏にあるか。鉄道の層でだけ入れる。
    walk: bool,
    /// 共同の駐車場の埋まり具合の段階（0〜3）の最大。駐車場の層でだけ入れる。
    lot_fill: Option<u8>,
    /// 車を持てない世帯のいる住宅のマスの数。駐車場の層でだけ入れる。
    carless: u32,
    /// 信号のある交差点があるか。
    signal: bool,
    /// 横断歩道のある交差点（3 本以上の区間が集まるノード）があるか。
    crosswalk: bool,
    /// 横断歩道を外した腕のある、信号のある交差点があるか。
    signal_removed: bool,
    /// 横断歩道を外した腕のある、信号のない交差点があるか。
    removed: bool,
    /// 道路の最大の種類。0 は道路なし。
    road: u8,
    building: [u32; Zone::ALL.len()],
    /// 駐車場に切り欠いたマスの数（1 マスの一部だけの帯も 1 と数える）。
    lot: u32,
    /// 私道の通るマスの数。
    lane: u32,
    /// 電気が届いていない建物のマスの数。
    unpowered: u32,
    /// 水が届いていない建物のマスの数。
    dry: u32,
    zone: [u32; Zone::ALL.len()],
    /// 水辺と買った土地は、全部のマスではなく見本のマスで数える。
    samples: u32,
    wet: u32,
    owned: u32,
}

fn argmax<const N: usize>(counts: &[u32; N]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, c) in counts.iter().enumerate() {
        if *c > 0 && best.is_none_or(|b| *c > counts[b]) {
            best = Some(i);
        }
    }
    best
}

fn zone_letter(z: Zone) -> char {
    match z {
        Zone::Residential => 'R',
        Zone::ResidentialLow => 'D',
        Zone::Commercial => 'C',
        Zone::Industrial => 'I',
        Zone::Office => 'O',
    }
}

/// [`Zone::ALL`] の中での位置。
fn zone_slot(z: Zone) -> usize {
    usize::from(z as u8 - 1)
}

fn facility_letter(k: ServiceKind) -> char {
    match k {
        ServiceKind::PowerPlant => 'P',
        ServiceKind::WaterPump => 'W',
        ServiceKind::ParkingLot => 'L',
        ServiceKind::ParkingGarage => 'G',
        ServiceKind::Station => 'E',
    }
}

impl Cell {
    /// 載せる文字。公共施設、駅、踏切、信号、横断歩道のある交差点（この 2 つは交通の層では道路のあと）、道路、線路、私道、駐車場、建物、区画、水辺、買っていない土地、空きマスの順に選び、
    /// 同じ種類の中では数の多いものを選ぶ（道路は種類の大きいもの）。
    fn glyph(&self) -> char {
        if let Some(i) = argmax(&self.facility) {
            let kind = ServiceKind::ALL[i];
            return match self.lot_fill {
                Some(level) if kind.is_parking() => fill_glyph(level),
                _ => facility_letter(kind),
            };
        }
        if self.station > 0 {
            return if self.crowded {
                '!'
            } else {
                facility_letter(ServiceKind::Station)
            };
        }
        if self.crossing > 0 {
            return 'F';
        }
        // 交通の層では、道路の混み具合を信号より先に出す。
        if self.signal && self.congestion.is_none() {
            return if self.signal_removed { 's' } else { 'S' };
        }
        if self.crosswalk && self.congestion.is_none() {
            return if self.removed { 'Y' } else { 'Z' };
        }
        match self.road {
            0 => {}
            HIGHWAY => return 'H',
            k => {
                return self
                    .congestion
                    .map_or(char::from(b'0' + k), congestion_glyph);
            }
        }
        if self.rail > 0 {
            return match self.line {
                Some((true, _)) => 'x',
                Some((false, i)) => LINE_GLYPHS[(i % 8) as usize],
                None => 'T',
            };
        }
        // 私道と駐車場に切り欠いたマスは、建物より先に出す。セルの中で建物のマスより少なければ、建物を出す。
        if self.lane > 0 && self.lane + self.lot >= self.building.iter().sum::<u32>() {
            return '=';
        }
        if self.lot > 0 && self.lot >= self.building.iter().sum::<u32>() {
            return 'p';
        }
        if let Some(i) = argmax(&self.building) {
            // 電気か水が届いていない建物は、区画の種類より先にそれを出す。
            if Zone::ALL[i].kind() == ZoneKind::Residential && self.carless > 0 {
                return '!';
            }
            return match (self.unpowered > 0, self.dry > 0) {
                (true, true) => '%',
                (true, false) => '*',
                (false, true) => '^',
                (false, false) => zone_letter(Zone::ALL[i]),
            };
        }
        if let Some(i) = argmax(&self.zone) {
            return zone_letter(Zone::ALL[i]).to_ascii_lowercase();
        }
        if self.wet * 2 >= self.samples {
            '~'
        } else if self.owned * 2 < self.samples {
            ','
        } else if self.walk {
            ':'
        } else {
            '.'
        }
    }
}

/// 台数 `spaces` の共同の駐車場の、借りられた枠と外来の車の和 `taken` の段階（0〜3）。
/// 画面の「駐車場」のレイヤーの段階と同じ割り方である。
fn fill_level(spaces: u32, taken: u32) -> u8 {
    if taken >= spaces {
        3
    } else if u64::from(taken) * 10 >= u64::from(spaces) * 8 {
        2
    } else {
        u8::from(u64::from(taken) * 2 >= u64::from(spaces))
    }
}

/// 範囲を `scale`×`scale` マスずつのセルに分けた格子。
struct Grid {
    rect: TileRect,
    scale: i64,
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    /// 凡例のあとに足す行。鉄道の層の路線ごとの数である。
    notes: Vec<String>,
}

impl Grid {
    fn cell_mut(&mut self, t: TilePos) -> Option<&mut Cell> {
        let r = self.rect;
        if t.x < r.min.x || t.x > r.max.x || t.y < r.min.y || t.y > r.max.y {
            return None;
        }
        let col = ((t.x - r.min.x) / self.scale) as usize;
        let row = ((t.y - r.min.y) / self.scale) as usize;
        Some(&mut self.cells[row * self.cols + col])
    }

    /// 長方形のうち、範囲に入るマスをすべて渡す。
    fn each_tile_in(&mut self, area: TileRect, mut f: impl FnMut(&mut Cell, TilePos)) {
        let (x0, x1) = (
            area.min.x.max(self.rect.min.x),
            area.max.x.min(self.rect.max.x),
        );
        let (y0, y1) = (
            area.min.y.max(self.rect.min.y),
            area.max.y.min(self.rect.max.y),
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                let t = TilePos::new(x, y);
                if let Some(cell) = self.cell_mut(t) {
                    f(cell, t);
                }
            }
        }
    }
}

/// 範囲がセルの上限に収まる最小の縮尺。
fn min_scale(width: u64, height: u64) -> u64 {
    let fits = |n: u64| {
        u128::from(width.div_ceil(n)) * u128::from(height.div_ceil(n)) <= u128::from(MAX_CELLS)
    };
    // 縮尺が大きいほどセルは減るので、二分探索で求める。最大の辺の長さなら 1 セルに収まる。
    let (mut lo, mut hi) = (1, width.max(height));
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// 見本のマスの位置。セルの幅 `len` マスを `SAMPLES_PER_SIDE` 点までに間引く。
fn sample_offsets(len: i64) -> impl Iterator<Item = i64> {
    let n = len.min(SAMPLES_PER_SIDE);
    (0..n).map(move |k| k * len / n)
}

/// 範囲 `rect` の地図を、縮尺 `scale` で描く。
pub fn render(
    world: &World,
    rect: TileRect,
    scale: u64,
    layer: MapLayer,
) -> Result<String, String> {
    if scale == 0 {
        return Err("--scale は 1 以上にする".into());
    }
    if !rect.in_world() {
        return Err("範囲が世界の外にはみ出している".into());
    }
    let width = rect.max.x.abs_diff(rect.min.x) + 1;
    let height = rect.max.y.abs_diff(rect.min.y) + 1;
    if scale > width.max(height) {
        return Err(format!(
            "--scale {scale} は範囲の長い辺 {} マスより大きい。1 以上、{} 以下にする",
            width.max(height),
            width.max(height)
        ));
    }
    let (cols, rows) = (width.div_ceil(scale), height.div_ceil(scale));
    // 世界のほぼ全体を指しても掛け算があふれないよう、広い型で数える。
    let cells = u128::from(cols) * u128::from(rows);
    if cells > u128::from(MAX_CELLS) {
        return Err(format!(
            "範囲 {width}×{height} マスを縮尺 {scale} で描くと {cols}×{rows} = {} 文字になり、上限の {MAX_CELLS} 文字を超える。\
             --scale {} 以上にするか、範囲をせまくする",
            cells,
            min_scale(width, height)
        ));
    }
    let mut grid = Grid {
        rect,
        scale: i64::try_from(scale).map_err(|e| format!("--scale {scale}: {e}"))?,
        cols: cols as usize,
        rows: rows as usize,
        cells: vec![Cell::default(); (cols * rows) as usize],
        notes: Vec::new(),
    };

    // 水辺と買った土地は、セルごとに見本のマスで調べる。
    let seed = world.seed();
    for row in 0..grid.rows {
        for col in 0..grid.cols {
            let (x0, y0) = (
                rect.min.x + col as i64 * grid.scale,
                rect.min.y + row as i64 * grid.scale,
            );
            let w = grid.scale.min(rect.max.x - x0 + 1);
            let h = grid.scale.min(rect.max.y - y0 + 1);
            let cell = &mut grid.cells[row * grid.cols + col];
            for dy in sample_offsets(h) {
                for dx in sample_offsets(w) {
                    let t = TilePos::new(x0 + dx, y0 + dy);
                    cell.samples += 1;
                    cell.wet += u32::from(is_water(seed, t));
                    cell.owned += u32::from(t.chunk().is_some_and(|c| world.is_owned(c)));
                }
            }
        }
    }

    // 区画は、メモリにあるチャンクだけが持っている。
    for chunk in world.chunks() {
        let Some(tiles) = chunk.layer(Layer::Ground) else {
            continue;
        };
        let origin = chunk.coord().origin();
        let last = CHUNK_SIDE as i64 - 1;
        let area = TileRect {
            min: origin,
            max: TilePos::new(origin.x + last, origin.y + last),
        };
        grid.each_tile_in(area, |cell, t| {
            let (lx, ly) = t.local();
            if let Some(z) = tiles.zone[local_index(lx, ly)] {
                cell.zone[zone_slot(z)] += 1;
            }
        });
    }
    for (_, b) in world.buildings().iter() {
        grid.each_tile_in(b.area, |cell, _| {
            cell.building[zone_slot(b.zone)] += 1;
            cell.unpowered += u32::from(!b.supply.power);
            cell.dry += u32::from(!b.supply.water);
        });
    }
    for (_, b) in world.buildings().iter() {
        for t in lot_tiles(world.network(), b, world.seed()) {
            if let Some(cell) = grid.cell_mut(t) {
                cell.lot += 1;
            }
        }
        for t in lane_tiles(world.network(), b) {
            if let Some(cell) = grid.cell_mut(t) {
                cell.lane += 1;
            }
        }
    }
    for f in world.facilities() {
        grid.each_tile_in(f.area, |cell, _| cell.facility[f.kind.index()] += 1);
    }
    let rail = world.rail();
    for (_, s) in rail.stations() {
        grid.each_tile_in(s.area, |cell, _| cell.station += 1);
    }
    for (id, track) in rail.tracks() {
        if track.station.is_some() {
            continue;
        }
        for t in rail
            .track_line(id)
            .into_iter()
            .flat_map(|l| l.tiles().collect::<Vec<_>>())
        {
            // 駅の端のノードのマスは駅の中なので、駅の外の線路にも数えない。
            if rail.station_at(t).is_none()
                && let Some(cell) = grid.cell_mut(t)
            {
                cell.rail += 1;
            }
        }
    }
    for t in rail.crossings() {
        if let Some(cell) = grid.cell_mut(t) {
            cell.crossing += 1;
        }
    }
    if layer == MapLayer::Rail {
        mark_rail_layer(world, &mut grid);
    }
    if layer == MapLayer::Parking {
        // 画面の「駐車場」のレイヤー（chizu-wasm の layers）と同じ割合の段階にする
        for f in world.facilities().iter().filter(|f| f.kind.is_parking()) {
            let lot = world.trips().shared_lot(f.area.min);
            let taken = lot.rented + lot.visitors.parked + lot.visitors.reserved;
            let level = fill_level(f.kind.parking_spaces(), taken);
            grid.each_tile_in(f.area, |cell, _| {
                cell.lot_fill = cell.lot_fill.max(Some(level));
            });
        }
        let homes: std::collections::BTreeSet<_> = world
            .carless_households()
            .into_iter()
            .map(|c| c.home)
            .collect();
        for id in homes {
            if let Some(b) = world.buildings().get(id) {
                grid.each_tile_in(b.area, |cell, _| cell.carless += 1);
            }
        }
    }
    // 混み具合は画面の交通のレイヤーと同じ計算で、車のいない区間は載らない（空いている）
    let congestion = (layer == MapLayer::Traffic).then(|| world.road_congestion());
    for (id, segment) in world.network().segments() {
        let kind = match segment.kind {
            SegmentKind::Road(k) => k as u8,
            SegmentKind::Highway => HIGHWAY,
        };
        let level = congestion
            .as_ref()
            .map(|m| m.get(&id).copied().unwrap_or_default());
        let Some(line) = world.network().segment_line(id) else {
            continue;
        };
        for t in line.tiles() {
            if let Some(cell) = grid.cell_mut(t) {
                cell.road = cell.road.max(kind);
                cell.congestion = cell.congestion.max(level);
            }
        }
    }

    for (_, n) in world.network().nodes() {
        if let Some(cell) = grid.cell_mut(n.tile()) {
            cell.signal |= n.control == NodeControl::Signal;
            // 高速道路の腕には横断歩道がないので、ほかの腕がある交差点だけを数える。
            let roads = n.segments.iter().filter(|s| {
                world
                    .network()
                    .segment(**s)
                    .is_some_and(|s| s.kind != SegmentKind::Highway)
            });
            let junction = n.segments.len() >= 3 && roads.count() > 0;
            cell.crosswalk |= junction;
            let removed = junction && n.no_crosswalk != 0;
            let signal = n.control == NodeControl::Signal;
            cell.signal_removed |= signal && removed;
            cell.removed |= !signal && removed;
        }
    }

    Ok(text(&grid, layer))
}

/// 鉄道の層の中身を入れる。線路は通る路線で、駅は混み具合で、空きマスは駅まで歩く範囲で見分ける。
/// 駅まで歩く範囲は、中核が駅を探す物差し（家の正面のマスから駅の出入口のマスまで、直線で 64 マス以内）と同じで、
/// セルの真ん中のマスで測る。
fn mark_rail_layer(world: &World, grid: &mut Grid) {
    let rail = world.rail();
    let net = world.network();
    for (id, line) in rail.lines() {
        let key = (line.stopped, id.index);
        let mut seen = std::collections::BTreeSet::new();
        for step in &line.path {
            if !seen.insert(step.track) {
                continue;
            }
            let Some(l) = rail.track_line(step.track) else {
                continue;
            };
            for t in l.tiles() {
                if rail.station_at(t).is_some() {
                    continue;
                }
                if let Some(cell) = grid.cell_mut(t) {
                    cell.line = Some(cell.line.map_or(key, |old| old.min(key)));
                }
            }
        }
    }
    for (id, s) in rail.stations() {
        let left_behind = rail.left_behind().iter().any(|(st, _, _)| *st == id);
        if left_behind || rail.waiting_at(id) >= CROWDED_WAITING {
            grid.each_tile_in(s.area, |cell, _| cell.crowded = true);
        }
    }
    let doors: Vec<TilePos> = rail
        .stations()
        .filter_map(|(_, s)| chizu_core::movement::facility_spot(net, &s.site()).map(|p| p.tile))
        .collect();
    let reach = STATION_WALK_RADIUS / grid.scale + 2;
    for door in doors {
        let (col0, row0) = (
            (door.x - grid.rect.min.x).div_euclid(grid.scale),
            (door.y - grid.rect.min.y).div_euclid(grid.scale),
        );
        for row in (row0 - reach).max(0)..=(row0 + reach).min(grid.rows as i64 - 1) {
            for col in (col0 - reach).max(0)..=(col0 + reach).min(grid.cols as i64 - 1) {
                let at = TilePos::new(
                    grid.rect.min.x + col * grid.scale + grid.scale / 2,
                    grid.rect.min.y + row * grid.scale + grid.scale / 2,
                );
                if in_walk_range(at, door) {
                    grid.cells[row as usize * grid.cols + col as usize].walk = true;
                }
            }
        }
    }
    for (id, line) in rail.lines() {
        let name = format!(
            "{}線",
            LINE_COLOR_NAMES[(id.index % LINE_GLYPHS.len() as u32) as usize]
        );
        let waiting: usize = rail
            .queues()
            .filter(|((_, l, _), _)| *l == id)
            .map(|(_, q)| q.len())
            .sum();
        let running = rail.trains().filter(|(_, t)| t.line == id).count();
        let state = if line.stopped {
            "、止まっている（線路がつながっていない）"
        } else if line.cap() == 0 {
            "、走れない（本数の上限が 0）"
        } else {
            ""
        };
        grid.notes.push(format!(
            "  {} {}（路線 {}）：本数 {}、走っている {}、いちばん混んだ列車 {}%、待っている人 {}{}",
            LINE_GLYPHS[(id.index % LINE_GLYPHS.len() as u32) as usize],
            name,
            id.index,
            line.trains,
            running,
            rail.line_load_permille(id) / 10,
            waiting,
            state
        ));
    }
}

/// 上の目盛りの基準にする数。縮尺 1 のときは x、そうでなければ列の番号で、絶対値の下 1 桁が 0 の列に x を書く。
fn key(grid: &Grid, col: usize) -> i64 {
    if grid.scale == 1 {
        grid.rect.min.x + col as i64
    } else {
        col as i64
    }
}

fn text(grid: &Grid, layer: MapLayer) -> String {
    let rect = grid.rect;
    let mut out = format!(
        "地図 x {}..{} y {}..{}（1 文字 = {s}×{s} マス、{}×{} 文字）\n",
        rect.min.x,
        rect.max.x,
        rect.min.y,
        rect.max.y,
        grid.cols,
        grid.rows,
        s = grid.scale
    );
    match layer {
        MapLayer::Terrain => out.push_str(LEGEND),
        MapLayer::Parking => {
            let lines: Vec<&str> = LEGEND
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with("P W L G") {
                        PARKING_FACILITY_LEGEND_LINE
                    } else {
                        l
                    }
                })
                .collect();
            out.push_str(&lines.join("\n"));
            out.push('\n');
            out.push_str(PARKING_LEGEND_LINES);
        }
        MapLayer::Traffic => {
            let lines: Vec<&str> = LEGEND
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with("H 1 2 3") {
                        TRAFFIC_LEGEND_LINE
                    } else {
                        l
                    }
                })
                .collect();
            out.push_str(&lines.join("\n"));
        }
        MapLayer::Rail => {
            let lines: Vec<&str> = LEGEND
                .lines()
                .map(|l| {
                    if l.trim_start().starts_with("T ") {
                        RAIL_LEGEND_LINES
                    } else {
                        l
                    }
                })
                .collect();
            out.push_str(&lines.join("\n"));
            if !grid.notes.is_empty() {
                out.push_str("\n路線（文字は線路のマスの文字）\n");
                out.push_str(&grid.notes.join("\n"));
            }
        }
    }
    out.push('\n');
    if grid.scale == 1 {
        out.push_str("座標：行頭が y、上の数は x（下の行は x の絶対値の下 1 桁で、-74 なら 4）\n");
    } else {
        out.push_str("座標：行頭は各行の先頭のセルの y、上の数は各列の先頭のセルの x（10 列ごと）。下の行はその列の番号の下 1 桁\n");
    }

    let row_ys: Vec<i64> = (0..grid.rows)
        .map(|r| rect.min.y + r as i64 * grid.scale)
        .collect();
    let label_width = row_ys
        .iter()
        .map(|y| y.to_string().len())
        .max()
        .unwrap_or(1);
    let pad = " ".repeat(label_width + 1);

    // x の目盛り。数が重ならない列にだけ置く。
    let mut ruler: Vec<char> = Vec::new();
    let mut digits = String::new();
    for col in 0..grid.cols {
        let k = key(grid, col);
        digits.push(char::from(b'0' + (k.unsigned_abs() % 10) as u8));
        if k.unsigned_abs() % 10 == 0 {
            let label = (rect.min.x + col as i64 * grid.scale).to_string();
            let from = col.max(if ruler.is_empty() { 0 } else { ruler.len() + 1 });
            if from == col {
                ruler.resize(col, ' ');
                ruler.extend(label.chars());
            }
        }
    }
    out.push_str(&pad);
    out.push_str(&ruler.into_iter().collect::<String>());
    out.push('\n');
    out.push_str(&pad);
    out.push_str(&digits);
    out.push('\n');

    for (row, y) in row_ys.iter().enumerate() {
        out.push_str(&format!("{y:>label_width$} "));
        out.extend(
            grid.cells[row * grid.cols..(row + 1) * grid.cols]
                .iter()
                .map(Cell::glyph),
        );
        out.push('\n');
    }
    // 呼び出し側が改行を足して出す。
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chizu_core::{Command, CommandLog, TICKS_PER_DAY, replay};

    /// 詰まりが起きる街（`chizu-core/tests/gridlock/city.rec`）の、車が混み合う朝を再生した世界。
    /// 詰まる道ができる tick は、育ち方の小さな違いで前後するので、67 日目の朝のうち、詰まった道が
    /// できた最初の tick まで進める。
    fn jammed_city() -> World {
        let log = city_log(Zone::Residential);
        let mut world = World::new(0);
        replay(&mut world, &log, 66 * TICKS_PER_DAY + 1_500);
        for _ in 0..150 {
            if world
                .road_congestion()
                .values()
                .any(|c| *c == Congestion::Jammed)
            {
                break;
            }
            replay(&mut world, &log, 10);
        }
        world
    }

    /// 人が遊んだ街（種 0）の記録の高密度の住宅の塗りを、`home` の塗りに書き換えた記録。
    fn city_log(home: Zone) -> CommandLog {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../chizu-core/tests/gridlock/city.rec");
        let src = CommandLog::decode(&std::fs::read(path).unwrap()).unwrap();
        let mut log = CommandLog::new();
        for e in src.entries() {
            let mut e = *e;
            if let Command::PaintZone { zone, .. } = &mut e.command
                && *zone == Some(Zone::Residential)
            {
                *zone = Some(home);
            }
            log.push(e).unwrap();
        }
        log
    }

    /// [`city_log`] を `ticks` まで再生した街。
    fn city_with_homes(ticks: u64, home: Zone) -> World {
        let mut world = World::new(0);
        replay(&mut world, &city_log(home), ticks);
        world
    }

    /// 区間の最初のマスを真ん中にした 5 マスの横長の範囲を描き、真ん中のマスの文字を返す。
    fn glyph_at(world: &World, t: TilePos, layer: MapLayer) -> char {
        let rect = TileRect::from_corners(TilePos::new(t.x - 2, t.y), TilePos::new(t.x + 2, t.y));
        let text = render(world, rect, 1, layer).unwrap();
        let row = text.lines().last().unwrap();
        row.chars().rev().nth(2).unwrap()
    }

    /// 混み具合の段階ごとに、その段階の区間の途中のマスを選び、段階の文字になることを見る。
    /// 区間の途中のマスはノードでなく、ほかの区間も通らない。(41, 500) は高速道路である。
    #[test]
    fn traffic_layer_marks_jammed_and_free_roads() {
        let world = jammed_city();
        let at = |t: TilePos, layer| glyph_at(&world, t, layer);
        let levels = world.road_congestion();
        let mut seen = Vec::new();
        let mut jammed = None;
        for (id, seg) in world.network().segments() {
            let line = world.network().segment_line(id).unwrap();
            if line.len < 2 || seg.kind == SegmentKind::Highway {
                continue;
            }
            let level = levels.get(&id).copied().unwrap_or_default();
            let t = line.tile(1);
            assert_eq!(at(t, MapLayer::Traffic), congestion_glyph(level), "{t:?}");
            seen.push(level);
            if level == Congestion::Jammed {
                jammed = Some(t);
            }
        }
        // 詰まった道と、車のいない道がある街である。
        assert!(seen.contains(&Congestion::Free));
        let jammed = jammed.expect("詰まった道がない");
        assert_eq!(at(TilePos::new(41, 500), MapLayer::Traffic), 'H');
        // 層を付けなければ、道路の種類のまま
        assert!(matches!(at(jammed, MapLayer::Terrain), '1'..='3'));
        let text = render(
            &world,
            TileRect::from_corners(
                TilePos::new(jammed.x - 2, jammed.y),
                TilePos::new(jammed.x + 2, jammed.y),
            ),
            1,
            MapLayer::Traffic,
        )
        .unwrap();
        assert!(text.contains("X 詰まっている"), "{text}");
        assert!(!text.contains("H 1 2 3"), "{text}");
    }

    /// 駐車場に切り欠いたマスは p、残りの建物のマスは区画の文字になる。凡例にも p がある。
    #[test]
    fn parking_lots_are_drawn_before_buildings() {
        let world = jammed_city();
        let net = world.network();
        let mut lots = 0;
        let mut bodies = 0;
        for (_, b) in world.buildings().iter().take(60) {
            let cut = lot_tiles(net, b, world.seed());
            let rect = b.area;
            let text = render(&world, rect, 1, MapLayer::Terrain).unwrap();
            let rows: Vec<&str> = text
                .lines()
                .rev()
                .take(rect.max.y.abs_diff(rect.min.y) as usize + 1)
                .collect();
            for (r, row) in rows.iter().rev().enumerate() {
                let glyphs: Vec<char> = row.chars().skip_while(|c| *c != ' ').skip(1).collect();
                for (c, g) in glyphs.iter().enumerate() {
                    let t = TilePos::new(rect.min.x + c as i64, rect.min.y + r as i64);
                    if cut.contains(&t) {
                        assert_eq!(*g, 'p', "{t:?} は駐車場");
                        lots += 1;
                    } else {
                        assert_ne!(*g, 'p', "{t:?} は駐車場でない");
                        bodies += 1;
                    }
                }
            }
        }
        assert!(lots > 0 && bodies > 0, "{lots} {bodies}");
        let text = render(
            &world,
            TileRect::from_corners(TilePos::new(0, 0), TilePos::new(1, 1)),
            1,
            MapLayer::Terrain,
        )
        .unwrap();
        assert!(text.contains("  p "), "{text}");
    }

    /// 私道の通るマスは =、私道の敷地の残りのマスは区画の文字になる。凡例にも = がある。
    #[test]
    fn private_lanes_are_drawn_before_buildings() {
        // 私道の敷地は低密度にだけ建つので、住宅の塗りを低密度に書き換えた街で見る
        let world = city_with_homes(8 * TICKS_PER_DAY, Zone::ResidentialLow);
        let net = world.network();
        let (mut lanes, mut rest) = (0, 0);
        for (_, b) in world.buildings().iter() {
            let lane = lane_tiles(net, b);
            if lane.is_empty() {
                continue;
            }
            let rect = b.area;
            let text = render(&world, rect, 1, MapLayer::Terrain).unwrap();
            let rows: Vec<&str> = text
                .lines()
                .rev()
                .take(rect.max.y.abs_diff(rect.min.y) as usize + 1)
                .collect();
            for (r, row) in rows.iter().rev().enumerate() {
                let glyphs: Vec<char> = row.chars().skip_while(|c| *c != ' ').skip(1).collect();
                for (c, g) in glyphs.iter().enumerate() {
                    let t = TilePos::new(rect.min.x + c as i64, rect.min.y + r as i64);
                    if lane.contains(&t) {
                        assert_eq!(*g, '=', "{t:?} は私道");
                        lanes += 1;
                    } else {
                        assert_ne!(*g, '=', "{t:?} は私道でない");
                        rest += 1;
                    }
                }
            }
        }
        assert!(lanes > 0 && rest > 0, "{lanes} {rest}");
        assert!(LEGEND.contains("  =  "), "{LEGEND}");
    }

    /// 線路は T、駅は E で出す。駅の端のノードのマスは駅の中なので E である。凡例にも載る。
    #[test]
    fn rails_and_stations_are_drawn() {
        use chizu_core::{ApplyOutcome, Command, RoadKind};
        let t = TilePos::new;
        let mut w = World::new(0);
        for c in [
            Command::BuildRoad {
                from: t(0, 10),
                to: t(40, 10),
                kind: RoadKind::Small,
            },
            Command::PlaceBuilding {
                kind: ServiceKind::Station,
                at: t(12, 9),
                rotation: 2,
            },
            Command::BuildRail {
                from: t(12, 8),
                to: t(16, 8),
            },
        ] {
            assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{c:?}");
        }
        let row = TileRect::from_corners(t(2, 8), t(17, 8));
        let glyphs: String = glyphs_of(&w, row, MapLayer::Terrain).into_iter().collect();
        assert_eq!(glyphs, ".EEEEEEEEEETTTT.");
        assert!(
            LEGEND.contains("  E  ") && LEGEND.contains("  T  "),
            "{LEGEND}"
        );
    }

    /// 駅 2 つを線路で結び、路線を作った街。駅の線路は y = 8 で、x 3〜12 と x 16〜25 にある。
    fn rail_world() -> World {
        use chizu_core::{ApplyOutcome, Command, RoadKind};
        let t = TilePos::new;
        let mut w = World::new(0);
        for c in [
            Command::BuildRoad {
                from: t(0, 10),
                to: t(40, 10),
                kind: RoadKind::Small,
            },
            Command::PlaceBuilding {
                kind: ServiceKind::Station,
                at: t(12, 9),
                rotation: 2,
            },
            Command::PlaceBuilding {
                kind: ServiceKind::Station,
                at: t(25, 9),
                rotation: 2,
            },
            // 線路は 2 本に分けて引く。片方だけ取り壊しても、もう片方が残る
            Command::BuildRail {
                from: t(12, 8),
                to: t(14, 8),
            },
            Command::BuildRail {
                from: t(14, 8),
                to: t(16, 8),
            },
        ] {
            assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{c:?}");
        }
        let ids: Vec<_> = w.rail().stations().map(|(id, _)| id).collect();
        let c = Command::CreateLine {
            from: ids[0],
            to: ids[1],
        };
        assert_eq!(w.apply(&c), ApplyOutcome::Applied, "{c:?}");
        w
    }

    /// 鉄道の層は、線路を路線の色の番号の文字（路線 0 は A）にし、止まっている路線は x にする。
    /// 層を付けなければ T のまま。凡例の下に路線ごとの行が付く。
    #[test]
    fn rail_layer_draws_lines_and_stopped_lines() {
        use chizu_core::{ApplyOutcome, Command};
        let t = TilePos::new;
        let mut w = rail_world();
        let row = TileRect::from_corners(t(12, 8), t(16, 8));
        let glyphs = |w: &World, layer| glyphs_of(w, row, layer).into_iter().collect::<String>();
        assert_eq!(glyphs(&w, MapLayer::Terrain), "ETTTE");
        assert_eq!(glyphs(&w, MapLayer::Rail), "EAAAE");
        let text = render(&w, row, 1, MapLayer::Rail).unwrap();
        assert!(
            text.contains("A 青線（路線 0）：本数 0、走っている 0"),
            "{text}"
        );
        assert!(text.contains("A B J K M N Q U"), "{text}");
        assert!(text.contains("  x  "), "{text}");
        // 線路を 1 本取り壊すと路線が止まり、残った線路は x になる
        let cut = Command::Bulldoze {
            area: TileRect::from_corners(t(15, 8), t(15, 8)),
        };
        assert_eq!(w.apply(&cut), ApplyOutcome::Applied);
        let after = glyphs(&w, MapLayer::Rail);
        assert!(after.contains('x') && !after.contains('A'), "{after}");
        assert!(
            render(&w, row, 1, MapLayer::Rail)
                .unwrap()
                .contains("止まっている")
        );
    }

    /// 駅まで歩く範囲は、駅の出入口から 64 マス以内の空きマスを : にする。範囲の外は . のまま。
    /// ほかのものが載るマス（道路など）は隠さない。
    #[test]
    fn rail_layer_marks_the_walk_range_of_the_stations() {
        let t = TilePos::new;
        let w = rail_world();
        let near = TileRect::from_corners(t(2, 20), t(26, 20));
        let far = TileRect::from_corners(t(2, 100), t(26, 100));
        let all = |rect, layer| glyphs_of(&w, rect, layer).into_iter().collect::<String>();
        assert_eq!(all(near, MapLayer::Terrain), ".".repeat(25));
        assert_eq!(all(near, MapLayer::Rail), ":".repeat(25));
        assert_eq!(all(far, MapLayer::Rail), ".".repeat(25));
        // 道路の上（y = 10）は道路のまま
        let road = TileRect::from_corners(t(2, 10), t(6, 10));
        assert_eq!(all(road, MapLayer::Rail), "22222".replace('2', "1"));
        // まとめた文字（縮尺 8）でも、セルの真ん中が範囲に入れば :
        let text = render(
            &w,
            TileRect::from_corners(t(0, 64), t(63, 71)),
            8,
            MapLayer::Rail,
        )
        .unwrap();
        assert!(text.lines().last().unwrap().contains(':'), "{text}");
    }

    #[test]
    fn unknown_layer_is_refused() {
        assert!(MapLayer::parse("rail").is_ok());
        assert!(MapLayer::parse("traffic").is_ok());
        assert!(MapLayer::parse("parking").is_ok());
        assert!(MapLayer::parse("power").is_err());
    }

    fn glyphs_of(world: &World, rect: TileRect, layer: MapLayer) -> Vec<char> {
        let text = render(world, rect, 1, layer).unwrap();
        let rows = rect.max.y.abs_diff(rect.min.y) as usize + 1;
        text.lines()
            .rev()
            .take(rows)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .flat_map(|row| {
                row.chars()
                    .skip_while(|c| *c != ' ')
                    .skip(1)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// 駐車場の層は、車を持てない世帯のいる住宅を !、共同の駐車場を埋まり具合の文字にする。
    /// 層を付けなければ、住宅は区画の文字、駐車場は L のまま。
    #[test]
    fn parking_layer_marks_carless_homes_and_the_fill_of_lots() {
        use crate::agent::test_world::{carless_world, place, run};
        let (mut w, _guard) = carless_world(2);
        let homes: Vec<_> = w
            .carless_households()
            .iter()
            .map(|c| c.home)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(!homes.is_empty());
        let rect_of = |w: &World, id| w.buildings().get(id).unwrap().area;
        for id in &homes {
            let rect = rect_of(&w, *id);
            let glyphs = glyphs_of(&w, rect, MapLayer::Parking);
            // 駐車場に切り欠いたマス（p）と私道（=）は、建物より先に出す
            assert!(glyphs.contains(&'!'), "{rect:?} {glyphs:?}");
            assert!(
                glyphs.iter().all(|g| matches!(*g, '!' | 'p' | '=')),
                "{glyphs:?}"
            );
            let plain = glyphs_of(&w, rect, MapLayer::Terrain);
            assert!(!plain.contains(&'!'), "{plain:?}");
        }
        let text = render(&w, rect_of(&w, homes[0]), 1, MapLayer::Parking).unwrap();
        assert!(text.contains("! 車を持てない世帯のいる住宅"), "{text}");
        assert!(!text.contains("P W L G"), "{text}");
        let terrain = render(&w, rect_of(&w, homes[0]), 1, MapLayer::Terrain).unwrap();
        assert!(!terrain.contains("車を持てない世帯"), "{terrain}");

        // 置いたばかり（借りる前）は半分に満たず、借りられると満車になる
        place(&mut w, ServiceKind::ParkingLot, 24);
        let lot = w.facilities().last().unwrap().area;
        let glyphs = glyphs_of(&w, lot, MapLayer::Parking);
        assert!(glyphs.iter().all(|g| *g == '-'), "{glyphs:?}");
        assert!(
            glyphs_of(&w, lot, MapLayer::Terrain)
                .iter()
                .all(|g| *g == 'L')
        );
        run(&mut w, 120);
        let glyphs = glyphs_of(&w, lot, MapLayer::Parking);
        assert!(glyphs.iter().all(|g| *g == 'X'), "{glyphs:?}");
    }

    #[test]
    fn fill_levels_step_with_the_share_taken() {
        let levels: Vec<u8> = [0, 4, 5, 7, 8, 9, 10, 11]
            .iter()
            .map(|t| fill_level(10, *t))
            .collect();
        assert_eq!(levels, [0, 0, 1, 1, 2, 2, 3, 3]);
    }
}
