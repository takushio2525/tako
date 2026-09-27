//! **#1763 の番犬**: テーマを settings.json から適用する経路は、起動時も実行中の読み直しも
//! 同じ 1 実装（`load_theme_logged` → `ThemeWarningLog::reload`）を通る。
//!
//! ## なぜ止めるのか
//!
//! #1756 で、読めない色（`#赤色` 等）はその色だけ既定へ落として理由を persist.log へ残す
//! ようにした。ただし残していたのは**起動時だけ**で、実行中の読み直し
//! （`ControlHost::reload_theme`。`tako theme` / MCP `tako_theme` / 設定画面が呼ぶ）と
//! タブバーのトグルは `resolve_theme()` の警告を `_` / `.0` で捨てていた。実行中に
//! settings.json を手で直した値がなぜ効かないのかを追えない（#1763 の実測: 起動時 1 行・
//! 読み直し 0 行）。
//!
//! ## 何を固定するか
//!
//! 挙動（どの行を出すか・同じ警告を積まないか）は `settings.rs` の単体テスト
//! （`issue1763_*`）と実経路 `scripts/test-theme-reload-1763.sh` が見る。ここが止めるのは
//! **構造**で、1 行で戻せてしまう形がこれだけある:
//!
//! 1. [`main_rsはresolve_themeを直に呼ばない`] — 警告を捨てる旧の形（`resolve_theme().0` /
//!    `let (theme, _) = …`）が 1 か所でも戻る
//! 2. [`load_theme_loggedは帳簿の行をpersist_logへ流す`] — 1 実装の中で行を捨てる
//! 3. [`起動時と読み直しとトグルが同じ1本を通る`] — どれかの経路が 1 本から外れる
//!    （寄せた副作用で**起動時の記録**が落ちることも含む）
//! 4. [`帳簿のreloadは解決した警告をそのまま記録へ渡す`] — tako-control 側で警告を捨てる
//!
//! `about_window.rs` / `settings_window.rs` の `resolve_theme()` は**その窓自身の描画色**を
//! 引くためのもので、本体のテーマを差し替えない（毎回の描画で呼ぶので、そこで記録すると
//! 同じ行が積もる）。だから走査は本体のテーマを持つ `main.rs` に絞る。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。切らずにテスト領域だけを潰すので行番号が保たれる
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

const APP: &str = "crates/tako-app/src/main.rs";
const SETTINGS: &str = "crates/tako-control/src/settings.rs";

/// 1-origin の行番号（file:line で名指しするため）
fn line_of(src: &str, idx: usize) -> usize {
    src[..idx].matches('\n').count() + 1
}

/// 行コメントを落とす（doc / 注記は旧の形を説明のために書くので、実行されるコードだけを見る）
fn code_only(src: &str) -> String {
    src.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `head` で始まる定義の本体を `tail`（閉じ括弧の行）まで切り出す。
/// **見つからないことも FAILED**（走査範囲が空になるとどんな回帰でも通る番犬になる）
fn body<'a>(src: &'a str, rel: &str, head: &str, tail: &str) -> (&'a str, usize) {
    let start = src
        .find(head)
        .unwrap_or_else(|| panic!("{rel}: 目印 {head:?} が消えている（走査範囲を作れない）"));
    let rest = &src[start..];
    let end = rest
        .find(tail)
        .unwrap_or_else(|| panic!("{rel}: {head:?} の終わり {tail:?} が見つからない"));
    (&rest[..end + tail.len()], line_of(src, start))
}

/// 本体のテーマを settings から解決している（= 1 実装を通っていない）行
fn direct_resolve_hits(prod: &str) -> Vec<usize> {
    let code = code_only(prod);
    code.lines()
        .enumerate()
        .filter(|(_, l)| l.contains(concat!("resolve_", "theme(")))
        .map(|(i, _)| i + 1)
        .collect()
}

fn app_production() -> String {
    production_range::production(&read(APP), APP)
}

#[test]
fn main_rsはresolve_themeを直に呼ばない() {
    let prod = app_production();
    let hits = direct_resolve_hits(&prod);
    let named: Vec<String> = hits.iter().map(|l| format!("{APP}:{l}")).collect();
    assert!(
        hits.is_empty(),
        "{named:?} が `resolve_theme()` を直に呼んでいる。読めない色の警告を捨てる形（#1763）。\
         本体のテーマは `load_theme_logged(&mut self.theme_warning_log)` の 1 本で適用すること"
    );
}

