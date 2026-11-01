//! CLI でゴールデンの記録を再生し、記録したハッシュと一致するかを見る。

use std::path::PathBuf;
use std::process::Command;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../chizu-core/tests/golden")
        .join(name)
}

#[test]
fn replays_golden_record() {
    // 種と tick 数は chizu-core/tests/golden.rs の clock_and_commands と同じ。
    let out = Command::new(env!("CARGO_BIN_EXE_chizu-cli"))
        .arg(golden("clock_and_commands.rec"))
        .args(["42", "14400"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let expected = std::fs::read_to_string(golden("clock_and_commands.hash")).unwrap();
    assert_eq!(String::from_utf8(out.stdout).unwrap(), expected);
}

#[test]
fn fails_on_bad_arguments() {
    let out = Command::new(env!("CARGO_BIN_EXE_chizu-cli"))
        .arg("only-one")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
}
