//! `chizu-cli agent` で遊んだセッションを、記録の再生と突き合わせる。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn cli(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_chizu-cli"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

fn session(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn write(path: &Path, text: &str) -> String {
    std::fs::write(path, text).unwrap();
    path.to_str().unwrap().to_string()
}

/// new、act、advance をまぜて遊んだあとの observe のハッシュが、記録を CLI で再生したハッシュと同じになる。
#[test]
fn observe_matches_replay() {
    let dir = session("agent-observe");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let highway = json(&cli(&["agent", "observe", d]))["highway"]["end"].clone();
    let [x, y] = [highway[0].as_i64().unwrap(), highway[1].as_i64().unwrap()];
    // 高速道路の端から道路を引き、沿いに住宅と商業を塗る。最後の 1 つは土地が離れていて断られる。
    let first = write(
        &dir.join("first.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[x,y],"to":[x+40,y],"kind":"Medium"},
            {"type":"PaintZone","area":[[x+1,y+1],[x+40,y+6]],"zone":"Residential"},
            {"type":"PaintZone","area":[[x+1,y-6],[x+40,y-1]],"zone":"Commercial"},
            {"type":"BuyChunk","chunk":[9,9]},
        ])
        .to_string(),
    );
    let acted = json(&cli(&["agent", "act", d, &first]));
    let outcomes: Vec<&str> = acted["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["Applied", "Applied", "Applied", "Denied"]);
    cli(&["agent", "advance", d, "3000"]);
    let second = write(
        &dir.join("second.json"),
        r#"{"type":"SetTaxRate","zone":"Residential","permille":120}"#,
    );
    cli(&["agent", "act", d, &second]);
    cli(&["agent", "advance", d, "4200"]);

    let seen = json(&cli(&["agent", "observe", d]));
    assert_eq!(seen["tick"], 7200);
    assert_eq!(seen["time"]["text"], "2 日目 00:00");
    assert!(seen["buildings"]["total"].as_u64().unwrap() > 0, "{seen}");
    let rec = dir.join("commands.rec");
    let replayed = cli(&[rec.to_str().unwrap(), "42", "7200"]);
    assert_eq!(replayed.trim(), seen["stateHash"].as_str().unwrap());
    let state = json(&std::fs::read_to_string(dir.join("state.json")).unwrap());
    assert_eq!(state["stateHash"], seen["stateHash"]);
    assert_eq!(state["seed"], "42");
}

fn cli_fails(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_chizu-cli"))
        .args(args)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{args:?} が通ってしまった");
    String::from_utf8(out.stderr).unwrap()
}

/// 遊んだセッションを、画面が書くフィードバックの形に直す。
fn make_feedback(name: &str, unrecorded_ops: u32) -> PathBuf {
    let dir = session(name);
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "5"]);
    let cmd = write(
        &dir.join("c.json"),
        r#"{"type":"SetTaxRate","zone":"Office","permille":200}"#,
    );
    cli(&["agent", "act", d, &cmd]);
    cli(&["agent", "advance", d, "10"]);
    let mut state = json(&std::fs::read_to_string(dir.join("state.json")).unwrap());
    let map = state.as_object_mut().unwrap();
    map.remove("agentSession");
    map.insert("camera".into(), serde_json::json!({"x":1,"y":2,"zoom":3}));
    map.insert("unrecordedOps".into(), unrecorded_ops.into());
    std::fs::write(dir.join("state.json"), state.to_string()).unwrap();
    std::fs::remove_file(dir.join("session.lock")).unwrap();
    std::fs::remove_file(dir.join("replay-cache.bin")).unwrap();
    std::fs::remove_file(dir.join("c.json")).unwrap();
    dir
}

fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let name = e.file_name().into_string().unwrap();
            (name, std::fs::read(e.path()).unwrap())
        })
        .collect();
    out.sort();
    out
}

/// フィードバックのディレクトリには書かず、new --from で写したセッションで続きを遊ぶ。
#[test]
fn continues_from_feedback_dir() {
    let fb = make_feedback("agent-feedback", 0);
    let f = fb.to_str().unwrap();
    let before = files(&fb);
    let err = cli_fails(&["agent", "advance", f, "10"]);
    assert!(err.contains("--from"), "{err}");
    cli(&["agent", "observe", f]);
    assert_eq!(files(&fb), before, "フィードバックのディレクトリが変わった");

    let dir = session("agent-feedback-copy");
    let d = dir.to_str().unwrap();
    let made = json(&cli(&["agent", "new", d, "--from", f]));
    assert_eq!(made["copiedFrom"]["matches"], true, "{made}");
    assert!(made["copiedFrom"].get("note").is_none(), "{made}");
    assert_eq!(made["tick"], 10);
    cli(&["agent", "advance", d, "10"]);
    let state = json(&std::fs::read_to_string(dir.join("state.json")).unwrap());
    assert_eq!(state["camera"]["zoom"], 3);
    assert_eq!(state["tick"], 20);
    assert_eq!(state["agentSession"], 1);
    assert!(state.get("unrecordedOps").is_none(), "{state}");
    let log = std::fs::read(dir.join("commands.rec")).unwrap();
    assert_eq!(chizu_core::CommandLog::decode(&log).unwrap().len(), 1);
}

/// もうファイルのあるディレクトリには new を断り、ロックのファイルも足さない。
#[test]
fn new_refuses_existing_dir_without_adding_files() {
    let fb = make_feedback("agent-new-existing", 0);
    let f = fb.to_str().unwrap();
    let before = files(&fb);
    cli_fails(&["agent", "new", f, "--from", f]);
    cli_fails(&["agent", "new", f, "9"]);
    assert_eq!(files(&fb), before);
    assert!(!fb.join("session.lock").exists());
}

/// 記録に残らない操作のあったフィードバックは、食い違うのが当然であることを添える。
#[test]
fn notes_unrecorded_ops_when_copying() {
    let fb = make_feedback("agent-feedback-unrecorded", 2);
    let mut state = json(&std::fs::read_to_string(fb.join("state.json")).unwrap());
    state["stateHash"] = "0000000000000000".into();
    std::fs::write(fb.join("state.json"), state.to_string()).unwrap();
    let dir = session("agent-feedback-unrecorded-copy");
    let made = json(&cli(&[
        "agent",
        "new",
        dir.to_str().unwrap(),
        "--from",
        fb.to_str().unwrap(),
    ]));
    assert_eq!(made["copiedFrom"]["matches"], false, "{made}");
    assert!(made["copiedFrom"]["note"].is_string(), "{made}");
    // 新しいセッションには再生し直したハッシュを書くので、そのまま遊べる。
    let saved = json(&std::fs::read_to_string(dir.join("state.json")).unwrap());
    assert_eq!(saved["stateHash"], made["stateHash"]);
    cli(&["agent", "advance", dir.to_str().unwrap(), "1"]);
}

/// 一時ファイルに書けなければ、記録も state.json も変えずに失敗する。
#[test]
fn failed_write_leaves_session_unchanged() {
    let dir = session("agent-write-fail");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "3"]);
    let cmd = write(
        &dir.join("c.json"),
        r#"{"type":"SetTaxRate","zone":"Office","permille":200}"#,
    );
    let before = (
        std::fs::read(dir.join("commands.rec")).unwrap(),
        std::fs::read(dir.join("state.json")).unwrap(),
    );
    std::fs::create_dir(dir.join("state.json.tmp")).unwrap();
    cli_fails(&["agent", "act", d, &cmd]);
    let after = (
        std::fs::read(dir.join("commands.rec")).unwrap(),
        std::fs::read(dir.join("state.json")).unwrap(),
    );
    assert_eq!(after, before);
    std::fs::remove_dir(dir.join("state.json.tmp")).unwrap();
    let _ = std::fs::remove_file(dir.join("commands.rec.tmp"));
    cli(&["agent", "act", d, &cmd]);
    let log = std::fs::read(dir.join("commands.rec")).unwrap();
    assert_eq!(chizu_core::CommandLog::decode(&log).unwrap().len(), 1);
}

/// 記録だけ置き換わって state.json が古いまま（置き換えのあいだで止まった）なら、書き込まずに止まる。
#[test]
fn refuses_when_record_and_hash_disagree() {
    let dir = session("agent-torn");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "3"]);
    let old_state = std::fs::read(dir.join("state.json")).unwrap();
    let cmd = write(
        &dir.join("c.json"),
        r#"{"type":"SetTaxRate","zone":"Office","permille":200}"#,
    );
    cli(&["agent", "act", d, &cmd]);
    std::fs::write(dir.join("state.json"), &old_state).unwrap();
    let rec = std::fs::read(dir.join("commands.rec")).unwrap();
    for args in [vec!["act", d, &cmd], vec!["advance", d, "5"]] {
        let err = cli_fails(&[&["agent"], args.as_slice()].concat());
        assert!(err.contains("observe"), "{err}");
        assert!(err.contains("--from"), "{err}");
    }
    assert_eq!(std::fs::read(dir.join("commands.rec")).unwrap(), rec);
    assert_eq!(std::fs::read(dir.join("state.json")).unwrap(), old_state);
}

/// 同じセッションに同時に打っても、操作は消えない。
#[test]
fn concurrent_acts_keep_every_command() {
    let dir = session("agent-concurrent");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "3"]);
    let cmd = write(
        &dir.join("c.json"),
        r#"{"type":"SetTaxRate","zone":"Office","permille":200}"#,
    );
    let n = 8;
    let children: Vec<_> = (0..n)
        .map(|_| {
            Command::new(env!("CARGO_BIN_EXE_chizu-cli"))
                .args(["agent", "act", d, &cmd])
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    let log = std::fs::read(dir.join("commands.rec")).unwrap();
    assert_eq!(chizu_core::CommandLog::decode(&log).unwrap().len(), n);
}

#[test]
fn reports_invalid_commands_without_recording() {
    let dir = session("agent-invalid");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "1"]);
    let bad = write(
        &dir.join("bad.json"),
        r#"[{"type":"SetTaxRate","zone":"Office","permille":999},{"type":"Fly"}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &bad]));
    for r in acted["results"].as_array().unwrap() {
        assert_eq!(r["outcome"], "Invalid", "{r}");
        assert!(r["message"].is_string(), "{r}");
    }
    let rec = std::fs::read(dir.join("commands.rec")).unwrap();
    let log = chizu_core::CommandLog::decode(&rec).unwrap();
    assert!(log.is_empty());
}

