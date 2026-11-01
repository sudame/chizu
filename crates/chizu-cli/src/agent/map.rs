//! `agent map`：範囲の地図を、1 マス 1 文字のマス目のテキストにする。
//!
//! 道路や区画をどこに置くかを、数値の observe だけでは決めにくいので、地図を文字で見せる。
//! 書き込みはせず、中核の公開の読み出し口だけを使う。

use chizu_core::chunk::Layer;
use chizu_core::network::SegmentKind;
use chizu_core::tile::local_index;
use chizu_core::{CHUNK_SIDE, ServiceKind, TilePos, TileRect, World, ZoneKind, is_water};

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
  H 1 2 3  道路（H 高速道路、1 2 3 は Small Medium Large）
  R C I O  建物（住宅、商業、工業、オフィスの区画に建っている）
  r c i o  建物のない区画（同じ並び）
  ~        水辺（海か川）
  ,        買っていない土地
  .        買った土地の空きマス（標高はいまどこも 0 の平らな地図なので出さない）";

/// セルにまとめたマスの数え上げ。
#[derive(Clone, Default)]
struct Cell {
    facility: [u32; 2],
    /// 道路の最大の種類。0 は道路なし。
    road: u8,
    building: [u32; 4],
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
    /// 載せる文字。公共施設、道路、建物、区画、水辺、買っていない土地、空きマスの順に選び、
    /// 同じ種類の中では数の多いものを選ぶ（道路は種類の大きいもの）。
    fn glyph(&self) -> char {
        if let Some(i) = argmax(&self.facility) {
            return facility_letter(ServiceKind::ALL[i]);
        }
        match self.road {
            0 => {}
            HIGHWAY => return 'H',
            k => return char::from(b'0' + k),
        }
        if let Some(i) = argmax(&self.building) {
            return zone_letter(ZoneKind::ALL[i]);
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
pub fn render(world: &World, rect: TileRect, scale: u64) -> Result<String, String> {
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
        grid.each_tile_in(b.area, |cell, _| cell.building[b.zone.index()] += 1);
    }
    for f in world.facilities() {
        grid.each_tile_in(f.area, |cell, _| cell.facility[f.kind.index()] += 1);
    }
    for (id, segment) in world.network().segments() {
        let kind = match segment.kind {
            SegmentKind::Road(k) => k as u8,
            SegmentKind::Highway => HIGHWAY,
        };
        let Some(line) = world.network().segment_line(id) else {
            continue;
        };
        for t in line.tiles() {
            if let Some(cell) = grid.cell_mut(t) {
                cell.road = cell.road.max(kind);
            }
        }
    }

    Ok(text(&grid))
}

/// 上の目盛りの基準にする数。縮尺 1 のときは x、そうでなければ列の番号で、絶対値の下 1 桁が 0 の列に x を書く。
fn key(grid: &Grid, col: usize) -> i64 {
    if grid.scale == 1 {
        grid.rect.min.x + col as i64
    } else {
        col as i64
    }
}

fn text(grid: &Grid) -> String {
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
    out.push_str(LEGEND);
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
