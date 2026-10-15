//! **#1952 の番犬**: `.ino` の基板とポートは `sketch.yaml` が正本で、書くのは `arduino-cli board attach`
//! だけ。tako は書式を真似しない・コアを代わりに入れない・UI スレッドで arduino-cli を待たない。
//!
//! ## なぜ止めるのか
//!
//! 1. **書式を真似しない**: `sketch.yaml` は Arduino IDE / arduino-cli と共有するファイル。tako が自分で
//!    書くと、IDE が書く形（`profiles:` やコメントの保ち方・将来の鍵）と食い違う。書くのは
//!    `board attach`（実測 1.2.0 でコメント・`profiles:` を保つ）に任せる
//! 2. **コアを代わりに入れない**: `arduino-cli core install` は数百 MB のツールチェーンを落とす
//!    （設計 §5.5）。コアが無ければ導入のコマンドを**示すだけ**。起こしてよい arduino-cli の引数は
//!    `board list` / `board listall` / `board attach` の 3 つ
//! 3. **UI スレッドで待たない**: `board list` は 1.7〜3.2 秒（実測）。ドロップダウンの書き込みは
//!    `dispatch_off_ui`、IPC は `prepare_offload` の `RunConfigSketch` で background へ出す
//!
//! ## 何を固定するか
//!
//! - `tako-core` / `tako-control` の `sketch_board.rs` の本番コードにファイルの書き込みが無い
//! - `tako-control` の `sketch_board.rs` で子プロセスを起こす口（`prober.output(`）は `fn run_cli` の中だけで、
//!   `run_cli(` へ渡す引数は `LIST_ARGS` / `LISTALL_ARGS` / `attach_args` の結果だけ
//! - `run_menu_ui.rs` の `fn run_menu_attach` は `dispatch_off_ui(` を通り、同期の `tako_control::dispatch(` を
//!   呼ばない。`fn submit_run_menu_edit` は同期の書き込み（`run_menu_apply(`）より先に基板・ポートの欄を分ける
//! - `dispatch.rs` の `prepare_offload` が `.ino` の実行設定を background のジョブにし、ジョブは候補を聞く
//!   （`handle_path` の `probe = true`）・同期の腕は聞かない（`false`）
//!
//! ## 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを、[`注入した逆戻りを名指しで落とす`] で
//! **修正前に戻した形**（tako が sketch.yaml を書く・core install を起こす・UI スレッドで dispatch する）が
//! `file:line` で名指しされることを確かめる（注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const CORE: &str = "crates/tako-core/src/sketch_board.rs";
const CONTROL: &str = "crates/tako-control/src/sketch_board.rs";
const RUN_MENU: &str = "crates/tako-app/src/run_menu_ui.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";

/// ファイルへ書く口（sketch.yaml を tako が書く逆戻り）
const WRITERS: &[&str] = &[
    "fs::write(",
    "File::create(",
    "OpenOptions::new(",
    ".write_all(",
];

/// `run_cli(` へ渡してよい引数（どれかを含むこと）
const ALLOWED_ARGS: &[&str] = &["sb::LIST_ARGS", "sb::LISTALL_ARGS", "&argv"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの「コードの眺め」（文字列・コメントは空白に潰れる。バイト長と行番号は保たれる）
fn code_of(src: &str, rel: &str) -> String {
    production_range::code_view_of(&production_range::production(src, rel))
}

fn line_of(code: &str, at: usize) -> usize {
    code[..at].matches('\n').count() + 1
}

/// `head`（`fn name(` 等）から始まる関数の窓（開始位置・本文）。本文は最初の `{` から対応する `}` まで
fn window(code: &str, head: &str) -> Option<(usize, String)> {
    let at = code.find(head)?;
    let open = at + code[at..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in code.bytes().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((at, code[at..=i].to_string()));
                }
            }
            _ => {}
        }
    }
    None
}

/// 1. ファイルの書き込み（sketch.yaml を tako が書く）
fn writers(code: &str, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, l) in code.lines().enumerate() {
        for w in WRITERS {
            if l.contains(w) {
                out.push(format!(
                    "{rel}:{} — `{w}` でファイルを書いている。sketch.yaml は Arduino IDE と共有する正本で、\
                     書くのは `arduino-cli board attach` だけ（書式を tako が真似しない。#1952）",
                    i + 1
                ));
            }
        }
    }
    out
}

