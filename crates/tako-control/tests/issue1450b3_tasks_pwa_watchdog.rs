//! 番犬: スマホからタスクを片付ける経路と画面を**宣言から外さない**（#1450 B3）。
//!
//! ## 何を止めたいのか
//!
//! B3 は「操作を新設しない」ことが設計の中身で、PWA の `#/tasks` は B1 の
//! `Request::UserTask` を daemon が素通しする 4 本と、**既存のファイル API**
//! （`/api/files/download` = #1079）だけを呼ぶ。だから壊れ方は
//!
//! 1. PWA が**表に無い API** を呼び始める（= role の判断からこぼれる。#1405）
//! 2. 表の role が**黙って緩む**（Interact → Observe = 見るだけの端末が返答できる）
//! 3. **添付の配信が認可 1 実装を通らなくなる**（タスク側が自前でファイルを開く）
//! 4. **ポーリングが止まらなくなる**（画面を離れてもスマホが叩き続ける）
//! 5. 語彙が PC 版（B2）とずれる（とくに `sent` を「届いた」と書く）
//! 6. 絵文字が混ざる（UI の規約）
//!
//! の 6 つ。ここはその 6 つをソース走査で押さえる。**落ちるときは file:line で名指し**。
//!
//! ## 相方
//!
//! 「observe には操作の節が出ない」「押した先が 403」「添付が実際に落ちてくる」は
//! e2e（`web/tako-remote/e2e/tasks-1450b3.spec.js`）と実経路テスト
//! （`scripts/test-remote-tasks-1450b3.sh`）が見る。ここは**宣言と実装の一致**だけ。

use std::path::{Path, PathBuf};

use tako_control::remote_auth::DeviceRole;
use tako_control::remote_tasks::{self, TaskPurpose, SHARE_MAX_BYTES, TASK_ROUTES};

// 本番コードだけの眺めは 1 実装を通す（#1420）
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const TASKS_REL: &str = "crates/tako-control/src/remote_tasks.rs";
const REMOTE_REL: &str = "crates/tako-control/src/remote.rs";
const FILES_REL: &str = "crates/tako-control/src/remote_files.rs";
const PWA_REL: &str = "web/tako-remote/src/pages/tasks.jsx";
const PWA_API_REL: &str = "web/tako-remote/src/api.js";
const PWA_PANES_REL: &str = "web/tako-remote/src/pages/panes.jsx";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("///") || t.starts_with('*')
}

/// テスト領域だけを空白へ潰した眺め（バイト長と行番号は保たれる）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

/// 関数 1 本の本体（署名の行から、同じ字下げの `}` まで）。
/// **見つからないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn fn_body(rel: &str, src: &str, signature: &str) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(signature))
        .unwrap_or_else(|| panic!("{rel}: 目印 {signature:?} が消えている（走査範囲を作れない）"));
    let indent = " ".repeat(lines[start].len() - lines[start].trim_start().len());
    let close = format!("{indent}}}");
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}: {signature:?} の本体を閉じる `}}` が無い"));
    let body: Vec<(usize, String)> = (start..end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    assert!(
        body.len() >= 3,
        "{rel}:{}: {signature:?} の走査範囲が {} 行しかない（範囲取りが壊れている）",
        start + 1,
        body.len()
    );
    (body, start + 1)
}

/// JS の関数 1 本の本体（`function name(` から、同じ字下げの `}` まで）
fn js_body(rel: &str, src: &str, signature: &str) -> (String, usize) {
    let (body, at) = fn_body(rel, src, signature);
    (
        body.iter()
            .map(|(_, l)| l.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        at,
    )
}

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or(0)
}

// --------------------------------------------- 1. 経路の宣言

