//! 建物の駐車場の形。敷地を切り欠いた所の長方形と、枠の並びを、画面に送る数に直す。
//!
//! 切り欠く形（前面の帯か脇の駐車場か、列の数、マスの数）は中核の `Building::parking_layout` が決め、
//! 間口のどちらの端に寄せるかは中核の `lot_end`（出入りの位置 `lot_spot` と同じ決まり）が決める。
//! ここはその値を、ワールドのマスの座標の長方形に置くだけで、状態は変えない。
//! 決め方は `docs/design.md` の 3.6「敷地の切り欠き方」にある。
//! 私道の敷地は、私道と家の長方形も送る。形は中核の `Building::lane_plan` が決め、3.8 にある。

use chizu_core::building::{
    Building, LANE_WIDTH_MM, ParkingCut, SLOT_DEPTH_MM, SLOT_WIDTH_MM, SLOTS_PER_TILE,
};
use chizu_core::movement::{LotEnd, SharedLot, facility_end, lot_end};
use chizu_core::network::{Network, TILE_MM};
use chizu_core::service::Facility;
use chizu_core::world::World;
use chizu_core::{ServiceKind, TilePos};

/// 1 つの枠の奥行き（マス）。5m である。
const SLOT_DEPTH: f64 = SLOT_DEPTH_MM as f64 / TILE_MM as f64;
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
    /// 私道の長方形。私道の敷地でなければ空。
    pub lanes: Vec<Rect>,
    /// 私道の脇の家の長方形。道路の側から並べる。空でなければ、建物の本体はこの家だけになる。
    pub houses: Vec<Rect>,
}

/// 敷地の座標系。間口に沿う向きの座標 u は `front` の辺を 0、道路に直角な座標 v は道路に面した辺を 0 として、
/// 建物（施設）の中へ増える。`far` なら u を折り返し、間口の `front` と反対の端を 0 とする。
struct Frame {
    front: (f64, f64),
    toward: (i64, i64),
    frontage: f64,
    far: bool,
}

impl Frame {
    fn new(front: TilePos, toward: (i64, i64), frontage: u64, far: bool) -> Self {
        Self {
            front: (front.x as f64 + 0.5, front.y as f64 + 0.5),
            toward,
            frontage: frontage as f64,
            far,
        }
    }

    fn world(&self, u: f64, v: f64) -> (f64, f64) {
        let (tx, ty) = self.toward;
        let (ax, ay) = (-ty, tx);
        let u = if self.far { self.frontage - u } else { u } - 0.5;
        let v = v - 0.5;
        (
            self.front.0 + ax as f64 * u - tx as f64 * v,
            self.front.1 + ay as f64 * u - ty as f64 * v,
        )
    }

    fn rect(&self, u0: f64, v0: f64, u1: f64, v1: f64) -> Rect {
        let (x0, y0) = self.world(u0, v0);
        let (x1, y1) = self.world(u1, v1);
        [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]
    }

    /// 局所の向き (du, dv) をワールドの向きに直す。
    fn direction(&self, du: i64, dv: i64) -> (i64, i64) {
        let (tx, ty) = self.toward;
        let (ax, ay) = (-ty, tx);
        let du = if self.far { -du } else { du };
        (ax * du - tx * dv, ay * du - ty * dv)
    }
}

