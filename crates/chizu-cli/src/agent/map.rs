//! `agent map`：範囲の地図を、1 マス 1 文字のマス目のテキストにする。
//!
//! 道路や区画をどこに置くかを、数値の observe だけでは決めにくいので、地図を文字で見せる。
//! 書き込みはせず、中核の公開の読み出し口だけを使う。

use chizu_core::chunk::Layer;
use chizu_core::movement::{lane_tiles, lot_tiles};
use chizu_core::network::{NodeControl, SegmentKind};
use chizu_core::tile::local_index;
use chizu_core::{
    CHUNK_SIDE, Congestion, ServiceKind, TilePos, TileRect, World, ZoneKind, is_water,
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
  P W      公共施設（P 発電所、W 浄水場）
  S s      信号のある交差点（s は横断歩道を外した腕がある。--layer traffic では混み具合の文字を先に出す）
  Z Y      信号のない交差点。Z は高速道路でないどの腕にも横断歩道があり、Y は横断歩道を外した腕がある
           （--layer traffic では混み具合の文字を先に出す）
  H 1 2 3  道路（H 高速道路、1 2 3 は Small Medium Large）
  =        私道の通るマス（私道の敷地の住宅の、端の列。私道の脇の枠もこの列にある。駐車場と建物より先に出す）
  p        建物の駐車場に切り欠いたマス（建物より先に出す。前面の帯のように 1 マスの一部だけの切り欠きも p にする）
  * ^ %    電気か水が届いていない建物（* 電気だけ、^ 水だけ、% 両方。R C I O の代わりに出す）
  R C I O  建物（住宅、商業、工業、オフィスの区画に建っている。電気も水も届いている）
  r c i o  建物のない区画（同じ並び）
  ~        水辺（海か川）
  ,        買っていない土地
  .        買った土地の空きマス（標高はいまどこも 0 の平らな地図なので出さない）";

/// 凡例の「H 1 2 3」の行の代わりに出す、交通の層の道路の凡例。
const TRAFFIC_LEGEND_LINE: &str = "  - + # X  道路の混み具合（- 空いている、+ やや混んでいる、# 混んでいる、X 詰まっている。H 高速道路は混み具合を数えない）";

/// 地図に重ねる層。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MapLayer {
    /// 道路を種類で出す。
    #[default]
    Terrain,
    /// 道路を混み具合の段階で出す。
    Traffic,
}

impl MapLayer {
    pub fn parse(name: &str) -> Result<MapLayer, String> {
        match name {
            "traffic" => Ok(MapLayer::Traffic),
            _ => Err(format!("--layer {name:?} は知らない層。traffic だけ使える")),
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

/// セルにまとめたマスの数え上げ。
#[derive(Clone, Default)]
struct Cell {
    /// 通る区間の混み具合の最大。交通の層でだけ入れる。
    congestion: Option<Congestion>,
    facility: [u32; 2],
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
    building: [u32; 4],
    /// 駐車場に切り欠いたマスの数（1 マスの一部だけの帯も 1 と数える）。
    lot: u32,
    /// 私道の通るマスの数。
    lane: u32,
    /// 電気が届いていない建物のマスの数。
    unpowered: u32,
    /// 水が届いていない建物のマスの数。
    dry: u32,
    zone: [u32; 4],
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

fn zone_letter(z: ZoneKind) -> char {
    match z {
        ZoneKind::Residential => 'R',
        ZoneKind::Commercial => 'C',
        ZoneKind::Industrial => 'I',
        ZoneKind::Office => 'O',
    }
}

fn facility_letter(k: ServiceKind) -> char {
    match k {
        ServiceKind::PowerPlant => 'P',
        ServiceKind::WaterPump => 'W',
    }
}

impl Cell {
    /// 載せる文字。公共施設、信号、横断歩道のある交差点（この 2 つは交通の層では道路のあと）、道路、私道、駐車場、建物、区画、水辺、買っていない土地、空きマスの順に選び、
    /// 同じ種類の中では数の多いものを選ぶ（道路は種類の大きいもの）。
    fn glyph(&self) -> char {
        if let Some(i) = argmax(&self.facility) {
            return facility_letter(ServiceKind::ALL[i]);
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
        // 私道と駐車場に切り欠いたマスは、建物より先に出す。セルの中で建物のマスより少なければ、建物を出す。
        if self.lane > 0 && self.lane + self.lot >= self.building.iter().sum::<u32>() {
            return '=';
        }
        if self.lot > 0 && self.lot >= self.building.iter().sum::<u32>() {
            return 'p';
        }
        if let Some(i) = argmax(&self.building) {
            // 電気か水が届いていない建物は、区画の種類より先にそれを出す。
            return match (self.unpowered > 0, self.dry > 0) {
                (true, true) => '%',
                (true, false) => '*',
                (false, true) => '^',
                (false, false) => zone_letter(ZoneKind::ALL[i]),
            };
        }
        if let Some(i) = argmax(&self.zone) {
            return zone_letter(ZoneKind::ALL[i]).to_ascii_lowercase();
        }
        if self.wet * 2 >= self.samples {
            '~'
        } else if self.owned * 2 < self.samples {
            ','
        } else {
            '.'
        }
    }
}

/// 範囲を `scale`×`scale` マスずつのセルに分けた格子。
struct Grid {
    rect: TileRect,
    scale: i64,
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
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
                cell.zone[z.index()] += 1;
            }
        });
    }
    for (_, b) in world.buildings().iter() {
        grid.each_tile_in(b.area, |cell, _| {
            cell.building[b.zone.index()] += 1;
            cell.unpowered += u32::from(!b.supply.power);
            cell.dry += u32::from(!b.supply.water);
        });
    }
    for (_, b) in world.buildings().iter() {
        for t in lot_tiles(world.network(), b) {
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
    use chizu_core::{CommandLog, TICKS_PER_DAY, replay};

    /// 詰まりが起きる街（`chizu-core/tests/gridlock/city.rec`）の、車が混み合う朝を再生した世界。
    fn jammed_city() -> World {
        city_at(66 * TICKS_PER_DAY + 2_000)
    }

    /// 人が遊んだ街（種 0）を、`ticks` まで再生した街。
    fn city_at(ticks: u64) -> World {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../chizu-core/tests/gridlock/city.rec");
        let log = CommandLog::decode(&std::fs::read(path).unwrap()).unwrap();
        let mut world = World::new(0);
        replay(&mut world, &log, ticks);
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
            let cut = lot_tiles(net, b);
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
        // 育ちきった街の住宅は段階 3 以上で私道がないので、住宅が建ち始めた頃の街で見る
        let world = city_at(8 * TICKS_PER_DAY);
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

    #[test]
    fn unknown_layer_is_refused() {
        assert!(MapLayer::parse("traffic").is_ok());
        assert!(MapLayer::parse("power").is_err());
    }
}
