//! `SIGTERM` の graceful quit が「握るだけで死なないアプリ」へ戻らないための番犬（#777 / #770）
//!
//! 落とすのは 4 つの再発形:
//!
//! 1. **本番で仕掛けが入らない**。#770 の実装は `TAKO_ISOLATED` のときだけ読み替えており、
//!    本番の `kill -TERM` は即死 = 直前 2 秒ぶんの構成が layout.json に載らなかった
//! 2. **ウォッチドッグ無しで握る**（最も危険な再発）。シグナルを握ったアプリはハングすると
//!    `kill` が効かなくなる。読み替えと「猶予を過ぎたら必ず `std::process::exit`」は
//!    **対でしか入れられない**
//! 3. **応答が 2 秒 tick へ戻る**。読み替えを拾うのは 500ms ループ側（新しいタイマーを
//!    増やさずに最悪待ちを 1/4 にする判断）
//! 4. **シグナルハンドラで async-signal-safe でないことをする**。ハンドラの中で
//!    ログ・割り当て・ロックを触ると、運が悪いときにそこでデッドロックして
//!    「SIGTERM で固まるアプリ」になる
//!
//! ソース走査なので、実装が別の書き方に変わっても「その材料を見ているか」で落ちる。

use std::path::{Path, PathBuf};

/// 走査するツリー。`TAKO_777_WATCHDOG_ROOT` で差し替えられる（A/B 専用:
/// 修正前のソースを取り出した一時ツリーを指すと、この番犬が落ちることを実測できる）
fn workspace_root() -> PathBuf {
    if let Some(root) = std::env::var_os("TAKO_777_WATCHDOG_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読めない: {e}"))
}