/// 再生の途中のキャッシュが壊れているか、ほかのセッションのものなら、捨てて記録の先頭から作り直す。
#[test]
fn discards_broken_or_foreign_caches() {
    const CACHE: &str = "replay-cache.bin";
    let replayed = |dir: &Path, tick: &str| {
        let rec = dir.join("commands.rec");
        // small_town は種 42 で作る。
        cli(&[rec.to_str().unwrap(), "42", tick]).trim().to_string()
    };
    let dir = small_town("agent-cache-broken");
    let d = dir.to_str().unwrap();
    cli(&["agent", "advance", d, "400"]);
    assert!(dir.join(CACHE).exists(), "キャッシュを書いていない");

    // 中ほどの 1 バイトを壊す。
    let mut bytes = std::fs::read(dir.join(CACHE)).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x55;
    std::fs::write(dir.join(CACHE), &bytes).unwrap();
    let seen = json(&cli(&["agent", "observe", d]));
    assert_eq!(seen["stateHash"].as_str().unwrap(), replayed(&dir, "400"));
    assert_ne!(
        std::fs::read(dir.join(CACHE)).unwrap(),
        bytes,
        "壊れたキャッシュを書き直していない"
    );

    // 同じ種で、最初の道路の違う街のキャッシュを持ってくる。
    let other = session("agent-cache-foreign");
    let o = other.to_str().unwrap();
    cli(&["agent", "new", o, "42"]);
    let cmds = write(
        &other.join("c.json"),
        r#"[{"type":"BuildRoad","from":[0,0],"to":[30,0],"kind":"Small"},
            {"type":"PaintZone","area":[[0,1],[24,6]],"zone":"Residential"}]"#,
    );
    cli(&["agent", "act", o, &cmds]);
    cli(&["agent", "advance", o, "400"]);
    std::fs::copy(dir.join(CACHE), other.join(CACHE)).unwrap();
    let seen = json(&cli(&["agent", "observe", o]));
    assert_eq!(seen["stateHash"].as_str().unwrap(), replayed(&other, "400"));
    assert_ne!(
        seen["stateHash"],
        json(&cli(&["agent", "observe", d]))["stateHash"]
    );
    // act と advance も、ハッシュの突き合わせを通って書き込める。
    std::fs::copy(dir.join(CACHE), other.join(CACHE)).unwrap();
    cli(&["agent", "advance", o, "15"]);
    let state = json(&std::fs::read_to_string(other.join("state.json")).unwrap());
    assert_eq!(
        state["stateHash"].as_str().unwrap(),
        replayed(&other, "415")
    );
}

/// 道路と区画を置いた小さな街。道路は y=0 の x=0〜20 を Medium で引き、区画は x=0〜24、y=1〜6 の住宅を塗るが、塗れるのは道路に沿った x=0〜20 だけである。
fn small_town(name: &str) -> PathBuf {
    let dir = session(name);
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[0,0],"to":[20,0],"kind":"Medium"},
            {"type":"PaintZone","area":[[0,1],[24,6]],"zone":"Residential"}]"#,
    );
    cli(&["agent", "act", d, &cmds]);
    dir
}

/// 地図の本体の行だけを、行頭の y の目盛りを外して取り出す。
fn grid_rows(map: &str, rows: usize, cols: usize) -> Vec<String> {
    let lines: Vec<&str> = map.lines().collect();
    lines[lines.len() - rows..]
        .iter()
        .map(|l| l.chars().skip(l.chars().count() - cols).collect())
        .collect()
}

#[test]
fn map_shows_roads_zones_and_land() {
    let dir = small_town("agent-map");
    let d = dir.to_str().unwrap();
    // x -2〜22、y -2〜7。道路の端の外（x=21 以降）は間口がなく、塗れない。
    let map = cli(&["agent", "map", d, "-2", "-2", "22", "7"]);
    assert!(map.contains("凡例"), "{map}");
    assert!(map.contains("25×10 文字"), "{map}");
    let flat = ".".repeat(25);
    let road = format!("..{}..", "2".repeat(21));
    let zone = format!("..{}..", "r".repeat(21));
    let mut want = vec![flat.clone(), flat.clone(), road];
    want.extend(std::iter::repeat_n(zone, 6));
    want.push(flat);
    assert_eq!(grid_rows(&map, 10, 25), want, "{map}");
    // 角の順番を問わない。
    assert_eq!(cli(&["agent", "map", d, "22", "7", "-2", "-2"]), map);
}

#[test]
fn map_scale_picks_by_priority() {
    let dir = small_town("agent-map-scale");
    let d = dir.to_str().unwrap();
    // 5×5 マスで 1 文字。道路のあるセルは区画が混ざっていても道路になる。
    let map = cli(&["agent", "map", d, "0", "0", "24", "24", "--scale", "5"]);
    assert!(map.contains("5×5 マス"), "{map}");
    assert_eq!(
        grid_rows(&map, 5, 5),
        ["22222", "rrrrr", ".....", ".....", "....."],
        "{map}"
    );
    // 買っていない土地と高速道路は、縮尺が大きくても見える。
    let wide = cli(&[
        "agent", "map", d, "-100", "-100", "160", "160", "--scale", "10",
    ]);
    assert!(wide.contains(','), "{wide}");
    assert!(wide.contains('H'), "{wide}");
}

#[test]
fn map_refuses_too_large_ranges() {
    let dir = small_town("agent-map-large");
    let d = dir.to_str().unwrap();
    let err = cli_fails(&["agent", "map", d, "-500", "-500", "500", "500"]);
    assert!(err.contains("--scale 11"), "{err}");
    cli(&[
        "agent", "map", d, "-500", "-500", "500", "500", "--scale", "11",
    ]);
    // 上限ちょうどの 96×96 マスは通る。
    cli(&["agent", "map", d, "0", "0", "95", "95"]);
    cli_fails(&["agent", "map", d, "0", "0", "96", "95"]);
    cli_fails(&["agent", "map", d, "0", "0", "5", "5", "--scale", "0"]);
}

#[test]
fn map_leaves_feedback_dir_untouched() {
    let fb = make_feedback("agent-map-feedback", 0);
    let before = files(&fb);
    let map = cli(&["agent", "map", fb.to_str().unwrap(), "0", "0", "9", "9"]);
    assert!(map.contains("凡例"), "{map}");
    assert_eq!(files(&fb), before);
}

#[test]
fn observe_gives_owned_bounds() {
    let dir = session("agent-owned-bounds");
    let d = dir.to_str().unwrap();
    let seen = json(&cli(&["agent", "new", d, "1"]));
    assert_eq!(
        seen["land"]["ownedBounds"],
        serde_json::json!([[-64, -64], [127, 127]])
    );
}

/// 税率が基準を超えると、observe の jobs.keptPermille と taxEffect に出る。
#[test]
fn observe_shows_the_tax_effect() {
    let dir = small_town("agent-tax-effect");
    let d = dir.to_str().unwrap();
    let seen = json(&cli(&["agent", "observe", d]));
    assert_eq!(seen["jobs"]["keptPermille"]["Commercial"], 1000);
    assert_eq!(seen["taxEffect"]["residentialDemandShift"], 0);
    let cmds = write(
        &dir.join("tax.json"),
        r#"[{"type":"SetTaxRate","zone":"Residential","permille":150},
            {"type":"SetTaxRate","zone":"Commercial","permille":150}]"#,
    );
    cli(&["agent", "act", d, &cmds]);
    let seen = json(&cli(&["agent", "observe", d]));
    assert_eq!(seen["jobs"]["keptPermille"]["Commercial"], 629);
    assert_eq!(seen["jobs"]["keptPermille"]["Industrial"], 1000);
    assert_eq!(seen["taxEffect"]["residentialDemandShift"], -25);
}