/// 注入 ①: PWA が表に無い `/api/tasks…` を呼ぶ / 呼び口の名前を表から外す
#[test]
fn pwaが呼ぶタスクapiはすべて表に在る() {
    let declared: Vec<&str> = TASK_ROUTES.iter().map(|r| r.sample_path).collect();
    // 可変部（`<id>`）を代表値へ潰した形で比べる
    let shapes: Vec<String> = declared.iter().map(|p| p.replace("u-1", "<id>")).collect();

    let mut stray: Vec<String> = Vec::new();
    for rel in [PWA_API_REL, PWA_REL, PWA_PANES_REL] {
        let text = read(rel);
        for (i, line) in text.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            let mut rest = line;
            while let Some(at) = rest.find("/api/tasks") {
                let raw = take_path(&rest[at..]);
                // `${encodeURIComponent(id)}` のようなテンプレート片を `<id>` へ潰す
                let shaped = normalize_template(&raw);
                if !shapes.contains(&shaped) {
                    stray.push(format!("{rel}:{}: {raw}", i + 1));
                }
                rest = &rest[at + "/api/tasks".len()..];
            }
        }
    }
    assert!(
        stray.is_empty(),
        "PWA が経路表（`remote_tasks::TASK_ROUTES`）に無い API を呼んでいる。\n\
         表に載せるまで role の判断からこぼれる（#1405）:\n{}",
        stray.join("\n")
    );

    // 表の呼び口が `api.js` に実在する（表が飾りにならない）
    let api = read(PWA_API_REL);
    for r in TASK_ROUTES {
        assert!(
            api.contains(&format!("{}(", r.client_method)),
            "{PWA_API_REL}: 表の呼び口 {} が無い（{} {}）",
            r.client_method,
            r.method,
            r.sample_path
        );
    }
    // 画面が呼んでいるのは表の呼び口だけ（`client.xxx(` の xxx が表 or 既存の共有経路）
    let known_shared = ["base", "tasks", "pair", "me"];
    let pwa = read(PWA_REL);
    for (i, line) in pwa.lines().enumerate() {
        if is_comment(line) {
            continue;
        }
        for (name, _) in client_calls(line) {
            let ok = remote_tasks::route_for_client_method(&name).is_some()
                || known_shared.contains(&name.as_str());
            assert!(
                ok,
                "{PWA_REL}:{}: `client.{name}(` が経路表にも共有経路にも無い",
                i + 1
            );
        }
    }
}

