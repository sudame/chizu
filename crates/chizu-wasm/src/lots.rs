//! 建物の駐車場の形。敷地を切り欠いた所の長方形と、枠の並びを、画面に送る数に直す。
//!
//! 切り欠く形（前面の帯か脇の駐車場か、列の数、マスの数）は中核の `Building::parking_layout` が決め、
//! 間口のどちらの端に寄せるかは中核の `lot_end`（出入りの位置 `lot_spot` と同じ決まり）が決める。
//! ここはその値を、ワールドのマスの座標の長方形に置くだけで、状態は変えない。
//! 決め方は `docs/design.md` の 3.6「敷地の切り欠き方」にある。

use chizu_core::building::{Building, ParkingCut, SLOT_WIDTH_MM, SLOTS_PER_TILE};
use chizu_core::movement::{LotEnd, lot_end};
use chizu_core::network::{Network, TILE_MM};

/// 1 つの枠の奥行き（マス）。5m である。
const SLOT_DEPTH: f64 = 5_000.0 / TILE_MM as f64;
/// 1 つの枠の幅（マス）。2.5m である。
const SLOT_WIDTH: f64 = SLOT_WIDTH_MM as f64 / TILE_MM as f64;
/// 脇の駐車場の列の中の通路の幅（マス）。3m である。
const AISLE: f64 = 1.0 - SLOT_DEPTH;

/// ワールドのマスの座標での長方形。`[x0, y0, x1, y1]`。
pub type Rect = [f64; 4];

/// 枠の並び 1 本。枠は `rect` の長い辺に沿って `count` 個並び、`open` の向きの辺が車の入る側になる。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strip {
    pub rect: Rect,
    /// 車の入る側へ向かう単位ベクトル。縦か横である。
    pub open: (i64, i64),
    pub count: u64,
}

/// 切り欠いた駐車場の形。
#[derive(Clone, Debug, PartialEq)]
pub struct LotShape {
    /// 切り欠いた所を覆う長方形。重ならない。
    pub faces: Vec<Rect>,
    pub strips: Vec<Strip>,
}