/// 2. 子プロセスの口と引数（core install を起こさない）
fn spawns(code: &str, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    let run_cli = window(code, "fn run_cli(");
    let (run_from, run_to) = run_cli
        .as_ref()
        .map(|(at, body)| (*at, at + body.len()))
        .unwrap_or((0, 0));
    let mut from = 0;
    while let Some(rel_at) = code[from..].find("prober.output(") {
        let at = from + rel_at;
        from = at + 1;
        if !(run_from..run_to).contains(&at) {
            out.push(format!(
                "{rel}:{} — `prober.output(` を `fn run_cli` の外から呼んでいる。arduino-cli を起こすのは \
                 run_cli の 1 口だけにし、引数は board list / listall / attach に限る（#1952）",
                line_of(code, at)
            ));
        }
    }
    let mut from = 0;
    while let Some(rel_at) = code[from..].find("run_cli(") {
        let at = from + rel_at;
        from = at + 1;
        // 定義そのもの
        if code[..at].ends_with("fn ") {
            continue;
        }
        // 呼び出しの引数（対応する `)` まで）
        let mut depth = 0usize;
        let mut end = at;
        for (i, b) in code.bytes().enumerate().skip(at + "run_cli".len()) {
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let args = &code[at..=end];
        if !ALLOWED_ARGS.iter().any(|a| args.contains(a)) {
            out.push(format!(
                "{rel}:{} — `run_cli(` へ board list / listall / attach 以外の引数を渡している。\
                 コアの導入（core install）は代行せず、導入のコマンドを示すだけにする（#1952）",
                line_of(code, at)
            ));
        }
    }
    if let Some((at, body)) = window(code, "fn handle_path_with(") {
        if body.contains("let argv") && !body.contains("sb::attach_args(") {
            out.push(format!(
                "{rel}:{} — `argv` を `attach_args` 以外で組んでいる（#1952）",
                line_of(code, at)
            ));
        }
    }
    out
}

/// 3. UI の書き込みは background
fn ui_offenders(code: &str, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(code, "fn run_menu_attach(") {
        Some((at, body)) => {
            if !body.contains("dispatch_off_ui(") || body.contains("tako_control::dispatch(") {
                out.push(format!(
                    "{rel}:{} — `run_menu_attach` が同期の dispatch で基板・ポートを書いている。\
                     arduino-cli（board listall / attach）を UI スレッドで待つので `dispatch_off_ui` で送る（#1952）",
                    line_of(code, at)
                ));
            }
        }
        None => out.push(format!("{rel}: `fn run_menu_attach(` が無い（#1952）")),
    }
    match window(code, "fn submit_run_menu_edit(") {
        Some((at, body)) => {
            let route = body.find("writes_sketch()");
            let apply = body.find("run_menu_apply(");
            if !matches!((route, apply), (Some(r), Some(a)) if r < a) {
                out.push(format!(
                    "{rel}:{} — `submit_run_menu_edit` が基板・ポートの欄を同期の `run_menu_apply` より先に \
                     分けていない（Enter で arduino-cli を UI スレッドで待つ。#1952）",
                    line_of(code, at)
                ));
            }
        }
        None => out.push(format!("{rel}: `fn submit_run_menu_edit(` が無い（#1952）")),
    }
    out
}

/// 4. IPC の経路は background
fn dispatch_offenders(code: &str, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    match window(code, "pub fn prepare_offload(") {
        Some((at, body)) => {
            if !body.contains("run_config_sketch_prepare(") {
                out.push(format!(
                    "{rel}:{} — `prepare_offload` が `.ino` の実行設定（`RunnerDefaults {{ path }}`）を \
                     background へ出していない（CLI / MCP の基板の読み書きが UI スレッドで arduino-cli を待つ。#1952）",
                    line_of(code, at)
                ));
            }
        }
        None => out.push(format!("{rel}: `pub fn prepare_offload(` が無い")),
    }
    match window(code, "impl RunConfigSketchJob") {
        Some((at, body)) => {
            if !(body.contains("handle_path(") && body.contains("true,")) {
                out.push(format!(
                    "{rel}:{} — background のジョブが候補を聞いていない（`handle_path` の probe = true。#1952）",
                    line_of(code, at)
                ));
            }
        }
        None => out.push(format!("{rel}: `impl RunConfigSketchJob` が無い（#1952）")),
    }
    out
}

// --------------------------------------------------------------- テスト

#[test]
fn sketch_yamlを書くのはarduino_cliだけ() {
    let mut found = Vec::new();
    for rel in [CORE, CONTROL] {
        found.extend(writers(&code_of(&read(rel), rel), rel));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 起こすのはboard_list_listall_attachだけ() {
    let found = spawns(&code_of(&read(CONTROL), CONTROL), CONTROL);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn ui_とipcの書き込みはbackground() {
    let mut found = ui_offenders(&code_of(&read(RUN_MENU), RUN_MENU), RUN_MENU);
    found.extend(dispatch_offenders(
        &code_of(&read(DISPATCH), DISPATCH),
        DISPATCH,
    ));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 走査が空振りしていない（改名・移動や範囲取りの破損で黙って 0 件にならない）
#[test]
fn 走査が空振りしていない() {
    let control = code_of(&read(CONTROL), CONTROL);
    let (_, run_cli) = window(&control, "fn run_cli(").expect("fn run_cli の窓");
    assert!(
        run_cli.contains("prober.output("),
        "run_cli が子プロセスの口を持つ"
    );
    let calls = control.matches("run_cli(").count();
    assert!(calls >= 4, "run_cli の呼び出しが拾えていない（{calls}）");
    for arg in ALLOWED_ARGS {
        assert!(
            control.contains(arg),
            "`{arg}` を渡す呼び出しが見つからない"
        );
    }
    let menu = code_of(&read(RUN_MENU), RUN_MENU);
    assert!(window(&menu, "fn run_menu_attach(").is_some());
    assert!(window(&menu, "fn submit_run_menu_edit(").is_some());
    // 案内の文（core install のコマンド）は文字列なので、眺めでは潰れている = 呼び出しと混ざらない
    let core = read(CORE);
    assert!(core.contains("core install"), "導入の案内の文がある");
    assert!(!code_of(&core, CORE).contains("core install"));
}

/// 番犬に検出力があること: 修正前に戻した形を `file:line` で名指しする
#[test]
fn 注入した逆戻りを名指しで落とす() {
    let view = production_range::code_view_of;
    // ① tako が sketch.yaml を自分で書く
    let found = writers(
        &view("fn attach(p: &Path) {\n    let y = p.join(\"sketch.yaml\");\n    std::fs::write(&y, b);\n}\n"),
        "注入.rs",
    );
    assert!(
        found.iter().any(|f| f.starts_with("注入.rs:3 —")),
        "{found:?}"
    );

    // ② コアを代わりに入れる（run_cli に別の引数）・run_cli の外から起こす
    let src = "fn run_cli(prober: &dyn Prober, a: &[&str]) -> R {\n    prober.output(x, a)\n}\n\
               fn ok() {\n    run_cli(p, sb::LIST_ARGS);\n}\n\
               fn bad() {\n    run_cli(p, &[\"core\", \"install\", core]);\n    prober.output(c, &[]);\n}\n";
    let found = spawns(&view(src), "注入.rs");
    assert!(
        found.iter().any(|f| f.starts_with("注入.rs:8 —")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|f| f.starts_with("注入.rs:9 —")),
        "{found:?}"
    );
    assert_eq!(found.len(), 2, "安全な呼び出しまで拾った: {found:?}");

    // ③ UI スレッドの同期の dispatch で書く・Enter が同期の書き込みへ先に落ちる
    let src = "fn run_menu_attach(&mut self) {\n    tako_control::dispatch(self, r, o);\n}\n\
               fn submit_run_menu_edit(&mut self) {\n    self.run_menu_apply(p, w, cx);\n    if e.target.writes_sketch() {}\n}\n";
    let found = ui_offenders(&view(src), "注入.rs");
    assert!(
        found.iter().any(|f| f.starts_with("注入.rs:1 —")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|f| f.starts_with("注入.rs:4 —")),
        "{found:?}"
    );

    // ④ prepare_offload の腕が無い
    let src = "pub fn prepare_offload(h: H) {\n    match r { _ => None }\n}\n\
               impl RunConfigSketchJob {\n    fn run(self) { handle_path(a, b, true, c) }\n}\n";
    let found = dispatch_offenders(&view(src), "注入.rs");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].starts_with("注入.rs:1 —"), "{found:?}");
}