/// 公共施設は区画より優先して出る。発電所は道路の下側、浄水場は上側に置く。
#[test]
fn map_shows_facilities() {
    let dir = small_town("agent-map-facilities");
    let d = dir.to_str().unwrap();
    let cmds = write(
        &dir.join("p.json"),
        r#"[{"type":"PlaceBuilding","kind":"PowerPlant","at":[5,1],"rotation":0},
            {"type":"PlaceBuilding","kind":"WaterPump","at":[12,-1],"rotation":2}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    assert_eq!(acted["results"][0]["outcome"], "Applied", "{acted}");
    assert_eq!(acted["results"][1]["outcome"], "Applied", "{acted}");
    let map = cli(&["agent", "map", d, "5", "-2", "13", "2"]);
    assert_eq!(
        grid_rows(&map, 5, 9),
        [
            "......WW.",
            "......WW.",
            "222222222",
            "PPPPrrrrr",
            "PPPPrrrrr"
        ],
        "{map}"
    );
}

/// 共同の駐車場も act で置け、map では L（平面）と G（駐車場ビル）で出る。observe には台数と維持費が出る。
#[test]
fn act_places_shared_parking_and_observe_and_map_show_it() {
    let dir = small_town("agent-shared-parking");
    let d = dir.to_str().unwrap();
    let cmds = write(
        &dir.join("p.json"),
        r#"[{"type":"PlaceBuilding","kind":"ParkingLot","at":[17,1],"rotation":0},
            {"type":"PlaceBuilding","kind":"ParkingGarage","at":[12,-1],"rotation":2}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    assert_eq!(acted["results"][0]["outcome"], "Applied", "{acted}");
    assert_eq!(acted["results"][1]["outcome"], "Applied", "{acted}");
    let map = cli(&["agent", "map", d, "9", "-2", "18", "2"]);
    assert_eq!(
        grid_rows(&map, 5, 10),
        [
            "GGGG......",
            "GGGG......",
            "2222222222",
            "rrrrrrrrLL",
            "rrrrrrrrLL"
        ],
        "{map}"
    );
    let o = json(&cli(&["agent", "observe", d]));
    let lots = o["parking"]["sharedLots"]
        .as_array()
        .expect("sharedLots がない");
    assert_eq!(lots.len(), 2, "{lots:?}");
    assert_eq!(lots[0]["kind"], "ParkingLot");
    assert_eq!(lots[0]["at"], serde_json::json!([17, 1]));
    assert_eq!(lots[0]["spaces"], 12);
    assert_eq!(lots[1]["kind"], "ParkingGarage");
    assert_eq!(lots[1]["at"], serde_json::json!([9, -4]));
    assert_eq!(lots[1]["spaces"], 240);
    for lot in lots {
        for k in [
            "rented",
            "parked",
            "rentedParked",
            "visitorParked",
            "visitorReserved",
        ] {
            assert_eq!(lot[k], 0, "{k}");
        }
    }
    assert_eq!(o["parking"]["households"]["shared"], 0);
    // 世帯がいないので、車を持てない世帯も、世帯の多い所もない。理由の欄はそろっている
    assert_eq!(o["parking"]["households"]["none"], 0);
    let reasons = &o["parking"]["households"]["noneReasons"];
    for k in [
        "noLotInRange",
        "full",
        "feeTooHigh",
        "waiting",
        "lotNotOnRoad",
    ] {
        assert_eq!(reasons[k], 0, "{k}");
    }
    assert_eq!(o["parking"]["carlessAreas"], serde_json::json!([]));
    // 駐車場の層では、埋まり具合の文字（借りられた枠がなければ半分に満たない -）で出る
    let layer = cli(&[
        "agent", "map", d, "9", "-2", "18", "2", "--layer", "parking",
    ]);
    assert_eq!(
        grid_rows(&layer, 5, 10),
        [
            "----......",
            "----......",
            "2222222222",
            "rrrrrrrr--",
            "rrrrrrrr--"
        ],
        "{layer}"
    );
    assert!(
        layer.contains("- + # X  共同の駐車場の埋まり具合"),
        "{layer}"
    );
    assert!(!layer.contains("L 平面の共同の駐車場"), "{layer}");
    let err = cli_fails(&["agent", "map", d, "9", "-2", "18", "2", "--layer", "power"]);
    assert!(err.contains("traffic か parking"), "{err}");
    assert_eq!(o["facilities"]["ParkingLot"], 1);
    assert_eq!(o["facilities"]["ParkingGarage"], 1);
    let upkeep = &o["financesPerDay"]["facilityUpkeep"];
    assert_eq!(upkeep["ParkingLot"], 96);
    assert_eq!(upkeep["ParkingGarage"], 2400);
    // 道路に面している間は、前の道路がない駐車場はない。
    assert_eq!(o["parking"]["lotsNotOnRoad"]["count"], 0);
    assert_eq!(o["parking"]["lotsNotOnRoad"]["lots"], serde_json::json!([]));
    // 前の道路を消すと、その駐車場の数と場所が出て、sharedLots には残る。
    let bulldoze = write(
        &dir.join("b.json"),
        r#"[{"type":"Bulldoze","area":[[9,0],[18,0]]}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &bulldoze]));
    assert_eq!(acted["results"][0]["outcome"], "Applied", "{acted}");
    let o = json(&cli(&["agent", "observe", d]));
    let off = &o["parking"]["lotsNotOnRoad"];
    assert_eq!(off["count"], 2, "{off}");
    let lots = off["lots"].as_array().unwrap();
    assert_eq!(lots[0]["kind"], "ParkingLot");
    assert_eq!(lots[0]["at"], serde_json::json!([17, 1]));
    assert_eq!(lots[1]["kind"], "ParkingGarage");
    assert_eq!(lots[1]["at"], serde_json::json!([9, -4]));
    assert_eq!(o["parking"]["sharedLots"].as_array().unwrap().len(), 2);
    assert_eq!(
        o["sinceLastObserve"]["changes"]["parking.lotsNotOnRoad.count"]["delta"],
        2
    );
}

/// 駅を置いて線路を引くと、observe の rail と map に出る。取り壊すと bulldozed に駅と線路のマスが出る。
#[test]
fn act_builds_rail_and_a_station_and_observe_and_map_show_them() {
    let dir = session("agent-rail");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "0"]);
    let cmds = write(
        &dir.join("r.json"),
        r#"[{"type":"BuildRoad","from":[0,10],"to":[40,10],"kind":"Small"},
            {"type":"PlaceBuilding","kind":"Station","at":[12,9],"rotation":2},
            {"type":"BuildRail","from":[12,8],"to":[16,8]},
            {"type":"BuildRail","from":[16,8],"to":[16,0]},
            {"type":"BuildRail","from":[20,0],"to":[20,20]}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    let outcomes: Vec<(&str, Option<&str>)> = acted["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["outcome"].as_str().unwrap(), r["reason"].as_str()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("Applied", None),
            ("Applied", None),
            ("Applied", None),
            ("Denied", Some("RailTooSharp")),
            ("Denied", Some("Occupied")),
        ],
        "{acted}"
    );
    let o = json(&cli(&["agent", "observe", d]));
    let rail = &o["rail"];
    assert_eq!(rail["tiles"], 4, "{rail}");
    assert_eq!(rail["tracks"], 1, "{rail}");
    assert_eq!(
        rail["deadEnds"],
        serde_json::json!([[3, 8], [16, 8]]),
        "{rail}"
    );
    let stations = rail["stations"].as_array().unwrap();
    assert_eq!(stations.len(), 1, "{rail}");
    assert_eq!(stations[0]["ends"], serde_json::json!([[3, 8], [12, 8]]));
    assert_eq!(stations[0]["area"], serde_json::json!([[3, 7], [12, 9]]));
    assert_eq!(stations[0]["facesRoad"], true);
    // 駅 800 と、4 マスの線路 6000 の 1%。
    assert_eq!(o["financesPerDay"]["railUpkeep"], 860);
    let map = cli(&["agent", "map", d, "2", "8", "17", "8"]);
    assert_eq!(grid_rows(&map, 1, 16), [".EEEEEEEEEETTTT."], "{map}");
    let gone = write(
        &dir.join("b.json"),
        r#"{"type":"Bulldoze","area":[[5,7],[5,7]]}"#,
    );
    let acted = json(&cli(&["agent", "act", d, &gone]));
    let removed = &acted["results"][0]["bulldozed"];
    assert_eq!(removed["stations"], 1, "{acted}");
    assert_eq!(removed["railTiles"], 9, "{acted}");
    let o = json(&cli(&["agent", "observe", d]));
    assert_eq!(o["rail"]["stations"], serde_json::json!([]));
    assert_eq!(
        o["sinceLastObserve"]["changes"]["rail.stations"]["delta"],
        -1
    );
}

/// 大きすぎる縮尺は、あふれずに断る。
#[test]
fn map_refuses_huge_scale() {
    let dir = small_town("agent-map-huge-scale");
    let d = dir.to_str().unwrap();
    for scale in ["18446744073709551615", "9223372036854775808", "193"] {
        let err = cli_fails(&[
            "agent", "map", d, "-64", "-64", "127", "127", "--scale", scale,
        ]);
        assert!(err.contains("--scale"), "{err}");
        assert!(!err.contains("panicked"), "{err}");
    }
    cli(&[
        "agent", "map", d, "-64", "-64", "127", "127", "--scale", "192",
    ]);
}

/// 世界のほぼ全体を指しても、文字数の計算があふれない。
#[test]
fn map_handles_whole_world() {
    let dir = small_town("agent-map-world");
    let d = dir.to_str().unwrap();
    let (lo, hi) = ("-2147483648", "2147483648");
    let err = cli_fails(&["agent", "map", d, lo, lo, hi, hi]);
    assert!(!err.contains("panicked"), "{err}");
    assert!(err.contains("--scale 44"), "{err}");
    let err = cli_fails(&["agent", "map", d, lo, lo, hi, hi, "--scale", "2"]);
    assert!(!err.contains("panicked"), "{err}");
    assert!(err.contains("--scale 44"), "{err}");
}

/// 負の x の目盛りは、符号を除いた下 1 桁になる。
#[test]
fn map_ruler_uses_absolute_digits() {
    let dir = small_town("agent-map-ruler");
    let d = dir.to_str().unwrap();
    let map = cli(&["agent", "map", d, "-74", "0", "-65", "0"]);
    let lines: Vec<&str> = map.lines().collect();
    let digits = lines[lines.len() - 2].trim();
    assert_eq!(digits, "4321098765", "{map}");
    assert!(lines[lines.len() - 3].contains("-70"), "{map}");
}

/// 住宅と工場を置いて数日進めた街で、人と車の集計が出て、数が食い違わない。
#[test]
fn observe_gives_people_and_traffic_counts() {
    let dir = session("agent-traffic");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    // 始めたばかりの街でも、欄は出て、空である。
    let fresh = json(&cli(&["agent", "observe", d]));
    assert_eq!(fresh["traffic"]["cars"]["total"], 0);
    assert_eq!(
        fresh["traffic"]["trips"]["yesterday"],
        serde_json::Value::Null
    );
    assert!(fresh["traffic"]["trips"]["today"]["avgTravelTicks"].is_null());
    let highway = fresh["highway"]["end"].clone();
    let [x, y] = [highway[0].as_i64().unwrap(), highway[1].as_i64().unwrap()];
    // 種 42 の高速道路は横向きで、内側へは外向きの逆に道路を引く。
    let inward = -fresh["highway"]["outward"][0].as_i64().unwrap();
    assert_ne!(inward, 0);
    let (a, b) = (x + inward, x + 40 * inward);
    let cmds = write(
        &dir.join("c.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[x,y],"to":[b,y],"kind":"Medium"},
            {"type":"PaintZone","area":[[a,y+1],[b,y+6]],"zone":"Residential"},
            {"type":"PaintZone","area":[[a,y-6],[b,y-1]],"zone":"Industrial"},
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[x+10*inward,y-1],"rotation":2},
            {"type":"PlaceBuilding","kind":"WaterPump","at":[x+16*inward,y-1],"rotation":2},
            {"type":"PlaceBuilding","kind":"WaterPump","at":[x+19*inward,y-1],"rotation":2},
        ])
        .to_string(),
    );
    cli(&["agent", "act", d, &cmds]);
    cli(&["agent", "advance", d, &(3 * 7200 + 2400).to_string()]);
    let o = json(&cli(&["agent", "observe", d]));
    let n = |v: &serde_json::Value| v.as_u64().unwrap();
    let people = &o["citizenPlaces"];
    let sum: u64 = [
        "atHome", "atWork", "atShop", "walking", "driving", "homeless",
    ]
    .iter()
    .map(|k| n(&people[k]))
    .sum();
    assert_eq!(sum, n(&o["population"]["citizens"]));
    assert!(sum > 0, "誰も住んでいない");
    let cars = &o["traffic"]["cars"];
    assert_eq!(
        n(&cars["total"]),
        n(&cars["moving"]) + n(&cars["stopped"]) + n(&cars["waitingAtCurb"])
    );
    assert_eq!(n(&cars["stoppedAfterTicks"]), 100);
    // 速さ 0 の車は、駐車場の中で待つ車を含まない。
    assert!(n(&cars["queued"]) + n(&cars["waitingAtCurb"]) <= n(&cars["total"]));
    // 出発の建物ごとの上位は、上限の件数までで、台数の和が waitingAtCurb を超えず、待ちが長い車は待つ車の内数である。
    let waits = cars["curbWaits"].as_array().unwrap();
    assert!(waits.len() <= 10);
    assert!(waits.iter().map(|w| n(&w["waiting"])).sum::<u64>() <= n(&cars["waitingAtCurb"]));
    assert!(waits.iter().all(|w| n(&w["long"]) <= n(&w["waiting"])));
    assert!(
        waits
            .windows(2)
            .all(|w| n(&w[0]["waiting"]) >= n(&w[1]["waiting"]))
    );
    let trips = &o["traffic"]["trips"];
    let yesterday = &trips["yesterday"];
    assert!(n(&yesterday["arrived"]) > 0, "前の日に誰も着いていない");
    assert!(yesterday["avgTravelTicks"].as_f64().unwrap() > 0.0);
    let m = &o["migration"];
    assert!(m["vacantHomes"].is_u64() && m["blockedHouseholds"].is_u64());
    // 通勤した住民は、通勤の時間を測ってある。
    let time = &o["commute"]["time"];
    assert_eq!(n(&time["commuters"]), n(&o["commute"]["commuters"]));
    assert!(n(&time["measured"]) > 0, "通勤の時間を測った住民がいない");
    assert!(n(&time["measured"]) <= n(&time["commuters"]));
    assert!(n(&time["medianMinutes"]) <= n(&time["p90Minutes"]));
    assert!(n(&time["overHour"]) <= n(&time["commuters"]));
    // 共同の駐車場のない街では、自宅の枠のない世帯は車を持てず、どの移動も歩く。
    let households = &o["parking"]["households"];
    assert!(n(&households["home"]) > 0, "自宅の枠を持つ世帯がいない");
    assert!(n(&households["none"]) > 0, "自宅の枠のない世帯がいない");
    assert_eq!(n(&households["shared"]), 0);
    assert!(n(&households["home"]) + n(&households["none"]) <= n(&o["population"]["households"]));
    assert!(
        n(&o["parking"]["noPlace"]) > 0,
        "枠がなくて歩いた移動がない"
    );
    let parts = &o["population"]["happinessParts"];
    for k in ["commute", "services", "wealth"] {
        assert!(n(&parts[k]) <= 100, "{k}");
    }
    // 同じ記録を再生した observe は、前の observe からの増減を除いて、毎回同じ値になる。
    let mut again = json(&cli(&["agent", "observe", d]));
    assert_eq!(again["sinceLastObserve"]["changes"], serde_json::json!({}));
    again["sinceLastObserve"] = o["sinceLastObserve"].clone();
    // 余りの減り方（trend）も observe の積み重ねで決まるので、同じく除く。
    for kind in ["power", "water"] {
        again["utilities"][kind]["main"]["trend"] = o["utilities"][kind]["main"]["trend"].clone();
    }
    assert_eq!(o, again);
}

