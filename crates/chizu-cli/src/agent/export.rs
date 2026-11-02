//! `agent export`：セッションの再現手順を、Issue にそのまま貼れる markdown にする。
//!
//! 種、tick、状態ハッシュ、ビルドしたコミットと、`commands.rec` の base64 を入れる。
//! 受け取った人は、手順の貼り付けだけで `agent new --from` に渡せるディレクトリを作れる。

use serde_json::{Map, Value, json};

/// ビルドしたコミット。git がなければ `unknown`。
pub const BUILD_COMMIT: &str = env!("CHIZU_GIT_COMMIT");

/// 1 行に並べる base64 の文字数。`base64 -d` は改行を読み飛ばす。
const LINE: usize = 76;

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// 標準の base64（パディングあり）。
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0_u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// 再現に要る欄だけの `state.json`。カメラの位置など、再現に関わらない欄は入れない。
pub fn repro_state(state: &Map<String, Value>, seed: u64, tick: u64, hash: &str) -> String {
    let mut text = serde_json::to_string_pretty(&json!({
        "seed": seed.to_string(),
        "tick": tick,
        "money": state.get("money").cloned().unwrap_or(Value::Null),
        "stateHash": hash,
    }))
    .expect("JSON に書けない");
    text.push('\n');
    text
}

pub struct Export<'a> {
    pub seed: u64,
    pub tick: u64,
    /// `state.json` に書いてあるハッシュ。なければ `None`。
    pub recorded_hash: Option<&'a str>,
    /// 記録を再生して求めたハッシュ。
    pub replayed_hash: &'a str,
    pub commands: usize,
    pub record: &'a [u8],
    pub state_json: &'a str,
}

pub fn markdown(e: &Export) -> String {
    let hash = e.recorded_hash.unwrap_or(e.replayed_hash);
    let encoded = base64(e.record);
    let wrapped: Vec<&str> = encoded
        .as_bytes()
        .chunks(LINE)
        .map(|c| std::str::from_utf8(c).expect("base64 は ASCII"))
        .collect();
    let wrapped = wrapped.join("\n");
    let mut out = String::new();
    out.push_str("## 再現の手順\n\n");
    out.push_str("| 項目 | 値 |\n| --- | --- |\n");
    out.push_str(&format!("| 種 | `{}` |\n", e.seed));
    out.push_str(&format!("| tick | `{}` |\n", e.tick));
    out.push_str(&format!("| stateHash | `{hash}` |\n"));
    out.push_str(&format!("| ビルドしたコミット | `{BUILD_COMMIT}` |\n"));
    out.push_str(&format!("| 記録したコマンド | {} 件 |\n", e.commands));
    if let Some(recorded) = e.recorded_hash
        && recorded != e.replayed_hash
    {
        out.push_str(&format!(
            "\n注意：記録を再生したハッシュは `{}` で、state.json の stateHash と食い違っている。\
             画面の側にだけ残る操作があるか、シミュレーションの規則が変わったおそれがある。\n",
            e.replayed_hash
        ));
    }
    out.push_str(
        "\nリポジトリを上のコミットに合わせてから、次を貼り付けて実行する。\
         `base64 -d` で記録に戻し、`agent new --from` で続きを遊べるセッションを作る。\n\n",
    );
    out.push_str("```sh\nmkdir repro\nbase64 -d > repro/commands.rec <<'EOF'\n");
    out.push_str(&wrapped);
    out.push_str("\nEOF\ncat > repro/state.json <<'EOF'\n");
    out.push_str(e.state_json);
    out.push_str("EOF\ncargo run -p chizu-cli -- agent new play --from repro\n```\n\n");
    out.push_str(&format!(
        "出力の `copiedFrom.matches` が `true` なら、再生したハッシュが `{hash}` と一致していて、同じ街を作り直せている。\
         そのあとは `agent observe play` で状態を、`agent map play ...` で地図を見られる。\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+");
    }
}