/// 引用符（`'` / `"` / `` ` ``）かクエリ（`?`）までを 1 本のパスとして取る。
/// テンプレートの `${...}` は**中に `(` や `/` があっても丸ごと**含める
fn take_path(tail: &str) -> String {
    let mut out = String::new();
    let mut chars = tail.chars().peekable();
    while let Some(c) = chars.next() {
        if matches!(c, '\'' | '"' | '`' | '?' | ' ' | ')' | ',') {
            break;
        }
        if c == '$' && chars.peek() == Some(&'{') {
            out.push('$');
            let mut depth = 0usize;
            for c2 in chars.by_ref() {
                out.push(c2);
                match c2 {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// `${...}` を潰した形にする。
/// `/` の直後なら可変の id（`<id>`）、そうでなければクエリ片（`${qs}`）なので落とす
fn normalize_template(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        if out.ends_with('/') {
            out.push_str("<id>");
        }
        // 対応する `}` まで飛ばす（入れ子も数える）
        let mut depth = 0usize;
        let mut skip_to = rest.len();
        for (i, c) in rest[at..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        skip_to = at + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &rest[skip_to..];
    }
    out.push_str(rest);
    out
}

fn client_calls(line: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for prefix in ["client.", "createClient()."] {
        let mut rest = line;
        while let Some(at) = rest.find(prefix) {
            let after = &rest[at + prefix.len()..];
            let name: String = after
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && after[name.len()..].starts_with('(') {
                out.push((name, 0));
            }
            rest = &rest[at + prefix.len()..];
        }
    }
    out
}

/// 注入 ②: 表の role を緩める / `remote.rs` が表を引かなくなる
#[test]
fn 操作はinteract以上で読むだけはobserve() {
    for r in TASK_ROUTES {
        let expect = match r.purpose {
            TaskPurpose::List => DeviceRole::Observe,
            _ => DeviceRole::Interact,
        };
        assert_eq!(
            r.role, expect,
            "{} ({} {}) の role が宣言とずれている（#1450 の記載: \
             一覧は Observe・返答 / 完了 / 却下は Interact）",
            r.client_method, r.method, r.sample_path
        );
        assert_eq!(
            remote_tasks::role_for(r.method, r.sample_path),
            Some(r.role),
            "{} の role が表から引けない",
            r.client_method
        );
    }
    // 表に無い受け口は Manage の床へ落ちる（未知の経路が弱い role へこぼれない）
    assert_eq!(
        remote_tasks::role_for("POST", "/api/tasks/u-1/purge"),
        Some(DeviceRole::Manage)
    );

    // `remote.rs` の認可が**表を引いている**（写しの分岐を持たない）
    let remote = production(REMOTE_REL);
    let (body, at) = fn_body(REMOTE_REL, &remote, "fn required_role(method:");
    let text = joined(&body);
    assert!(
        text.contains("remote_tasks::role_for("),
        "{REMOTE_REL}:{at}: required_role が `remote_tasks::role_for` を引いていない \
         = タスク経路の role が表から外れる（#1450 B3）"
    );
    assert!(
        !text.contains("\"/api/tasks\""),
        "{REMOTE_REL}:{at}: required_role がタスク経路の判定を**直書き**している \
         = 表と実際の認可が 2 か所に割れる"
    );
    // ルータがこのモジュールへ渡している（受け口が配線されている）
    let route_at = line_of(&remote, "remote_tasks::handle_tasks_request(");
    assert!(
        route_at > 0,
        "{REMOTE_REL}: `/api/tasks*` の受け口がルータに繋がっていない"
    );
    let owns_at = line_of(&remote, "remote_tasks::owns_path(");
    assert!(
        owns_at > 0 && owns_at < route_at,
        "{REMOTE_REL}:{route_at}: 受け口が `owns_path` の枝の中に居ない"
    );
}

/// PWA 側でも「操作は interact 以上」を宣言している（押せないボタンを出さない）
#[test]
fn 画面がobserveに操作を出さない() {
    let pwa = read(PWA_REL);
    for needle in [
        "const ACT_ROLE = 'interact'",
        "roleAtLeast(",
        "PermissionRequest",
    ] {
        assert!(
            pwa.contains(needle),
            "{PWA_REL}: {needle} が無い = observe の端末に操作を出している / \
             権限不足の導線（#1452）へ繋いでいない"
        );
    }
    // 返答フォームと完了 / 却下が **canAct の内側**に居る
    let (detail, at) = js_body(PWA_REL, &pwa, "function TaskDetail(");
    assert!(
        detail.contains("canAct ? (") && detail.contains("canAct &&"),
        "{PWA_REL}:{at}: 返答フォーム / 完了・却下が role の門の内側に無い"
    );
    // **門そのものが role から作られている**（`canAct = true` のような素通しを落とす）
    for (need, what) in [("canAct", "ACT_ROLE"), ("canDownload", "DOWNLOAD_ROLE")] {
        let line = detail
            .lines()
            .find(|l| l.contains(&format!("const {need} =")))
            .unwrap_or_default();
        assert!(
            line.contains("roleAtLeast(") && line.contains(what),
            "{PWA_REL}:{at}: {need} が role から作られていない（{what} を見ていない）:\n  {}",
            line.trim()
        );
    }
}

// --------------------------------------------- 2. 添付の配信

/// 注入 ③: タスク側が自前でファイルを開く / 独自の配下判定を持つ
#[test]
fn 添付の配信は認可1実装を通る() {
    let src = production(TASKS_REL);

    // (a) 解決は #1451 の 1 実装を通る（判定そのものを書き直していない）
    let (resolve, resolve_at) = fn_body(TASKS_REL, &src, "fn resolve_target(");
    let resolve_text = joined(&resolve);
    assert!(
        resolve_text.contains("remote_files::shortcut_target("),
        "{TASKS_REL}:{resolve_at}: 添付の解決が `remote_files::shortcut_target` を通っていない \
         = 認可（`resolve_in_root`）と別の判断が生える（#1450 B3 / #1451）"
    );
    // (a') 添付 1 件を組む側が、その 1 本を通る（横に別の解決が生えていない）
    let (body, at) = fn_body(TASKS_REL, &src, "fn decorate_attachment(");
    let text = joined(&body);
    assert!(
        text.contains("resolve_target(roots"),
        "{TASKS_REL}:{at}: 添付の解決が `resolve_target` を通っていない"
    );

    // (b) ルートは門（`local_roots_for`）を通った一覧しか使わない
    assert!(
        src.contains("remote_files::roots_for("),
        "{TASKS_REL}: ルートを `remote_files::roots_for`（= 門を通る 1 本）から取っていない"
    );
    for forbidden in ["fs_roots()", "local_roots_for(", "TreeRoot {"] {
        let hits: Vec<String> = src
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(forbidden) && !is_comment(l))
            .map(|(i, l)| format!("{TASKS_REL}:{}: {}", i + 1, l.trim()))
            .collect();
        assert!(
            hits.is_empty(),
            "タスク側がルートを自分で組んでいる（門を迂回する写し）:\n{}",
            hits.join("\n")
        );
    }

    // (c) ファイルを自分で開かない・自分で配らない（配信経路を増やさない）
    for forbidden in [
        "File::open",
        "from_file(",
        "std::fs::read(",
        "Content-Disposition",
        "resolve_in_root(",
    ] {
        let hits: Vec<String> = src
            .lines()
            .enumerate()
            .filter(|(_, l)| l.contains(forbidden) && !is_comment(l))
            .map(|(i, l)| format!("{TASKS_REL}:{}: {}", i + 1, l.trim()))
            .collect();
        assert!(
            hits.is_empty(),
            "タスク側が独自の配信 / 配下判定を持っている（認可が 2 実装に割れる）:\n{}",
            hits.join("\n")
        );
    }

    // (d) PWA が叩くダウンロードは**既存のファイル API**（新しい配信経路が生えていない）
    let pwa = read(PWA_REL);
    assert!(
        pwa.contains("/api/files/download?root="),
        "{PWA_REL}: 添付のダウンロードが #1079 のファイル API を使っていない"
    );
    assert!(
        tako_control::remote_files::required_role_for_method("GET", "/api/files/download")
            == Some(DeviceRole::Interact),
        "添付の配信に使う経路の role が `FILE_ROUTES` から外れた"
    );

    // (e) 操作は B1 の dispatch を素通しする（タスクの実装をここに写さない）
    let (route, route_at) = fn_body(TASKS_REL, &src, "fn dispatch_route(");
    let route_text = joined(&route);
    assert!(
        route_text.contains("Request::UserTask"),
        "{TASKS_REL}:{route_at}: 受け口が B1 の `Request::UserTask` を通っていない"
    );
    for forbidden in ["user_tasks::", "TaskStore", "user-tasks.yaml"] {
        assert!(
            !src.contains(forbidden),
            "{TASKS_REL}: 正本（{forbidden}）を daemon 側から直接触っている \
             = 通知・配送・排他を飛ばす経路ができる（#1450 B1）"
        );
    }
}

// --------------------------------------------- 3. ポーリング

/// 注入 ④: cleanup の `clearInterval` を消す / `visibilityState` の条件を外す /
/// 2 本目のタイマーを足す
#[test]
fn ポーリングは画面を離れると止まる() {
    let pwa = read(PWA_REL);
    let (body, at) = js_body(PWA_REL, &pwa, "export function usePolling(");
    for needle in ["clearInterval(", "visibilityState", "return () => {"] {
        assert!(
            body.contains(needle),
            "{PWA_REL}:{at}: usePolling に {needle} が無い \
             = 画面を離れてもスマホが叩き続ける（#1450 B3 / #1452 と同じ作法）"
        );
    }
    assert!(
        body.contains("removeEventListener("),
        "{PWA_REL}:{at}: visibilitychange の購読を外していない（リスナが溜まる）"
    );

    // タイマーの実装は 1 本だけ（画面とバッジが同じものを使う）
    let timers: Vec<String> = [PWA_REL, PWA_PANES_REL]
        .iter()
        .flat_map(|rel| {
            let text = read(rel);
            text.lines()
                .enumerate()
                .filter(|(_, l)| l.contains("setInterval(") && !is_comment(l))
                .map(|(i, l)| format!("{rel}:{}: {}", i + 1, l.trim()))
                .collect::<Vec<_>>()
        })
        .filter(|hit| !hit.starts_with(PWA_PANES_REL) || hit.contains("POLL_MS"))
        .collect();
    assert_eq!(
        timers.len(),
        // panes.jsx の既存ループ（#621）と usePolling の 2 本だけ
        2,
        "タスクのポーリングが別実装で増えている（止め忘れが 1 か所で見られなくなる）:\n{}",
        timers.join("\n")
    );
    // バッジも同じ 1 実装を通る
    assert!(
        pwa.contains("export function useOpenTaskCount(") && pwa.contains("usePolling(()"),
        "{PWA_REL}: バッジの件数取得が usePolling を通っていない"
    );
    assert!(
        read(PWA_PANES_REL).contains("useOpenTaskCount("),
        "{PWA_PANES_REL}: バッジが `useOpenTaskCount`（= 止め方つき）を使っていない"
    );
}

// --------------------------------------------- 4. 語彙と見た目

/// 注入 ⑤: `sent` を「届いた」と書く / 配送の状態を落とす
#[test]
fn 配送の語彙がpc版とずれない() {
    let pwa = read(PWA_REL);
    let (body, at) = js_body(PWA_REL, &pwa, "const DELIVERY_LABEL = {");
    for state in ["sent", "launched", "delivered", "failed"] {
        assert!(
            body.contains(&format!("{state}:")),
            "{PWA_REL}:{at}: 配送の状態 {state} の文言が無い（無言の状態ができる）"
        );
    }
    // `sent` の行に「届」を書かない（未確定を届いたと騙らない = B1 / B2 の物差し）
    let sent_line = body
        .lines()
        .find(|l| l.trim_start().starts_with("sent:"))
        .unwrap_or_default();
    assert!(
        !sent_line.contains('届'),
        "{PWA_REL}:{at}: `sent` を「届いた」と書いている（確認前を確定と言わない）:\n  {}",
        sent_line.trim()
    );
    // 確定したものだけ色を付ける（`sent` を緑にしない）
    let (tone, tone_at) = js_body(PWA_REL, &pwa, "const DELIVERY_TONE = {");
    assert!(
        !tone.contains("sent:"),
        "{PWA_REL}:{tone_at}: 未確定（sent）に色を付けている"
    );

    // 判断の 4 択と「選ぶまで送れない」が残っている（B2 と同じ誤爆防止）
    for needle in [
        "'approve'",
        "'reject'",
        "'needs_change'",
        "'answered'",
        "判断を選ぶと返せます",
        "disabled={!decision",
    ] {
        assert!(
            pwa.contains(needle),
            "{PWA_REL}: {needle} が無い = PC 版（B2）と語彙 / 誤爆防止がずれた"
        );
    }
}

/// 注入 ⑥: 絵文字を混ぜる（UI の規約。PWA も同じ）
#[test]
fn 画面に絵文字が無い() {
    for rel in [PWA_REL, PWA_API_REL] {
        let text = read(rel);
        let hits: Vec<String> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| l.chars().any(is_emoji))
            .map(|(i, l)| format!("{rel}:{}: {}", i + 1, l.trim()))
            .collect();
        assert!(
            hits.is_empty(),
            "UI に絵文字を使わない（アイコンは SVG / 状態は色と字で出す）:\n{}",
            hits.join("\n")
        );
    }
}

/// 絵文字の範囲は GUI 側の 1 定義（`ui_text::assert_no_emoji`）と同じにする。
/// 矢印（U+2190〜）は絵文字ではないので**含めない**（規約を 2 種類にしない）
fn is_emoji(c: char) -> bool {
    let cp = c as u32;
    (0x1F000..=0x1FAFF).contains(&cp) || (0x2600..=0x27BF).contains(&cp) || cp == 0xFE0F
}

/// 共有の上限が daemon 側の宣言と一致する（片方だけ変わらない）
#[test]
fn 共有の上限がpwaと一致する() {
    let pwa = read(PWA_REL);
    let declared = "export const SHARE_MAX_BYTES = 64 * 1024 * 1024;";
    assert!(
        pwa.contains(declared),
        "{PWA_REL}: 共有の上限が daemon 側（`remote_tasks::SHARE_MAX_BYTES` = {SHARE_MAX_BYTES}）と\
         同じ値で宣言されていない"
    );
    assert_eq!(SHARE_MAX_BYTES, 64 * 1024 * 1024);
    // 上限を超える添付は共有に載せない（ページが落ちる）
    assert!(
        pwa.contains("size > SHARE_MAX_BYTES"),
        "{PWA_REL}: 大きい添付を共有シートへ載せない判断が消えている"
    );
}

/// A/B は 1 軸（daemon の env と PWA のクエリの 2 つだけ）
#[test]
fn abは1軸に閉じている() {
    let src = production(TASKS_REL);
    assert!(
        src.contains("TAKO_1450B3_LEGACY"),
        "{TASKS_REL}: A/B の軸（TAKO_1450B3_LEGACY）が消えた = 回帰を再現できない"
    );
    let pwa = read(PWA_REL);
    assert!(
        pwa.contains("tako_1450b3_legacy"),
        "{PWA_REL}: PWA 側の A/B（?tako_1450b3_legacy=1）が消えた"
    );
    let escapes: Vec<String> = [PWA_REL, PWA_PANES_REL]
        .iter()
        .flat_map(|rel| {
            let text = read(rel);
            text.lines()
                .enumerate()
                .filter(|(_, l)| l.contains("tako_1450b3_legacy") && !is_comment(l))
                .map(|(i, l)| format!("{rel}:{}: {}", i + 1, l.trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        escapes.len(),
        1,
        "PWA の逃げ道は `legacy1450b3()` の 1 か所だけ（増えると腕が食い違う）:\n{}",
        escapes.join("\n")
    );
}

/// 添付の解決が role で変わる（= 門が効いている）ことを両アームで押さえる
#[test]
fn 添付の解決範囲はroleで変わる() {
    // `fs` ルートは manage 以上でしか一覧に載らない（#1451 の門）。
    // タスクはそのルートへ載せるだけなので、interact 端末はツリー配下しか開けない
    assert!(!tako_control::remote_files::allows_full_browse(
        DeviceRole::Interact,
        false
    ));
    assert!(tako_control::remote_files::allows_full_browse(
        DeviceRole::Manage,
        false
    ));
    let files = production(FILES_REL);
    let (body, at) = fn_body(FILES_REL, &files, "pub fn roots_for(");
    let text = joined(&body);
    assert!(
        text.contains("current_roots("),
        "{FILES_REL}:{at}: タスク用の入口が門（current_roots → local_roots_for）を通っていない"
    );
}
