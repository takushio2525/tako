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
//!
//! ## 別ファイルの同じ罠（#2196）
//!
//! `crates/tako-core/src/scroll_mirror.rs` の `ホイールレポートのtmux直接注入が内側に生で届く`
//! も外側 `mouse_reporting()` をアンカーにしていて、この番犬の対象外だった。
//! こちらは `send-keys -H`（器のマウス処理を通らない直接注入）なので copy-mode には
//! 食われない代わりに、内側アプリの `stty raw -echo` より先に届いた注入を cooked の tty が
//! echo（ECHOCTL の `^[`）し、raw になってから内側アプリも同じバイトを読んで echo する
//! = **2 回見える**（CI の macOS で上方向の SGR が 3 → 6 個・80 桁の折り返しで X10 が分断）。
//! 本体（`TAKO_2196_LEGACY_ARM` を除く）に器のペイン側のアンカー
//! （`history_state(` = `#{mouse_sgr_flag}`）があり、`mouse_reporting()` と自前の
//! `std::thread::sleep` が無いことを見る

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

/// 切り出しとアーム除去は #1265 の番犬と共有する（前処理を 2 か所に書かない）
#[path = "common/test_source.rs"]
mod test_source;

#[path = "common/code_view.rs"]
mod code_view;
use test_source::{body_of, strip_arms};

fn strip_legacy_arms(body: &str) -> (String, usize) {
    strip_arms(body, ARM_BEGIN, ARM_END)
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
    // 器のペイン側のアンカーの実装そのものが在ること。**コメントを落とした眺め**で
    // 見る（全文だと、この綴りを書いた説明コメントで緑になる = #1609）。
    // A/B のアーム除去はコメントの目印で測るので、そちらは `src` のまま
    let code = code_view::without_comments_checked(&src, "crates/tako-core/src/tmux_backend.rs");
    assert!(
        code.contains("fn wait_pane_mouse_ready("),
        "アンカーの実装（wait_pane_mouse_ready）が消えている"
    );
    assert!(
        code.contains("#{mouse_sgr_flag}"),
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

/// #2196 の現場（`scroll_mirror.rs`）。同じ偽アンカーの罠を別ファイルで踏んでいた
fn scroll_mirror_target() -> PathBuf {
    repo_root().join("crates/tako-core/src/scroll_mirror.rs")
}

const SCROLL_MIRROR_GUARDED: &str = "ホイールレポートのtmux直接注入が内側に生で届く";
const ARM_BEGIN_2196: &str = "TAKO_2196_LEGACY_ARM 開始";
const ARM_END_2196: &str = "TAKO_2196_LEGACY_ARM 終了";

#[test]
fn ホイール直接注入e2eは器のペイン側のアンカーで待っている() {
    let src = std::fs::read_to_string(scroll_mirror_target()).expect("scroll_mirror.rs を読める");
    let body = body_of(&src, SCROLL_MIRROR_GUARDED)
        .unwrap_or_else(|| panic!("テスト本体が見つからない: {SCROLL_MIRROR_GUARDED}"));
    let (code, _dropped) = strip_arms(&body, ARM_BEGIN_2196, ARM_END_2196);
    // コメントを落とした眺めで見る（本体の説明コメントが `mouse_reporting()` と
    // 綴っているので、全文だと誤検知する = #1609）
    let code = code_view::without_comments(&code);
    let mut offenders: Vec<&str> = Vec::new();
    if !code.contains("history_state(") {
        offenders.push("器のペイン側のアンカー（history_state = #{mouse_sgr_flag}）を使っていない");
    }
    if code.contains("mouse_reporting()") {
        offenders.push("外側 `mouse_reporting()` を見ている");
    }
    if code.contains("std::thread::sleep") {
        offenders.push("自前の固定待ちを入れている（`std::thread::sleep`）");
    }
    assert!(
        offenders.is_empty(),
        "{SCROLL_MIRROR_GUARDED}: 外側 `mouse_reporting()` は conf の `set -g mouse on` で\n\
         attach 時に真になり、内側の `stty raw -echo` を待たない。その前に `send-keys -H` で\n\
         注入すると cooked の tty の echo と内側アプリの echo で 2 回見える（#2196）。\n\
         器のペインの `#{{mouse_sgr_flag}}` を待つこと（`history_state`）:\n  {}",
        offenders.join("\n  ")
    );
}

/// #2196 の走査先を取り違えて**何も見ていない番犬**になっていないこと
#[test]
fn 番犬が2196の走査対象を見つけている() {
    let path = scroll_mirror_target();
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    let body = body_of(&src, SCROLL_MIRROR_GUARDED)
        .unwrap_or_else(|| panic!("#2196 の現場（{SCROLL_MIRROR_GUARDED}）が切り出せていない"));
    let (_, dropped) = strip_arms(&body, ARM_BEGIN_2196, ARM_END_2196);
    assert!(
        dropped > 0,
        "{SCROLL_MIRROR_GUARDED} に A/B の旧経路アーム（{ARM_BEGIN_2196}）が無い。\
         マーカーを消したなら番犬の前提が崩れているので、番犬側も直すこと"
    );
    // アンカーの実体（`history_state`）が器のペイン側のフラグを問い合わせていること。
    // コメントを落とした眺めで見る（説明コメントの綴りで緑にならないように = #1609）
    let code = code_view::without_comments_checked(&src, "crates/tako-core/src/scroll_mirror.rs");
    assert!(
        code.contains("pub fn history_state(") && code.contains("#{mouse_sgr_flag}"),
        "history_state が器のペイン側のフラグ（#{{mouse_sgr_flag}}）を問い合わせていない"
    );
}
