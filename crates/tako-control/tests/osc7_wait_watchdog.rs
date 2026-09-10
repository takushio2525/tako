//! 番犬: シェル統合の OSC 7 e2e が**固定回数の窓**へ戻っていない（#1265）
//!
//! ## なぜ止めるのか
//!
//! `tmux_backend` の 3 本（`osc7はtmuxパススルーで外へ届く` /
//! `器のサーバーが別インスタンスを指していてもosc7が届く` /
//! `ソケット名がtakoで始まらなくてもosc7が届く`）は元々どれも
//! `for _ in 0..100 { … sleep 100ms }` = **固定 10 秒窓**で待ち、尽きたら
//! tako 側の画面だけを出して panic していた。
//!
//! そのため落ちたときの手掛かりが「打った行のエコーしか無い画面」だけになり、
//! **窓が足りないのか / 器がそもそも動いていないのか**を区別できなかった。
//! #1265 の最初の見立て（窓不足）が実測で外れたのはこのため:
//!
//! - CPU だけの人工負荷（load 11〜46）で修正前のまま 150 回 → **0 FAILED**。
//!   OSC 7 の到達は 744〜1,171 ms（p50 858 ms・90 サンプル）で 10 秒窓に
//!   8 倍以上の余裕がある
//! - 元の 8/150 を採った回は `/dev/ttys*` 404 / `kern.tty.ptmx_max` 511・
//!   tmux 135 本で、tmux **サーバー**が `spawn_pane → forkpty → openpty` で
//!   止まっている `sample` が採れていた = **機の PTY 枯渇**
//!
//! 予算を伸ばしても PTY 枯渇そのものは救えない。それでも状態待ちへ寄せるのは
//! **次に落ちたときに原因が診断から分かる**ようにするためで、固定窓へ戻ると
//! その診断がまるごと消える。だからここで止める。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 対象は `crates/tako-core/src/tmux_backend.rs` の**上記 3 テストの本体だけ**。
//! `// TAKO_1265_LEGACY_ARM 開始` 〜 `終了` で囲んだ A/B の旧経路
//! （`TAKO_1265_LEGACY=1` で #1265 前を再現するアーム）は対象外にする。
//!
//! 1. 本体に `wait_osc7_cwd(` が無い = 状態待ちの共通ドライバを通していない
//! 2. 本体に `std::thread::sleep` がある = 自前の固定待ち
//! 3. 本体に `for _ in 0..` がある = 固定回数の窓
//!
//! 3 は #1252 の番犬が**あえて見ていない**条件（あちらの洪水テストは
//! `for _ in 0..700 { scroll_wheel(…) }` が待ちではなく**主題**なので、
//! 回数を違反にすると誤検知になる）。この 3 本には数える主題が無いので、
//! 固定回数の窓は素直に違反にできる

use std::path::{Path, PathBuf};

/// 切り出しとアーム除去は #1252 の番犬と共有する（前処理を 2 か所に書かない）
#[path = "common/test_source.rs"]
mod test_source;
use test_source::{body_of, strip_arms};

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

/// #1265 で直した 3 本（どれもシェル統合の OSC 7 の到達を待つ）
const GUARDED: [&str; 3] = [
    "osc7はtmuxパススルーで外へ届く",
    "器のサーバーが別インスタンスを指していてもosc7が届く",
    "ソケット名がtakoで始まらなくてもosc7が届く",
];

const ARM_BEGIN: &str = "TAKO_1265_LEGACY_ARM 開始";
const ARM_END: &str = "TAKO_1265_LEGACY_ARM 終了";

#[test]
fn osc7e2eは状態待ちで待っている() {
    let src = std::fs::read_to_string(target()).expect("tmux_backend.rs を読める");
    let mut offenders: Vec<String> = Vec::new();
    for name in GUARDED {
        let body =
            body_of(&src, name).unwrap_or_else(|| panic!("テスト本体が見つからない: {name}"));
        // アームが在ることの確認は別のテストに分けてある（実体の違反を名指しできるよう、
        // ここでは落とせた件数を見ない）
        let (code, _dropped) = strip_arms(&body, ARM_BEGIN, ARM_END);
        if !code.contains("wait_osc7_cwd(") {
            offenders.push(format!(
                "{name}: 状態待ちの共通ドライバ（wait_osc7_cwd）を通していない"
            ));
        }
        if code.contains("std::thread::sleep") {
            offenders.push(format!(
                "{name}: 自前の固定待ちを入れている（`std::thread::sleep`）"
            ));
        }
        if code.contains("for _ in 0..") {
            offenders.push(format!(
                "{name}: 固定回数の窓（`for _ in 0..N`）で待っている"
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "固定回数の窓は「窓が足りない」のか「器が動いていない」のかを診断から\n\
         消してしまう（#1265 の見立てが外れた原因）。待ちは `wait_osc7_cwd`\n\
         （状態待ち + `state_wait_budget` + 器のペインの状態を出す診断）を通すこと:\n  {}",
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
            .unwrap_or_else(|| panic!("#1265 の現場（{name}）が切り出せていない"));
        // A/B の旧経路アームが在り、除去が効いていること（マーカーの綴り違いで
        // **何も落とさない番犬**になっていたら、旧経路が本体扱いで誤検知する）
        let (stripped, dropped) = strip_arms(&body, ARM_BEGIN, ARM_END);
        assert_eq!(
            dropped, 1,
            "{name} の A/B 旧経路アーム（{ARM_BEGIN}）が 1 つでない。\
             マーカーを消したなら番犬の前提が崩れているので、番犬側も直すこと"
        );
        // 旧経路アームの中身は「固定回数の窓」そのもの = 除去が効いていれば消える
        assert!(
            body.contains("for _ in 0..100"),
            "{name} の旧経路アームが固定回数の窓を再現していない（A/B が忠実でない）"
        );
        assert!(
            !stripped.contains("for _ in 0..100"),
            "{name} のアーム除去が効いていない"
        );
    }
    // 状態待ちの実装そのものが在ること
    assert!(
        src.contains("fn wait_osc7_cwd("),
        "状態待ちの実装（wait_osc7_cwd）が消えている"
    );
    assert!(
        src.contains("fn probe_osc7("),
        "予算切れの診断（probe_osc7）が消えている"
    );
    // 診断の採取そのものに期限が付いていること。**この診断がいちばん要るのは
    // 器が応答しない場面**なので、期限が無いと FAILED すら出ずに固まる（#1271 と同じ罠）
    assert!(
        src.contains("fn probe_osc7_bounded(") && src.contains("recv_timeout("),
        "診断の採取から期限（probe_osc7_bounded / recv_timeout）が消えている"
    );
    // 診断が「原因を切り分けられる材料」を持っていること（#1265 の主目的）
    for needle in [
        "#{pane_dead}",
        "#{pane_current_command}",
        "#{pane_current_path}",
        "TAKO_1265_WAIT",
    ] {
        assert!(
            src.contains(needle),
            "予算切れの診断から {needle} が消えている（不着の原因を切り分けられなくなる）"
        );
    }
}