/// 施設のない街では、足りない網の代表のマスが出て、余りは負か 0 になる。
#[test]
fn observe_points_at_the_short_network() {
    let dir = session("agent-short-network");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let fresh = json(&cli(&["agent", "observe", d]));
    let power = &fresh["utilities"]["power"];
    assert!(power["main"].is_null(), "{power}");
    assert_eq!(power["others"]["headroom"], 0);
    assert!(
        power.get("headroom").is_none(),
        "合計の余りを出している: {power}"
    );
    assert_eq!(power["shortAt"], serde_json::json!([]));
    assert_eq!(fresh["shortages"], serde_json::json!([]));
    assert_eq!(power["isolatedFacilities"], serde_json::json!([]));
    let [x, y] = [
        fresh["highway"]["end"][0].as_i64().unwrap(),
        fresh["highway"]["end"][1].as_i64().unwrap(),
    ];
    let inward = -fresh["highway"]["outward"][0].as_i64().unwrap();
    let (a, b) = (x + inward, x + 40 * inward);
    let cmds = write(
        &dir.join("c.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[x,y],"to":[b,y],"kind":"Medium"},
            {"type":"PaintZone","area":[[a,y+1],[b,y+6]],"zone":"Residential"},
        ])
        .to_string(),
    );
    cli(&["agent", "act", d, &cmds]);
    cli(&["agent", "advance", d, &(2 * 7200).to_string()]);
    let o = json(&cli(&["agent", "observe", d]));
    let power = &o["utilities"]["power"];
    assert_eq!(power["capacity"], 0);
    assert!(power["demand"].as_u64().unwrap() > 0);
    assert!(power["main"]["headroom"].as_i64().unwrap() < 0);
    // 足りないとき lowHeadroom は偽だが、shortage と shortages で足りないことが分かる。
    assert_eq!(power["main"]["lowHeadroom"], false);
    assert_eq!(power["main"]["shortage"], true);
    assert_eq!(
        power["main"]["shortBy"].as_i64().unwrap(),
        -power["main"]["headroom"].as_i64().unwrap()
    );
    assert!(
        o["shortages"].as_array().unwrap().contains(&"power".into()),
        "{o}"
    );
    assert_eq!(power["main"]["at"], power["shortAt"][0]["at"]);
    assert_eq!(power["shortNetworks"], 1);
    // 足りないときは、足りない人数が負の値で出る。人口が 0 なら人数は決まらず null。
    let people = o["population"]["housedCitizens"].as_i64().unwrap();
    if people == 0 {
        assert!(power["main"]["headroomPeople"].is_null(), "{power}");
    } else {
        assert!(
            power["main"]["headroomPeople"].as_i64().unwrap() < 0,
            "{power}"
        );
    }
    let at = &power["shortAt"][0]["at"];
    assert!(at[0].is_i64() && at[1].is_i64(), "{power}");
}

/// 供給が 1 以上ある網でも、需要が供給を超えたら shortage と shortBy が出て、shortages に種類が並ぶ。
/// そのとき lowHeadroom は偽である。電気は足りていて、水道だけが足りない街を作る。
#[test]
fn observe_shows_a_shortage_when_supply_is_positive() {
    let dir = session("agent-short-positive-supply");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let fresh = json(&cli(&["agent", "observe", d]));
    let [x, y] = [
        fresh["highway"]["end"][0].as_i64().unwrap(),
        fresh["highway"]["end"][1].as_i64().unwrap(),
    ];
    let inward = -fresh["highway"]["outward"][0].as_i64().unwrap();
    let (a, b) = (x + inward, x + 90 * inward);
    let cmds = write(
        &dir.join("c.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[x,y],"to":[b,y],"kind":"Medium"},
            {"type":"PaintZone","area":[[a,y+1],[b,y+6]],"zone":"Residential"},
            {"type":"PaintZone","area":[[x+22*inward,y-6],[b,y-1]],"zone":"Industrial"},
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[x+10*inward,y-1],"rotation":2},
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[x+13*inward,y-1],"rotation":2},
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[x+16*inward,y-1],"rotation":2},
            {"type":"PlaceBuilding","kind":"WaterPump","at":[x+19*inward,y-1],"rotation":2},
        ])
        .to_string(),
    );
    cli(&["agent", "act", d, &cmds]);
    cli(&["agent", "advance", d, &(20 * 7200).to_string()]);
    let o = json(&cli(&["agent", "observe", d]));
    let water = &o["utilities"]["water"];
    let main = &water["main"];
    assert!(main["capacity"].as_u64().unwrap() > 0, "{water}");
    let headroom = main["headroom"].as_i64().unwrap();
    assert!(headroom < 0, "足りない街にならなかった: {water}");
    assert_eq!(main["shortage"], true);
    assert_eq!(main["lowHeadroom"], false);
    assert_eq!(main["shortBy"].as_i64().unwrap(), -headroom);
    // 1 棟分（48）だけ足りない。#141 と同じ止まり方である。
    assert_eq!(main["shortBy"], 48);
    // 余りが負なので、足りない人数が負の値で出る。式は headroom × 人口 ÷ 需要の合計の切り捨て。
    let citizens = i128::from(o["population"]["housedCitizens"].as_u64().unwrap());
    let demand = i128::from(water["demand"].as_u64().unwrap());
    assert!(citizens > 0 && demand > 0, "{o}");
    let want = (i128::from(headroom) * citizens).div_euclid(demand);
    assert_eq!(i128::from(main["headroomPeople"].as_i64().unwrap()), want);
    assert!(want < 0);
    assert_eq!(o["utilities"]["power"]["main"]["shortage"], false);
    assert_eq!(o["shortages"], serde_json::json!(["water"]), "{o}");
    // 施設 1 基で増える人数は、1 基の供給量を headroomPeople と同じ 1 人あたりの需要で割った値。
    let per = |kind: &str, capacity: i128| {
        let m = &o["utilities"][kind]["main"];
        assert_eq!(
            i128::from(m["perFacilityCapacity"].as_i64().unwrap()),
            capacity
        );
        let d = i128::from(o["utilities"][kind]["demand"].as_u64().unwrap());
        assert_eq!(
            i128::from(m["perFacilityPeople"].as_i64().unwrap()),
            (capacity * citizens).div_euclid(d),
            "{m}"
        );
    };
    per("power", 3600);
    per(
        "water",
        i128::from(main["perFacilityCapacity"].as_i64().unwrap()),
    );
    assert!(main["perFacilityPeople"].as_i64().unwrap() > 0);
    // 1 日前をはさむ需要の控えがまだないので、減り方は見積もれない。理由が付く。
    let trend = &main["trend"];
    assert_eq!(trend["state"], "unknown", "{trend}");
    assert!(trend["basis"].is_null() && trend["reason"].as_str().unwrap().len() > 5);
    // 足りないまま 1 日たった次の observe は、足りないと言う。尽きるまでの時間は出さない。
    cli(&["agent", "advance", d, "7200"]);
    let next = json(&cli(&["agent", "observe", d]));
    let trend = &next["utilities"]["water"]["main"]["trend"];
    assert_eq!(trend["state"], "short", "{trend}");
    assert_eq!(trend["basis"], "demandSameTimeYesterday");
    assert_eq!(trend["targetTick"], 20 * 7200);
    assert!(trend["ticksUntilOut"].is_null() && trend["hoursUntilOut"].is_null());
}

/// Issue #139 の街（発電所 2 基が主の網から切れた道路の脇にある）では、孤立した発電所が出て、
/// つながっていない分の余りが主の網の余りに混ざらない。
/// 記録は当時のもので、いまの中核で再生するとハッシュは食い違うが、置いた施設の位置は変わらない。
#[test]
fn observe_shows_power_plants_cut_off_from_the_main_network() {
    let dir = session("agent-isolated-facilities");
    let repro = dir.join("repro");
    std::fs::create_dir_all(&repro).unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/issue-139.rec");
    std::fs::copy(fixture, repro.join("commands.rec")).unwrap();
    write(
        &repro.join("state.json"),
        r#"{"money":"284512","seed":"202","stateHash":"082ad631a6fde5fa","tick":89550}"#,
    );
    let play = dir.join("play");
    let d = play.to_str().unwrap();
    cli(&["agent", "new", d, "--from", repro.to_str().unwrap()]);
    let o = json(&cli(&["agent", "observe", d]));
    let power = &o["utilities"]["power"];
    assert_eq!(
        power["isolatedFacilities"],
        serde_json::json!([{"at":[10,-59]},{"at":[14,-59]}]),
        "{power}"
    );
    assert_eq!(
        o["utilities"]["water"]["isolatedFacilities"],
        serde_json::json!([])
    );
    // 孤立した発電所の分は others にあり、主の網には入らない。
    let others = &power["others"];
    assert_eq!(others["demand"], 0, "{power}");
    assert!(others["headroom"].as_i64().unwrap() > 0, "{power}");
    let main = &power["main"];
    // 主の網は足りない。合計では余って見えるが、孤立した発電所の分は主の網に届かない。
    assert!(main["headroom"].as_i64().unwrap() < 0, "{power}");
    assert!(power["capacity"].as_i64().unwrap() > power["demand"].as_i64().unwrap());
    assert_eq!(
        main["headroom"].as_i64().unwrap(),
        main["capacity"].as_i64().unwrap() - main["demand"].as_i64().unwrap()
    );
    assert!(power.get("headroom").is_none(), "{power}");
    assert_eq!(main["shortage"], true, "{power}");
    // 孤立した施設は、置いた道路ごとにもうひとつの網として others に数えられる。
    assert!(others["networks"].as_u64().unwrap() >= 1, "{power}");
    // 人数は網ごとに出る。主の網は足りないので負、ほかの網は余りがあるので正になる。
    let each = others["each"].as_array().unwrap();
    assert_eq!(each.len() as u64, others["networks"].as_u64().unwrap());
    assert!(main["headroomPeople"].as_i64().unwrap() < 0, "{power}");
    assert!(
        each.iter()
            .any(|n| n["headroomPeople"].as_i64().unwrap() > 0),
        "{power}"
    );
}

/// 塗っていない種類は、需要が正でも noFreeLot が偽である（塗った区画が建ち尽くしたときだけ真になる）。
#[test]
fn observe_no_free_lot_is_false_for_zones_never_painted() {
    let dir = session("agent-no-free-lot");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let obs = json(&cli(&["agent", "observe", d]));
    assert!(obs["demand"]["Residential"].as_i64().unwrap() > 0, "{obs}");
    assert_eq!(obs["growth"]["Residential"]["fronts"], 0, "{obs}");
    assert_eq!(obs["growth"]["Residential"]["noFreeLot"], false, "{obs}");
    let setup = write(
        &dir.join("setup.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[0,0],"to":[30,0],"kind":"Small"},
            {"type":"PaintZone","area":[[0,1],[30,6]],"zone":"Residential"},
        ])
        .to_string(),
    );
    cli(&["agent", "act", d, &setup]);
    let obs = json(&cli(&["agent", "observe", d]));
    assert_eq!(obs["growth"]["Residential"]["noFreeLot"], false, "{obs}");
}

