//! 同一バイナリの A/B: `TAKO_757_LEGACY=1` で **#757 前の誤分類**が再現する
//!
//! #757 の症状は「ログインが失効した worker が `api_error` と分類され、
//! `recommended_action: resume` が返る」。resume しても 1 リクエストも通らないので
//! master は永久に空回りする（2026-08-01 / 08-03 / 08-05 に 3 回）。
//! この env で旧挙動（失効を専用種別にせず、画面に残る `API Error` 行へ落ちる）へ戻る。
//!
//! env はプロセス全体の状態でしかも一度しか読まない（`OnceLock`）ので、
//! 他のテストと混ざらないよう**専用の統合テストバイナリ**に置き、さらに
//! **1 本のテスト関数**の中で見る。既定側は判断を引数で渡す
//! `detect_worker_error_in(legacy=false)` を使う（公開関数はこれに `legacy_757()` を
//! 渡すだけなので、env が唯一の差になる）。

use tako_control::orchestrator::wait::{self, WorkerErrorKind};

/// #757 の実観測の順序（接続エラーが先に出て、失効はその下に出る）。
/// 個人情報はプレースホルダ（#927）
const EXPIRED_AFTER_API_ERROR: &str = "\
⏺ テストを実行します

  ⎿  API Error: Unable to connect to API (ENOTFOUND api.anthropic.com)
  ⎿  OAuth refresh token is no longer valid; run /login to re-authenticate

─────────────────────────────────────────────
❯
─────────────────────────────────────────────";

/// 接続エラーが 1 行も無い失効画面（`/model` を送って初めて明示されたケース）
const EXPIRED_ONLY: &str = "\
⏺ 続けます

  ⎿  Login expired · Please run /login

─────────────────────────────────────────────
❯
─────────────────────────────────────────────";

#[test]
fn legacyでログイン失効のapi_error誤分類が再現する() {
    // --- 既定（#757 の修正あり）: 失効は専用種別 + relogin ---
    let (kind, detail) =
        wait::detect_worker_error_in(EXPIRED_AFTER_API_ERROR, false).expect("既定で検知されない");
    assert_eq!(kind, WorkerErrorKind::LoginExpired);
    assert_eq!(kind.recommended_action(), "relogin");
    assert!(detail.contains("OAuth refresh token"), "{detail}");
    eprintln!(
        "[757-ab] 既定: kind={} action={}",
        kind.as_str(),
        kind.recommended_action()
    );

    let (kind, _) = wait::detect_worker_error_in(EXPIRED_ONLY, false).expect("既定で検知されない");
    assert_eq!(kind, WorkerErrorKind::LoginExpired);

    // --- legacy: #757 前 = 接続エラーとして resume を返す / 単独なら何も言わない ---
    std::env::set_var("TAKO_757_LEGACY", "1");
    assert!(wait::legacy_757(), "env が読まれていない");

    let (kind, detail) = wait::detect_worker_error(EXPIRED_AFTER_API_ERROR)
        .expect("legacy でも api_error としては検知される");
    assert_eq!(
        kind,
        WorkerErrorKind::ApiError,
        "legacy なのに失効を分離している（A/B が成立していない）"
    );
    assert_eq!(
        kind.recommended_action(),
        "resume",
        "#757 の実害そのもの（resume しても永久に復帰しない）"
    );
    assert!(detail.contains("Unable to connect to API"), "{detail}");
    eprintln!(
        "[757-ab] legacy: kind={} action={}",
        kind.as_str(),
        kind.recommended_action()
    );

    // 接続エラー行が無い画面は legacy では **何も検知されない** = idle（作業完了）に見える
    assert_eq!(
        wait::detect_worker_error(EXPIRED_ONLY),
        None,
        "legacy では失効だけの画面は停止として拾われない"
    );
}
