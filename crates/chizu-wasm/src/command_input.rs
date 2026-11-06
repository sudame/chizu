//! 画面から届いたコマンドのオブジェクトを `Command` に直す。
//!
//! JS の値を読む部分は lib.rs にあり、ここには読んだ値を確かめる部分だけを置く。
//! ネイティブの `cargo test` で確かめられるようにするためである。

use core::fmt;

use chizu_core::rail::{LineId, StationId};
use chizu_core::{
    ChunkCoord, Command, Heading, NodeId, RoadKind, ServiceKind, SimSpeed, TilePos, TileRect, Zone,
    ZoneKind,
};

/// オブジェクトのフィールドを読んだ値。
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Missing,
    Number(f64),
    Text(String),
    Bool(bool),
    /// 数でも文字列でも真偽値でもない値。
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputError {
    NotObject,
    MissingKind,
    UnknownKind(String),
    BadSpeed,
    /// 整数で読めないフィールド。
    BadInteger(&'static str),
    BadRoadKind,
    BadZone,
    /// 税率をかける区画の種類でない（消す指定の Clear は取れない）。
    BadTaxZone,
    BadService,
    /// 真偽値で読めないフィールド。
    BadBool(&'static str),
    /// 信号や横断歩道を変えるマスにノードがない。
    NoNode,
    /// 腕の向き（dx、dy）が 8 方向の 1 歩でない。
    BadArm,
    /// 路線に入れる駅を指したマスに駅がない。
    NoStation,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::NotObject => write!(f, "コマンドがオブジェクトでない"),
            InputError::MissingKind => write!(f, "コマンドに文字列の kind がない"),
            InputError::UnknownKind(kind) => write!(f, "知らないコマンド {kind}"),
            InputError::BadSpeed => write!(
                f,
                "速度は {} から {} までの整数で指定する",
                SimSpeed::NORMAL.multiplier(),
                SimSpeed::MAX.multiplier()
            ),
            InputError::BadInteger(name) => write!(f, "{name} が範囲の中の整数でない"),
            InputError::BadRoadKind => write!(f, "road は Small、Medium、Large のどれかで指定する"),
            InputError::BadService => write!(
                f,
                "service は PowerPlant、WaterPump、ParkingLot、ParkingGarage、Station のどれかで指定する"
            ),
            InputError::BadBool(name) => write!(f, "{name} が真偽値でない"),
            InputError::NoNode => write!(f, "そのマスに交差点がない"),
            InputError::BadArm => write!(f, "腕の向き dx、dy は -1、0、1 の 8 方向で指定する"),
            InputError::NoStation => write!(f, "そのマスに駅がない"),
            InputError::BadTaxZone => write!(
                f,
                "zone は Residential、Commercial、Industrial、Office のどれかで指定する"
            ),
            InputError::BadZone => write!(
                f,
                "zone は Residential、Commercial、Industrial、Office、Clear のどれかで指定する"
            ),
        }
    }
}

impl std::error::Error for InputError {}