/// 施設のない道路網に建つ分は電気と水道の余裕で止まり、observe の growth と networksWithoutFacility に出る。
#[test]
fn observe_shows_why_growth_is_blocked() {
    let dir = session("agent-growth");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    // 1 マス離した 2 本の道路。どちらも外の道路や施設につながらない。
    let setup = write(
        &dir.join("setup.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[0,0],"to":[30,0],"kind":"Small"},
            {"type":"BuildRoad","from":[32,0],"to":[80,0],"kind":"Small"},
            {"type":"PaintZone","area":[[0,1],[80,6]],"zone":"Residential"},
        ])
        .to_string(),
    );
    cli(&["agent", "act", d, &setup]);
    let obs = json(&cli(&["agent", "observe", d]));
    for zone in ["Residential", "Commercial", "Industrial", "Office"] {
        let g = &obs["growth"][zone];
        for key in ["fronts", "blocked", "noPower", "noWater"] {
            assert!(g[key].is_u64(), "{zone}.{key}: {g}");
        }
        assert!(g["noFreeLot"].is_boolean(), "{zone}: {g}");
    }
    cli(&["agent", "advance", d, "6000"]);
    let obs = json(&cli(&["agent", "observe", d]));
    let res = &obs["growth"]["Residential"];
    assert!(res["fronts"].as_u64().unwrap() > 0, "{res}");
    assert_eq!(res["blocked"], res["fronts"], "{res}");
    assert_eq!(res["noPower"], res["fronts"], "{res}");
    assert_eq!(res["noWater"], res["fronts"], "{res}");
    let stalled = obs["growth"]["stalledAt"].as_array().unwrap();
    assert!(!stalled.is_empty(), "{obs}");
    for n in stalled {
        assert!(n["at"][0].is_i64() && n["at"][1].is_i64(), "{n}");
        assert_eq!(n["noPower"], true, "{n}");
        assert_eq!(n["noWater"], true, "{n}");
    }
    assert!(
        obs["utilities"]["power"]["networksWithoutFacility"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert!(
        obs["utilities"]["water"]["networksWithoutFacility"]
            .as_u64()
            .unwrap()
            >= 1
    );
}

#[test]
fn act_places_a_signal_by_tile_and_observe_and_map_show_it() {
    let dir = session("agent-signal");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[0,5],"to":[10,5],"kind":"Small"},
            {"type":"BuildRoad","from":[5,0],"to":[5,10],"kind":"Small"},
            {"type":"SetSignal","at":[5,5],"on":true},
            {"type":"SetSignal","at":[0,5],"on":true},
            {"type":"SetSignal","at":[3,3],"on":true}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds, "--verbose"]));
    let results = acted["results"].as_array().unwrap();
    let outcomes: Vec<&str> = results
        .iter()
        .map(|r| r["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(
        outcomes,
        ["Applied", "Applied", "Applied", "Denied", "Invalid"]
    );
    assert_eq!(results[3]["reason"], "NotJunction");
    // 記録に入るのはノードの ID の形である。
    assert!(results[2]["command"]["node"]["index"].is_u64(), "{acted}");
    let seen = json(&cli(&["agent", "observe", d]));
    let signals = seen["signals"].as_array().unwrap();
    assert_eq!(signals.len(), 1, "{seen}");
    assert_eq!(signals[0]["at"], serde_json::json!([5, 5]));
    let arms = signals[0]["arms"].as_array().unwrap();
    assert_eq!(arms.len(), 4);
    let greens = arms.iter().filter(|a| a["light"] == "Green").count();
    assert!(greens <= 2, "{seen}");
    let map = cli(&["agent", "map", d, "4", "4", "6", "6"]);
    assert_eq!(grid_rows(&map, 3, 3), [".1.", "1S1", ".1."], "{map}");
    // 交通の層では、混み具合が信号を隠さない。
    let traffic = cli(&["agent", "map", d, "4", "4", "6", "6", "--layer", "traffic"]);
    assert_eq!(
        grid_rows(&traffic, 3, 3),
        [".-.", "---", ".-."],
        "{traffic}"
    );
}

/// act の結果は既定では短く、--verbose を付けたときだけ記録に入った形のコマンドを添える。
#[test]
fn act_is_short_unless_verbose() {
    let dir = session("agent-act-verbose");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"SetTaxRate","zone":"Office","permille":120}]"#,
    );
    let short = json(&cli(&["agent", "act", d, &cmds]));
    assert_eq!(short["results"][0]["outcome"], "Applied", "{short}");
    assert!(short["results"][0].get("command").is_none(), "{short}");
    let long = json(&cli(&["agent", "act", d, &cmds, "--verbose"]));
    assert_eq!(
        long["results"][0]["command"]["type"], "SetTaxRate",
        "{long}"
    );
    cli_fails(&["agent", "act", d, &cmds, "--loud"]);
}

/// NotOwned で断ったときは、買っていないマスとそのチャンクを返す。
#[test]
fn not_owned_names_the_tiles_and_chunks() {
    let dir = session("agent-not-owned");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    // 買った土地は x が 127 まで。128 からの 8 マスが外にはみ出す。
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[120,0],"to":[135,0],"kind":"Small"},
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[126,20],"rotation":0}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    let road = &acted["results"][0];
    assert_eq!(road["reason"], "NotOwned", "{acted}");
    assert_eq!(road["notOwned"]["count"], 8, "{acted}");
    assert_eq!(road["notOwned"]["tiles"][0], serde_json::json!([128, 0]));
    assert_eq!(road["notOwned"]["chunks"], serde_json::json!([[2, 0]]));
    // 発電所は 4×4 で、x が 126〜129 に延びるので、128 と 129 の 2 列が外にある。
    let plant = &acted["results"][1];
    assert_eq!(plant["reason"], "NotOwned", "{acted}");
    assert_eq!(plant["notOwned"]["count"], 8, "{acted}");
    assert_eq!(plant["notOwned"]["chunks"], serde_json::json!([[2, 0]]));
    assert_eq!(
        plant["notOwned"]["chunksNotAdjacent"],
        serde_json::json!([])
    );
    // 買った土地と角でしか接しないチャンクは、買えるほうに入れない。
    let diagonal = write(
        &dir.join("diag.json"),
        r#"{"type":"BuildRoad","from":[120,120],"to":[135,135],"kind":"Small"}"#,
    );
    let acted = json(&cli(&["agent", "act", d, &diagonal]));
    let diag = &acted["results"][0]["notOwned"];
    assert_eq!(diag["chunks"], serde_json::json!([]), "{acted}");
    assert_eq!(
        diag["chunksNotAdjacent"],
        serde_json::json!([[2, 2]]),
        "{acted}"
    );
    // 返されたチャンクを買えば、同じ道路が通る。
    let buy = write(
        &dir.join("buy.json"),
        r#"[{"type":"BuyChunk","chunk":[2,0]},
            {"type":"BuildRoad","from":[120,0],"to":[135,0],"kind":"Small"}]"#,
    );
    let bought = json(&cli(&["agent", "act", d, &buy]));
    assert_eq!(bought["results"][0]["outcome"], "Applied", "{bought}");
    assert_eq!(bought["results"][1]["outcome"], "Applied", "{bought}");
}

/// observe は、前の observe からの増減を出す。フィードバックのディレクトリには何も書かない。
#[test]
fn observe_shows_changes_since_the_last_observe() {
    let dir = session("agent-observe-diff");
    let d = dir.to_str().unwrap();
    let made = json(&cli(&["agent", "new", d, "42"]));
    assert!(made.get("sinceLastObserve").is_none(), "{made}");
    let first = json(&cli(&["agent", "observe", d]));
    assert_eq!(first["sinceLastObserve"]["fromTick"], 0, "{first}");
    assert_eq!(
        first["sinceLastObserve"]["changes"],
        serde_json::json!({}),
        "{first}"
    );
    let cmds = write(
        &dir.join("c.json"),
        r#"{"type":"BuildRoad","from":[0,0],"to":[10,0],"kind":"Small"}"#,
    );
    cli(&["agent", "act", d, &cmds]);
    cli(&["agent", "advance", d, "100"]);
    let second = json(&cli(&["agent", "observe", d]));
    let diff = &second["sinceLastObserve"];
    assert_eq!(diff["fromTick"], 0, "{second}");
    assert_eq!(diff["toTick"], 100, "{second}");
    assert_eq!(diff["changes"]["money"]["from"], 500000, "{second}");
    assert!(
        diff["changes"]["money"]["delta"].as_i64().unwrap() < 0,
        "{second}"
    );
    let third = json(&cli(&["agent", "observe", d]));
    assert_eq!(third["sinceLastObserve"]["fromTick"], 100, "{third}");
    assert_eq!(third["sinceLastObserve"]["changes"], serde_json::json!({}));

    let fb = make_feedback("agent-observe-diff-feedback", 0);
    let before = files(&fb);
    let seen = json(&cli(&["agent", "observe", fb.to_str().unwrap()]));
    assert!(seen["sinceLastObserve"].is_null(), "{seen}");
    assert_eq!(files(&fb), before, "フィードバックのディレクトリが変わった");
}

/// 電気が届かなくなった建物は、地図で区画の文字の代わりに印が付く。
#[test]
fn map_marks_buildings_without_power_or_water() {
    let (dir, x, y) = supplied_town("agent-map-unsupplied");
    let d = dir.to_str().unwrap();
    let supplied = town_map(d, x, y);
    assert!(map_body(&supplied).contains('R'), "{supplied}");
    for mark in ['*', '^', '%'] {
        assert!(!map_body(&supplied).contains(mark), "{supplied}");
    }
    // 発電所を壊すと、建物は残って電気だけが届かなくなる。
    bulldoze(&dir, [x + 27, y - 4, x + 30, y - 1]);
    let dark = town_map(d, x, y);
    assert!(map_body(&dark).contains('*'), "{dark}");
    assert!(!map_body(&dark).contains('R'), "{dark}");
    assert!(dark.contains("電気か水が届いていない建物"), "{dark}");
    // 浄水場も壊すと、両方届かない。
    bulldoze(&dir, [x + 36, y - 2, x + 37, y - 1]);
    let both = town_map(d, x, y);
    assert!(map_body(&both).contains('%'), "{both}");
    assert!(!map_body(&both).contains('*'), "{both}");
    assert!(!map_body(&both).contains('^'), "{both}");
}

/// 浄水場だけを壊すと、水だけが届かない建物が `^` になる。
#[test]
fn map_marks_buildings_without_water_only() {
    let (dir, x, y) = supplied_town("agent-map-dry");
    let d = dir.to_str().unwrap();
    bulldoze(&dir, [x + 36, y - 2, x + 37, y - 1]);
    let dry = town_map(d, x, y);
    assert!(map_body(&dry).contains('^'), "{dry}");
    assert!(!map_body(&dry).contains('*'), "{dry}");
    assert!(!map_body(&dry).contains('%'), "{dry}");
    assert!(!map_body(&dry).contains('R'), "{dry}");
}

/// 凡例を除いた、地図の本体の行。
fn map_body(text: &str) -> String {
    text.lines().skip(14).collect::<Vec<_>>().join("\n")
}

fn town_map(d: &str, x: i64, y: i64) -> String {
    let (x0, y0, x1, y1) = (
        x.to_string(),
        (y - 6).to_string(),
        (x + 40).to_string(),
        (y + 7).to_string(),
    );
    cli(&["agent", "map", d, &x0, &y0, &x1, &y1])
}

fn bulldoze(dir: &Path, [x0, y0, x1, y1]: [i64; 4]) {
    let file = write(
        &dir.join("b.json"),
        &serde_json::json!({"type":"Bulldoze","area":[[x0,y0],[x1,y1]]}).to_string(),
    );
    cli(&["agent", "act", dir.to_str().unwrap(), &file]);
}

/// 高速道路から道路を引き、住宅と工業の区画、発電所、浄水場を置いて 2 日進めた街。(x, y) は道路の始点である。
/// 職場がないと、職のない住民が増えて住宅の需要が止まり、町が育たない。
fn supplied_town(name: &str) -> (PathBuf, i64, i64) {
    let dir = session(name);
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let end = json(&cli(&["agent", "observe", d]))["highway"]["end"].clone();
    let [x, y] = [end[0].as_i64().unwrap(), end[1].as_i64().unwrap()];
    let setup = write(
        &dir.join("setup.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[x,y],"to":[x+40,y],"kind":"Medium"},
            {"type":"PaintZone","area":[[x+1,y+1],[x+40,y+6]],"zone":"Residential"},
            {"type":"PaintZone","area":[[x+1,y-6],[x+20,y-1]],"zone":"Industrial"},
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[x+30,y-1],"rotation":2},
            {"type":"PlaceBuilding","kind":"WaterPump","at":[x+36,y-1],"rotation":2},
        ])
        .to_string(),
    );
    let acted = json(&cli(&["agent", "act", d, &setup]));
    for r in acted["results"].as_array().unwrap() {
        assert_eq!(r["outcome"], "Applied", "{acted}");
    }
    cli(&["agent", "advance", d, &(2 * 7200).to_string()]);
    (dir, x, y)
}

