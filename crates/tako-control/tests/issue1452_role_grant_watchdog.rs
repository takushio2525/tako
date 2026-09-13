//! 番犬: 端末の権限を**上げる**経路を PC の人の操作から外させない（#1452）。
//!
//! ## 何を止めたいのか
//!
//! #1452 は「権限が足りません」に導線を付けただけに見えるが、中身は
//! **昇格の境界を触る変更**。壊れ方は 5 つある:
//!
//! 1. 昇格できる経路が **CLI / MCP から叩ける**ようになる（= AI が自分で上げられる）。
//!    しかも MCP / CLI の IPC は **tako-app の中の dispatch** で走るので、daemon 側の
//!    呼び出し元ゲートからは GUI と区別が付かない —— だから
//!    `remote::devices_set_role` が daemon へ行く前に断る形が要で、ここが崩れると
//!    「番犬が通っているのに上げられる」状態になる
//! 2. PWA が**表に無い API** を呼び始める（= role の判断からこぼれる。#1405 の懸念）
//! 3. 承認 UI が**別実装**を持つ（同じ判断が 2 か所に分かれ、片方だけ古くなる）
//! 4. リクエストが**無言で落ちる**（送ったのに誰も気づかない = #1399 系の再来）
//! 5. ユーザータスク（#1450）から `remote_auth::approve` への**逆向きの経路**が生える
//!    （`tako todo done` が権限を与えるようになる = 正本の 2 層分けが崩れる）
//!
//! ここはその 5 つをソース走査で押さえる。**落ちるときは file:line で名指し**
//! （直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 表そのものの不変条件（昇格経路は 2 本・どちらも `gui_only`）は
//! `remote_role` の単体テスト。実際に 403 が返ることと画面の見え方は
//! 実経路テスト `scripts/test-remote-role-1452.sh` と e2e
//! `web/tako-remote/e2e/permission-request-1452.spec.js` が見る。
//! ここは**宣言と実装の一致**だけを見る。

use std::path::{Path, PathBuf};

use tako_control::remote_role::{self, ROLE_ROUTES};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリのルートを解決できる")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// `needle` を含む行を `(行番号, 行)` で拾う。コメント行は除く
/// （doc に経路名を書いただけで落ちると、説明を書けなくなる）
fn hits(text: &str, needle: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            !t.starts_with("//") && !t.starts_with("*") && !t.starts_with("#")
        })
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .collect()
}

