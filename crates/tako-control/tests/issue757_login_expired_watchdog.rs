//! ログイン失効の検知（#757）が**静かに戻らない**ようにする番犬
//!
//! #757 の実害は「失効が `api_error` に見えて `resume` が返る」こと。直したあとに
//! 静かに戻る道が 3 つあるので、そこを機械で塞ぐ:
//!
//! 1. **検知段の位置**。失効は上限メッセージ（段 1）より先に見ないと、解除後も画面に
//!    残る `You've hit your … limit` の残骸で `wait_reset` を返し続ける（#757 の
//!    「二段で詰まる」形）。逆にライブのダイアログ（段 0）には勝たせない
//! 2. **正本の一本化**。失効の文言は起動時の未認証検知（#983）と同じ語句なので、
//!    片方に書き足すともう片方が取りこぼす。`login_expired_line` を唯一の持ち主にする
//! 3. **手順書の言い切り**。`relogin` は `resume` では解けないと master へ明示していないと、
//!    種別だけ増えて対処は誤ったまま（#748 で踏んだ形）
//!
//! さらに **手順書が自分の文言で自分を釣らない**ことも見る（`tako orchestrator guide
//! monitoring` を worker のペインで実行した画面が失効の検知にかかってはいけない）。
//!
//! 検出力は「修正前と同じ形へ戻した文字列」を作って、同じ検査が名指しで落とすことを
//! 同じファイルの中で確かめる（見逃す側へ倒れる検査は番犬にならない）。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// 段の目印（コメントの見出し）。並び順そのものが仕様なので、目印で位置を測る
const DIALOG_MARKER: &str = "// 0. claude の limit 対処ダイアログ";
const LOGIN_MARKER: &str = "// 0b. ログイン失効（#757）";
const LIMIT_MARKER: &str = "// 1. usage limit 到達";

/// 3 つの目印の位置関係を検査する。`Err` の中身はそのまま失敗メッセージになる
fn check_stage_order(src: &str) -> Result<(usize, usize, usize), String> {
    let find = |marker: &str| -> Result<usize, String> {
        src.find(marker)
            .ok_or_else(|| format!("段の目印が見つからない: {marker}"))
    };
    let dialog = find(DIALOG_MARKER)?;
    let login = find(LOGIN_MARKER)?;
    let limit = find(LIMIT_MARKER)?;
    if dialog >= login {
        return Err(format!(
            "ログイン失効の段がライブのダイアログより先にある（dialog={dialog} login={login}）: \
             ダイアログは応答が要る UI なので先に返さないといけない"
        ));
    }
    if login >= limit {
        return Err(format!(
            "ログイン失効の段が上限メッセージより後にある（login={login} limit={limit}）: \
             解除後も残る上限の残骸で wait_reset を返し続ける（#757 の二段で詰まる形）"
        ));
    }
    Ok((dialog, login, limit))
}

#[test]
fn ログイン失効の検知段はダイアログの後かつ上限より前にある() {
    let src = read("crates/tako-control/src/orchestrator/wait.rs");
    let (dialog, login, limit) = check_stage_order(&src).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("[757-watchdog] 段の位置: dialog={dialog} login={login} limit={limit}");

    // 検出力: 段 0b をまるごと消した（= #757 前の）ソースでは落ちる
    let without_login = src.replace(LOGIN_MARKER, "// (removed)");
    assert!(
        check_stage_order(&without_login).is_err(),
        "段を消しても通る検査になっている"
    );
    // 検出力: 段 0b を上限の後ろへ動かした形でも落ちる
    let moved = src.replace(LOGIN_MARKER, "// (moved away)").replacen(
        LIMIT_MARKER,
        &format!("{LIMIT_MARKER}\n    {LOGIN_MARKER}"),
        1,
    );
    let err = check_stage_order(&moved).expect_err("段を後ろへ動かしても通る検査になっている");
    assert!(err.contains("上限メッセージより後"), "{err}");
}

#[test]
fn 検知段は文言の正本を呼んでいる() {
    let src = read("crates/tako-control/src/orchestrator/wait.rs");
    let (_, login, limit) = check_stage_order(&src).expect("段の順序");
    let stage = &src[login..limit];
    assert!(
        stage.contains("agent_cli::login_expired_line"),
        "検知段が文言の正本（agent_cli::login_expired_line）を呼んでいない: \
         語句をこの段へ直書きすると #983 の未認証検知と二重管理になる"
    );
    assert!(
        stage.contains("LOGIN_EXPIRED_TAIL"),
        "検知段が窓（LOGIN_EXPIRED_TAIL）を使っていない: \
         窓を外すとスクロールバックの引用で誤検知する"
    );
}

/// #757 の実観測 3 文言（小文字。正本が持っている形）
const OBSERVED_PHRASES: [&str; 3] = [
    "oauth refresh token is no longer valid",
    "login expired",
    "please run /login",
];

/// 正本のファイルに小文字の語句が何回現れるか（fixture は実際の大文字小文字なので当たらない）
fn lowercase_hits(src: &str, phrase: &str) -> usize {
    src.matches(&format!("\"{phrase}\"")).count()
}

#[test]
fn 失効の文言は正本1か所だけが持つ() {
    let agent_cli = read("crates/tako-control/src/orchestrator/agent_cli.rs");
    assert!(
        agent_cli.contains("pub fn login_expired_line"),
        "正本 login_expired_line が無い"
    );
    // 未認証検知（#983）が正本へ委譲していること = 語句の持ち主が 1 つ
    assert!(
        agent_cli.contains("if login_expired_line(line) {"),
        "looks_unauthenticated_screen が正本へ委譲していない: \
         語句が 2 か所になると片方に足したときもう片方が取りこぼす"
    );
    for phrase in OBSERVED_PHRASES {
        let hits = lowercase_hits(&agent_cli, phrase);
        assert_eq!(
            hits, 1,
            "#757 の実観測文言 `{phrase}` が正本に {hits} 回ある（1 回であること）"
        );
    }
    // 検出力: 語句を 1 つ落としたソースでは 0 回になる
    let dropped = agent_cli.replace("    \"please run /login\",\n", "");
    assert_eq!(
        lowercase_hits(&dropped, "please run /login"),
        0,
        "語句を落としても数が変わらない検査になっている"
    );
}

#[test]
fn 手順書はreloginがresumeで解けないと言い切っている() {
    let guide = read("crates/tako-control/src/orchestrator/guides/monitoring.md");
    assert!(
        guide.contains("`login_expired` (action: relogin)"),
        "手順書に login_expired の項目が無い"
    );
    assert!(
        guide.contains("never solved by\n  `resume`") || guide.contains("never solved by `resume`"),
        "手順書が「relogin は resume では解けない」と言い切っていない"
    );
    assert!(
        guide.contains("`/login`"),
        "手順書がユーザーへ依頼するコマンドを書いていない"
    );
    // 手順書が自分の文言で自分を釣らないこと（worker のペインで guide を実行した画面が
    // 失効の検知にかかってはいけない）
    let lower = guide.to_ascii_lowercase();
    for phrase in OBSERVED_PHRASES {
        assert!(
            !lower.contains(phrase),
            "手順書に検知語句 `{phrase}` が入っている: \
             worker のペインで guide を表示した画面が login_expired と誤検知される"
        );
    }
}