/// `is_object` は入力がオブジェクトか、`field` は名前を渡すとそのフィールドを読む関数である。
/// `node_at` はマスにあるノードを引く関数で、マスで指す信号のコマンドをノードの ID に直すのに使う。
/// `station_at` は敷地にマスを含む駅を引く関数で、マスで指す路線のコマンドを駅の ID に直すのに使う。
pub fn parse(
    is_object: bool,
    field: impl Fn(&str) -> Field,
    node_at: impl Fn(TilePos) -> Option<NodeId>,
    station_at: impl Fn(TilePos) -> Option<StationId>,
) -> Result<Command, InputError> {
    if !is_object {
        return Err(InputError::NotObject);
    }
    let Field::Text(kind) = field("kind") else {
        return Err(InputError::MissingKind);
    };
    match kind.as_str() {
        "SetSimSpeed" => {
            let speed = match field("speed") {
                Field::Number(n) => integer_u16(n).and_then(SimSpeed::new),
                _ => None,
            }
            .ok_or(InputError::BadSpeed)?;
            Ok(Command::SetSimSpeed { speed })
        }
        "BuildRoad" | "UpgradeRoad" => {
            let road = match field("road") {
                Field::Text(t) if t == "Small" => RoadKind::Small,
                Field::Text(t) if t == "Medium" => RoadKind::Medium,
                Field::Text(t) if t == "Large" => RoadKind::Large,
                _ => return Err(InputError::BadRoadKind),
            };
            let (from, to) = (tile(&field, "fromX", "fromY")?, tile(&field, "toX", "toY")?);
            Ok(if kind == "BuildRoad" {
                Command::BuildRoad {
                    from,
                    to,
                    kind: road,
                }
            } else {
                Command::UpgradeRoad {
                    from,
                    to,
                    kind: road,
                }
            })
        }
        "PaintZone" => {
            let zone = match field("zone") {
                Field::Text(t) => match t.as_str() {
                    "Residential" => Some(Zone::Residential),
                    "ResidentialLow" => Some(Zone::ResidentialLow),
                    "Commercial" => Some(Zone::Commercial),
                    "Industrial" => Some(Zone::Industrial),
                    "Office" => Some(Zone::Office),
                    "Clear" => None,
                    _ => return Err(InputError::BadZone),
                },
                _ => return Err(InputError::BadZone),
            };
            Ok(Command::PaintZone {
                area: TileRect {
                    min: tile(&field, "minX", "minY")?,
                    max: tile(&field, "maxX", "maxY")?,
                },
                zone,
            })
        }
        "SetTaxRate" => {
            let zone = match field("zone") {
                Field::Text(t) => match t.as_str() {
                    "Residential" => ZoneKind::Residential,
                    "Commercial" => ZoneKind::Commercial,
                    "Industrial" => ZoneKind::Industrial,
                    "Office" => ZoneKind::Office,
                    _ => return Err(InputError::BadTaxZone),
                },
                _ => return Err(InputError::BadTaxZone),
            };
            // 上限の検査は Command::validate に任せる
            let permille = integer(&field, "permille")
                .and_then(|v| u16::try_from(v).map_err(|_| InputError::BadInteger("permille")))?;
            Ok(Command::SetTaxRate { zone, permille })
        }
        "SetParkingFee" => {
            // 上限の検査は Command::validate に任せる
            let fee = integer(&field, "fee")
                .and_then(|v| u16::try_from(v).map_err(|_| InputError::BadInteger("fee")))?;
            Ok(Command::SetParkingFee { fee })
        }
        "Bulldoze" => Ok(Command::Bulldoze {
            area: TileRect {
                min: tile(&field, "minX", "minY")?,
                max: tile(&field, "maxX", "maxY")?,
            },
        }),
        "BuildRail" => Ok(Command::BuildRail {
            from: tile(&field, "fromX", "fromY")?,
            to: tile(&field, "toX", "toY")?,
        }),
        "CreateLine" => {
            let station = |x, y| -> Result<StationId, InputError> {
                station_at(tile(&field, x, y)?).ok_or(InputError::NoStation)
            };
            Ok(Command::CreateLine {
                from: station("fromX", "fromY")?,
                to: station("toX", "toY")?,
            })
        }
        "ExtendLine" => Ok(Command::ExtendLine {
            line: line_id(&field)?,
            station: station_at(tile(&field, "atX", "atY")?).ok_or(InputError::NoStation)?,
        }),
        "InsertLineStation" => {
            let after_index = integer(&field, "afterIndex")
                .and_then(|v| u8::try_from(v).map_err(|_| InputError::BadInteger("afterIndex")))?;
            Ok(Command::InsertLineStation {
                line: line_id(&field)?,
                after_index,
                station: station_at(tile(&field, "atX", "atY")?).ok_or(InputError::NoStation)?,
            })
        }
        "RemoveLine" => Ok(Command::RemoveLine {
            line: line_id(&field)?,
        }),
        "SetLineTrains" => {
            // 上限の検査は Command::validate に任せる
            let trains = integer(&field, "trains")
                .and_then(|v| u8::try_from(v).map_err(|_| InputError::BadInteger("trains")))?;
            Ok(Command::SetLineTrains {
                line: line_id(&field)?,
                trains,
            })
        }
        "PlaceBuilding" => {
            let kind = match field("service") {
                Field::Text(t) if t == "PowerPlant" => ServiceKind::PowerPlant,
                Field::Text(t) if t == "WaterPump" => ServiceKind::WaterPump,
                Field::Text(t) if t == "ParkingLot" => ServiceKind::ParkingLot,
                Field::Text(t) if t == "ParkingGarage" => ServiceKind::ParkingGarage,
                Field::Text(t) if t == "Station" => ServiceKind::Station,
                _ => return Err(InputError::BadService),
            };
            let rotation = integer(&field, "rotation")
                .and_then(|v| u8::try_from(v).map_err(|_| InputError::BadInteger("rotation")))?;
            Ok(Command::PlaceBuilding {
                kind,
                at: tile(&field, "atX", "atY")?,
                rotation,
            })
        }
        "BuyChunk" => {
            let coord = |name: &'static str| {
                integer(&field, name)
                    .and_then(|v| i32::try_from(v).map_err(|_| InputError::BadInteger(name)))
            };
            Ok(Command::BuyChunk {
                coord: ChunkCoord::new(coord("cx")?, coord("cy")?),
            })
        }
        "SetSignal" => {
            let on = match field("on") {
                Field::Bool(b) => b,
                _ => return Err(InputError::BadBool("on")),
            };
            let node = node_at(tile(&field, "x", "y")?).ok_or(InputError::NoNode)?;
            Ok(Command::SetSignal { node, on })
        }
        "SetCrosswalk" => {
            let on = match field("on") {
                Field::Bool(b) => b,
                _ => return Err(InputError::BadBool("on")),
            };
            let step = (integer(&field, "dx")?, integer(&field, "dy")?);
            let arm = Heading::of_step(step).ok_or(InputError::BadArm)?;
            let node = node_at(tile(&field, "x", "y")?).ok_or(InputError::NoNode)?;
            Ok(Command::SetCrosswalk { node, arm, on })
        }
        _ => Err(InputError::UnknownKind(kind)),
    }
}

