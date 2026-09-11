//! 同一バイナリの A/B: `TAKO_651_LEGACY=1` で **#651 前の永久待機**が再現する
//!
//! #651 の症状は「実行ペインが狭いと `tako run` / `tako run-interactive --wait` が
//! 永久に待ち続ける」。終了マーカー `__TAKO_EXIT=<code>` は最短 13 文字なので、
//! それより狭いペインでは端末が物理行を割る。物理行 1 本の中だけを探す判定は
//! そこで必ず外れ、`--wait` のポーリングに上限が無いのでそのまま固まる
//! （実測: 幅 10 桁で 40 秒待って返らない）。
//!
//! env はプロセス全体の状態でしかも一度しか読まない（`OnceLock`）ので、
//! 他のテストと混ざらないよう**専用の統合テストバイナリ**に置き、さらに
//! **1 本のテスト関数**の中で見る。既定側は判断を引数で渡す
//! `find_exit_marker_in(rows, legacy=false)` を使う（公開関数はこれに `legacy_651()` を
//! 渡すだけなので、env が唯一の差になる）。

use tako_control::dispatch::{find_exit_marker, find_exit_marker_in, legacy_651};

/// 幅 `cols` 桁の画面（`cols` 桁ちょうどまで埋まった行 = 折り返しの続きがありうる）。
/// fixture は ASCII なので桁数 = 文字数
fn rows_at(cols: usize, lines: &[&str]) -> Vec<(String, bool)> {
    lines
        .iter()
        .map(|l| ((*l).to_string(), l.chars().count() >= cols))
        .collect()
}

#[test]
fn legacyで狭いペインの終了コード検知が外れる() {
    // 幅 10 桁の実採取（隔離 GUI・`tako list` の cols=10。#651 の再現）
    let narrow = rows_at(10, &["hello-651-", "wait", "__TAKO_EXI", "T=0"]);
    // 幅 38 桁の実採取（割れない = 修正前でも拾えていた形）
    let wide = rows_at(38, &["hello-651-w40", "__TAKO_EXIT=0"]);

    // --- 既定（#651 の修正あり）: 割れていても拾う ---
    assert_eq!(
        find_exit_marker_in(&narrow, false),
        Some(0),
        "既定で折り返したマーカーを拾えない"
    );
    assert_eq!(find_exit_marker_in(&wide, false), Some(0));
    eprintln!(
        "[651-ab] 既定: 幅 10 桁 = {:?}",
        find_exit_marker_in(&narrow, false)
    );

    // --- legacy: #651 前 = 割れた画面は「まだ実行中」に見える ---
    std::env::set_var("TAKO_651_LEGACY", "1");
    assert!(legacy_651(), "env が読まれていない");

    assert_eq!(
        find_exit_marker(&narrow),
        None,
        "legacy なのに折り返したマーカーを拾っている（A/B が成立していない）"
    );
    // 広いペインは legacy でも拾える = 直したのは「割れたときだけ」
    assert_eq!(
        find_exit_marker(&wide),
        Some(0),
        "legacy で割れていないマーカーまで落ちている"
    );
    eprintln!(
        "[651-ab] legacy: 幅 10 桁 = {:?}",
        find_exit_marker(&narrow)
    );
}