fn report(rel: &str, found: &[(usize, String)]) -> String {
    found
        .iter()
        .map(|(line, text)| format!("  {rel}:{line}: {text}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// --- 1. 昇格経路は CLI / MCP のソースから叩かれない ---------------------------

/// AI・CLI 側の面（ここから昇格経路の path が見えてはいけない）
const AI_FACING: [&str; 4] = [
    "crates/tako-cli/src/main.rs",
    "crates/tako-control/src/dispatch.rs",
    "crates/tako-control/src/mcp/request.rs",
    "crates/tako-control/src/mcp/catalog.rs",
];

#[test]
fn 昇格経路はcliとmcpのソースから叩かれていない() {
    let mut problems = Vec::new();
    for route in ROLE_ROUTES.iter().filter(|r| r.grants_upgrade) {
        for rel in AI_FACING {
            let found = hits(&read(rel), route.sample_path);
            if !found.is_empty() {
                problems.push(format!(
                    "昇格できる経路 {} {} が AI から届く面に現れた:\n{}",
                    route.method,
                    route.sample_path,
                    report(rel, &found)
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "#1452 の不変条件が壊れた（権限を上げる操作は PC の人の操作だけ）:\n{}\n\
         上げる経路は tako-app が `admin_request` で直接叩くものだけにする。\
         CLI / MCP から届くのは `remote::devices_set_role`（降格専用）だけ",
        problems.join("\n")
    );
}

#[test]
fn 降格専用の関数が方向の判断をdaemonの手前で行っている() {
    let rel = "crates/tako-control/src/remote.rs";
    let src = read(rel);
    let start = src
        .find("pub fn devices_set_role(")
        .unwrap_or_else(|| panic!("{rel}: devices_set_role が無い（#1452 の 1 段目）"));
    // 関数の終わり = 次の `\n}` の直後まで（この関数は入れ子の `}` を持たない形に保つ）
    let end = src[start..]
        .find("\n}\n")
        .map(|i| start + i)
        .unwrap_or(src.len());
    let body = &src[start..end];
    let line_of = |needle: &str| body.find(needle).map(|i| src[..start + i].lines().count());
    assert!(
        body.contains("CallerCheck::NotGui"),
        "{rel}:{}: devices_set_role が `CallerCheck::NotGui` で問うていない。\n\
         MCP / CLI の dispatch は **tako-app の中**で走るので、daemon 側のゲートからは\n\
         GUI と区別が付かない。方向の判断を daemon の手前で済ませないと AI から上げられる",
        src[..start].lines().count() + 1
    );
    let decide_at = line_of("remote_role::decide").unwrap_or(0);
    let admin_at = line_of("admin_request(").unwrap_or(usize::MAX);
    assert!(
        decide_at < admin_at,
        "{rel}:{admin_at}: devices_set_role が判断より先に admin_request を撃っている。\n\
         昇格は HTTP へ出る前に断ること（daemon には tako-app の接続に見えるため）"
    );
}

// --- 2. PWA の呼び口はすべて表に在る -----------------------------------------

/// 権限リクエストの画面（ここが呼ぶ API は全部 role の判断を通る必要がある）
const PWA_PERMISSION_UI: &str = "web/tako-remote/src/components/permission-request.jsx";

/// `client.foo(` / `createClient().foo(` の `foo` を行番号つきで拾う
fn client_calls(text: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for prefix in ["client.", "createClient()."] {
            let mut rest = line;
            while let Some(at) = rest.find(prefix) {
                let after = &rest[at + prefix.len()..];
                let name: String = after
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() && after[name.len()..].starts_with('(') {
                    out.push((name, i + 1));
                }
                rest = &rest[at + prefix.len()..];
            }
        }
    }
    out
}

#[test]
fn 権限リクエスト画面が呼ぶapiはすべて経路表に在る() {
    let src = read(PWA_PERMISSION_UI);
    let calls = client_calls(&src);
    assert!(
        !calls.is_empty(),
        "{PWA_PERMISSION_UI}: API の呼び口が 1 つも無い（走査が空振りしている）"
    );
    let unknown: Vec<String> = calls
        .iter()
        .filter(|(name, _)| remote_role::route_for_client_method(name).is_none())
        .map(|(name, line)| format!("  {PWA_PERMISSION_UI}:{line}: client.{name}()"))
        .collect();
    assert!(
        unknown.is_empty(),
        "経路表（remote_role::ROLE_ROUTES）に無い API を PWA が呼んでいる:\n{}\n\
         表に載せてから使うこと（載らない経路は role の判断からこぼれる = #1405）",
        unknown.join("\n")
    );
}

#[test]
fn 権限リクエスト画面は生のfetchで管理apiを叩かない() {
    let src = read(PWA_PERMISSION_UI);
    let mut problems = Vec::new();
    for needle in ["/api/admin/", "fetch("] {
        let found = hits(&src, needle);
        if !found.is_empty() {
            problems.push(report(PWA_PERMISSION_UI, &found));
        }
    }
    assert!(
        problems.is_empty(),
        "権限リクエスト画面が管理 API / 生 fetch を持っている:\n{}\n\
         スマホから管理 API へは到達できない（`check_admin` が XFF 付きを弾く）。\
         呼び口は `api.js` の 1 か所に保つ",
        problems.join("\n")
    );
}

// --- 3. 承認 UI は 1 実装 ------------------------------------------------------

#[test]
fn 承認ダイアログと承認の呼び口は1か所ずつ() {
    let panel = read("crates/tako-app/src/remote_panel.rs");
    let dialogs = hits(&panel, "fn render_pairing_dialog");
    assert_eq!(
        dialogs.len(),
        1,
        "承認ダイアログの実装が 1 つでない:\n{}",
        report("crates/tako-app/src/remote_panel.rs", &dialogs)
    );

    // 承認の口（`/api/admin/pair/approve`）を持ってよいのは remote_panel.rs だけ
    let approve_path = ROLE_ROUTES
        .iter()
        .find(|r| r.purpose == remote_role::RolePurpose::Approve)
        .expect("承認の経路が表に在る")
        .sample_path;
    let mut problems = Vec::new();
    for rel in [
        "crates/tako-app/src/settings_window.rs",
        "crates/tako-app/src/main.rs",
        "crates/tako-app/src/right_panel.rs",
    ] {
        let found = hits(&read(rel), approve_path);
        if !found.is_empty() {
            problems.push(report(rel, &found));
        }
    }
    assert!(
        problems.is_empty(),
        "承認の呼び口が remote_panel.rs 以外にも生えた:\n{}\n\
         同じ判断を 2 か所に置くと片方だけ古くなる。設定画面から与えたいときは\
         `/api/admin/devices/role`（権限を選び直す 1 経路）を使うこと",
        problems.join("\n")
    );
    let panel_hits = hits(&panel, approve_path);
    assert_eq!(
        panel_hits.len(),
        1,
        "remote_panel.rs の承認の呼び口が 1 か所でない:\n{}",
        report("crates/tako-app/src/remote_panel.rs", &panel_hits)
    );
}

// --- 4. 無言で落ちない ---------------------------------------------------------

#[test]
fn リクエストの起票と権限変更の失敗が無言にならない() {
    // daemon: 依頼を起票したら結果（起票 / 重複 / 届かない）を診断へ 1 行
    let remote = read("crates/tako-control/src/remote.rs");
    let file_at = remote
        .find("remote_role::file_upgrade_task(")
        .unwrap_or_else(|| panic!("remote.rs: 依頼の起票が無い"));
    let tail = &remote[file_at..];
    let window_end = tail.find("return respond_sensitive").unwrap_or(tail.len());
    assert!(
        tail[..window_end].contains("persist_log"),
        "crates/tako-control/src/remote.rs:{}: 依頼の起票結果を診断へ出していない\n\
         （届かなかったことに誰も気づけない = #1399 系の再来）",
        remote[..file_at].lines().count() + 1
    );

    // GUI: 権限の変更に失敗したら画面へ出す
    let settings = read("crates/tako-app/src/settings_window.rs");
    let admin_at = settings
        .find("fn remote_admin(")
        .unwrap_or_else(|| panic!("settings_window.rs: remote_admin が無い"));
    let body_end = settings[admin_at..]
        .find("\n    }\n")
        .map(|i| admin_at + i)
        .unwrap_or(settings.len());
    assert!(
        settings[admin_at..body_end].contains("self.message = Some((e, true))"),
        "crates/tako-app/src/settings_window.rs:{}: 権限変更の失敗を画面へ出していない\n\
         （昇格を断られたことが分からないと、ユーザーは押し続ける）",
        settings[..admin_at].lines().count() + 1
    );

    // PWA: 送信に失敗したら理由を出す
    let pwa = read(PWA_PERMISSION_UI);
    let send_at = pwa
        .find("async function send(")
        .unwrap_or_else(|| panic!("{PWA_PERMISSION_UI}: send が無い"));
    let send_end = pwa[send_at..]
        .find("\n  }\n")
        .map(|i| send_at + i)
        .unwrap_or(pwa.len());
    assert!(
        pwa[send_at..send_end].contains("setError("),
        "{PWA_PERMISSION_UI}:{}: 送信の失敗を画面へ出していない",
        pwa[..send_at].lines().count() + 1
    );
}

#[test]
fn 承認待ちのポーリングは必ず止まる() {
    let pwa = read(PWA_PERMISSION_UI);
    // setInterval を張ったら、承認・拒否・打ち切りのどれでも止める口が要る
    let starts = hits(&pwa, "setInterval(");
    assert_eq!(
        starts.len(),
        1,
        "承認待ちのポーリングが 1 か所でない（止め忘れが生まれる）:\n{}",
        report(PWA_PERMISSION_UI, &starts)
    );
    for needle in ["clearInterval(", "clearTimeout(", "setTimeout("] {
        assert!(
            !hits(&pwa, needle).is_empty(),
            "{PWA_PERMISSION_UI}: `{needle}` が無い。\n\
             承認待ちの間だけ見に行き、承認 / 拒否 / 打ち切りで必ず止めること\
             （押していない画面がポーリングを続けない）"
        );
    }
}

// --- 5. ユーザータスクから権限は動かない ---------------------------------------

#[test]
fn ユーザータスクから権限を動かす経路は無い() {
    let mut problems = Vec::new();
    for rel in [
        "crates/tako-control/src/user_tasks.rs",
        "crates/tako-core/src/user_task.rs",
    ] {
        let src = read(rel);
        for needle in ["remote_auth", "DeviceRole", "set_role", "devices_set_role"] {
            let found = hits(&src, needle);
            if !found.is_empty() {
                problems.push(report(rel, &found));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "ユーザータスク側から権限の世界へ手が伸びている:\n{}\n\
         #1452 の正本は 2 層（授権 = DeviceRegistry.pending / 記録 = tako todo）で、\
         **タスクを done / dismiss しても権限は 1 ミリも動かない**のが不変条件。\
         逆向きの依存を作らないこと",
        problems.join("\n")
    );
}

// --- 検出力（注入を戻したら名指しできるか） ------------------------------------

#[test]
fn i1452_注入_検査の材料がすべて実在する() {
    // 走査対象が消えている / 名前が変わっていると、上の検査は「空振りで緑」になる。
    // ここで実在だけを確かめておく（回帰を隠さない番犬にするための土台）
    for rel in AI_FACING {
        assert!(!read(rel).is_empty(), "{rel} が空（走査対象が消えている）");
    }
    assert!(
        repo_root().join(PWA_PERMISSION_UI).is_file(),
        "{PWA_PERMISSION_UI} が無い（権限リクエストの画面が別ファイルへ移った？）"
    );
    assert_eq!(
        ROLE_ROUTES.iter().filter(|r| r.grants_upgrade).count(),
        2,
        "昇格できる経路の数が変わった。増やすなら gui_only にし、\
         この番犬の走査対象（AI_FACING）にも現れないことを確かめること"
    );
    // 昇格経路の具体形が空文字だと `hits` が全行に当たって常時 FAILED になる
    for route in ROLE_ROUTES.iter().filter(|r| r.grants_upgrade) {
        assert!(
            route.sample_path.starts_with("/api/admin/"),
            "昇格経路 {} が管理 API の形をしていない",
            route.sample_path
        );
    }
}
