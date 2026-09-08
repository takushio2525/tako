//! 番犬: codex の「走っている行」と「残り続ける履歴行」を混ぜない（#1015）
//!
//! ## なぜ止めるのか
//!
//! codex の背景ターミナル待ちには**別物の 2 行**があり、どちらも `• ` で始まる。
//!
//! | 実採取（codex-cli 0.153.0） | 意味 |
//! |---|---|
//! | `• Waiting for background terminal (1m 04s •…` | いま待っている（狭いペインでは codex が行末を切る） |
//! | `• Waited for background terminal · sleep 300; …` | 完了済みツールの履歴（実測で同一画面に 5 回残った） |
//!
//! 前者を busy と読めないと `WORKER_IDLE` + `prompt_undelivered` の誤検知になり
//! （#1015 の実害。自動再送で二重指示事故）、後者を busy と読むと worker が
//! **永遠に完了しなくなる**（#571 / #120 の「永久 busy」）。
//!
//! 単体テストは「今の判定結果」を固定するが、**同じ形の再発**（文言で判定する実装へ
//! 戻す・経過時間の条件を外す・未達の断定を無条件に戻す）はソースの形でしか止められない。
//!
//! ## 3 本立て
//!
//! 1. [`完了済みの履歴語を強マーカーにしていない`] — `Waited for` / `Worked for` を
//!    マーカーの照合文字列に使っていないこと（Issue 本文が提案した形。永久 busy へ戻る）
//! 2. [`背景ターミナル待ちの判定は経過時間で決めている`] — 判定が `•` + 括弧内の
//!    経過時間を見ていること（`esc to interrupt` は狭いペインで消えるので当てにできない）
//! 3. [`未達の断定は一次シグナルを読めたときだけ`] — `OverdueSuspect` を返す腕が
//!    「読めなかった」で降格されること（読めていないのに再送を撃たせない）

use std::path::{Path, PathBuf};

fn crate_file(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// テストモジュールより手前だけを見る（番犬自身の文字列や fixture に当たらない）。
/// **`mod tests` の直前で切る**（`#[cfg(test)]` はテスト用ヘルパにも付く）
fn production_source(src: &str) -> &str {
    let cut = src
        .find("\n#[cfg(test)]\nmod tests")
        .or_else(|| src.find("\n#[cfg(test)]\nmod issue984_screen_markers"));
    match cut {
        Some(i) => &src[..i],
        None => src,
    }
}

/// コメント行を落とす（説明文に書いた実採取の引用を拾わない）
fn without_comments(body: &str) -> String {
    body.lines()
        .filter(|l| {
            let t = l.trim_start();
            !(t.starts_with("//") || t.starts_with("//!"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 指定した関数の本文（シグネチャから次の行頭 `}` まで）を切り出す
fn fn_body(src: &str, signature: &str) -> String {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} が見つからない（改名したら番犬も直す）"));
    let rest = &src[start..];
    let end = rest.find("\n}\n").unwrap_or(rest.len());
    rest[..end].to_string()
}

fn wait_production() -> String {
    let src = std::fs::read_to_string(crate_file("src/orchestrator/wait.rs")).expect("wait.rs");
    without_comments(production_source(&src))
}

#[test]
fn 完了済みの履歴語を強マーカーにしていない() {
    let code = wait_production();
    for needle in ["\"Waited for", "\"Worked for", "\"Waiting for"] {
        assert!(
            !code.contains(needle),
            "codex の表示語（{needle}…）を照合文字列にしている。\
             `Waited for background terminal` / `Worked for 5m 08s` は**完了後も残り続ける\
             履歴行**なので、語で busy を判定すると worker が永遠に完了しない\
             （#571 / #120 の永久 busy）。判定は `•` + 括弧内の経過時間で行うこと（#1015）"
        );
    }
}

#[test]
fn 背景ターミナル待ちの判定は経過時間で決めている() {
    let src = std::fs::read_to_string(crate_file("src/orchestrator/wait.rs")).expect("wait.rs");
    let body = fn_body(
        production_source(&src),
        "pub(crate) fn codex_running_footer_in(",
    );
    let code = without_comments(&body);
    assert!(
        code.contains("elapsed"),
        "判定が経過時間を見ていない（#1015）。狭いペインでは codex が行末を `…` で切るので \
         `esc to interrupt` は残らない:\n{code}"
    );
    // 経過時間の**直後**が区切りであることまで見ていること（見ないと codex の通常
    // メッセージの `(12s ago)` を拾って永久 busy になる）
    assert!(
        code.contains("'•'") && code.contains("')'"),
        "経過時間の直後が区切りかを見ていない（#1015。永久 busy の芽）:\n{code}"
    );
    assert!(
        code.contains("starts_with('•')"),
        "codex のフッター行に限定していない（他系統の出力の `(12s` に誤爆する。#1015）:\n{code}"
    );
    // 強マーカーの経路に載っていること（載っていないと判定が使われない）
    let strong = fn_body(production_source(&src), "fn strong_marker_in(");
    assert!(
        without_comments(&strong).contains("codex_running_footer_in"),
        "強マーカーが codex のフッター判定を通っていない（#1015）:\n{strong}"
    );
}

#[test]
fn 未達の断定は一次シグナルを読めたときだけ() {
    let src =
        std::fs::read_to_string(crate_file("src/orchestrator/registry.rs")).expect("registry.rs");
    let body = fn_body(
        production_source(&src),
        "pub fn prompt_delivery_assessment_with(",
    );
    let code = without_comments(&body);
    assert!(
        code.contains("primary_signal_unreadable"),
        "一次シグナルの読めなさを見ていない（#1015）。読めていないのに未達と断定すると \
         supervisor の自動再送が働いている worker へ同じ依頼を二度渡す:\n{code}"
    );
    // 猶予超過の腕（`DeliveryObservation::Structured` → `OverdueSuspect`）に
    // 読めなさのガードが挟まっていること。**関数のどこかに文字列がある**だけでは
    // 「宣言したが効かせていない」形を通してしまう
    let structured = code
        .find("DeliveryObservation::Structured")
        .expect("猶予超過の腕（DeliveryObservation::Structured）が見つからない");
    let arm = &code[structured..];
    let arm_end = arm
        .find("PromptDelivery::OverdueSuspect")
        .expect("Structured の腕が OverdueSuspect を返していない");
    assert!(
        arm[..arm_end].contains("primary_signal_unreadable"),
        "OverdueSuspect（= prompt_undelivered + 自動再送）を返す腕が \
         「読めなかった」で降格されていない（#1015）:\n{}",
        &arm[..arm_end]
    );
}
