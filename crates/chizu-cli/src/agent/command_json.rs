//! コマンドの JSON の形。中核の `Command` と 1 対 1 に対応させ、名前は中核の型の名前にそろえる。
//!
//! 中核と JSON を行き来する match は、どれも `_` や `..` を使わずにすべての種類と欄を書く。
//! 中核にコマンドの種類や欄を足したとき、ここがコンパイルで止まって直し忘れに気づけるようにするためである。

use chizu_core::{
    ChunkCoord, Command, Heading, MAX_PARKING_FEE, MAX_TAX_PERMILLE, NodeId, RoadKind, ServiceKind,
    SimSpeed, TilePos, TileRect, Zone, ZoneKind,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
// 塗る区画。`Residential` は高密度の住宅で、低密度は `ResidentialLow` である。税率の区画（`ZoneJson`）には低密度はない。
mirror_enum!(
    PaintZoneJson,
    Zone {
        Residential,
        Commercial,
        Industrial,
        Office,
        ResidentialLow
    }
);
mirror_enum!(
    HeadingJson,
    Heading {
        E,
        SE,
        S,
        SW,
        W,
        NW,
        N,
        NE
    }
);
mirror_enum!(
    ServiceJson,
    ServiceKind {
        PowerPlant,
        WaterPump,
        ParkingLot,
        ParkingGarage,
        Station
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
        zone: Option<PaintZoneJson>,
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
    SetCrosswalk {
        node: NodeJson,
        arm: HeadingJson,
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
    SetParkingFee {
        fee: u16,
    },
    UpgradeRoad {
        from: [i64; 2],
        to: [i64; 2],
        kind: RoadKindJson,
    },
    BuildRail {
        from: [i64; 2],
        to: [i64; 2],
    },
}

/// `SetSignal` と `SetCrosswalk` をマス `at` で書いたものを、そのマスのノードの ID で書いた形に直す。
/// ほかのコマンドはそのまま返す。マスで指せるのは act の入力だけで、記録と出力はノードの ID の形にそろえる。
pub fn resolve_node_at(
    value: &Value,
    node_at: impl Fn(TilePos) -> Option<NodeId>,
) -> Result<Value, String> {
    let (Some(kind @ ("SetSignal" | "SetCrosswalk")), Some(at)) =
        (value.get("type").and_then(Value::as_str), value.get("at"))
    else {
        return Ok(value.clone());
    };
    if value.get("node").is_some() {
        return Err(format!("{kind} には at と node のどちらか一方だけを書く"));
    }
    let at: [i64; 2] = serde_json::from_value(at.clone()).map_err(|e| format!("at: {e}"))?;
    let node = node_at(tile(at)).ok_or_else(|| format!("マス {at:?} にノードがない"))?;
    let mut out = value.clone();
    let obj = out
        .as_object_mut()
        .expect("type のある値はオブジェクトである");
    obj.remove("at");
    obj.insert(
        "node".into(),
        serde_json::json!({"index": node.index, "generation": node.generation}),
    );
    Ok(out)
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
                zone: zone.map(PaintZoneJson::from),
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
            Command::SetCrosswalk {
                node: NodeId { index, generation },
                arm,
                on,
            } => CommandJson::SetCrosswalk {
                node: NodeJson { index, generation },
                arm: arm.into(),
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
            Command::SetParkingFee { fee } => CommandJson::SetParkingFee { fee },
            Command::UpgradeRoad { from, to, kind } => CommandJson::UpgradeRoad {
                from: tile_json(from),
                to: tile_json(to),
                kind: kind.into(),
            },
            Command::BuildRail { from, to } => CommandJson::BuildRail {
                from: tile_json(from),
                to: tile_json(to),
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
                zone: zone.map(Zone::from),
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
            CommandJson::SetCrosswalk {
                node: NodeJson { index, generation },
                arm,
                on,
            } => Command::SetCrosswalk {
                node: NodeId { index, generation },
                arm: arm.into(),
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
            CommandJson::SetParkingFee { fee } => Command::SetParkingFee { fee },
            CommandJson::UpgradeRoad { from, to, kind } => Command::UpgradeRoad {
                from: tile(from),
                to: tile(to),
                kind: kind.into(),
            },
            CommandJson::BuildRail { from, to } => Command::BuildRail {
                from: tile(from),
                to: tile(to),
            },
        })
    }
}

/// JSON の値 1 つをコマンドに読む。形が違うか、値が範囲の外なら理由を返す。
/// 範囲の検査は [`Command::validate`] でも行うので、ここでは中核の型に収まらない値だけを断る。
pub fn parse(value: &serde_json::Value) -> Result<Command, String> {
    if let Some(message) = explain_out_of_range(value) {
        return Err(message);
    }
    CommandJson::deserialize(value)
        .map_err(|e| e.to_string())?
        .into_command()
}

/// serde の生の文が出る前に、範囲の外の整数と、選べない区画の名前を日本語の文で断る。
/// 中核の [`Command::validate`] の文（上限を超えた値）と同じ形にそろえる。
fn explain_out_of_range(value: &Value) -> Option<String> {
    let kind = value.get("type")?.as_str()?;
    let int_field = |field: &str, label: &str, min: i128, max: i128| -> Option<String> {
        let v = value.get(field)?;
        let n = v
            .as_i64()
            .map(i128::from)
            .or_else(|| v.as_u64().map(i128::from))?;
        (n < min || n > max).then(|| format!("{label} {n} は {min} から {max} の外にある"))
    };
    match kind {
        "SetTaxRate" => {
            let zone = value.get("zone").and_then(Value::as_str);
            if zone == Some("ResidentialLow") {
                return Some(
                    "税率の区画に ResidentialLow は選べない。住宅の税率は低密度と高密度で 1 つで、Residential で決める"
                        .into(),
                );
            }
            if let Some(zone) = zone
                && !["Residential", "Commercial", "Industrial", "Office"].contains(&zone)
            {
                return Some(format!(
                    "税率の区画 {zone} は選べない。Residential、Commercial、Industrial、Office のどれかを書く"
                ));
            }
            int_field("permille", "税率", 0, MAX_TAX_PERMILLE.into())
        }
        "SetParkingFee" => int_field("fee", "駐車場の料金", 0, MAX_PARKING_FEE.into()),
        "SetSimSpeed" => int_field(
            "speed",
            "速度",
            SimSpeed::NORMAL.multiplier().into(),
            SimSpeed::MAX.multiplier().into(),
        ),
        "PlaceBuilding" => int_field("rotation", "建物の向き", 0, 3),
        _ => None,
    }
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
            json!({"type":"PaintZone","area":[[0,0],[2,2]],"zone":"ResidentialLow"}),
            json!({"type":"PaintZone","area":[[0,0],[2,2]],"zone":null}),
            json!({"type":"PlaceBuilding","kind":"WaterPump","at":[4,4],"rotation":2}),
            json!({"type":"SetSignal","node":{"index":3,"generation":1},"on":true}),
            json!({"type":"BuyChunk","chunk":[2,-1]}),
            json!({"type":"SetTaxRate","zone":"Commercial","permille":120}),
            json!({"type":"SetSimSpeed","speed":4}),
            json!({"type":"SetParkingFee","fee":30}),
            json!({"type":"UpgradeRoad","from":[0,1],"to":[5,1],"kind":"Large"}),
            json!({"type":"SetCrosswalk","node":{"index":3,"generation":1},"arm":"SW","on":false}),
            json!({"type":"BuildRail","from":[0,1],"to":[5,6]}),
            json!({"type":"PlaceBuilding","kind":"Station","at":[4,4],"rotation":2}),
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

    #[test]
    fn set_signal_at_a_tile_becomes_the_node_form() {
        let node_at = |t: TilePos| {
            (t == TilePos::new(4, -2)).then_some(NodeId {
                index: 5,
                generation: 1,
            })
        };
        let at = json!({"type":"SetSignal","at":[4,-2],"on":true});
        let resolved = resolve_node_at(&at, node_at).unwrap();
        assert_eq!(
            resolved,
            json!({"type":"SetSignal","node":{"index":5,"generation":1},"on":true})
        );
        assert!(parse(&resolved).is_ok());
        assert!(
            resolve_node_at(&json!({"type":"SetSignal","at":[0,0],"on":true}), node_at).is_err()
        );
        let both =
            json!({"type":"SetSignal","at":[4,-2],"node":{"index":5,"generation":1},"on":true});
        assert!(resolve_node_at(&both, node_at).is_err());
        // ほかのコマンドと、ノードで書いた信号はそのまま通す。
        let road = json!({"type":"BuildRoad","from":[0,1],"to":[5,1],"kind":"Medium"});
        assert_eq!(resolve_node_at(&road, node_at).unwrap(), road);
        // 横断歩道もマスで指せる。
        let cw = json!({"type":"SetCrosswalk","at":[4,-2],"arm":"N","on":false});
        let resolved = resolve_node_at(&cw, node_at).unwrap();
        assert_eq!(
            resolved,
            json!({"type":"SetCrosswalk","node":{"index":5,"generation":1},"arm":"N","on":false})
        );
        assert!(parse(&resolved).is_ok());
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
                zone: Some(Zone::Industrial),
            },
            Command::PaintZone {
                area,
                zone: Some(Zone::ResidentialLow),
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
            Command::UpgradeRoad {
                from: t(-5, 1),
                to: t(5, 1),
                kind: RoadKind::Medium,
            },
            Command::SetCrosswalk {
                node: NodeId {
                    index: 4,
                    generation: 0,
                },
                arm: Heading::NW,
                on: false,
            },
            Command::SetParkingFee { fee: 50 },
            Command::BuildRail {
                from: t(-5, 1),
                to: t(5, -9),
            },
            Command::PlaceBuilding {
                kind: ServiceKind::Station,
                at: t(7, -8),
                rotation: 1,
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
                Command::UpgradeRoad { .. } => 8,
                Command::SetCrosswalk { .. } => 9,
                Command::SetParkingFee { .. } => 10,
                Command::BuildRail { .. } => 11,
            }
        }
        let mut seen = [false; 12];
        for c in commands {
            seen[kind(&c)] = true;
            let json = to_json(c);
            assert_eq!(parse(&json), Ok(c), "{json}");
        }
        assert!(seen.iter().all(|&s| s), "{seen:?}");
        for z in ZoneKind::ALL {
            assert_eq!(ZoneKind::from(ZoneJson::from(z)), z);
        }
        for z in Zone::ALL {
            assert_eq!(Zone::from(PaintZoneJson::from(z)), z);
        }
        for k in ServiceKind::ALL.into_iter().chain([ServiceKind::Station]) {
            assert_eq!(ServiceKind::from(ServiceJson::from(k)), k);
        }
        for r in [RoadKind::Small, RoadKind::Medium, RoadKind::Large] {
            assert_eq!(RoadKind::from(RoadKindJson::from(r)), r);
        }
        for h in Heading::ALL {
            assert_eq!(Heading::from(HeadingJson::from(h)), h);
        }
    }

    /// 範囲の外の値は、serde の生の文ではなく、日本語の分かる文で断る。
    #[test]
    fn explains_out_of_range_values() {
        let cases = [
            (
                json!({"type":"SetParkingFee","fee":-1}),
                "駐車場の料金 -1 は 0 から 50 の外にある",
            ),
            (
                json!({"type":"SetParkingFee","fee":70000}),
                "駐車場の料金 70000 は 0 から 50 の外にある",
            ),
            (
                json!({"type":"SetTaxRate","zone":"Office","permille":-5}),
                "税率 -5 は 0 から 300 の外にある",
            ),
            (
                json!({"type":"SetSimSpeed","speed":-1}),
                "速度 -1 は 1 から 288 の外にある",
            ),
            (
                json!({"type":"PlaceBuilding","kind":"ParkingLot","at":[0,0],"rotation":-1}),
                "建物の向き -1 は 0 から 3 の外にある",
            ),
        ];
        for (c, expected) in &cases {
            assert_eq!(parse(c).unwrap_err(), *expected, "{c}");
        }
        let low = parse(&json!({"type":"SetTaxRate","zone":"ResidentialLow","permille":100}))
            .unwrap_err();
        assert!(low.contains("ResidentialLow は選べない"), "{low}");
        assert!(!low.contains("unknown variant"), "{low}");
        let other = parse(&json!({"type":"SetTaxRate","zone":"Park","permille":100})).unwrap_err();
        assert!(other.contains("Park は選べない"), "{other}");
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
            json!({"type":"SetTaxRate","zone":"ResidentialLow","permille":100}),
            json!({"type":"SetParkingFee","fee":-1}),
            json!({"type":"SetParkingFee","fee":51}),
            json!({"type":"SetParkingFee"}),
            json!({"type":"SetCrosswalk","node":{"index":3,"generation":1},"arm":"Up","on":false}),
            json!({"type":"BuildRail","from":[0,1]}),
            json!({"type":"BuildRail","from":[0,1],"to":[5,1],"kind":"Small"}),
        ];
        for c in &cases {
            assert!(parse(c).is_err(), "{c}");
        }
    }
}
