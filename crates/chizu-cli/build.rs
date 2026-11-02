//! ビルドしたコミットを `CHIZU_GIT_COMMIT` に埋め込む。`agent export` が再現の手順に書く。

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // HEAD と、HEAD が指すブランチの先が動いたら埋め直す。
    for target in ["HEAD", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", target]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"])
        && let Some(path) = git(&["rev-parse", "--git-path", &branch])
    {
        println!("cargo:rerun-if-changed={path}");
    }
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CHIZU_GIT_COMMIT={commit}");
}