/// 建物の駐車場の切り欠いた形。切り欠かない建物（斜めの道路に面した建物と、自宅の駐車場を持たない家）は `None`。
/// `seed` は街の種である。
pub fn lot_shape(net: &Network, b: &Building, seed: u64) -> Option<LotShape> {
    let layout = b.parking_layout(seed);
    if layout.cut == ParkingCut::None {
        return None;
    }
    let (tx, _) = b.toward_road;
    let w = b.area.max.x.abs_diff(b.area.min.x) + 1;
    let h = b.area.max.y.abs_diff(b.area.min.y) + 1;
    let (frontage, depth) = if tx == 0 { (w, h) } else { (h, w) };
    let frame = Frame::new(
        b.front,
        b.toward_road,
        frontage,
        lot_end(net, b, seed) == LotEnd::Far,
    );
    let depth = depth as f64;
    let rect = |u0, v0, u1, v1| frame.rect(u0, v0, u1, v1);
    let direction = |du, dv| frame.direction(du, dv);
    let mut shape = LotShape {
        faces: Vec::new(),
        strips: Vec::new(),
        lanes: Vec::new(),
        houses: Vec::new(),
    };
    match layout.cut {
        ParkingCut::None => return None,
        ParkingCut::Lane { .. } => {
            let plan = b.lane_plan().expect("私道の切り欠きなのに私道の形がない");
            let mm = |v: u64| v as f64 / TILE_MM as f64;
            shape
                .lanes
                .push(rect(0.0, 0.0, mm(LANE_WIDTH_MM), mm(plan.length)));
            let (hu0, hu1) = plan.house_u();
            let (su0, su1) = plan.slot_u();
            for p in 0..plan.houses {
                let (hv0, hv1) = plan.house_v(p);
                let (sv0, sv1) = plan.slot_v(p);
                shape.houses.push(rect(mm(hu0), mm(hv0), mm(hu1), mm(hv1)));
                let slot = rect(mm(su0), mm(sv0), mm(su1), mm(sv1));
                shape.faces.push(slot);
                shape.strips.push(Strip {
                    rect: slot,
                    open: direction(-1, 0),
                    count: 1,
                });
            }
        }
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
/// `[建物の min の x, min の y, 平面の台数, 建物の中の台数, 長方形の数, 枠の並びの数, 私道の数, 家の数,`
/// `長方形ごとに x0, y0, x1, y1, 枠の並びごとに x0, y0, x1, y1, 向きの x, 向きの y, 枠の数,`
/// `私道ごとに x0, y0, x1, y1, 家ごとに x0, y0, x1, y1]`。
/// `seed` は街の種である。
pub fn encode<'a>(
    net: &Network,
    seed: u64,
    buildings: impl Iterator<Item = &'a Building>,
) -> Vec<f64> {
    let mut out = Vec::new();
    for b in buildings {
        let Some(shape) = lot_shape(net, b, seed) else {
            continue;
        };
        let layout = b.parking_layout(seed);
        out.extend([
            b.area.min.x as f64,
            b.area.min.y as f64,
            layout.surface as f64,
            layout.indoor as f64,
            shape.faces.len() as f64,
            shape.strips.len() as f64,
            shape.lanes.len() as f64,
            shape.houses.len() as f64,
        ]);
        for f in &shape.faces {
            out.extend(*f);
        }
        for s in &shape.strips {
            out.extend(s.rect);
            out.extend([s.open.0 as f64, s.open.1 as f64, s.count as f64]);
        }
        for r in shape.lanes.iter().chain(&shape.houses) {
            out.extend(*r);
        }
    }
    out
}

/// 共同の駐車場の枠の並び。平面の駐車場は敷地の全面に、駐車場ビルは屋上に置く。
///
/// 並べ方は建物の脇の駐車場と同じで、間口に沿って 1 マスずつの列を並べ、偶数の列は通路を端の側に、
/// 奇数の列は通路を内の側に置く。枠の数は、敷地のマスの数 × 1 マスの枠の数（駐車場ビルは屋上の 1 層分）である。
/// 枠の並びは敷地の奥行きの真ん中に寄せる。共同の駐車場でなければ `None`。
pub fn facility_shape(net: &Network, f: &Facility) -> Option<LotShape> {
    if !f.kind.is_parking() {
        return None;
    }
    let (tx, _) = f.toward_road;
    let w = f.area.max.x.abs_diff(f.area.min.x) + 1;
    let h = f.area.max.y.abs_diff(f.area.min.y) + 1;
    let (frontage, depth) = if tx == 0 { (w, h) } else { (h, w) };
    let frame = Frame::new(
        f.front,
        f.toward_road,
        frontage,
        facility_end(net, f) == LotEnd::Far,
    );
    let per_column = depth * SLOTS_PER_TILE;
    let margin = (depth as f64 - per_column as f64 * SLOT_WIDTH) / 2.0;
    let mut shape = LotShape {
        faces: Vec::new(),
        strips: Vec::new(),
        lanes: Vec::new(),
        houses: Vec::new(),
    };
    for j in 0..frontage {
        let col = j as f64;
        let (u0, u1, open) = if j % 2 == 0 {
            (col + AISLE, col + 1.0, frame.direction(-1, 0))
        } else {
            (col, col + SLOT_DEPTH, frame.direction(1, 0))
        };
        shape.strips.push(Strip {
            rect: frame.rect(u0, margin, u1, margin + per_column as f64 * SLOT_WIDTH),
            open,
            count: per_column,
        });
    }
    Some(shape)
}

/// 屋上に枠を置く台数。駐車場ビルは 5 層に積むので、屋上の枠は台数の 1/5 である。平面の駐車場は全部の枠が見える。
fn visible_spaces(kind: ServiceKind) -> u32 {
    let (w, d) = kind.size();
    let surface = u32::try_from(w * d * SLOTS_PER_TILE).expect("枠の数が u32 に収まらない");
    surface.min(kind.parking_spaces())
}

/// 共同の駐車場の形。[`encode`] と同じ並びで、共同の駐車場だけを続ける。建物のように切り欠かないので、
/// 長方形は 0 個、平面の台数は見える枠の数、建物の中の台数は見えない台数（駐車場ビルの下の層）である。
/// 鍵は施設の占めるマスの min の x と y で、建物とは重ならない。
pub fn encode_facilities<'a>(
    net: &Network,
    facilities: impl Iterator<Item = &'a Facility>,
) -> Vec<f64> {
    let mut out = Vec::new();
    for f in facilities {
        let Some(shape) = facility_shape(net, f) else {
            continue;
        };
        let visible = visible_spaces(f.kind);
        out.extend([
            f.area.min.x as f64,
            f.area.min.y as f64,
            f64::from(visible),
            f64::from(f.kind.parking_spaces() - visible),
            0.0,
            shape.strips.len() as f64,
            0.0,
            0.0,
        ]);
        for s in &shape.strips {
            out.extend(s.rect);
            out.extend([s.open.0 as f64, s.open.1 as f64, s.count as f64]);
        }
    }
    out
}

