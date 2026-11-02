//! ビルドしたコミットを `CHIZU_GIT_COMMIT` に埋め込む。`agent export` が再現の手順に書く。
//! コミットしていない変更があるときは、末尾に `-dirty` を付ける。

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// ファイルがあるときだけ見張る。ないファイルを指すと、cargo は毎回作り直す。
fn watch(git_path: &str) {
    if let Some(path) = git(&["rev-parse", "--git-path", git_path])
        && Path::new(&path).exists()
    {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // HEAD、HEAD が指すブランチの先、index（git add）が動いたら埋め直す。
    watch("HEAD");
    watch("packed-refs");
    watch("index");
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch(&branch);
    }
    let commit = git(&["rev-parse", "HEAD"])
        .filter(|s| !s.is_empty())
        .map(|sha| {
            // 追跡していないファイルも含めて、作業ツリーに変更があるか。
            let dirty = git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty());
            if dirty { format!("{sha}-dirty") } else { sha }
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=CHIZU_GIT_COMMIT={commit}");
}
