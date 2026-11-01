//! 画面から届いたコマンドのオブジェクトを `Command` に直す。
//!
//! JS の値を読む部分は lib.rs にあり、ここには読んだ値を確かめる部分だけを置く。
//! ネイティブの `cargo test` で確かめられるようにするためである。

use core::fmt;

use chizu_core::{Command, SimSpeed};

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
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InputError::NotObject => write!(f, "コマンドがオブジェクトでない"),
            InputError::MissingKind => write!(f, "コマンドに文字列の kind がない"),
            InputError::UnknownKind(kind) => write!(f, "知らないコマンド {kind}"),
            InputError::BadSpeed => write!(f, "速度は 1 から 48 までの整数で指定する"),
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
        _ => Err(InputError::UnknownKind(kind)),
    }
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