/// 町が育って需要が伸びているとき、trend は 1 日前の同じ時刻からの需要の伸びと、尽きるまでの時間を出す。
/// 数は、前の observe の需要と今回の余りから式どおりに求めた値と一致する。
#[test]
fn observe_gives_the_trend_of_growing_demand() {
    let (dir, _, _) = supplied_town("agent-trend-shrinking");
    let d = dir.to_str().unwrap();
    let first = json(&cli(&["agent", "observe", d]));
    let before = first["utilities"]["power"]["main"]["demand"]
        .as_i64()
        .unwrap();
    cli(&["agent", "advance", d, "7200"]);
    let o = json(&cli(&["agent", "observe", d]));
    let power = &o["utilities"]["power"];
    let main = &power["main"];
    let demand = main["demand"].as_i64().unwrap();
    let headroom = i128::from(main["headroom"].as_i64().unwrap());
    assert!(
        demand > before,
        "需要が伸びる街にならなかった: {before} -> {demand}"
    );
    assert!(headroom > 0, "足りなくなった: {headroom}");
    let trend = &main["trend"];
    assert_eq!(trend["state"], "shrinking", "{trend}");
    assert_eq!(trend["basis"], "demandSameTimeYesterday");
    assert_eq!(trend["targetTick"], 7200 * 2);
    assert_eq!(trend["demandBefore"], before);
    assert_eq!(trend["demandNow"], demand);
    let growth = i128::from(demand - before);
    assert_eq!(
        i128::from(trend["demandGrowthPerDay"].as_i64().unwrap()),
        growth
    );
    let need = i128::from(power["demand"].as_u64().unwrap());
    let housed = i128::from(o["population"]["housedCitizens"].as_u64().unwrap());
    assert_eq!(
        i128::from(trend["changePerDay"].as_i64().unwrap()),
        (-growth * housed).div_euclid(need)
    );
    let ticks = headroom * 7200 / growth;
    assert_eq!(i128::from(trend["ticksUntilOut"].as_i64().unwrap()), ticks);
    let hours = trend["hoursUntilOut"].as_f64().unwrap();
    assert!((hours - ticks as f64 / 300.0).abs() < 0.06, "{trend}");
}

/// 60 tick 以内にもう一度 observe しても、ちょうど 1 日前の控えは消えず、1 日後の trend が見積もれる。
#[test]
fn observe_trend_survives_a_second_observe_just_after() {
    let (dir, _, _) = supplied_town("agent-trend-close");
    let d = dir.to_str().unwrap();
    cli(&["agent", "observe", d]);
    cli(&["agent", "advance", d, "30"]);
    cli(&["agent", "observe", d]);
    cli(&["agent", "advance", d, "7170"]);
    let o = json(&cli(&["agent", "observe", d]));
    let trend = &o["utilities"]["power"]["main"]["trend"];
    assert_eq!(trend["basis"], "demandSameTimeYesterday", "{trend}");
    assert_eq!(trend["targetTick"], 7200 * 2);
}

/// 前の observe から発電所を足していても、供給量は需要の伸びに入らないので、減り方を見積もれる。
#[test]
fn observe_trend_is_estimated_across_a_new_facility() {
    let (dir, x, y) = supplied_town("agent-trend-facility");
    let d = dir.to_str().unwrap();
    let first = json(&cli(&["agent", "observe", d]));
    let more = write(
        &dir.join("more.json"),
        &serde_json::json!([
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[x+24,y-1],"rotation":2},
        ])
        .to_string(),
    );
    let acted = json(&cli(&["agent", "act", d, &more]));
    assert_eq!(acted["results"][0]["outcome"], "Applied", "{acted}");
    cli(&["agent", "advance", d, "7200"]);
    let o = json(&cli(&["agent", "observe", d]));
    let main = &o["utilities"]["power"]["main"];
    assert!(
        main["capacity"].as_u64().unwrap()
            > first["utilities"]["power"]["main"]["capacity"]
                .as_u64()
                .unwrap(),
        "供給量が増えていない: {main}"
    );
    let trend = &main["trend"];
    assert_eq!(trend["basis"], "demandSameTimeYesterday", "{trend}");
    assert_eq!(trend["state"], "shrinking", "{trend}");
    let growth = main["demand"].as_i64().unwrap()
        - first["utilities"]["power"]["main"]["demand"]
            .as_i64()
            .unwrap();
    assert_eq!(trend["demandGrowthPerDay"], growth);
    // 余りは増えた供給量の分も含むが、伸びは需要だけで決まる。
    assert_eq!(
        i128::from(trend["ticksUntilOut"].as_i64().unwrap()),
        i128::from(main["headroom"].as_i64().unwrap()) * 7200 / i128::from(growth)
    );
}

/// 1 日前の tick をはさむ控えがないと、前の observe との差には戻らず、見積もらずに理由を出す。
#[test]
fn observe_trend_is_not_estimated_without_samples_around_yesterday() {
    let (dir, _, _) = supplied_town("agent-trend-long");
    let d = dir.to_str().unwrap();
    cli(&["agent", "observe", d]);
    cli(&["agent", "advance", d, "7201"]);
    let o = json(&cli(&["agent", "observe", d]));
    let trend = &o["utilities"]["power"]["main"]["trend"];
    assert_eq!(trend["state"], "unknown", "{trend}");
    assert!(trend["basis"].is_null());
    assert!(
        trend["reason"]
            .as_str()
            .unwrap()
            .contains("はさむ需要の控え"),
        "{trend}"
    );
}

/// 前の observe の記録が壊れていても、observe は落ちず、前の値なしとして上書きする。
#[test]
fn observe_survives_a_broken_last_observe_file() {
    let dir = session("agent-observe-broken");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let path = dir.join("last-observe.json");
    let broken = [
        "ここは JSON ではない",
        "[1, 2]",
        r#"{"tick":"x","values":{}}"#,
        r#"{"tick":0,"values":{"money":"文字列"}}"#,
        r#"{"tick":0,"values":{"money":-9223372036854775808}}"#,
        r#"{"tick":0,"values":{"money":18446744073709551615}}"#,
    ];
    for text in broken {
        std::fs::write(&path, text).unwrap();
        let seen = json(&cli(&["agent", "observe", d]));
        let diff = &seen["sinceLastObserve"];
        assert!(
            diff.is_null() || diff["changes"].is_object(),
            "{text}: {seen}"
        );
        // 上書きされて、次は普通に増減が出る。
        let again = json(&cli(&["agent", "observe", d]));
        assert_eq!(again["sinceLastObserve"]["changes"], serde_json::json!({}));
    }
}

/// export は、種、tick、ハッシュ、コミット、記録の base64 を、貼れる markdown にして出す。
#[test]
fn export_prints_a_reproduction_in_markdown() {
    let dir = session("agent-export");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let cmds = write(
        &dir.join("c.json"),
        r#"{"type":"BuildRoad","from":[0,0],"to":[10,0],"kind":"Small"}"#,
    );
    cli(&["agent", "act", d, &cmds]);
    cli(&["agent", "advance", d, "50"]);
    let before = files(&dir);
    let md = cli(&["agent", "export", d]);
    assert_eq!(files(&dir), before, "export が書き込んだ");
    let state = json(&std::fs::read_to_string(dir.join("state.json")).unwrap());
    let hash = state["stateHash"].as_str().unwrap();
    assert!(md.contains(&format!("`{hash}`")), "{md}");
    assert!(md.contains("| 種 | `42` |"), "{md}");
    assert!(md.contains("| tick | `50` |"), "{md}");
    assert!(md.contains("base64 -d > repro/commands.rec"), "{md}");
    assert!(md.contains("agent new play --from repro"), "{md}");
    let commit = md
        .lines()
        .find_map(|l| l.strip_prefix("| ビルドしたコミット | `"))
        .unwrap()
        .trim_end_matches("` |");
    let sha = commit.strip_suffix("-dirty").unwrap_or(commit);
    assert!(
        sha == "unknown" || (sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())),
        "{commit}"
    );
    assert_eq!(
        md.contains("コミットしていない変更を入れてビルドした"),
        commit.ends_with("-dirty")
    );
    // 貼られた 2 つのファイルを取り出して、new --from で同じ街を作り直せる。
    let block = |start: &str| -> String {
        let (_, rest) = md.split_once(start).unwrap();
        rest.split_once("EOF\n").unwrap().0.to_string()
    };
    let record = decode_base64(&block("base64 -d > repro/commands.rec <<'EOF'\n"));
    assert_eq!(record, std::fs::read(dir.join("commands.rec")).unwrap());
    let repro = session("agent-export-repro");
    std::fs::create_dir_all(&repro).unwrap();
    std::fs::write(repro.join("commands.rec"), record).unwrap();
    std::fs::write(
        repro.join("state.json"),
        block("cat > repro/state.json <<'EOF'\n"),
    )
    .unwrap();
    let play = session("agent-export-play");
    let made = json(&cli(&[
        "agent",
        "new",
        play.to_str().unwrap(),
        "--from",
        repro.to_str().unwrap(),
    ]));
    assert_eq!(made["copiedFrom"]["matches"], true, "{made}");
    assert_eq!(made["stateHash"], hash);
}

/// テスト用の base64 の復号（改行は読み飛ばす）。
fn decode_base64(text: &str) -> Vec<u8> {
    const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0_u32, 0);
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        acc = acc << 6 | ALPHABET.find(c).unwrap() as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xff) as u8);
        }
    }
    out
}

#[test]
fn map_marks_junctions_with_crosswalks_and_observe_counts_crossing_walkers() {
    let dir = session("agent-crosswalk");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[0,5],"to":[10,5],"kind":"Small"},
            {"type":"BuildRoad","from":[5,0],"to":[5,10],"kind":"Small"}]"#,
    );
    cli(&["agent", "act", d, &cmds]);
    let map = cli(&["agent", "map", d, "4", "4", "6", "6"]);
    assert_eq!(grid_rows(&map, 3, 3), [".1.", "1Z1", ".1."], "{map}");
    assert!(map.contains("Z Y      信号のない交差点"), "{map}");
    let seen = json(&cli(&["agent", "observe", d]));
    let walkers = &seen["traffic"]["walkers"];
    assert_eq!(walkers["atCrosswalks"], 0, "{seen}");
    assert_eq!(walkers["midblock"], 0, "{seen}");
    assert_eq!(seen["removedCrosswalks"], serde_json::json!([]), "{seen}");
}