/// 停めてある台数。駐車場のある建物のうち、1 台以上停めてある建物だけを、次の並びで続ける。
///
/// `[建物の min の x, min の y, 停めてある台数, そのうち建物の中に停めてある台数]`。建物の番号の順に並べ、そのあとに共同の駐車場を続ける。
/// 建物は `encode` と同じ鍵（min の x と y）で引く。台数は平面と建物の中の合計で、
/// 平面の枠を先に埋めるものとして、平面の台数を超えた分を建物の中の台数にする。
/// 共同の駐車場は、建物の中の台数を 0 とし、停まって見える台数（借りた世帯の停めた車と外来の停めた車。
/// 駐車場ビルは屋上の枠に比べて縮めた台数）を送る。鍵は施設の占めるマスの min の x と y である。
/// どの枠に置くかは画面が決める。状態は変えない。
pub fn encode_parked(world: &World) -> Vec<f64> {
    let mut out = Vec::new();
    for (id, lot) in world.trips().lots() {
        if lot.parked == 0 {
            continue;
        }
        let Some(b) = world.buildings().get(id) else {
            continue;
        };
        let layout = b.parking_layout(world.seed());
        if layout.surface + layout.indoor == 0 {
            continue;
        }
        let parked = u64::from(lot.parked);
        out.extend([
            b.area.min.x as f64,
            b.area.min.y as f64,
            parked as f64,
            parked.saturating_sub(layout.surface).min(layout.indoor) as f64,
        ]);
    }
    for f in world.facilities().iter().filter(|f| f.kind.is_parking()) {
        let n = visible_cars(world.trips().shared_lot(f.area.min), f.kind);
        if n == 0 {
            continue;
        }
        out.extend([f.area.min.x as f64, f.area.min.y as f64, n as f64, 0.0]);
    }
    out
}

