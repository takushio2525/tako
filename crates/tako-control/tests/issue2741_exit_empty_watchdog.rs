//! **#2741 の番犬**: 器（tmux）のサーバーは、最後のセッションが消えても終わらない（`exit-empty off`）。
//!
//! ## なぜ縛るのか
//!
//! tmux の既定（`exit-empty on`）では、最後のセッションが消えるとサーバーは「終わる」と決め、プロセスが
//! 消えるまで待ち受けのソケットを開いたままにする。その間に作った器のペインのクライアント
//! （`new-session -A -D`）は connect が通るのに受け付けられず、`server exited unexpectedly` で終わる =
//! 作った直後の器のペインが即死する（隔離セルフテストの項目 739 / 749 の間欠。混んだ機で隙間が延びる）。
//! 器の conf が `exit-empty off` を立て、走っているサーバーへも `sync_conf` で届けることで隙間そのものを無くした。
//!
//! ## 何を確かめるか（外れたら file:line を名指して落とす）
//!
//! 1. 器の conf の本文（`conf_body`）が `exit_empty_conf(` を A/B のゲート `issue2741_legacy()` つきで通る
//! 2. `exit_empty_conf` は能力 `keeps_empty_server` を見て `exit-empty` を書く
//!    （A/B は tmux の既定の `on` を書く = 走っているサーバーも `sync_conf` で戻る）
//! 3. 走っているサーバーへの再適用 `sync_conf` が `ensure_conf()` を `source-file` で読ませる
//! 4. 器のペインのクライアント（`wrap_options_for`）が `-f` で `ensure_conf()` を渡して器を起こす
//! 5. tmux の器の能力が `keeps_empty_server: true`（psmux は未実測なので false = 1 行も書かない。#974）
//! 6. A/B のゲート `issue2741_legacy` は `TAKO_2741_LEGACY` を読む 1 か所
//!
//! 利用者の tmux（`tako tmux open`）へはこの conf を読ませない（器のソケットにだけ効く）ことは、
//! `ensure_conf()` を使うのが器の入口 2 つ（`wrap_options_for` と `sync_conf`）だけであることで縛る（7）。

use std::path::{Path, PathBuf};