#[test]
fn act_removes_and_restores_a_crosswalk_by_tile_and_observe_and_map_show_it() {
    let dir = session("agent-crosswalk-toggle");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[0,5],"to":[10,5],"kind":"Small"},
            {"type":"BuildRoad","from":[5,0],"to":[5,10],"kind":"Small"},
            {"type":"SetCrosswalk","at":[5,5],"arm":"S","on":false},
            {"type":"SetCrosswalk","at":[5,5],"arm":"N","on":false},
            {"type":"SetCrosswalk","at":[5,5],"arm":"S","on":false},
            {"type":"SetCrosswalk","at":[5,5],"arm":"SE","on":false},
            {"type":"SetCrosswalk","at":[0,5],"arm":"E","on":false},
            {"type":"SetCrosswalk","at":[3,3],"arm":"E","on":false},
            {"type":"SetCrosswalk","at":[5,5],"arm":"N","on":true}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds, "--verbose"]));
    let results = acted["results"].as_array().unwrap();
    let outcomes: Vec<(&str, Option<&str>)> = results
        .iter()
        .map(|r| (r["outcome"].as_str().unwrap(), r["reason"].as_str()))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("Applied", None),
            ("Applied", None),
            ("Applied", None),
            ("Applied", None),
            ("Denied", Some("CrosswalkUnchanged")),
            ("Denied", Some("NoCrosswalkArm")),
            ("Denied", Some("NotJunction")),
            ("Invalid", None),
            ("Applied", None),
        ],
        "{acted}"
    );
    // 記録に入るのはノードの ID の形である。
    assert!(results[2]["command"]["node"]["index"].is_u64(), "{acted}");
    assert_eq!(results[2]["command"]["arm"], "S");
    let seen = json(&cli(&["agent", "observe", d]));
    let removed = seen["removedCrosswalks"].as_array().unwrap();
    assert_eq!(removed.len(), 1, "{seen}");
    assert_eq!(removed[0]["at"], serde_json::json!([5, 5]));
    assert_eq!(removed[0]["arms"], serde_json::json!(["S"]));
    let map = cli(&["agent", "map", d, "4", "4", "6", "6"]);
    assert_eq!(grid_rows(&map, 3, 3), [".1.", "1Y1", ".1."], "{map}");
    // 戻すと Z に戻る。
    let back = write(
        &dir.join("back.json"),
        r#"{"type":"SetCrosswalk","at":[5,5],"arm":"S","on":true}"#,
    );
    cli(&["agent", "act", d, &back]);
    let map = cli(&["agent", "map", d, "4", "4", "6", "6"]);
    assert_eq!(grid_rows(&map, 3, 3), [".1.", "1Z1", ".1."], "{map}");
    let seen = json(&cli(&["agent", "observe", d]));
    assert_eq!(seen["removedCrosswalks"], serde_json::json!([]), "{seen}");
}

#[test]
fn map_marks_a_signal_with_removed_crosswalks_only_from_the_same_junction() {
    let dir = session("agent-crosswalk-scale");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    // (5, 5) の信号のある交差点と、(7, 5) の横断歩道を外した交差点を、6×6 マスの 1 文字にまとめる。
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[0,5],"to":[10,5],"kind":"Small"},
            {"type":"BuildRoad","from":[5,0],"to":[5,10],"kind":"Small"},
            {"type":"BuildRoad","from":[7,0],"to":[7,10],"kind":"Small"},
            {"type":"SetSignal","at":[5,5],"on":true},
            {"type":"SetCrosswalk","at":[7,5],"arm":"N","on":false}]"#,
    );
    cli(&["agent", "act", d, &cmds]);
    let cell = || {
        let map = cli(&["agent", "map", d, "3", "3", "8", "8", "--scale", "6"]);
        grid_rows(&map, 1, 1)[0].clone()
    };
    assert_eq!(cell(), "S");
    let more = write(
        &dir.join("more.json"),
        r#"{"type":"SetCrosswalk","at":[5,5],"arm":"N","on":false}"#,
    );
    cli(&["agent", "act", d, &more]);
    assert_eq!(cell(), "s");
}

/// 高速道路の端の隣から引いた道路は、端まで延びてつながる。延ばしたことは結果の joined に出る。
#[test]
fn act_joins_a_road_started_next_to_the_highway_end() {
    let dir = session("agent-join-highway");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let seen = json(&cli(&["agent", "observe", d]));
    let [x, y] = [
        seen["highway"]["end"][0].as_i64().unwrap(),
        seen["highway"]["end"][1].as_i64().unwrap(),
    ];
    let [ox, oy] = [
        seen["highway"]["outward"][0].as_i64().unwrap(),
        seen["highway"]["outward"][1].as_i64().unwrap(),
    ];
    let (from, to) = ([x - ox, y - oy], [x - ox * 11, y - oy * 11]);
    let cmds = write(
        &dir.join("c.json"),
        &serde_json::json!([{"type":"BuildRoad","from":from,"to":to,"kind":"Small"}]).to_string(),
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    let r = &acted["results"][0];
    assert_eq!(r["outcome"], "Applied", "{acted}");
    assert_eq!(r["joined"], serde_json::json!({"from": [x, y], "to": to}));
    assert!(r.get("warning").is_none(), "{acted}");
}

/// 道路の隣から引いた道路は交差点でつながり、どれにもつながらない道路には warning が付く。
#[test]
fn act_joins_next_to_a_road_and_warns_about_an_isolated_road() {
    let dir = session("agent-join-road");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[0,0],"to":[0,20],"kind":"Small"},
            {"type":"BuildRoad","from":[1,10],"to":[11,10],"kind":"Small"},
            {"type":"BuildRoad","from":[1,0],"to":[1,20],"kind":"Small"},
            {"type":"BuildRoad","from":[30,30],"to":[40,30],"kind":"Small"}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    let results = acted["results"].as_array().unwrap();
    assert!(results.iter().all(|r| r["outcome"] == "Applied"), "{acted}");
    // 1 本目は、高速道路にも届かないまま互いにだけつながるので、最後でも警告が残る。
    assert!(results[0]["warning"].is_string(), "{acted}");
    assert_eq!(results[1]["joined"]["from"], serde_json::json!([0, 10]));
    assert!(results[1].get("warning").is_none(), "{acted}");
    // 平行に隣り合う道路は、つながらず、延ばしもしない。
    assert!(results[2].get("joined").is_none(), "{acted}");
    assert!(results[3]["warning"].is_string(), "{acted}");
}

/// 配列の最後に、act の前からあった網へつながっていれば warning は出ない。
/// 互いにだけつながる塊と、離れた道路には warning が残る。
#[test]
fn act_warns_only_about_roads_still_isolated_at_the_end() {
    let dir = session("agent-join-order");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let first = write(
        &dir.join("a.json"),
        r#"[{"type":"BuildRoad","from":[0,0],"to":[0,20],"kind":"Small"}]"#,
    );
    cli(&["agent", "act", d, &first]);
    // 先に離れた縦の道路を引き、あとで横の連絡路が元の道路とつなぐ。
    // 30 番台の 2 本は互いにだけつながり、元の網には届かない。
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[10,0],"to":[10,20],"kind":"Small","join":false},
            {"type":"BuildRoad","from":[0,10],"to":[10,10],"kind":"Small","join":false},
            {"type":"BuildRoad","from":[30,30],"to":[40,30],"kind":"Small"},
            {"type":"BuildRoad","from":[35,25],"to":[35,35],"kind":"Small"}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    let results = acted["results"].as_array().unwrap();
    assert!(results.iter().all(|r| r["outcome"] == "Applied"), "{acted}");
    assert!(results[0].get("warning").is_none(), "{acted}");
    assert!(results[1].get("warning").is_none(), "{acted}");
    // 30 番台の 1 本目は元の網に届かない。2 本目はそれに交わるので、引いた時点で警告は付かない。
    assert!(results[2]["warning"].is_string(), "{acted}");
    assert!(results[3].get("warning").is_none(), "{acted}");
}

/// 同じ act で元の道路を壊し、その端のマスから引いた道路は、元の網に届いたとはみなさない。
#[test]
fn act_warns_about_a_road_drawn_from_the_end_of_a_bulldozed_road() {
    let dir = session("agent-join-bulldoze");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let first = write(
        &dir.join("a.json"),
        r#"[{"type":"BuildRoad","from":[30,30],"to":[40,30],"kind":"Small"}]"#,
    );
    cli(&["agent", "act", d, &first]);
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"Bulldoze","area":[[30,30],[40,30]]},
            {"type":"BuildRoad","from":[40,30],"to":[40,40],"kind":"Small"}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    let results = acted["results"].as_array().unwrap();
    assert_eq!(results[1]["outcome"], "Applied", "{acted}");
    assert!(results[1]["warning"].is_string(), "{acted}");
}

/// Bulldoze が Applied のとき、結果に消えた道路のマス、施設、建物の数と、家をなくした世帯の数が付く。
/// 道路は範囲にかかった区間ごと消えるので、1 マスを打っても区間の全体が数に出る。
#[test]
fn act_reports_what_a_bulldoze_removed() {
    let (dir, x, y) = supplied_town("agent-bulldoze-report");
    let d = dir.to_str().unwrap();

    // 発電所だけの範囲では、施設が 1 つ消え、道路と建物は消えない。
    let cmds = write(
        &dir.join("b1.json"),
        &serde_json::json!([{"type":"Bulldoze","area":[[x+30,y-4],[x+30,y-1]]}]).to_string(),
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    let r = &acted["results"][0];
    assert_eq!(r["outcome"], "Applied", "{acted}");
    assert_eq!(r["bulldozed"]["facilities"], 1, "{acted}");
    assert_eq!(r["bulldozed"]["roadTiles"], 0, "{acted}");
    assert_eq!(r["bulldozed"]["buildings"], 0, "{acted}");

    // 住宅はすべて、この道路に面している。取り壊す前に住んでいる世帯を数える。
    let observed = json(&cli(&["agent", "observe", d]));
    let by_density = &observed["buildings"]["residentialByDensity"];
    let housed = by_density["high"]["housedHouseholds"].as_u64().unwrap()
        + by_density["low"]["housedHouseholds"].as_u64().unwrap();
    assert!(housed > 0, "{observed}");

    // 道路の 1 マスだけを打っても、40 マスの区間が全部消え、沿道の建物も消える。
    let cmds = write(
        &dir.join("b2.json"),
        &serde_json::json!([{"type":"Bulldoze","area":[[x+10,y],[x+10,y]]}]).to_string(),
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    let r = &acted["results"][0];
    assert_eq!(r["outcome"], "Applied", "{acted}");
    assert_eq!(r["bulldozed"]["roadTiles"], 40, "{acted}");
    assert!(r["bulldozed"]["buildings"].as_u64().unwrap() > 0, "{acted}");
    assert_eq!(r["bulldozed"]["displacedHouseholds"], housed, "{acted}");
}

/// join:false なら隣にあっても延ばさない。join の書き方が違うときや BuildRoad 以外では Invalid になる。
#[test]
fn act_does_not_join_when_join_is_false() {
    let dir = session("agent-join-off");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let cmds = write(
        &dir.join("c.json"),
        r#"[{"type":"BuildRoad","from":[0,0],"to":[0,20],"kind":"Small"},
            {"type":"BuildRoad","from":[1,10],"to":[11,10],"kind":"Small","join":false},
            {"type":"BuildRoad","from":[1,5],"to":[11,5],"kind":"Small","join":true},
            {"type":"BuildRoad","from":[1,6],"to":[11,6],"kind":"Small","join":1},
            {"type":"SetSignal","at":[0,5],"on":true,"join":false}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmds, "--verbose"]));
    let r = acted["results"].as_array().unwrap();
    assert_eq!(r[1]["outcome"], "Applied", "{acted}");
    assert!(r[1].get("joined").is_none(), "{acted}");
    assert_eq!(r[1]["command"]["from"], serde_json::json!([1, 10]));
    assert_eq!(r[2]["joined"]["from"], serde_json::json!([0, 5]), "{acted}");
    assert_eq!(r[3]["outcome"], "Invalid", "{acted}");
    assert_eq!(r[4]["outcome"], "Invalid", "{acted}");
}

