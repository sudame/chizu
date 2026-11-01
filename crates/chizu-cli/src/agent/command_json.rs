//! コマンドの JSON の形。中核の `Command` と 1 対 1 に対応させ、名前は中核の型の名前にそろえる。
//!
//! 中核と JSON を行き来する match は、どれも `_` や `..` を使わずにすべての種類と欄を書く。
//! 中核にコマンドの種類や欄を足したとき、ここがコンパイルで止まって直し忘れに気づけるようにするためである。

use chizu_core::{
    ChunkCoord, Command, NodeId, RoadKind, ServiceKind, SimSpeed, TilePos, TileRect, ZoneKind,
};
use serde::{Deserialize, Serialize};

/// 中核の列挙と、JSON で文字列にする列挙を、両方向ともすべての値を書いて対応させる。
macro_rules! mirror_enum {
    ($json:ident, $core:ident { $($v:ident),+ $(,)? }) => {
        #[derive(Clone, Copy, Deserialize, Serialize)]
        enum $json {
            $($v),+
        }

        impl From<$json> for $core {
            #[deny(clippy::wildcard_enum_match_arm)]
            fn from(v: $json) -> $core {
                match v {
                    $($json::$v => $core::$v),+
                }
            }
        }

        impl From<$core> for $json {
            #[deny(clippy::wildcard_enum_match_arm)]
            fn from(v: $core) -> $json {
                match v {
                    $($core::$v => $json::$v),+
                }
            }
        }
    };
}