/// 共同の駐車場に停まって見える車の台数。借りた世帯の停めた車と、借りていない車（外来）の停めた車の合計で、
/// 台数を超えない。借りた世帯の車が出かけているあいだは、その枠は空いて見える。
/// 駐車場ビルは屋上の枠が台数の 1/5 なので、見える台数も比べて縮める（端数は切り上げ、1 台でもあれば 1 台は見える）。
fn visible_cars(lot: SharedLot, kind: ServiceKind) -> u64 {
    let spaces = u64::from(kind.parking_spaces());
    let n = lot.parked().min(spaces);
    (n * u64::from(visible_spaces(kind))).div_ceil(spaces)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Sim;
    use chizu_core::movement::{building_spot, lot_spot};
    use chizu_core::world::growth::GROWTH_INTERVAL;
    use chizu_core::{Command, RoadKind, TilePos, TileRect, Zone, ZoneKind};

    /// 道路を引き、区画を塗って、建物が建つまで進めた街。
    fn town(zone: ZoneKind, road: (TilePos, TilePos), paint: TileRect) -> Sim {
        town_of(Zone::from(zone), road, paint)
    }

    fn town_of(zone: Zone, road: (TilePos, TilePos), paint: TileRect) -> Sim {
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
        let kinds = [Zone::Residential, Zone::ResidentialLow, Zone::Industrial];
        let mut checked = 0;
        let mut centered = 0;
        let mut lanes = 0;
        for zone in kinds {
            // 道路が南北のどちらの側にあっても確かめるため、道路の両側に区画を塗る
            let sim = town_of(
                zone,
                (TilePos::new(10, 20), TilePos::new(50, 20)),
                TileRect::from_corners(TilePos::new(10, 12), TilePos::new(50, 28)),
            );
            let world = sim.world();
            let net = world.network();
            for (_, b) in world.buildings().iter() {
                let layout = b.parking_layout(world.seed());
                let Some(shape) = lot_shape(net, b, world.seed()) else {
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
                // 地図に出すマス（lot_tiles、私道の敷地なら lane_tiles）は、切り欠いた長方形と私道にかかるマスと同じである
                let is_lane = matches!(layout.cut, ParkingCut::Lane { .. });
                lanes += usize::from(is_lane);
                let map_tiles = if is_lane {
                    chizu_core::movement::lane_tiles(net, b)
                } else {
                    chizu_core::movement::lot_tiles(net, b, world.seed())
                };
                let mut tiles: Vec<(i64, i64)> = map_tiles.iter().map(|t| (t.x, t.y)).collect();
                tiles.sort_unstable();
                let paved: Vec<Rect> = shape.faces.iter().chain(&shape.lanes).copied().collect();
                let mut covered: Vec<(i64, i64)> = (b.area.min.x..=b.area.max.x)
                    .flat_map(|x| (b.area.min.y..=b.area.max.y).map(move |y| (x, y)))
                    .filter(|(x, y)| {
                        let cell = [*x as f64, *y as f64, *x as f64 + 1.0, *y as f64 + 1.0];
                        paved.iter().any(|f| overlap(*f, cell))
                    })
                    .collect();
                covered.sort_unstable();
                assert_eq!(tiles, covered, "{zone:?} {b:?} {shape:?}");
                // 枠の数は平面の台数に一致する
                assert_eq!(
                    shape.strips.iter().map(|s| s.count).sum::<u64>(),
                    layout.surface
                );
                // 面積は、前面の帯なら枠の数ぶん、脇の駐車場ならマスの数ぶんになる
                let expect = match layout.cut {
                    ParkingCut::FrontStrip { slots } => slots as f64 * SLOT_WIDTH * SLOT_DEPTH,
                    ParkingCut::Side { tiles, .. } => tiles as f64,
                    ParkingCut::Lane { houses } => houses as f64 * SLOT_WIDTH * SLOT_DEPTH,
                    ParkingCut::None => unreachable!(),
                };
                assert!((area(&shape) - expect).abs() < 1e-9, "{zone:?} {b:?}");
                // 出入りの位置は、切り欠いた所の道路に面した辺の上にある。道路の側の辺の外で、辺の範囲の中に取る
                let spot = lot_spot(net, b, world.seed())
                    .expect("出入りの位置がない")
                    .world();
                let (px, py) = (
                    spot.x.0 as f64 / TILE_MM as f64,
                    spot.y.0 as f64 / TILE_MM as f64,
                );
                let (tx, ty) = b.toward_road;
                // 私道の敷地は、私道が道路に面する
                let road_side = if is_lane { &shape.lanes } else { &shape.faces };
                let road_side = road_side
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
                // 私道の敷地は、出入口も出入りの位置も同じ点で、どちらも私道の幅の中に入る（出入口に落とさない）
                if is_lane {
                    for (name, s) in [
                        ("lot_spot", Some(spot)),
                        ("building_spot", building_spot(net, b).map(|d| d.world())),
                    ] {
                        let s = s.expect("私道の入口がない");
                        let p = along(s.x.0 as f64 / TILE_MM as f64, s.y.0 as f64 / TILE_MM as f64);
                        assert!(
                            road_side.iter().any(|f| {
                                let (lo, hi) = if tx == 0 { (f[0], f[2]) } else { (f[1], f[3]) };
                                p >= lo - 1e-9 && p <= hi + 1e-9
                            }),
                            "{zone:?} {name} が私道の幅の外にある：{p} {shape:?} {b:?}"
                        );
                    }
                }
                // 出入口に落とさなかったときは、中心の位置が一致する。前面の帯なら帯の真ん中、脇の駐車場なら端の列の真ん中
                if Some(spot) != building_spot(net, b).map(|d| d.world()) {
                    let (lo, hi) = road_side.iter().fold((f64::MAX, f64::MIN), |(lo, hi), f| {
                        let (a, z) = if tx == 0 { (f[0], f[2]) } else { (f[1], f[3]) };
                        (lo.min(a), hi.max(z))
                    });
                    let p = along(px, py);
                    let ok = match layout.cut {
                        ParkingCut::FrontStrip { .. } | ParkingCut::Lane { .. } => {
                            (p - (lo + hi) / 2.0).abs() < 0.02
                        }
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
        assert!(lanes >= 1, "私道の敷地を確かめていない");
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
            let Some(shape) = lot_shape(net, b, world.seed()) else {
                continue;
            };
            if !matches!(b.parking_layout(world.seed()).cut, ParkingCut::Side { .. }) {
                continue;
            }
            // 道路は南にあり、間口は東西に並ぶ。切り欠きの端は、中核の選んだ端の外側の辺に接する
            let min = shape.faces.iter().map(|f| f[0]).fold(f64::MAX, f64::min);
            let max = shape.faces.iter().map(|f| f[2]).fold(f64::MIN, f64::max);
            let (lo, hi) = (b.area.min.x as f64, b.area.max.x as f64 + 1.0);
            match lot_end(net, b, world.seed()) {
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
        let data = encode(net, world.seed(), world.buildings().iter().map(|(_, b)| b));
        let mut i = 0;
        let mut records = 0;
        while i < data.len() {
            let faces = data[i + 4] as usize;
            let strips = data[i + 5] as usize;
            let rects = (data[i + 6] + data[i + 7]) as usize;
            i += 8 + faces * 4 + strips * 7 + rects * 4;
            records += 1;
        }
        assert_eq!(i, data.len());
        let cut = world
            .buildings()
            .iter()
            .filter(|(_, b)| b.parking_layout(world.seed()).cut != ParkingCut::None)
            .count();
        assert_eq!(records, cut);
        assert!(records > 0);
    }

    /// 実際に建った建物を雛形にして、大きさと向きと段階を変えた、通りに面した建物。出入口はそのままなので、形だけを確かめる。
    fn variant(
        template: &Building,
        zone: ZoneKind,
        (w, h): (i64, i64),
        toward: (i64, i64),
        level: u8,
    ) -> Building {
        let mut b = *template;
        b.form = chizu_core::building::LotForm::Street;
        b.zone = Zone::from(zone);
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
                            let layout = b.parking_layout(sim.world().seed());
                            let at = format!("{zone:?} {w}×{h} {toward:?} 段階 {level}");
                            let Some(shape) = lot_shape(net, &b, sim.world().seed()) else {
                                // 切り欠かないのは、斜めの道路に面した建物と、自宅の駐車場を持たない家だけである。
                                assert_eq!(layout.cut, ParkingCut::None, "{at}");
                                assert!(toward == (1, 1) || layout.total() == 0, "{at}");
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

    #[test]
    fn lane_lots_keep_the_lane_houses_and_slots_apart_inside_the_site() {
        let sim = town(
            ZoneKind::Residential,
            (TilePos::new(10, 20), TilePos::new(30, 20)),
            TileRect::from_corners(TilePos::new(10, 12), TilePos::new(30, 19)),
        );
        let (_, template) = sim.world().buildings().iter().next().expect("建物がない");
        let net = sim.world().network();
        let mut checked = 0;
        for depth in 4..=6_i64 {
            for toward in [(0, -1), (0, 1), (1, 0), (-1, 0)] {
                for level in [1, 2] {
                    let size = if toward.0 == 0 {
                        (2, depth)
                    } else {
                        (depth, 2)
                    };
                    let mut b = variant(template, ZoneKind::Residential, size, toward, level);
                    b.form = chizu_core::building::LotForm::Lane;
                    b.zone = Zone::ResidentialLow;
                    let at = format!("2×{depth} {toward:?} 段階 {level}");
                    let shape =
                        lot_shape(net, &b, sim.world().seed()).expect("私道の敷地に形がない");
                    let site = [
                        b.area.min.x as f64,
                        b.area.min.y as f64,
                        b.area.max.x as f64 + 1.0,
                        b.area.max.y as f64 + 1.0,
                    ];
                    assert_eq!(shape.lanes.len(), 1, "{at}");
                    assert_eq!(shape.houses.len() as u64, b.households(), "{at}");
                    let all: Vec<Rect> = shape
                        .lanes
                        .iter()
                        .chain(&shape.faces)
                        .chain(&shape.houses)
                        .copied()
                        .collect();
                    for (i, r) in all.iter().enumerate() {
                        assert!(inside(site, *r), "{at} {r:?}");
                        for q in &all[i + 1..] {
                            assert!(!overlap(*r, *q), "{at} {r:?} と {q:?} が重なる");
                        }
                    }
                    // 私道は道路に面した辺から始まり、奥の端に届かない
                    let lane = shape.lanes[0];
                    let (tx, ty) = toward;
                    let (near, back) = match (tx, ty) {
                        (0, -1) => (lane[1] - site[1], site[3] - lane[3]),
                        (0, 1) => (site[3] - lane[3], lane[1] - site[1]),
                        (-1, 0) => (lane[0] - site[0], site[2] - lane[2]),
                        _ => (site[2] - lane[2], lane[0] - site[0]),
                    };
                    assert!(near.abs() < 1e-9, "{at} 私道が道路に面しない");
                    assert!(back > 0.1, "{at} 私道が奥の端まで届く");
                    // 枠はどれも私道に接し、車の入る側が私道を向く
                    for s in &shape.strips {
                        assert_eq!(s.count, 1, "{at}");
                        let touches = (s.rect[0] - lane[2]).abs() < 1e-9
                            || (s.rect[2] - lane[0]).abs() < 1e-9
                            || (s.rect[1] - lane[3]).abs() < 1e-9
                            || (s.rect[3] - lane[1]).abs() < 1e-9;
                        assert!(touches, "{at} 枠が私道に接しない");
                        let (cx, cy) =
                            ((s.rect[0] + s.rect[2]) / 2.0, (s.rect[1] + s.rect[3]) / 2.0);
                        let (lx, ly) = ((lane[0] + lane[2]) / 2.0, (lane[1] + lane[3]) / 2.0);
                        let toward_lane = if s.open.0 != 0 {
                            (lx - cx) * s.open.0 as f64
                        } else {
                            (ly - cy) * s.open.1 as f64
                        };
                        assert!(toward_lane > 0.0, "{at} 枠が私道を向かない");
                    }
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 3 * 4 * 2);
    }

    #[test]
    fn encode_parked_lists_parked_cars_by_building_key() {
        use chizu_core::{CommandLog, World, replay};
        // 人が遊んだ街（種 0）。14 日目の昼まで再生すると、職場の建物の中の駐車場にも車が停まっている
        let log = CommandLog::decode(include_bytes!("../tests/fixtures/town.rec"))
            .expect("記録が読めない");
        let mut world = World::new(0);
        replay(&mut world, &log, 104_400);
        let data = encode_parked(&world);
        assert!(!data.is_empty(), "停めてある車が1台もない");
        assert_eq!(data.len() % 4, 0);
        let mut total = 0.0;
        let mut indoor_parked = 0.0;
        for rec in data.chunks(4) {
            let (id, b) = world
                .buildings()
                .iter()
                .find(|(_, b)| b.area.min.x as f64 == rec[0] && b.area.min.y as f64 == rec[1])
                .expect("鍵の建物がない");
            let layout = b.parking_layout(world.seed());
            assert!(layout.surface + layout.indoor > 0);
            let parked = u64::from(world.trips().lot(id).parked);
            assert_eq!(rec[2], parked as f64);
            // 建物の中の台数は、平面の枠を埋めたあとの分で、建物の中の駐車場の台数を超えない
            assert_eq!(
                rec[3],
                parked.saturating_sub(layout.surface).min(layout.indoor) as f64
            );
            assert!(rec[3] <= layout.indoor as f64);
            total += rec[2];
            indoor_parked += rec[3];
        }
        assert!(total >= 1.0);
        assert!(indoor_parked >= 1.0, "建物の中に停めた車が1台もない");
    }

    /// 共同の駐車場の枠は、敷地の中に重ならずに収まり、数が台数（駐車場ビルは屋上の 1 層分）と合う。
    #[test]
    fn shared_lot_slots_fit_inside_the_site() {
        use chizu_core::ServiceKind;
        let (mut sim, _guard) = crate::test_town::carless_town(1);
        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingLot, 24);
        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingGarage, 30);
        let world = sim.world();
        let net = world.network();
        let mut seen = 0;
        for f in world.facilities().iter().filter(|f| f.kind.is_parking()) {
            let shape = facility_shape(net, f).expect("駐車場の形がない");
            let site = [
                f.area.min.x as f64,
                f.area.min.y as f64,
                f.area.max.x as f64 + 1.0,
                f.area.max.y as f64 + 1.0,
            ];
            let slots: u64 = shape.strips.iter().map(|s| s.count).sum();
            assert_eq!(slots, u64::from(visible_spaces(f.kind)));
            assert!(shape.faces.is_empty());
            for s in &shape.strips {
                assert!(inside(site, s.rect), "{:?} {site:?}", s.rect);
                assert_eq!(s.open.0.abs() + s.open.1.abs(), 1);
            }
            for (i, a) in shape.strips.iter().enumerate() {
                for b in &shape.strips[i + 1..] {
                    assert!(!overlap(a.rect, b.rect), "{a:?} {b:?}");
                }
            }
            seen += 1;
        }
        assert_eq!(seen, 2);
        assert_eq!(visible_spaces(ServiceKind::ParkingLot), 12);
        assert_eq!(visible_spaces(ServiceKind::ParkingGarage), 48);
        // 敷地ごとに 1 件、枠の並びだけの記録が続く
        let data = encode_facilities(net, world.facilities().iter());
        assert_eq!(data.len(), (8 + 7 * 2) + (8 + 7 * 4));
    }

    /// 停まっていない共同の駐車場は記録に出ず、鍵は施設の角のマスと合う。
    #[test]
    fn idle_shared_lots_are_left_out_of_the_parked_records() {
        use chizu_core::ServiceKind;
        let (mut sim, _guard) = crate::test_town::carless_town(1);
        crate::test_town::place_lot(&mut sim, ServiceKind::ParkingLot, 24);
        sim.advance(chizu_core::TICKS_PER_DAY as u32);
        let world = sim.world();
        let lot = world.trips().shared_lot(TilePos::new(23, 8));
        assert_eq!(lot.rented, 12, "枠がすべて借りられていない");
        let rec = encode_parked(world)
            .chunks(4)
            .find(|r| r[0] == 23.0 && r[1] == 8.0)
            .map(|r| (r[2], r[3]));
        match visible_cars(lot, ServiceKind::ParkingLot) {
            0 => assert_eq!(rec, None),
            n => assert_eq!(rec, Some((n as f64, 0.0))),
        }
    }

    /// 見える台数は、停まっている車の合計（台数で頭打ち）を、見える枠の数に比べて縮めたものである。
    #[test]
    fn visible_cars_count_parked_cars_and_shrink_for_garages() {
        let lot = |rp, vp| SharedLot {
            rented: 12,
            rented_parked: rp,
            visitors: chizu_core::movement::Lot {
                parked: vp,
                reserved: 0,
            },
        };
        assert_eq!(visible_cars(lot(5, 3), ServiceKind::ParkingLot), 8);
        assert_eq!(visible_cars(lot(1, 0), ServiceKind::ParkingGarage), 1);
        assert_eq!(visible_cars(lot(5, 0), ServiceKind::ParkingGarage), 1);
        assert_eq!(visible_cars(lot(6, 0), ServiceKind::ParkingGarage), 2);
        assert_eq!(visible_cars(lot(200, 40), ServiceKind::ParkingGarage), 48);
    }
}
