//! 番犬: 器越しのマウスレポート e2e が**偽のアンカー**と固定回数の窓へ戻っていない（#1252）
//!
//! ## なぜ止めるのか
//!
//! `tmux_backend` のバックエンド conf は `set -g mouse on` を書く。tmux は
//! **クライアント attach の時点で**外側端末（tako の `Term`）のマウスレポートを
//! 有効にするので、外側の `TerminalSession::mouse_reporting()` は
//! **内側アプリの `\033[?1000h` とは無関係に**真になる（実測: spawn から 20 ms・
//! その時点で器のペインは `mouse_any_flag=0`）。
//!
//! 内側アプリが要求する前にホイールを打つと、**tmux 自身がそれを食って
//! copy-mode へ入る**（実測 `pane_in_mode=1`）。以後のホイールは copy-mode の
//! スクロールになるので、**待ちをいくら伸ばしても内側アプリへは永久に届かない**。
//!
//! これが #1252（`マウスレポートと拡張キーがtmux越しに生で届く` /
//! `マウスレポート洪水でも断片がテキスト化しない` が高負荷で約 1/15 落ち、
//! 画面が改行だけのまま assert に到達する）の正体。**待ちの長さの問題ではない**ので、
//! 窓を伸ばすだけの直しでは再発する。正しいアンカーは器のペイン側のフラグ
//! （`#{mouse_sgr_flag}` / `#{mouse_any_flag}`）だけ。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 対象は `crates/tako-core/src/tmux_backend.rs` の**上記 2 テストの本体だけ**。
//! `// TAKO_1252_LEGACY_ARM 開始` 〜 `終了` で囲んだ A/B の旧経路
//! （`TAKO_1252_LEGACY=1` で #1252 前を再現するアーム）は対象外にする。
//!
//! 1. 本体に `wait_pane_mouse_ready(` が無い = 器のペイン側のアンカーを使っていない
//! 2. 本体に `std::thread::sleep` がある = 自前の固定待ち（固定回数の窓も必ずここを踏む）
//!
//! **回数そのものは見ない**。洪水テストの `for _ in 0..700 { scroll_wheel(…) }` は
//! 待ちではなく**主題**（2,100 イベントを全速で連打する）なので、回数を違反にすると
//! 誤検知になる。待ちかどうかは「自分で寝ているか」で決まり、状態待ちの `sleep` は
//! 共通ドライバ `wait_for_state` の中（= 本体の外）にある
//!
//! 同じファイルの兄弟テスト（`ネストtmux越しのホイールで内側スクロールバックを遡れる` /
//! `通常ペインのホイールはcopy_modeで遡りインジケータを出さない` /
//! `alt_screenの非マウスペインでホイールが矢印に化けない`）は対象外。どれも
//! **内側が実際に出した内容**（`LINE-99`）か器の `#{alternate_on}`（`wait_alt_screen`）を
//! アンカーにしていて、外側 `mouse_reporting()` を単独のアンカーにしていない

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn target() -> PathBuf {
    repo_root().join("crates/tako-core/src/tmux_backend.rs")
}

/// #1252 で直した 2 テスト（この 2 本だけがマウス要求の到達順に依存する）
const GUARDED: [&str; 2] = [
    "マウスレポートと拡張キーがtmux越しに生で届く",
    "マウスレポート洪水でも断片がテキスト化しない",
];

const ARM_BEGIN: &str = "TAKO_1252_LEGACY_ARM 開始";
const ARM_END: &str = "TAKO_1252_LEGACY_ARM 終了";

/// `fn <name>(` から、インデント 4 の閉じ括弧までを関数本体として切り出す。
/// テストモジュールの中の関数は必ずこの形（`rustfmt` が保証する）
fn body_of(src: &str, name: &str) -> Option<String> {
    let head = format!("    fn {name}() {{");
    let start = src.find(&head)?;
    let rest = &src[start..];
    let end = rest.find("\n    }\n")?;
    Some(rest[..end].to_string())
}