/// JS の数で誤差なく表せる整数の上限。
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

fn integer(field: &impl Fn(&str) -> Field, name: &'static str) -> Result<i64, InputError> {
    match field(name) {
        Field::Number(v) if v.fract() == 0.0 && v.abs() <= MAX_SAFE_INTEGER => Ok(v as i64),
        _ => Err(InputError::BadInteger(name)),
    }
}

/// 路線の ID。画面は一覧に添えた添え字と世代をそのまま返す。
fn line_id(field: &impl Fn(&str) -> Field) -> Result<LineId, InputError> {
    let index = integer(field, "lineIndex")
        .and_then(|v| u32::try_from(v).map_err(|_| InputError::BadInteger("lineIndex")))?;
    let generation = integer(field, "lineGeneration")
        .and_then(|v| u16::try_from(v).map_err(|_| InputError::BadInteger("lineGeneration")))?;
    Ok(LineId { index, generation })
}

fn tile(
    field: &impl Fn(&str) -> Field,
    x: &'static str,
    y: &'static str,
) -> Result<TilePos, InputError> {
    Ok(TilePos::new(integer(field, x)?, integer(field, y)?))
}

fn integer_u16(v: f64) -> Option<u16> {
    (v.fract() == 0.0 && (0.0..=f64::from(u16::MAX)).contains(&v)).then_some(v as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 敷地のマス (1, 1) と (30, 30) にだけ駅がある。
    fn station_at(t: TilePos) -> Option<StationId> {
        let index = match (t.x, t.y) {
            (1, 1) => 0,
            (30, 30) => 1,
            _ => return None,
        };
        Some(StationId {
            index,
            generation: 0,
        })
    }

    fn object(fields: &[(&str, Field)]) -> Result<Command, InputError> {
        parse(
            true,
            |name| {
                fields
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or(Field::Missing, |(_, v)| v.clone())
            },
            |t| {
                (t == TilePos::new(3, 4)).then_some(NodeId {
                    index: 7,
                    generation: 2,
                })
            },
            station_at,
        )
    }

    #[test]
    fn set_signal_finds_the_node_on_the_tile() {
        let signal = |x: f64, on: Field| {
            object(&[
                ("kind", Field::Text("SetSignal".into())),
                ("x", Field::Number(x)),
                ("y", Field::Number(4.0)),
                ("on", on),
            ])
        };
        assert_eq!(
            signal(3.0, Field::Bool(false)),
            Ok(Command::SetSignal {
                node: NodeId {
                    index: 7,
                    generation: 2
                },
                on: false
            })
        );
        assert_eq!(signal(5.0, Field::Bool(true)), Err(InputError::NoNode));
        assert_eq!(
            signal(3.0, Field::Number(1.0)),
            Err(InputError::BadBool("on"))
        );
    }

    #[test]
    fn set_crosswalk_finds_the_node_and_the_arm() {
        let crosswalk = |x: f64, dx: f64, dy: f64| {
            object(&[
                ("kind", Field::Text("SetCrosswalk".into())),
                ("x", Field::Number(x)),
                ("y", Field::Number(4.0)),
                ("dx", Field::Number(dx)),
                ("dy", Field::Number(dy)),
                ("on", Field::Bool(false)),
            ])
        };
        assert_eq!(
            crosswalk(3.0, -1.0, 1.0),
            Ok(Command::SetCrosswalk {
                node: NodeId {
                    index: 7,
                    generation: 2
                },
                arm: Heading::SW,
                on: false
            })
        );
        assert_eq!(crosswalk(5.0, 1.0, 0.0), Err(InputError::NoNode));
        assert_eq!(crosswalk(3.0, 0.0, 0.0), Err(InputError::BadArm));
        assert_eq!(crosswalk(3.0, 2.0, 0.0), Err(InputError::BadArm));
    }

    #[test]
    fn line_commands_find_the_stations_by_tile() {
        let num = |v: f64| Field::Number(v);
        let create = |to_x: f64| {
            object(&[
                ("kind", Field::Text("CreateLine".into())),
                ("fromX", num(1.0)),
                ("fromY", num(1.0)),
                ("toX", num(to_x)),
                ("toY", num(30.0)),
            ])
        };
        assert_eq!(
            create(30.0),
            Ok(Command::CreateLine {
                from: station_at(TilePos::new(1, 1)).unwrap(),
                to: station_at(TilePos::new(30, 30)).unwrap(),
            })
        );
        assert_eq!(create(5.0), Err(InputError::NoStation));
        let line = |kind: &str, extra: &[(&str, Field)]| {
            let mut fields = vec![
                ("kind", Field::Text(kind.into())),
                ("lineIndex", num(4.0)),
                ("lineGeneration", num(1.0)),
            ];
            fields.extend(extra.iter().cloned());
            object(&fields)
        };
        let id = LineId {
            index: 4,
            generation: 1,
        };
        assert_eq!(
            line("RemoveLine", &[]),
            Ok(Command::RemoveLine { line: id })
        );
        assert_eq!(
            line("SetLineTrains", &[("trains", num(3.0))]),
            Ok(Command::SetLineTrains {
                line: id,
                trains: 3
            })
        );
        assert_eq!(
            line("SetLineTrains", &[("trains", num(300.0))]),
            Err(InputError::BadInteger("trains"))
        );
        assert_eq!(
            line("ExtendLine", &[("atX", num(30.0)), ("atY", num(30.0))]),
            Ok(Command::ExtendLine {
                line: id,
                station: station_at(TilePos::new(30, 30)).unwrap(),
            })
        );
        assert_eq!(
            line(
                "InsertLineStation",
                &[
                    ("afterIndex", num(1.0)),
                    ("atX", num(30.0)),
                    ("atY", num(30.0))
                ]
            ),
            Ok(Command::InsertLineStation {
                line: id,
                after_index: 1,
                station: station_at(TilePos::new(30, 30)).unwrap(),
            })
        );
        assert_eq!(
            line(
                "InsertLineStation",
                &[
                    ("afterIndex", num(300.0)),
                    ("atX", num(30.0)),
                    ("atY", num(30.0))
                ]
            ),
            Err(InputError::BadInteger("afterIndex"))
        );
        assert_eq!(
            line("ExtendLine", &[("atX", num(2.0)), ("atY", num(30.0))]),
            Err(InputError::NoStation)
        );
        assert_eq!(
            object(&[("kind", Field::Text("RemoveLine".into()))]),
            Err(InputError::BadInteger("lineIndex"))
        );
    }

    fn speed(value: Field) -> Result<Command, InputError> {
        object(&[
            ("kind", Field::Text("SetSimSpeed".into())),
            ("speed", value),
        ])
    }

    #[test]
    fn accepts_set_sim_speed() {
        assert_eq!(
            speed(Field::Number(288.0)),
            Ok(Command::SetSimSpeed {
                speed: SimSpeed::MAX
            })
        );
        assert_eq!(
            speed(Field::Number(1.0)),
            Ok(Command::SetSimSpeed {
                speed: SimSpeed::NORMAL
            })
        );
    }

    #[test]
    fn rejects_non_objects() {
        assert_eq!(
            parse(
                false,
                |_| Field::Text("SetSimSpeed".into()),
                |_| None,
                |_| None
            ),
            Err(InputError::NotObject)
        );
    }

    #[test]
    fn rejects_missing_or_unknown_kind() {
        assert_eq!(object(&[]), Err(InputError::MissingKind));
        assert_eq!(
            object(&[("kind", Field::Number(8.0))]),
            Err(InputError::MissingKind)
        );
        assert_eq!(
            object(&[("kind", Field::Text("Explode".into()))]),
            Err(InputError::UnknownKind("Explode".into()))
        );
    }

    #[test]
    fn rejects_bad_speeds() {
        for bad in [
            Field::Missing,
            Field::Text("4".into()),
            Field::Other,
            Field::Number(0.0),
            Field::Number(289.0),
            Field::Number(4.5),
            Field::Number(-1.0),
            Field::Number(f64::NAN),
            Field::Number(f64::INFINITY),
        ] {
            assert_eq!(speed(bad.clone()), Err(InputError::BadSpeed), "{bad:?}");
        }
    }

    fn numbers(kind: &str, fields: &[(&str, f64)]) -> Result<Command, InputError> {
        let mut all = vec![("kind", Field::Text(kind.into()))];
        all.extend(fields.iter().map(|(n, v)| (*n, Field::Number(*v))));
        object(&all)
    }

    #[test]
    fn accepts_build_road() {
        let mut fields = vec![
            ("kind", Field::Text("BuildRoad".into())),
            ("road", Field::Text("Medium".into())),
        ];
        for (n, v) in [("fromX", -3.0), ("fromY", 4.0), ("toX", 5.0), ("toY", 4.0)] {
            fields.push((n, Field::Number(v)));
        }
        assert_eq!(
            object(&fields),
            Ok(Command::BuildRoad {
                from: TilePos::new(-3, 4),
                to: TilePos::new(5, 4),
                kind: RoadKind::Medium,
            })
        );
        fields[0].1 = Field::Text("UpgradeRoad".into());
        assert_eq!(
            object(&fields),
            Ok(Command::UpgradeRoad {
                from: TilePos::new(-3, 4),
                to: TilePos::new(5, 4),
                kind: RoadKind::Medium,
            })
        );
        fields[1].1 = Field::Text("Huge".into());
        assert_eq!(object(&fields), Err(InputError::BadRoadKind));
    }

    #[test]
    fn accepts_bulldoze_and_buy_chunk() {
        let rect = [("minX", 0.0), ("minY", -2.0), ("maxX", 3.0), ("maxY", 1.0)];
        assert_eq!(
            numbers("Bulldoze", &rect),
            Ok(Command::Bulldoze {
                area: TileRect {
                    min: TilePos::new(0, -2),
                    max: TilePos::new(3, 1),
                }
            })
        );
        assert_eq!(
            numbers("BuyChunk", &[("cx", 2.0), ("cy", -1.0)]),
            Ok(Command::BuyChunk {
                coord: ChunkCoord::new(2, -1)
            })
        );
    }

    #[test]
    fn accepts_paint_zone() {
        let rect = [
            ("minX", Field::Number(1.0)),
            ("minY", Field::Number(2.0)),
            ("maxX", Field::Number(3.0)),
            ("maxY", Field::Number(4.0)),
        ];
        let paint = |zone: Field| {
            let mut fields = vec![("kind", Field::Text("PaintZone".into())), ("zone", zone)];
            fields.extend(rect.iter().cloned());
            object(&fields)
        };
        let area = TileRect {
            min: TilePos::new(1, 2),
            max: TilePos::new(3, 4),
        };
        assert_eq!(
            paint(Field::Text("Office".into())),
            Ok(Command::PaintZone {
                area,
                zone: Some(Zone::Office)
            })
        );
        assert_eq!(
            paint(Field::Text("ResidentialLow".into())),
            Ok(Command::PaintZone {
                area,
                zone: Some(Zone::ResidentialLow)
            })
        );
        assert_eq!(
            paint(Field::Text("Residential".into())),
            Ok(Command::PaintZone {
                area,
                zone: Some(Zone::Residential)
            })
        );
        assert_eq!(
            paint(Field::Text("Clear".into())),
            Ok(Command::PaintZone { area, zone: None })
        );
        assert_eq!(paint(Field::Text("Park".into())), Err(InputError::BadZone));
        assert_eq!(paint(Field::Missing), Err(InputError::BadZone));
    }

    #[test]
    fn accepts_build_rail() {
        let rail = object(&[
            ("kind", Field::Text("BuildRail".into())),
            ("fromX", Field::Number(1.0)),
            ("fromY", Field::Number(2.0)),
            ("toX", Field::Number(9.0)),
            ("toY", Field::Number(2.0)),
        ]);
        assert_eq!(
            rail,
            Ok(Command::BuildRail {
                from: TilePos::new(1, 2),
                to: TilePos::new(9, 2),
            })
        );
        assert_eq!(
            object(&[("kind", Field::Text("BuildRail".into()))]),
            Err(InputError::BadInteger("fromX"))
        );
    }

    #[test]
    fn accepts_place_building() {
        let place = |service: &str, rotation: f64| {
            object(&[
                ("kind", Field::Text("PlaceBuilding".into())),
                ("service", Field::Text(service.into())),
                ("atX", Field::Number(-3.0)),
                ("atY", Field::Number(7.0)),
                ("rotation", Field::Number(rotation)),
            ])
        };
        assert_eq!(
            place("WaterPump", 2.0),
            Ok(Command::PlaceBuilding {
                kind: ServiceKind::WaterPump,
                at: TilePos::new(-3, 7),
                rotation: 2,
            })
        );
        assert_eq!(
            place("Station", 1.0),
            Ok(Command::PlaceBuilding {
                kind: ServiceKind::Station,
                at: TilePos::new(-3, 7),
                rotation: 1,
            })
        );
        assert_eq!(place("Park", 0.0), Err(InputError::BadService));
        assert_eq!(
            place("PowerPlant", 300.0),
            Err(InputError::BadInteger("rotation"))
        );
    }

    #[test]
    fn accepts_set_tax_rate() {
        let tax = |zone: Field, permille: Field| {
            object(&[
                ("kind", Field::Text("SetTaxRate".into())),
                ("zone", zone),
                ("permille", permille),
            ])
        };
        assert_eq!(
            tax(Field::Text("Commercial".into()), Field::Number(120.0)),
            Ok(Command::SetTaxRate {
                zone: ZoneKind::Commercial,
                permille: 120
            })
        );
        // 区画を消す指定や知らない種類は、税率の対象にならない
        for zone in [
            Field::Text("Clear".into()),
            Field::Text("Park".into()),
            Field::Missing,
        ] {
            assert_eq!(tax(zone, Field::Number(100.0)), Err(InputError::BadTaxZone));
        }
        for bad in [
            Field::Number(-1.0),
            Field::Number(70_000.0),
            Field::Number(0.5),
            Field::Missing,
        ] {
            assert_eq!(
                tax(Field::Text("Office".into()), bad),
                Err(InputError::BadInteger("permille"))
            );
        }
    }

    #[test]
    fn accepts_set_parking_fee() {
        let fee =
            |fee: Field| object(&[("kind", Field::Text("SetParkingFee".into())), ("fee", fee)]);
        assert_eq!(
            fee(Field::Number(30.0)),
            Ok(Command::SetParkingFee { fee: 30 })
        );
        for bad in [
            Field::Number(-1.0),
            Field::Number(70_000.0),
            Field::Number(0.5),
            Field::Missing,
        ] {
            assert_eq!(fee(bad), Err(InputError::BadInteger("fee")));
        }
    }

    #[test]
    fn rejects_bad_coordinates() {
        assert_eq!(
            numbers(
                "Bulldoze",
                &[("minX", 0.5), ("minY", 0.0), ("maxX", 1.0), ("maxY", 1.0)]
            ),
            Err(InputError::BadInteger("minX"))
        );
        assert_eq!(
            numbers("Bulldoze", &[("minX", 0.0), ("minY", 0.0), ("maxX", 1e300)]),
            Err(InputError::BadInteger("maxX"))
        );
        assert_eq!(
            numbers("BuyChunk", &[("cx", 3e9), ("cy", 0.0)]),
            Err(InputError::BadInteger("cx"))
        );
    }

    #[test]
    fn integer_u16_rejects_fractions_and_out_of_range() {
        assert_eq!(integer_u16(4.0), Some(4));
        assert_eq!(integer_u16(4.5), None);
        assert_eq!(integer_u16(-1.0), None);
        assert_eq!(integer_u16(65_536.0), None);
        assert_eq!(integer_u16(f64::NAN), None);
        assert_eq!(integer_u16(f64::INFINITY), None);
    }
}