/// `needle` で始まるブロックを、波括弧の対応で切り出す（見つからなければ panic）
fn block_after(src: &str, needle: &str) -> String {
    let start = src
        .find(needle)
        .unwrap_or_else(|| panic!("目印 `{needle}` がソースに無い（実装が動いた?）"));
    let after = start + needle.len();
    let open = after + src[after..].find('{').expect("ブロックの開き波括弧が無い");
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return src[start..open + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("ブロックが閉じていない: {needle}");
}

/// コメント行を落とす（説明文が検査対象の形と一致して誤検知するのを防ぐ）
fn code_only(block: &str) -> String {
    block
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 再発形 1: 本番でも仕掛けが入る（隔離モード限定へ戻さない）
#[test]
fn sigterm_の読み替えは本番でも入る() {
    let root = workspace_root();
    let app = read(&root, "crates/tako-app/src/main.rs");
    assert!(
        !app.contains("install_for_isolated_verification"),
        "隔離モード限定の入口が残っている（#777 は本番でも読み替える）"
    );
    let main_fn = code_only(&block_after(&app, "\nfn main() {"));
    assert!(
        main_fn.contains("quit_signal::install("),
        "main() で SIGTERM の読み替えを入れていない:\n{main_fn}"
    );

    let core = read(&root, "crates/tako-core/src/platform/quit_signal.rs");
    let install = code_only(&block_after(&core, "pub fn install("));
    assert!(
        !install.contains("TAKO_ISOLATED") && !install.contains("isolated()"),
        "install が隔離モードを条件にしている（本番が素通しへ戻る）:\n{install}"
    );
    assert!(
        install.contains("legacy()"),
        "A/B の逃げ道（TAKO_777_LEGACY）が無い:\n{install}"
    );
}

/// 再発形 2: 握るならウォッチドッグと対で（`kill` が効かないアプリを作らない）
#[test]
fn 握るなら猶予後に必ず終わるウォッチドッグを対で持つ() {
    let root = workspace_root();
    let core = read(&root, "crates/tako-core/src/platform/quit_signal.rs");
    let install = code_only(&block_after(&core, "pub fn install("));
    assert!(
        install.contains("spawn_watchdog"),
        "読み替えを入れる場所でウォッチドッグを立てていない（ハング時に kill が効かなくなる）:\n{install}"
    );
    assert!(
        install.contains("grace()"),
        "猶予を引いていない（無期限に待つ形になっていないか）:\n{install}"
    );
    let watchdog = code_only(&block_after(&core, "fn spawn_watchdog("));
    assert!(
        watchdog.contains("std::process::exit"),
        "ウォッチドッグが強制終了しない = 猶予を過ぎても死なない:\n{watchdog}"
    );
    assert!(
        watchdog.contains("sleep(grace)"),
        "猶予ぶん待たずに終わらせている / 待ちが猶予に結びついていない:\n{watchdog}"
    );
    assert!(
        watchdog.contains("SIGTERM_SEEN"),
        "猶予の起点がシグナル到着になっていない（メインスレッドが固まったままだと\
         quit が撃たれず、起点も来ない = 永久に死なない）:\n{watchdog}"
    );
    assert!(
        core.contains("pub const DEFAULT_GRACE"),
        "猶予の既定値が定数として宣言されていない"
    );
}

/// `start` から次の `end` までを切り出す（`detach()` で閉じる `cx.spawn` ブロック用）
fn slice_to(src: &str, start: &str, end: &str) -> String {
    let from = src
        .find(start)
        .unwrap_or_else(|| panic!("目印 `{start}` がソースに無い（実装が動いた?）"));
    let len = src[from..]
        .find(end)
        .unwrap_or_else(|| panic!("`{start}` のあとに `{end}` が無い"));
    src[from..from + len].to_string()
}

/// 再発形 3: 応答周期は 500ms ループ（2 秒 tick へ戻さない）
#[test]
fn quit要求は500msループで拾う() {
    let root = workspace_root();
    let app = read(&root, "crates/tako-app/src/main.rs");
    // 500ms ポーリングの spawn ブロック（`.detach();` で閉じる）だけを見る
    let fast = code_only(&slice_to(
        &app,
        ".timer(Duration::from_millis(500))",
        ".detach();",
    ));
    assert!(
        fast.contains("take_quit_request"),
        "500ms ループで quit 要求を見ていない（2 秒 tick へ戻ると応答が最大 2 秒遅れる）:\n{fast}"
    );
    assert!(
        fast.contains("cx.quit()"),
        "SIGTERM 経路が Cmd+Q と同じ `cx.quit()` を通っていない\
         （通らないと on_app_quit の layout 保存もセッション保護も走らない）:\n{fast}"
    );
    assert!(
        !fast.contains("kill_server") && !fast.contains("process::exit"),
        "SIGTERM 経路が quit 以外でプロセス / セッションを畳んでいる\
         （tmux セッションは kill しない = #30 / #770 の不変条件）:\n{fast}"
    );
    assert!(
        fast.contains("received_log_line"),
        "SIGTERM 由来の quit だったことを persist.log に残していない（FR-5.7）:\n{fast}"
    );
}

/// 再発形 4: シグナルハンドラは async-signal-safe な操作だけ + 連打で二重に走らない
#[test]
fn シグナルハンドラはフラグ操作だけ行う() {
    let root = workspace_root();
    let core = read(&root, "crates/tako-core/src/platform/quit_signal.rs");
    let handler = code_only(&block_after(&core, "extern \"C\" fn on_sigterm("));
    for forbidden in ["println", "format!", "persist", "lock()", "String::"] {
        assert!(
            !handler.contains(forbidden),
            "シグナルハンドラで async-signal-safe でない操作（{forbidden}）をしている:\n{handler}"
        );
    }
    assert!(
        handler.contains("store(true"),
        "ハンドラがフラグを立てていない:\n{handler}"
    );

    let take = code_only(&block_after(&core, "pub fn take_quit_request("));
    assert!(
        take.contains("QUIT_DISPATCHED"),
        "連打された SIGTERM で終了処理が二重に走る形へ戻っている:\n{take}"
    );
}
