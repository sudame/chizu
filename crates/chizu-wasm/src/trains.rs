//! 列車の車両を、三重バッファの車両の欄の形（移動体と同じ語の並び）に直す。
//!
//! 画面は車両ごとに前の tick と今の tick のあいだを補間するので、中核の `Train` から、先頭の位置を
//! 起点に進路を後ろへたどって、車両の真ん中の位置と向きを求める。中核の状態は変えない。
//! 浮動小数点を使うが、描画のためだけで、状態にもハッシュにも入れない。

use chizu_core::rail::{RailLine, Train};
use chizu_core::{CHUNK_SIDE, ChunkCoord, Dir, TilePos, World};

use crate::car_path::angle_code;
use crate::snapshot::{MAX_TRAIN_CARS, Mover};

/// 種類の番号。画面の `web/src/map/trains.ts` の `KIND_TRAIN` と同じ。
/// 上位 16 ビットのうち、下位 2 ビットが種類、残りが路線の色の番号になる。
pub const KIND_TRAIN: u16 = 3;

/// 1 編成の両数。設計 3.10 の 3。
pub const CARS_PER_TRAIN: usize = 4;

/// 1 両の長さ（mm）。編成の全長を両数で割ったもの。
const CAR_LEN_MM: i64 = chizu_core::rail::train::TRAIN_LEN_MM / CARS_PER_TRAIN as i64;

/// 路線の色の数。画面の `LINE_COLORS` と同じ。
pub const LINE_COLOR_COUNT: u32 = 8;

/// 複線の上りと下りの線路の中心が、線路の中心線から離れる幅（マス）。画面の `TRACK_OFFSET` と同じ。
const LANE_OFFSET: f64 = 0.2;

/// 路線の色の番号。路線の添え字の順に、8 色を回して当てる。
pub fn line_color(index: u32) -> u32 {
    index % LINE_COLOR_COUNT
}

/// 進路の要素 `i` の、入るところから `s`（mm）進んだ点の位置（マス、ワールド座標）と、進む向きの角度。
/// 左側通行に合わせて、進む向きの左へ [`LANE_OFFSET`] ずらす。
fn point_on(world: &World, line: &RailLine, i: usize, s: i64) -> (f64, f64, f64) {
    let step = &line.path[i];
    let track = world
        .rail()
        .track_line(step.track)
        .expect("進路の線路がない");
    let (mut a, mut b) = (track.start, track.end());
    if step.dir == Dir::Backward {
        core::mem::swap(&mut a, &mut b);
    }
    let t = if step.len > 0 {
        (s.clamp(0, step.len) as f64) / (step.len as f64)
    } else {
        0.0
    };
    let (ax, ay) = (a.x as f64 + 0.5, a.y as f64 + 0.5);
    let (dx, dy) = ((b.x - a.x) as f64, (b.y - a.y) as f64);
    let norm = dx.hypot(dy).max(1.0);
    let (ux, uy) = (dx / norm, dy / norm);
    // 画面は y が下向きなので、東へ進むときの左（北）は (0, -1) になる
    let (lx, ly) = (uy, -ux);
    (
        ax + dx * t + lx * LANE_OFFSET,
        ay + dy * t + ly * LANE_OFFSET,
        uy.atan2(ux),
    )
}

/// 先頭から `behind`（mm）後ろの点を、進路を逆にたどって求める。
fn car_point(world: &World, line: &RailLine, train: &Train, behind: i64) -> (f64, f64, f64) {
    let n = line.path.len();
    let mut i = train.pos as usize % n;
    let mut s = train.offset.clamp(0, line.path[i].len);
    let mut rest = behind;
    for _ in 0..=n {
        if rest <= s {
            return point_on(world, line, i, s - rest);
        }
        rest -= s;
        i = (i + n - 1) % n;
        s = line.path[i].len;
    }
    point_on(world, line, i, 0)
}

/// いま閉じている踏切のマスを、マスの順に返す。車両と同じ枠に書いて、列車の位置と食い違わないようにする。
pub fn closed_crossings(world: &World) -> Vec<[i32; 2]> {
    let rail = world.rail();
    rail.crossings()
        .filter(|t| rail.closed_since(*t).is_some())
        .filter_map(|t| Some([i32::try_from(t.x).ok()?, i32::try_from(t.y).ok()?]))
        .collect()
}

/// 走っている列車の車両を `out` に入れ、車両の座標の基準にしたチャンクを返す。
/// 車両は列車の ID の順に、先頭の車両から並べる。上限を超える分は書かない。
pub fn collect_cars(world: &World, out: &mut Vec<Mover>) -> ChunkCoord {
    out.clear();
    let rail = world.rail();
    let mut cars: Vec<(f64, f64, u16, u32, u32, u16)> = Vec::new();
    'trains: for (id, train) in rail.trains() {
        let Some(line) = rail.line(train.line) else {
            continue;
        };
        if line.path.is_empty() {
            continue;
        }
        let color = line_color(train.line.index);
        for car in 0..CARS_PER_TRAIN {
            if cars.len() >= MAX_TRAIN_CARS {
                break 'trains;
            }
            // 車両の真ん中は、先頭から車両の半分の長さと、前の車両の長さのぶんだけ後ろにある
            let behind = CAR_LEN_MM / 2 + CAR_LEN_MM * car as i64;
            let (x, y, angle) = car_point(world, line, train, behind);
            let kind = KIND_TRAIN | ((color as u16) << 2);
            cars.push((
                x,
                y,
                angle_code(angle),
                id.index * CARS_PER_TRAIN as u32 + car as u32,
                u32::from(id.generation),
                kind,
            ));
        }
    }
    let base = cars
        .first()
        .map(|c| {
            TilePos::new(c.0.floor() as i64, c.1.floor() as i64)
                .chunk()
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let (ox, oy) = (
        i64::from(base.cx) * CHUNK_SIDE as i64,
        i64::from(base.cy) * CHUNK_SIDE as i64,
    );
    for (x, y, heading, id, generation, kind) in cars {
        out.push(Mover {
            x: (x - ox as f64) as f32,
            y: (y - oy as f64) as f32,
            heading,
            kind,
            id,
            generation,
            path: 0,
            via: 0,
            frac: 0.0,
        });
    }
    base
}

#[cfg(test)]
mod tests;