/// A/B の旧経路アーム（マーカーで囲んだ範囲）を落とす。
/// 落とした件数も返す（マーカーの綴り違いで**何も見ていない番犬**になるのを防ぐ）
fn strip_legacy_arms(body: &str) -> (String, usize) {
    let mut out = String::new();
    let mut dropped = 0usize;
    let mut inside = false;
    for line in body.lines() {
        if line.contains(ARM_BEGIN) {
            inside = true;
            dropped += 1;
            continue;
        }
        if line.contains(ARM_END) {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    (out, dropped)
}

#[test]
fn マウスレポートe2eは器のペイン側のアンカーで待っている() {
    let src = std::fs::read_to_string(target()).expect("tmux_backend.rs を読める");
    let mut offenders: Vec<String> = Vec::new();
    for name in GUARDED {
        let body =
            body_of(&src, name).unwrap_or_else(|| panic!("テスト本体が見つからない: {name}"));
        // アームが無ければ何も落とさない（= そのまま全体を見る）。
        // 「アームが在ること」は番犬が盲目になっていないかの確認なので、
        // 実体の違反を名指しできるようもう 1 本のテストへ分けてある
        let (code, _dropped) = strip_legacy_arms(&body);
        if !code.contains("wait_pane_mouse_ready(") {
            offenders.push(format!(
                "{name}: 器のペイン側のアンカー（wait_pane_mouse_ready）を使っていない"
            ));
        }
        if code.contains("std::thread::sleep") {
            offenders.push(format!(
                "{name}: 自前の固定待ちを入れている（`std::thread::sleep`）"
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "外側 `mouse_reporting()` は conf の `set -g mouse on` で attach 時に真になる\n\
         ため、内側アプリが立ち上がった証跡にならない。要求前に打ったホイールは\n\
         tmux が食って copy-mode へ入り、待ちを伸ばしても永久に届かない（#1252）。\n\
         器のペインの `#{{mouse_sgr_flag}}` を待つこと（`wait_pane_mouse_ready`）:\n  {}",
        offenders.join("\n  ")
    );
}

/// 走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が走査対象を見つけている() {
    let path = target();
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    for name in GUARDED {
        let body = body_of(&src, name)
            .unwrap_or_else(|| panic!("#1252 の現場（{name}）が切り出せていない"));
        // A/B の旧経路アームが在り、除去が効いていること（マーカーの綴り違いで
        // **何も落とさない番犬**になっていたら、旧経路が本体扱いで誤検知する）
        let (_, dropped) = strip_legacy_arms(&body);
        assert!(
            dropped > 0,
            "{name} に A/B の旧経路アーム（{ARM_BEGIN}）が無い。\
             マーカーを消したなら番犬の前提が崩れているので、番犬側も直すこと"
        );
    }
    // 器のペイン側のアンカーの実装そのものが在ること
    assert!(
        src.contains("fn wait_pane_mouse_ready("),
        "アンカーの実装（wait_pane_mouse_ready）が消えている"
    );
    assert!(
        src.contains("#{mouse_sgr_flag}"),
        "器のペイン側のフラグ（#{{mouse_sgr_flag}}）を問い合わせていない"
    );
    // 切り出しとアーム除去そのものが効くこと（本物のソースに無い形を合成して確かめる）
    let synthetic = "    fn だみー() {\n        std::thread::sleep(d);\n    }\n";
    assert_eq!(
        body_of(synthetic, "だみー").as_deref(),
        Some("    fn だみー() {\n        std::thread::sleep(d);")
    );
    let (stripped, dropped) = strip_legacy_arms(&format!(
        "        keep-a\n        // {ARM_BEGIN}\n        std::thread::sleep(d);\n        // {ARM_END}\n        keep-b\n"
    ));
    assert_eq!(dropped, 1);
    assert!(
        !stripped.contains("std::thread::sleep"),
        "アームを落とせていない"
    );
    assert!(stripped.contains("keep-a") && stripped.contains("keep-b"));
}
