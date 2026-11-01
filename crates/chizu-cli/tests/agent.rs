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

/// 道路と区画を置いた小さな街。道路は y=0 の x=0〜20 を Medium で引き、区画は x=0〜24、y=1〜6 の住宅である。
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
    // x -2〜22、y -2〜7。区画は x=24 までなので、右端の外までは出ない。
    let map = cli(&["agent", "map", d, "-2", "-2", "22", "7"]);
    assert!(map.contains("凡例"), "{map}");
    assert!(map.contains("25×10 文字"), "{map}");
    let flat = ".".repeat(25);
    let road = format!("..{}..", "2".repeat(21));
    let zone = format!("..{}", "r".repeat(23));
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