mirror_enum!(
    RoadKindJson,
    RoadKind {
        Small,
        Medium,
        Large
    }
);
mirror_enum!(
    ZoneJson,
    ZoneKind {
        Residential,
        Commercial,
        Industrial,
        Office
    }
);
mirror_enum!(
    ServiceJson,
    ServiceKind {
        PowerPlant,
        WaterPump
    }
);

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NodeJson {
    index: u32,
    generation: u16,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum CommandJson {
    BuildRoad {
        from: [i64; 2],
        to: [i64; 2],
        kind: RoadKindJson,
    },
    Bulldoze {
        area: [[i64; 2]; 2],
    },
    PaintZone {
        area: [[i64; 2]; 2],
        // 省いたときに区画を消してしまわないよう、null と書かせる。
        #[serde(deserialize_with = "Option::deserialize")]
        zone: Option<ZoneJson>,
    },
    PlaceBuilding {
        kind: ServiceJson,
        at: [i64; 2],
        rotation: u8,
    },
    SetSignal {
        node: NodeJson,
        on: bool,
    },
    BuyChunk {
        chunk: [i32; 2],
    },
    SetTaxRate {
        zone: ZoneJson,
        permille: u16,
    },
    SetSimSpeed {
        speed: u16,
    },
}

fn tile([x, y]: [i64; 2]) -> TilePos {
    TilePos::new(x, y)
}

fn rect([a, b]: [[i64; 2]; 2]) -> TileRect {
    TileRect::from_corners(tile(a), tile(b))
}

fn tile_json(TilePos { x, y }: TilePos) -> [i64; 2] {
    [x, y]
}

fn rect_json(TileRect { min, max }: TileRect) -> [[i64; 2]; 2] {
    [tile_json(min), tile_json(max)]
}

impl From<Command> for CommandJson {
    #[deny(clippy::wildcard_enum_match_arm)]
    fn from(c: Command) -> CommandJson {
        match c {
            Command::BuildRoad { from, to, kind } => CommandJson::BuildRoad {
                from: tile_json(from),
                to: tile_json(to),
                kind: kind.into(),
            },
            Command::Bulldoze { area } => CommandJson::Bulldoze {
                area: rect_json(area),
            },
            Command::PaintZone { area, zone } => CommandJson::PaintZone {
                area: rect_json(area),
                zone: zone.map(ZoneJson::from),
            },
            Command::PlaceBuilding { kind, at, rotation } => CommandJson::PlaceBuilding {
                kind: kind.into(),
                at: tile_json(at),
                rotation,
            },
            Command::SetSignal {
                node: NodeId { index, generation },
                on,
            } => CommandJson::SetSignal {
                node: NodeJson { index, generation },
                on,
            },
            Command::BuyChunk {
                coord: ChunkCoord { cx, cy },
            } => CommandJson::BuyChunk { chunk: [cx, cy] },
            Command::SetTaxRate { zone, permille } => CommandJson::SetTaxRate {
                zone: zone.into(),
                permille,
            },
            Command::SetSimSpeed { speed } => CommandJson::SetSimSpeed {
                speed: speed.multiplier(),
            },
        }
    }
}

impl CommandJson {
    #[deny(clippy::wildcard_enum_match_arm)]
    fn into_command(self) -> Result<Command, String> {
        Ok(match self {
            CommandJson::BuildRoad { from, to, kind } => Command::BuildRoad {
                from: tile(from),
                to: tile(to),
                kind: kind.into(),
            },
            CommandJson::Bulldoze { area } => Command::Bulldoze { area: rect(area) },
            CommandJson::PaintZone { area, zone } => Command::PaintZone {
                area: rect(area),
                zone: zone.map(ZoneKind::from),
            },
            CommandJson::PlaceBuilding { kind, at, rotation } => Command::PlaceBuilding {
                kind: kind.into(),
                at: tile(at),
                rotation,
            },
            CommandJson::SetSignal {
                node: NodeJson { index, generation },
                on,
            } => Command::SetSignal {
                node: NodeId { index, generation },
                on,
            },
            CommandJson::BuyChunk { chunk: [cx, cy] } => Command::BuyChunk {
                coord: ChunkCoord::new(cx, cy),
            },
            CommandJson::SetTaxRate { zone, permille } => Command::SetTaxRate {
                zone: zone.into(),
                permille,
            },
            CommandJson::SetSimSpeed { speed } => Command::SetSimSpeed {
                speed: SimSpeed::new(speed).ok_or_else(|| {
                    format!(
                        "速度 {speed} は {} から {} の外にある",
                        SimSpeed::NORMAL.multiplier(),
                        SimSpeed::MAX.multiplier()
                    )
                })?,
            },
        })
    }
}

/// JSON の値 1 つをコマンドに読む。形が違うか、値が範囲の外なら理由を返す。
/// 範囲の検査は [`Command::validate`] でも行うので、ここでは中核の型に収まらない値だけを断る。
pub fn parse(value: &serde_json::Value) -> Result<Command, String> {
    CommandJson::deserialize(value)
        .map_err(|e| e.to_string())?
        .into_command()
}

/// コマンドを [`parse`] で読める JSON にする。
pub fn to_json(command: Command) -> serde_json::Value {
    serde_json::to_value(CommandJson::from(command)).expect("コマンドを JSON にできない")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_every_kind() {
        let cases = [
            json!({"type":"BuildRoad","from":[0,1],"to":[5,1],"kind":"Medium"}),
            json!({"type":"Bulldoze","area":[[3,3],[1,1]]}),
            json!({"type":"PaintZone","area":[[0,0],[2,2]],"zone":"Office"}),
            json!({"type":"PaintZone","area":[[0,0],[2,2]],"zone":null}),
            json!({"type":"PlaceBuilding","kind":"WaterPump","at":[4,4],"rotation":2}),
            json!({"type":"SetSignal","node":{"index":3,"generation":1},"on":true}),
            json!({"type":"BuyChunk","chunk":[2,-1]}),
            json!({"type":"SetTaxRate","zone":"Commercial","permille":120}),
            json!({"type":"SetSimSpeed","speed":4}),
        ];
        for c in &cases {
            assert!(parse(c).is_ok(), "{c}");
        }
        assert_eq!(
            parse(&cases[1]).unwrap(),
            Command::Bulldoze {
                area: TileRect::from_corners(TilePos::new(1, 1), TilePos::new(3, 3))
            }
        );
    }

    /// すべての種類を JSON にして読み戻すと元に戻る。種類を足したら、ここにも 1 つ足す。
    #[test]
    fn round_trips_every_kind() {
        let t = TilePos::new;
        let area = TileRect::from_corners(t(-3, 2), t(4, 9));
        let commands = [
            Command::BuildRoad {
                from: t(-5, 1),
                to: t(5, 1),
                kind: RoadKind::Large,
            },
            Command::Bulldoze { area },
            Command::PaintZone {
                area,
                zone: Some(ZoneKind::Industrial),
            },
            Command::PaintZone { area, zone: None },
            Command::PlaceBuilding {
                kind: ServiceKind::PowerPlant,
                at: t(7, -8),
                rotation: 3,
            },
            Command::SetSignal {
                node: NodeId {
                    index: 9,
                    generation: 2,
                },
                on: false,
            },
            Command::BuyChunk {
                coord: ChunkCoord::new(-2, 3),
            },
            Command::SetTaxRate {
                zone: ZoneKind::Residential,
                permille: 300,
            },
            Command::SetSimSpeed {
                speed: SimSpeed::MAX,
            },
        ];
        // 種類ごとに 1 つはあることを、ワイルドカードなしの match で確かめる。
        #[deny(clippy::wildcard_enum_match_arm)]
        fn kind(c: &Command) -> usize {
            match c {
                Command::BuildRoad { .. } => 0,
                Command::Bulldoze { .. } => 1,
                Command::PaintZone { .. } => 2,
                Command::PlaceBuilding { .. } => 3,
                Command::SetSignal { .. } => 4,
                Command::BuyChunk { .. } => 5,
                Command::SetTaxRate { .. } => 6,
                Command::SetSimSpeed { .. } => 7,
            }
        }
        let mut seen = [false; 8];
        for c in commands {
            seen[kind(&c)] = true;
            let json = to_json(c);
            assert_eq!(parse(&json), Ok(c), "{json}");
        }
        assert!(seen.iter().all(|&s| s), "{seen:?}");
        for z in ZoneKind::ALL {
            assert_eq!(ZoneKind::from(ZoneJson::from(z)), z);
        }
        for k in ServiceKind::ALL {
            assert_eq!(ServiceKind::from(ServiceJson::from(k)), k);
        }
        for r in [RoadKind::Small, RoadKind::Medium, RoadKind::Large] {
            assert_eq!(RoadKind::from(RoadKindJson::from(r)), r);
        }
    }

    #[test]
    fn rejects_bad_shapes() {
        let cases = [
            json!({"type":"Fly"}),
            json!({"type":"BuildRoad","from":[0,1],"to":[5,1]}),
            json!({"type":"BuildRoad","from":[0,1],"to":[5,1],"kind":"Small","extra":1}),
            json!({"type":"PaintZone","area":[[0,0],[2,2]]}),
            json!({"type":"SetSimSpeed","speed":0}),
            json!({"type":"SetTaxRate","zone":"Commercial","permille":-1}),
        ];
        for c in &cases {
            assert!(parse(c).is_err(), "{c}");
        }
    }
}