/// demandFactors の値を足し合わせると demand になる。
#[test]
fn observe_demand_factors_add_up_to_the_demand() {
    let dir = session("agent-demand-factors");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let obs = json(&cli(&["agent", "observe", d]));
    for zone in ["Residential", "Commercial", "Industrial", "Office"] {
        let factors = obs["demandFactors"][zone].as_array().unwrap();
        assert!(!factors.is_empty(), "{zone}: {obs}");
        let sum: i64 = factors.iter().map(|f| f["value"].as_i64().unwrap()).sum();
        assert_eq!(sum, obs["demand"][zone].as_i64().unwrap(), "{zone}: {obs}");
    }
    assert_eq!(obs["demandFactors"]["Residential"][0]["factor"], "base");
}

#[test]
fn paint_zone_skips_tiles_cut_off_from_the_road_and_says_so() {
    let dir = session("agent-paint-frontage");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let road = write(
        &dir.join("road.json"),
        r#"[{"type":"BuildRoad","from":[0,0],"to":[20,0],"kind":"Medium"}]"#,
    );
    cli(&["agent", "act", d, &road]);
    // 道路の脇（y=1）を空けて、その奥（y=2〜6）だけを塗る。道路から切れているので 1 マスも塗れない。
    let cut = write(
        &dir.join("cut.json"),
        r#"[{"type":"PaintZone","area":[[2,2],[8,6]],"zone":"Residential"}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cut]));
    let r = &acted["results"][0];
    assert_eq!(r["outcome"], "Denied", "{acted}");
    assert_eq!(r["reason"], "NoZonableTile", "{acted}");
    // 脇から奥まで一度に塗れば、続く所は塗れる。道路の端の外は間口がなく、塗れない。
    let ok = write(
        &dir.join("ok.json"),
        r#"[{"type":"PaintZone","area":[[2,1],[24,6]],"zone":"Residential"}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &ok]));
    let r = &acted["results"][0];
    assert_eq!(r["outcome"], "AppliedPartly", "{acted}");
    assert_eq!(r["reason"], "NoZonableTile", "{acted}");
}

/// PaintZone の結果に、塗れたマスの数と、塗れなかったマスの数が理由ごとに付く。
#[test]
fn paint_zone_counts_the_tiles_it_left_out_by_reason() {
    let dir = session("agent-paint-breakdown");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let setup = write(
        &dir.join("setup.json"),
        r#"[{"type":"BuildRoad","from":[0,0],"to":[20,0],"kind":"Medium"},
            {"type":"BuildRoad","from":[100,0],"to":[127,0],"kind":"Medium"},
            {"type":"PlaceBuilding","kind":"PowerPlant","at":[5,1],"rotation":0}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &setup]));
    for r in acted["results"].as_array().unwrap() {
        assert_eq!(r["outcome"], "Applied", "{acted}");
    }
    let paint = |name: &str, area: &str, zone: &str| {
        let file = write(
            &dir.join(format!("{name}.json")),
            &format!(r#"[{{"type":"PaintZone","area":{area},"zone":"{zone}"}}]"#),
        );
        let acted = json(&cli(&["agent", "act", d, &file]));
        acted["results"][0].clone()
    };
    // 道路の端の外は、道路から 6 マス以内でも間口がない。
    let r = paint("end", "[[10,1],[24,6]]", "Residential");
    assert_eq!(r["outcome"], "AppliedPartly", "{r}");
    assert_eq!(r["paint"]["painted"], 11 * 6, "{r}");
    assert_eq!(r["paint"]["unpainted"]["notFronted"], 4 * 6, "{r}");
    assert_eq!(r["paint"]["unpainted"]["total"], 4 * 6, "{r}");
    // 道路から 6 マスを超えた奥。
    let r = paint("deep", "[[12,-8],[14,-1]]", "Residential");
    assert_eq!(r["outcome"], "AppliedPartly", "{r}");
    assert_eq!(r["paint"]["unpainted"]["beyondDepth"], 2 * 3, "{r}");
    assert_eq!(r["paint"]["unpainted"]["total"], 2 * 3, "{r}");
    // 買っていない土地。
    let r = paint("unowned", "[[120,1],[135,3]]", "Residential");
    assert_eq!(r["outcome"], "AppliedPartly", "{r}");
    assert_eq!(r["paint"]["unpainted"]["notOwned"], 8 * 3, "{r}");
    // 施設のあるマスに別の区画を塗ると、そのマスは塗れない。
    let r = paint("mixed", "[[3,1],[10,4]]", "Commercial");
    assert_eq!(r["outcome"], "AppliedPartly", "{r}");
    assert_eq!(r["paint"]["unpainted"]["building"], 16, "{r}");
    // 塗れるマスが 1 つもなく、理由が建物なら Occupied。
    let r = paint("occupied", "[[5,1],[8,4]]", "Industrial");
    assert_eq!(r["outcome"], "Denied", "{r}");
    assert_eq!(r["reason"], "Occupied", "{r}");
    assert_eq!(r["paint"]["painted"], 0, "{r}");
    assert_eq!(r["paint"]["unpainted"]["building"], 16, "{r}");
    // 全部塗れたときは Applied で、paint は付かない。
    let r = paint("whole", "[[16,-6],[18,-1]]", "Residential");
    assert_eq!(r["outcome"], "Applied", "{r}");
    assert!(r.get("paint").is_none(), "{r}");
}

/// 同じ区画が載ったマスは、空きでも建物が建っていても、塗れたものとして同じに数える。
#[test]
fn paint_zone_counts_same_zone_tiles_alike_whether_empty_or_built() {
    let dir = session("agent-paint-same-zone");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let setup = write(
        &dir.join("setup.json"),
        r#"[{"type":"BuildRoad","from":[-63,25],"to":[-20,25],"kind":"Medium"},
            {"type":"PaintZone","area":[[-60,26],[-25,31]],"zone":"Residential"}]"#,
    );
    cli(&["agent", "act", d, &setup]);
    cli(&["agent", "advance", d, "3000"]);
    let paint = |name: &str, area: &str| {
        let file = write(
            &dir.join(format!("{name}.json")),
            &format!(r#"[{{"type":"PaintZone","area":{area},"zone":"Residential"}}]"#),
        );
        json(&cli(&["agent", "act", d, &file]))["results"][0].clone()
    };
    // 奥行きの外 (y=32〜36) は塗れない。x=-33 の列を足しても、塗れたマスが増えるだけで理由は同じである。
    let a = paint("a", "[[-32,26],[-29,36]]");
    let b = paint("b", "[[-33,26],[-29,36]]");
    for r in [&a, &b] {
        assert_eq!(r["outcome"], "Denied", "{r}");
        assert_eq!(r["reason"], "NoZonableTile", "{r}");
    }
    assert_eq!(a["paint"]["unpainted"]["beyondDepth"], 5 * 4, "{a}");
    assert_eq!(b["paint"]["unpainted"]["beyondDepth"], 5 * 5, "{b}");
    assert_eq!(a["paint"]["painted"], 6 * 4, "{a}");
    assert_eq!(b["paint"]["painted"], 6 * 5, "{b}");
    // 塗れなかったマスがなければ ZoneUnchanged で、paint は付く。
    let r = paint("c", "[[-32,26],[-29,31]]");
    assert_eq!(r["outcome"], "Denied", "{r}");
    assert_eq!(r["reason"], "ZoneUnchanged", "{r}");
    assert_eq!(r["paint"]["unpainted"]["total"], 0, "{r}");
    assert_eq!(r["paint"]["painted"], 4 * 6, "{r}");
}

/// 低密度の住宅を塗ると、observe が密度ごとに数え、map が D と d で出す。
#[test]
fn low_density_homes_show_in_observe_and_map() {
    let dir = session("agent-low-density");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let highway = json(&cli(&["agent", "observe", d]))["highway"]["end"].clone();
    let [x, y] = [highway[0].as_i64().unwrap(), highway[1].as_i64().unwrap()];
    let cmds = write(
        &dir.join("low.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[x,y],"to":[x+40,y],"kind":"Medium"},
            {"type":"PaintZone","area":[[x+1,y+1],[x+40,y+6]],"zone":"ResidentialLow"},
            {"type":"PaintZone","area":[[x+1,y-6],[x+40,y-1]],"zone":"Commercial"},
        ])
        .to_string(),
    );
    let acted = json(&cli(&["agent", "act", d, &cmds]));
    for r in acted["results"].as_array().unwrap() {
        assert_eq!(r["outcome"], "Applied", "{acted}");
    }
    cli(&["agent", "advance", d, "3000"]);
    let seen = json(&cli(&["agent", "observe", d]));
    let density = &seen["buildings"]["residentialByDensity"];
    assert!(density["low"]["buildings"].as_u64().unwrap() > 0, "{seen}");
    assert_eq!(density["high"]["buildings"], 0, "{seen}");
    assert_eq!(
        density["low"]["buildings"],
        seen["buildings"]["byZone"]["Residential"]
    );
    assert!(density["low"]["households"].as_u64().unwrap() > 0, "{seen}");
    let tiles = &seen["zoneTiles"];
    assert_eq!(tiles["ResidentialLow"]["painted"], 40 * 6, "{seen}");
    assert_eq!(tiles["Residential"]["painted"], 0, "{seen}");
    assert!(
        tiles["ResidentialLow"]["empty"].as_u64().unwrap() < 40 * 6,
        "{seen}"
    );
    let (x0, x1) = ((x + 1).to_string(), (x + 40).to_string());
    let (y0, y1) = ((y + 1).to_string(), (y + 6).to_string());
    let map = cli(&["agent", "map", d, &x0, &y0, &x1, &y1]);
    // 行頭に y の目盛りのある行だけを見る。凡例にも R と D の文字がある。
    let body: String = map
        .lines()
        .filter_map(|l| l.split_once(' '))
        .filter(|(y, _)| y.parse::<i64>().is_ok())
        .map(|(_, row)| row)
        .collect();
    assert!(body.contains('D') || body.contains('d'), "{map}");
    assert!(!body.contains('R') && !body.contains('r'), "{map}");
}

/// act で駐車場の料金を決めると、observe の parking.fee に出て、範囲の外の料金は断られる。
#[test]
fn act_sets_the_parking_fee() {
    let dir = session("agent-parking-fee");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "8"]);
    let seen = json(&cli(&["agent", "observe", d]));
    assert_eq!(seen["parking"]["fee"], 10);
    assert_eq!(seen["financesPerDay"]["parkingFees"], 0);
    let cmd = write(
        &dir.join("fee.json"),
        r#"[{"type":"SetParkingFee","fee":30},{"type":"SetParkingFee","fee":51},{"type":"SetParkingFee","fee":-1},{"type":"SetTaxRate","zone":"ResidentialLow","permille":100}]"#,
    );
    let acted = json(&cli(&["agent", "act", d, &cmd]));
    let messages: Vec<&str> = acted["results"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|r| r["message"].as_str().unwrap())
        .collect();
    assert_eq!(messages[0], "駐車場の料金 51 は 0 から 50 の外にある");
    assert_eq!(messages[1], "駐車場の料金 -1 は 0 から 50 の外にある");
    assert!(
        messages[2].contains("ResidentialLow は選べない"),
        "{messages:?}"
    );
    let outcomes: Vec<&str> = acted["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(outcomes, ["Applied", "Invalid", "Invalid", "Invalid"]);
    let seen = json(&cli(&["agent", "observe", d]));
    assert_eq!(seen["parking"]["fee"], 30);
    assert_eq!(
        seen["sinceLastObserve"]["changes"]["parking.fee"]["delta"],
        20
    );
}
