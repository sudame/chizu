//! 画面から届いたコマンドのオブジェクトを `Command` に直す。
//!
//! JS の値を読む部分は lib.rs にあり、ここには読んだ値を確かめる部分だけを置く。
//! ネイティブの `cargo test` で確かめられるようにするためである。

use core::fmt;

use chizu_core::{ChunkCoord, Command, RoadKind, SimSpeed, TilePos, TileRect, ZoneKind};

/// オブジェクトのフィールドを読んだ値。
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Missing,
    Number(f64),
    Text(String),
    /// 数でも文字列でもない値。
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
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::NotObject => write!(f, "コマンドがオブジェクトでない"),
            InputError::MissingKind => write!(f, "コマンドに文字列の kind がない"),
            InputError::UnknownKind(kind) => write!(f, "知らないコマンド {kind}"),
            InputError::BadSpeed => write!(f, "速度は 1 から 48 までの整数で指定する"),
            InputError::BadInteger(name) => write!(f, "{name} が範囲の中の整数でない"),
            InputError::BadRoadKind => write!(f, "road は Small、Medium、Large のどれかで指定する"),
            InputError::BadZone => write!(
                f,
                "zone は Residential、Commercial、Industrial、Office、Clear のどれかで指定する"
            ),
        }
    }
}

impl std::error::Error for InputError {}

/// `is_object` は入力がオブジェクトか、`field` は名前を渡すとそのフィールドを読む関数である。
pub fn parse(is_object: bool, field: impl Fn(&str) -> Field) -> Result<Command, InputError> {
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
        "BuildRoad" => {
            let kind = match field("road") {
                Field::Text(t) if t == "Small" => RoadKind::Small,
                Field::Text(t) if t == "Medium" => RoadKind::Medium,
                Field::Text(t) if t == "Large" => RoadKind::Large,
                _ => return Err(InputError::BadRoadKind),
            };
            Ok(Command::BuildRoad {
                from: tile(&field, "fromX", "fromY")?,
                to: tile(&field, "toX", "toY")?,
                kind,
            })
        }
        "PaintZone" => {
            let zone = match field("zone") {
                Field::Text(t) => match t.as_str() {
                    "Residential" => Some(ZoneKind::Residential),
                    "Commercial" => Some(ZoneKind::Commercial),
                    "Industrial" => Some(ZoneKind::Industrial),
                    "Office" => Some(ZoneKind::Office),
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
        "Bulldoze" => Ok(Command::Bulldoze {
            area: TileRect {
                min: tile(&field, "minX", "minY")?,
                max: tile(&field, "maxX", "maxY")?,
            },
        }),
        "BuyChunk" => {
            let coord = |name: &'static str| {
                integer(&field, name)
                    .and_then(|v| i32::try_from(v).map_err(|_| InputError::BadInteger(name)))
            };
            Ok(Command::BuyChunk {
                coord: ChunkCoord::new(coord("cx")?, coord("cy")?),
            })
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

    fn object(fields: &[(&str, Field)]) -> Result<Command, InputError> {
        parse(true, |name| {
            fields
                .iter()
                .find(|(n, _)| *n == name)
                .map_or(Field::Missing, |(_, v)| v.clone())
        })
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
            speed(Field::Number(48.0)),
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
            parse(false, |_| Field::Text("SetSimSpeed".into())),
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
            Field::Number(49.0),
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
                zone: Some(ZoneKind::Office)
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