/// 建物の駐車場の切り欠いた形。切り欠かない建物（斜めの道路に面した建物）は `None`。
pub fn lot_shape(net: &Network, b: &Building) -> Option<LotShape> {
    let layout = b.parking_layout();
    if layout.cut == ParkingCut::None {
        return None;
    }
    let (tx, ty) = b.toward_road;
    let (ax, ay) = (-ty, tx);
    let w = b.area.max.x.abs_diff(b.area.min.x) + 1;
    let h = b.area.max.y.abs_diff(b.area.min.y) + 1;
    let (frontage, depth) = if tx == 0 { (w, h) } else { (h, w) };
    let frontage = frontage as f64;
    let depth = depth as f64;
    let far = lot_end(net, b) == LotEnd::Far;
    // 間口に沿う向きの座標 u は `front` の辺を 0、道路に直角な座標 v は道路に面した辺を 0 として、建物の中へ増える。
    // 切り欠きは u の小さい端に置いて作り、`Far` ならそこで u を折り返す。
    let (cx, cy) = (b.front.x as f64 + 0.5, b.front.y as f64 + 0.5);
    let fold = |u: f64| if far { frontage - u } else { u };
    let world = |u: f64, v: f64| {
        let (u, v) = (fold(u) - 0.5, v - 0.5);
        (
            cx + ax as f64 * u - tx as f64 * v,
            cy + ay as f64 * u - ty as f64 * v,
        )
    };
    let rect = |u0: f64, v0: f64, u1: f64, v1: f64| -> Rect {
        let (x0, y0) = world(u0, v0);
        let (x1, y1) = world(u1, v1);
        [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
    };
    // 局所の向き (du, dv) をワールドの向きに直す
    let direction = |du: i64, dv: i64| {
        let du = if far { -du } else { du };
        (ax * du - tx * dv, ay * du - ty * dv)
    };
    let mut shape = LotShape {
        faces: Vec::new(),
        strips: Vec::new(),
    };
    match layout.cut {
        ParkingCut::None => return None,
        ParkingCut::FrontStrip { slots } => {
            let length = slots as f64 * SLOT_WIDTH;
            let r = rect(0.0, 0.0, length, SLOT_DEPTH);
            shape.faces.push(r);
            shape.strips.push(Strip {
                rect: r,
                open: direction(0, -1),
                count: slots,
            });
        }
        ParkingCut::Side { columns, tiles } => {
            let rows = depth as u64;
            let last_rows = tiles - (columns - 1) * rows;
            let last_slots = layout.surface - (columns - 1) * rows * SLOTS_PER_TILE;
            // 満ちた列は 1 つの長方形にまとめ、浅い最後の列だけを別にする。深さがそろっていれば全体で 1 つにする
            let full = if last_rows == rows {
                columns
            } else {
                columns - 1
            };
            if full > 0 {
                shape.faces.push(rect(0.0, 0.0, full as f64, depth));
            }
            if last_rows != rows {
                let j = (columns - 1) as f64;
                shape.faces.push(rect(j, 0.0, j + 1.0, last_rows as f64));
            }
            for j in 0..columns {
                let n = if j + 1 == columns {
                    last_slots
                } else {
                    rows * SLOTS_PER_TILE
                };
                let col = j as f64;
                // 偶数の列は通路を端の側に、奇数の列は通路を内の側に置く。となり合う列の枠が背中合わせになる
                let (u0, u1, open) = if j % 2 == 0 {
                    (col + AISLE, col + 1.0, direction(-1, 0))
                } else {
                    (col, col + SLOT_DEPTH, direction(1, 0))
                };
                shape.strips.push(Strip {
                    rect: rect(u0, 0.0, u1, n as f64 * SLOT_WIDTH),
                    open,
                    count: n,
                });
            }
        }
    }
    Some(shape)
}

/// 画面に送る数。建物ごとに、切り欠く建物だけを次の並びで続ける。
///
/// `[建物の min の x, min の y, 平面の台数, 建物の中の台数, 長方形の数, 枠の並びの数,`
/// `長方形ごとに x0, y0, x1, y1, 枠の並びごとに x0, y0, x1, y1, 向きの x, 向きの y, 枠の数]`。
pub fn encode(net: &Network, buildings: impl Iterator<Item = Building>) -> Vec<f64> {
    let mut out = Vec::new();
    for b in buildings {
        let Some(shape) = lot_shape(net, &b) else {
            continue;
        };
        let layout = b.parking_layout();
        out.extend([
            b.area.min.x as f64,
            b.area.min.y as f64,
            layout.surface as f64,
            layout.indoor as f64,
            shape.faces.len() as f64,
            shape.strips.len() as f64,
        ]);
        for f in &shape.faces {
            out.extend(*f);
        }
        for s in &shape.strips {
            out.extend(s.rect);
            out.extend([s.open.0 as f64, s.open.1 as f64, s.count as f64]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Sim;
    use chizu_core::movement::{building_spot, lot_spot};
    use chizu_core::world::growth::GROWTH_INTERVAL;
    use chizu_core::{Command, RoadKind, TilePos, TileRect, ZoneKind};

    /// 道路を引き、区画を塗って、建物が建つまで進めた街。
    fn town(zone: ZoneKind, road: (TilePos, TilePos), paint: TileRect) -> Sim {
        let mut sim = Sim::new(0);
        sim.submit(Command::BuildRoad {
            from: road.0,
            to: road.1,
            kind: RoadKind::Small,
        })
        .unwrap();
        sim.submit(Command::PaintZone {
            area: paint,
            zone: Some(zone),
        })
        .unwrap();
        for _ in 0..30 {
            sim.advance(GROWTH_INTERVAL as u32);
        }
        sim.advance(1);
        sim
    }

    fn area(shape: &LotShape) -> f64 {
        shape
            .faces
            .iter()
            .map(|f| (f[2] - f[0]) * (f[3] - f[1]))
            .sum()
    }

    fn inside(outer: Rect, inner: Rect) -> bool {
        let eps = 1e-9;
        inner[0] >= outer[0] - eps
            && inner[1] >= outer[1] - eps
            && inner[2] <= outer[2] + eps
            && inner[3] <= outer[3] + eps
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        let eps = 1e-9;
        a[0] < b[2] - eps && b[0] < a[2] - eps && a[1] < b[3] - eps && b[1] < a[3] - eps
    }

    #[test]
    fn the_lot_follows_the_core_layout_and_entrance() {
        let kinds = [ZoneKind::Residential, ZoneKind::Industrial];
        let mut checked = 0;
        let mut centered = 0;
        for zone in kinds {
            // 道路が南北のどちらの側にあっても確かめるため、道路の両側に区画を塗る
            let sim = town(
                zone,
                (TilePos::new(10, 20), TilePos::new(50, 20)),
                TileRect::from_corners(TilePos::new(10, 12), TilePos::new(50, 28)),
            );
            let world = sim.world();
            let net = world.network();
            for (_, b) in world.buildings().iter() {
                let layout = b.parking_layout();
                let Some(shape) = lot_shape(net, b) else {
                    assert_eq!(layout.cut, ParkingCut::None);
                    continue;
                };
                checked += 1;
                let site = [
                    b.area.min.x as f64,
                    b.area.min.y as f64,
                    b.area.max.x as f64 + 1.0,
                    b.area.max.y as f64 + 1.0,
                ];
                // 切り欠きは建物の敷地の中にあり、重ならない
                for (i, f) in shape.faces.iter().enumerate() {
                    assert!(inside(site, *f), "{zone:?} {b:?} {f:?}");
                    for g in &shape.faces[i + 1..] {
                        assert!(!overlap(*f, *g), "{zone:?} 長方形が重なる");
                    }
                }
                for s in &shape.strips {
                    assert!(
                        shape.faces.iter().any(|f| inside(*f, s.rect)),
                        "{zone:?} 枠が切り欠きの外に出る"
                    );
                }
                // 枠の数は平面の台数に一致する
                assert_eq!(
                    shape.strips.iter().map(|s| s.count).sum::<u64>(),
                    layout.surface
                );
                // 面積は、前面の帯なら枠の数ぶん、脇の駐車場ならマスの数ぶんになる
                let expect = match layout.cut {
                    ParkingCut::FrontStrip { slots } => slots as f64 * SLOT_WIDTH * SLOT_DEPTH,
                    ParkingCut::Side { tiles, .. } => tiles as f64,
                    ParkingCut::None => unreachable!(),
                };
                assert!((area(&shape) - expect).abs() < 1e-9, "{zone:?} {b:?}");
                // 出入りの位置は、切り欠いた所の道路に面した辺の上にある。道路の側の辺の外で、辺の範囲の中に取る
                let spot = lot_spot(net, b).expect("出入りの位置がない").world();
                let (px, py) = (
                    spot.x.0 as f64 / TILE_MM as f64,
                    spot.y.0 as f64 / TILE_MM as f64,
                );
                let (tx, ty) = b.toward_road;
                let road_side = shape
                    .faces
                    .iter()
                    .filter(|f| {
                        if tx == 0 {
                            let edge = if ty < 0 { f[1] } else { f[3] };
                            let site_edge = if ty < 0 { site[1] } else { site[3] };
                            (edge - site_edge).abs() < 1e-9
                        } else {
                            let edge = if tx < 0 { f[0] } else { f[2] };
                            let site_edge = if tx < 0 { site[0] } else { site[2] };
                            (edge - site_edge).abs() < 1e-9
                        }
                    })
                    .collect::<Vec<_>>();
                assert!(!road_side.is_empty(), "{zone:?} 道路に面した辺がない");
                // 出入りの位置を間口の方向に写した座標が、道路に面した辺の範囲に入る
                let along = |x: f64, y: f64| if tx == 0 { x } else { y };
                let near = road_side.iter().any(|f| {
                    let (lo, hi) = if tx == 0 { (f[0], f[2]) } else { (f[1], f[3]) };
                    let p = along(px, py);
                    p >= lo - 1e-9 && p <= hi + 1e-9
                });
                assert!(
                    near,
                    "{zone:?} 出入りの位置 ({px}, {py}) が切り欠きの道路側の辺の範囲に入らない：{shape:?} {b:?}"
                );
                // 出入口に落とさなかったときは、中心の位置が一致する。前面の帯なら帯の真ん中、脇の駐車場なら端の列の真ん中
                if Some(spot) != building_spot(net, b).map(|d| d.world()) {
                    let (lo, hi) = road_side.iter().fold((f64::MAX, f64::MIN), |(lo, hi), f| {
                        let (a, z) = if tx == 0 { (f[0], f[2]) } else { (f[1], f[3]) };
                        (lo.min(a), hi.max(z))
                    });
                    let p = along(px, py);
                    let ok = match layout.cut {
                        ParkingCut::FrontStrip { .. } => (p - (lo + hi) / 2.0).abs() < 0.02,
                        _ => (p - (lo + 0.5)).abs() < 0.02 || (p - (hi - 0.5)).abs() < 0.02,
                    };
                    centered += 1;
                    assert!(
                        ok,
                        "{zone:?} 出入りの位置 {p} が切り欠きの真ん中とずれる：{lo}〜{hi} {shape:?}"
                    );
                }
            }
        }
        assert!(
            centered >= 2,
            "出入りの位置の一致を確かめた建物が少ない：{centered}"
        );
        assert!(checked >= 4, "確かめた建物が少ない：{checked}");
    }

    #[test]
    fn the_lot_hugs_the_end_chosen_by_the_core() {
        let sim = town(
            ZoneKind::Industrial,
            (TilePos::new(10, 20), TilePos::new(50, 20)),
            TileRect::from_corners(TilePos::new(10, 12), TilePos::new(50, 19)),
        );
        let world = sim.world();
        let net = world.network();
        let mut far = 0;
        let mut front = 0;
        for (_, b) in world.buildings().iter() {
            let Some(shape) = lot_shape(net, b) else {
                continue;
            };
            if !matches!(b.parking_layout().cut, ParkingCut::Side { .. }) {
                continue;
            }
            // 道路は南にあり、間口は東西に並ぶ。切り欠きの端は、中核の選んだ端の外側の辺に接する
            let min = shape.faces.iter().map(|f| f[0]).fold(f64::MAX, f64::min);
            let max = shape.faces.iter().map(|f| f[2]).fold(f64::MIN, f64::max);
            let (lo, hi) = (b.area.min.x as f64, b.area.max.x as f64 + 1.0);
            match lot_end(net, b) {
                LotEnd::Front => {
                    front += 1;
                    let end_x = if b.toward_road.1 > 0 { hi } else { lo };
                    let touches = if b.toward_road.1 > 0 {
                        (max - end_x).abs() < 1e-9
                    } else {
                        (min - end_x).abs() < 1e-9
                    };
                    assert!(touches, "front の端に接しない：{shape:?} {b:?}");
                }
                LotEnd::Far => {
                    far += 1;
                    let end_x = if b.toward_road.1 > 0 { lo } else { hi };
                    let touches = if b.toward_road.1 > 0 {
                        (min - end_x).abs() < 1e-9
                    } else {
                        (max - end_x).abs() < 1e-9
                    };
                    assert!(touches, "far の端に接しない：{shape:?} {b:?}");
                }
            }
        }
        assert!(far + front > 0, "脇の駐車場の建物が建たない");
    }

    #[test]
    fn encode_lists_only_buildings_with_a_cut() {
        let sim = town(
            ZoneKind::Residential,
            (TilePos::new(10, 20), TilePos::new(30, 20)),
            TileRect::from_corners(TilePos::new(10, 12), TilePos::new(30, 19)),
        );
        let world = sim.world();
        let net = world.network();
        let data = encode(net, world.buildings().iter().map(|(_, b)| b.clone()));
        let mut i = 0;
        let mut records = 0;
        while i < data.len() {
            let faces = data[i + 4] as usize;
            let strips = data[i + 5] as usize;
            i += 6 + faces * 4 + strips * 7;
            records += 1;
        }
        assert_eq!(i, data.len());
        let cut = world
            .buildings()
            .iter()
            .filter(|(_, b)| b.parking_layout().cut != ParkingCut::None)
            .count();
        assert_eq!(records, cut);
        assert!(records > 0);
    }

    /// 実際に建った建物を雛形にして、大きさと向きと段階を変えた建物。出入口はそのままなので、形だけを確かめる。
    fn variant(
        template: &Building,
        zone: ZoneKind,
        (w, h): (i64, i64),
        toward: (i64, i64),
        level: u8,
    ) -> Building {
        let mut b = template.clone();
        b.zone = zone;
        b.level = level;
        b.toward_road = toward;
        // 道路の向きに合わせて、`front` が `along` の小さい側の端になるように敷地を置く
        let (tx, ty) = toward;
        let (ax, ay) = (-ty, tx);
        let min = TilePos::new(100, 100);
        let max = TilePos::new(min.x + w - 1, min.y + h - 1);
        b.area = TileRect::from_corners(min, max);
        // 間口の端のマスのうち、`along` の向きに進むと間口の端に届く側
        let pick = |lo: i64, hi: i64, step: i64| if step > 0 { lo } else { hi };
        let front_x = if tx == 0 {
            pick(min.x, max.x, ax)
        } else if tx > 0 {
            max.x
        } else {
            min.x
        };
        let front_y = if ty == 0 {
            pick(min.y, max.y, ay)
        } else if ty > 0 {
            max.y
        } else {
            min.y
        };
        b.front = TilePos::new(front_x, front_y);
        b
    }

    #[test]
    fn every_size_and_direction_makes_a_lot_inside_the_site() {
        let sim = town(
            ZoneKind::Residential,
            (TilePos::new(10, 20), TilePos::new(30, 20)),
            TileRect::from_corners(TilePos::new(10, 12), TilePos::new(30, 19)),
        );
        let (_, template) = sim.world().buildings().iter().next().expect("建物がない");
        let net = sim.world().network();
        let mut multi_column = 0;
        for zone in ZoneKind::ALL {
            for w in 1..=4_i64 {
                for h in 1..=4_i64 {
                    for toward in [(0, -1), (0, 1), (1, 0), (-1, 0), (1, 1)] {
                        for level in [1, 3, 5] {
                            let b = variant(template, zone, (w, h), toward, level);
                            let layout = b.parking_layout();
                            let at = format!("{zone:?} {w}×{h} {toward:?} 段階 {level}");
                            let Some(shape) = lot_shape(net, &b) else {
                                assert_eq!(layout.cut, ParkingCut::None, "{at}");
                                assert_eq!(toward, (1, 1), "{at}");
                                continue;
                            };
                            let site = [
                                b.area.min.x as f64,
                                b.area.min.y as f64,
                                b.area.max.x as f64 + 1.0,
                                b.area.max.y as f64 + 1.0,
                            ];
                            for (i, f) in shape.faces.iter().enumerate() {
                                assert!(inside(site, *f), "{at} {f:?}");
                                for g in &shape.faces[i + 1..] {
                                    assert!(!overlap(*f, *g), "{at} 長方形が重なる");
                                }
                            }
                            for s in &shape.strips {
                                assert!(shape.faces.iter().any(|f| inside(*f, s.rect)), "{at}");
                                // 枠は、車の入る側に直角な向きに並ぶ
                                let stack = if s.open.0 != 0 {
                                    s.rect[3] - s.rect[1]
                                } else {
                                    s.rect[2] - s.rect[0]
                                };
                                assert!(
                                    (stack - s.count as f64 * SLOT_WIDTH).abs() < 1e-9,
                                    "{at} 枠の並びの長さ"
                                );
                            }
                            assert_eq!(
                                shape.strips.iter().map(|s| s.count).sum::<u64>(),
                                layout.surface,
                                "{at}"
                            );
                            if matches!(layout.cut, ParkingCut::Side { columns, .. } if columns > 1)
                            {
                                multi_column += 1;
                            }
                            // 切り欠きは道路に面した辺に接する
                            let (tx, ty) = toward;
                            assert!(
                                shape.faces.iter().any(|f| {
                                    if tx == 0 {
                                        (f[if ty < 0 { 1 } else { 3 }]
                                            - site[if ty < 0 { 1 } else { 3 }])
                                        .abs()
                                            < 1e-9
                                    } else {
                                        (f[if tx < 0 { 0 } else { 2 }]
                                            - site[if tx < 0 { 0 } else { 2 }])
                                        .abs()
                                            < 1e-9
                                    }
                                }),
                                "{at} 道路に面した辺に接しない"
                            );
                        }
                    }
                }
            }
        }
        assert!(multi_column > 0, "2 列にまたがる切り欠きを確かめていない");
    }
}