#[test]
fn load_theme_loggedは帳簿の行をpersist_logへ流す() {
    let prod = app_production();
    let code = code_only(&prod);
    let defs = code.matches("fn load_theme_logged(").count();
    assert_eq!(
        defs, 1,
        "{APP}: load_theme_logged の定義が {defs} 個（1 実装のはず）"
    );
    let (b, line) = body(
        &code,
        APP,
        "fn load_theme_logged(log: &mut tako_control::settings::ThemeWarningLog) -> Theme {",
        "\n}\n",
    );
    for needle in ["log.reload()", "for line in &lines", "persist_diag(line)"] {
        assert!(
            b.contains(needle),
            "{APP}:{line} の load_theme_logged に `{needle}` が無い。帳簿が決めた行を \
             persist.log へ流していない（#1763: 読み直しの警告を捨てる形）"
        );
    }
}

#[test]
fn 起動時と読み直しとトグルが同じ1本を通る() {
    let prod = app_production();
    let code = code_only(&prod);
    let call = "load_theme_logged(&mut self.theme_warning_log)";
    // 実行中の読み直し（dispatch の `ControlHost::reload_theme`）
    let (b, line) = body(&code, APP, "    fn reload_theme(&mut self) {", "\n    }\n");
    assert!(
        b.contains(call),
        "{APP}:{line} の reload_theme が `{call}` を通っていない（#1763）"
    );
    // タブバーのトグル（dispatch と同じく保存 → 読み直し）
    let (b, line) = body(
        &code,
        APP,
        "    pub(crate) fn toggle_theme(&mut self, cx: &mut Context<Self>) {",
        "\n    }\n",
    );
    assert!(
        b.contains(call),
        "{APP}:{line} の toggle_theme が `{call}` を通っていない（#1763）"
    );
    // 起動時: 同じ帳簿で解決し、その帳簿を本体へ渡す（寄せた副作用で起動時の記録を落とさない）
    let (b, line) = body(
        &code,
        APP,
        "let mut theme_warning_log = tako_control::settings::ThemeWarningLog::default();",
        "let mut app = Self {",
    );
    assert!(
        b.contains("let startup_theme = load_theme_logged(&mut theme_warning_log);"),
        "{APP}:{line} 起動時のテーマが load_theme_logged を通っていない（#1756 の起動時の記録が落ちる）"
    );
    let (b, line) = body(&code, APP, "let mut app = Self {", "theme_warning_log,");
    assert!(
        b.contains("theme: startup_theme,"),
        "{APP}:{line} 起動時に解決したテーマを本体へ渡していない"
    );
}

#[test]
fn 帳簿のreloadは解決した警告をそのまま記録へ渡す() {
    let prod = production_range::production(&read(SETTINGS), SETTINGS);
    let code = code_only(&prod);
    let (b, line) = body(
        &code,
        SETTINGS,
        "pub fn reload(&mut self) -> (tako_core::theme::Theme, Vec<String>) {",
        "\n    }\n",
    );
    for needle in [
        "let (theme, warnings) = load().resolve_theme();",
        "self.record(warnings)",
    ] {
        assert!(
            b.contains(needle),
            "{SETTINGS}:{line} の ThemeWarningLog::reload に `{needle}` が無い（#1763）"
        );
    }
}

/// 検出力の担保: #1763 以前の形そのもの（読み直し・トグル・起動時）を名指せること
#[test]
fn 番犬は警告を捨てる旧の形を名指せる() {
    let old = [
        concat!(
            "    fn reload_theme(&mut self) {\n",
            "        let settings = tako_control::settings::load();\n",
            "        let (theme, _) = settings.resolve_",
            "theme();\n",
            "        self.theme = theme;\n",
            "    }\n",
        ),
        concat!(
            "            self.theme = tako_control::settings::load().resolve_",
            "theme().0;\n",
        ),
        concat!(
            "        let (startup_theme, theme_warnings) = tako_control::settings::load().resolve_",
            "theme();\n",
        ),
    ];
    for src in old {
        let hits = direct_resolve_hits(src);
        assert!(!hits.is_empty(), "旧の形を見逃した: {src}");
    }
    // コメントで旧の形を説明している行は拾わない
    let doc = concat!("// 以前は `resolve_", "theme().0` で警告を捨てていた\n");
    assert!(
        direct_resolve_hits(doc).is_empty(),
        "コメントを違反と数えた"
    );
}