const TMUX_BACKEND: &str = "crates/tako-core/src/tmux_backend.rs";
const TMUX_CAPS: &str = "crates/tako-core/src/backend/tmux.rs";
const PSMUX_CAPS: &str = "crates/tako-core/src/backend/psmux.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す）
fn read(rel: &str) -> String {
    // tmux_backend.rs はテストが本番より厚い（本番の割合は 2 割台）ので下限を下げて読む（#2625 の番犬と同じ扱い）
    let floor = if rel == TMUX_BACKEND {
        0.20
    } else {
        production_range::MIN_COVERAGE
    };
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::code_view::without_comments(&production_range::production_with_floor(
        &src, rel, floor,
    ))
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置（文字列の中の括弧は数えない）
fn block_end(src: &str, open: usize) -> Option<usize> {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let start = open + code[open..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `sig` で始まる関数の本体（無ければ None）
fn body_of<'a>(src: &'a str, sig: &str) -> Option<(usize, &'a str)> {
    let at = src.find(sig)?;
    let end = block_end(src, at)?;
    Some((at, &src[at..end]))
}

/// 1〜4・6・7: 器の conf と入口（`tmux_backend.rs` の本番コード）
fn check_backend(rel: &str, src: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut need = |sig: &str, needles: &[&str], why: &str| match body_of(src, sig) {
        None => hits.push(format!("{rel}:1 `{sig}` が無い（{why}）")),
        Some((at, body)) => {
            for needle in needles {
                if !body.contains(needle) {
                    hits.push(format!(
                        "{rel}:{} `{sig}` に `{needle}` が無い（{why}）",
                        line_at(src, at)
                    ));
                }
            }
        }
    };
    need(
        "fn conf_body(",
        &["exit_empty_conf(", "issue2741_legacy()"],
        "器の conf が exit-empty を立てないと、最後のセッションが消えたサーバーの終わり際に作った器のペインが即死する",
    );
    need(
        "pub fn exit_empty_conf(",
        &["keeps_empty_server", "exit-empty"],
        "能力のある器へだけ exit-empty を書く。能力を見ないと psmux へ知らない行を書く = #974",
    );
    need(
        "pub fn sync_conf(",
        &["ensure_conf()", "\"source-file\""],
        "走っているサーバーは -f の conf を読み直さない。再適用が外れると、修正前に起きたサーバーへ exit-empty off が届かない",
    );
    need(
        "pub(crate) fn wrap_options_for(",
        &["ensure_conf()", "\"-f\""],
        "器のペインのクライアントが器を起こすときに conf を渡さないと、新しい器のサーバーに exit-empty off が立たない",
    );
    need(
        "pub fn issue2741_legacy(",
        &["\"TAKO_2741_LEGACY\""],
        "A/B のゲートはこの 1 か所",
    );
    // 7: 器の conf を使う口は器の入口 2 つだけ（利用者の tmux へ読ませない）
    let allowed: Vec<(usize, usize)> = ["pub(crate) fn wrap_options_for(", "pub fn sync_conf("]
        .iter()
        .filter_map(|sig| {
            let at = src.find(sig)?;
            Some((at, block_end(src, at)?))
        })
        .collect();
    let defined = src.find("pub fn ensure_conf(");
    for (at, _) in src.match_indices("ensure_conf()") {
        let inside = allowed.iter().any(|(s, e)| (*s..*e).contains(&at))
            || defined.is_some_and(|d| at < d + "pub fn ensure_conf(".len() + 2 && at >= d);
        if !inside {
            hits.push(format!(
                "{rel}:{} 器の入口（`wrap_options_for` / `sync_conf`）の外で `ensure_conf()` を使っている（器の conf = \
                 exit-empty off を利用者の tmux へ読ませうる。#2741 は器のサーバーにだけ効かせる）",
                line_at(src, at)
            ));
        }
    }
    hits
}

/// 5: 器の能力の申告
fn check_caps(rel: &str, src: &str, expect: bool) -> Vec<String> {
    let want = format!("keeps_empty_server: {expect}");
    if src.contains(&want) {
        return Vec::new();
    }
    let at = src.find("keeps_empty_server").unwrap_or(0);
    vec![format!(
        "{rel}:{} の器の能力に `{want}` が無い（tmux は exit-empty off で器を残せる = true、psmux は未実測 = false。#2741 / #974）",
        line_at(src, at)
    )]
}

#[test]
fn issue2741_器のサーバーは最後のセッションが消えても終わらない設定を器の入口から外さない() {
    let mut hits = check_backend(TMUX_BACKEND, &read(TMUX_BACKEND));
    hits.extend(check_caps(TMUX_CAPS, &read(TMUX_CAPS), true));
    hits.extend(check_caps(PSMUX_CAPS, &read(PSMUX_CAPS), false));
    assert!(
        hits.is_empty(),
        "#2741: 器のサーバーの exit-empty off が器の conf / 再適用 / 能力から外れている:\n{}",
        hits.join("\n")
    );
}

/// 検出力: 修正前の形（conf に exit-empty が無い・sync_conf が conf を読ませない・入口の外で conf を使う）を名指す
#[test]
fn issue2741_番犬は外れた形を名指しする() {
    let good = r#"
fn conf_body() -> String {
    let mut body = backend_conf(&caps);
    body.push_str(&exit_empty_conf(&caps, issue2741_legacy()));
    body
}
pub fn exit_empty_conf(caps: &BackendCapabilities, legacy: bool) -> String {
    if !caps.keeps_empty_server { return String::new(); }
    format!("set -s exit-empty {}\n", if legacy { "on" } else { "off" })
}
pub fn ensure_conf() -> PathBuf {
    PathBuf::new()
}
pub fn sync_conf(socket: &str) {
    let conf = ensure_conf();
    let _ = tmux_command(Some(socket)).arg("source-file").arg(&conf).output();
}
pub(crate) fn wrap_options_for() -> Vec<String> {
    vec!["-f".to_string(), ensure_conf().display().to_string()]
}
pub fn issue2741_legacy() -> bool {
    std::env::var("TAKO_2741_LEGACY").is_ok()
}
"#;
    assert_eq!(check_backend("x.rs", good), Vec::<String>::new());

    // 修正前: conf_body が exit_empty_conf を通らない
    let old = good.replace(
        "    body.push_str(&exit_empty_conf(&caps, issue2741_legacy()));\n",
        "",
    );
    let hits = check_backend("x.rs", &old);
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert!(
        hits[0].starts_with("x.rs:2 `fn conf_body(` に `exit_empty_conf(` が無い"),
        "{hits:?}"
    );

    // 再適用が conf を読ませない
    let no_sync = good.replace(".arg(\"source-file\")", ".arg(\"show-options\")");
    let hits = check_backend("x.rs", &no_sync);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].contains("`pub fn sync_conf(` に `\"source-file\"` が無い"),
        "{hits:?}"
    );

    // 能力を見ずに書く
    let no_caps = good.replace(
        "    if !caps.keeps_empty_server { return String::new(); }\n",
        "",
    );
    assert_eq!(check_backend("x.rs", &no_caps).len(), 1);

    // 入口の外で器の conf を使う（利用者の tmux へ読ませうる）
    let leak = format!("{good}\nfn tmux_open_user() {{\n    let _ = ensure_conf();\n}}\n");
    let hits = check_backend("x.rs", &leak);
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(
        hits[0].contains("器の入口（`wrap_options_for` / `sync_conf`）の外で"),
        "{hits:?}"
    );

    // 能力の申告
    assert!(check_caps("t.rs", "keeps_empty_server: true,", true).is_empty());
    assert_eq!(
        check_caps("t.rs", "keeps_exit_output: true,", true).len(),
        1
    );
    assert_eq!(
        check_caps("p.rs", "keeps_empty_server: true,", false).len(),
        1
    );
}
