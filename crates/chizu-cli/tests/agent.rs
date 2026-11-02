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
    assert_eq!(n(&cars["total"]), n(&cars["moving"]) + n(&cars["stopped"]));
    assert_eq!(n(&cars["stoppedAfterTicks"]), 100);
    let trips = &o["traffic"]["trips"];
    let yesterday = &trips["yesterday"];
    assert!(n(&yesterday["arrived"]) > 0, "前の日に誰も着いていない");
    assert!(yesterday["avgTravelTicks"].as_f64().unwrap() > 0.0);
    let m = &o["migration"];
    assert!(m["vacantHomes"].is_u64() && m["blockedHouseholds"].is_u64());
    // 同じ記録を再生した observe は、前の observe からの増減を除いて、毎回同じ値になる。
    let mut again = json(&cli(&["agent", "observe", d]));
    assert_eq!(again["sinceLastObserve"]["changes"], serde_json::json!({}));
    again["sinceLastObserve"] = o["sinceLastObserve"].clone();
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
    assert_eq!(power["headroom"], 0);
    assert!(power["headroomPermille"].is_null());
    assert_eq!(power["lowHeadroom"], false);
    assert_eq!(power["shortAt"], serde_json::json!([]));
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
    assert!(power["headroom"].as_i64().unwrap() < 0);
    assert_eq!(power["shortNetworks"], 1);
    let at = &power["shortAt"][0]["at"];
    assert!(at[0].is_i64() && at[1].is_i64(), "{power}");
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
    }
    cli(&["agent", "advance", d, "6000"]);
    let obs = json(&cli(&["agent", "observe", d]));
    let res = &obs["growth"]["Residential"];
    assert!(res["fronts"].as_u64().unwrap() > 0, "{res}");
    assert_eq!(res["blocked"], res["fronts"], "{res}");
    assert_eq!(res["noPower"], res["fronts"], "{res}");
    assert_eq!(res["noWater"], res["fronts"], "{res}");
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
    let dir = session("agent-map-unsupplied");
    let d = dir.to_str().unwrap();
    cli(&["agent", "new", d, "42"]);
    let end = json(&cli(&["agent", "observe", d]))["highway"]["end"].clone();
    let [x, y] = [end[0].as_i64().unwrap(), end[1].as_i64().unwrap()];
    let setup = write(
        &dir.join("setup.json"),
        &serde_json::json!([
            {"type":"BuildRoad","from":[x,y],"to":[x+40,y],"kind":"Medium"},
            {"type":"PaintZone","area":[[x+1,y+1],[x+40,y+6]],"zone":"Residential"},
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
    let (x0, y0, x1, y1) = (
        x.to_string(),
        (y - 6).to_string(),
        (x + 40).to_string(),
        (y + 7).to_string(),
    );
    let map = || cli(&["agent", "map", d, &x0, &y0, &x1, &y1]);
    // 凡例は先頭の 11 行ほどなので、道路の行以降だけを調べる。
    let body = |text: &str| text.lines().skip(11).collect::<Vec<_>>().join("\n");
    let supplied = map();
    assert!(body(&supplied).contains('R'), "{supplied}");
    assert!(!body(&supplied).contains('*'), "{supplied}");
    // 発電所を壊すと、建物は残って電気だけが届かなくなる。
    let bulldoze = write(
        &dir.join("b.json"),
        &serde_json::json!({"type":"Bulldoze","area":[[x+27,y-4],[x+30,y-1]]}).to_string(),
    );
    cli(&["agent", "act", d, &bulldoze]);
    let dark = map();
    assert!(body(&dark).contains('*'), "{dark}");
    assert!(!body(&dark).contains('R'), "{dark}");
    assert!(dark.contains("電気か水が届いていない建物"), "{dark}");
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
    assert!(
        commit == "unknown"
            || (commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit())),
        "{commit}"
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
