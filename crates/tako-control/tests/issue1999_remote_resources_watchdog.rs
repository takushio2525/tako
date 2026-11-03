//! 番犬: スマホからの順番待ちの変更（優先度・取り消し）を**権限外へ開かない**（#1999。エピック #1992 の S7）。
//!
//! ## 何を止めたいのか
//!
//! #1999 は「PC の resources ビュー・CLI `tako queue` と同じ `Request::Queue` を remote からも呼べるようにする」
//! だけの変更で、並び・段・取り消しのロジックは 1 行も足していない。だから壊れ方は中身ではなく**門**にある:
//!
//! 1. 表の role が**黙って緩む**（Manage → Observe / Interact = 見るだけの端末が他人のビルドを止める）
//! 2. `remote::required_role` が表を引かなくなる / 受け口が表を通らない枝へ繋がる
//! 3. 受け口の**二重の門**が外れる（配線を誤ったときの最後の網）
//! 4. スマホから上限の書き換え（`limits`）や wrapper 専用の操作（`job`）を渡せる口が生える
//! 5. PWA が表に無い `/api/resources…` を呼ぶ / manage 未満に変更の口を出す / 絵文字が混ざる
//! 6. persist.log / 監査にコマンドの表示名・ペインのタイトルが載る（AGENTS.md の絶対ルール）
//!
//! の 6 つ。ここはそれをソース走査で押さえる。**落ちるときは file:line で名指し**。
//!
//! ## 相方（実挙動）
//!
//! 「observe / interact / 降格直後 / 失効 / 未登録は 403・manage は通る」は実 HTTP の全経路テスト
//! `remote::tests::issue1999_権限外からの順番待ちの変更はapiで拒否される` が見る。画面の出し分けは
//! e2e（`web/tako-remote/e2e/resources-1999.spec.js`）、PC の `tako queue --json` との突き合わせは
//! `scripts/test-remote-resources-1999.sh`。
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① `remote_resources.rs` の Cancel の `role: DeviceRole::Manage` を `Observe` へ /
//! ② `remote.rs` の `required_role` から `remote_resources::role_for(` の分岐を消す /
//! ③ `handle_resources_request` の `if deps.role < route.role {` の門を消す /
//! ④ `dispatch_route` の `limits: None,` を `body` 由来へ変える /
//! ⑤ `web/tako-remote/src/pages/resources.jsx` の `MANAGE_ROLE` を `'interact'` へ

use std::path::{Path, PathBuf};

use tako_control::remote_auth::DeviceRole;
use tako_control::remote_resources::{self, ResourcePurpose, RESOURCE_ROUTES};

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

const RESOURCES_REL: &str = "crates/tako-control/src/remote_resources.rs";
const REMOTE_REL: &str = "crates/tako-control/src/remote.rs";
const PWA_API_REL: &str = "web/tako-remote/src/api.js";
const PWA_PAGE_REL: &str = "web/tako-remote/src/pages/resources.jsx";
const PWA_PANES_REL: &str = "web/tako-remote/src/pages/panes.jsx";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('*')
}

/// テスト領域だけを空白へ潰した眺め（バイト長と行番号は保たれる）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

/// 関数 1 本の本体（署名の行から、同じ字下げの `}` まで。コメント行は除く）。
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

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 本体の中で `needle` を含む最初の行番号
fn line_in(body: &[(usize, String)], needle: &str) -> Option<usize> {
    body.iter()
        .find(|(_, l)| l.contains(needle))
        .map(|(n, _)| *n)
}

// --------------------------------------------- 1. 表の role

/// 注入 ①: 変更の role を緩める。**表のその行**を名指す
#[test]
fn 読むのはobserveで変更はmanage以上() {
    let src = production(RESOURCES_REL);
    let lines: Vec<&str> = src.lines().collect();
    for r in RESOURCE_ROUTES {
        let (expect, marker) = match r.purpose {
            ResourcePurpose::Status => (DeviceRole::Observe, "purpose: ResourcePurpose::Status"),
            ResourcePurpose::JobPriority => {
                (DeviceRole::Manage, "purpose: ResourcePurpose::JobPriority")
            }
            ResourcePurpose::PanePriority => {
                (DeviceRole::Manage, "purpose: ResourcePurpose::PanePriority")
            }
            ResourcePurpose::Cancel => (DeviceRole::Manage, "purpose: ResourcePurpose::Cancel"),
        };
        let at = lines
            .iter()
            .position(|l| l.contains(marker))
            .unwrap_or_else(|| panic!("{RESOURCES_REL}: 表に {marker} が無い"));
        let role_line = lines
            .iter()
            .enumerate()
            .skip(at)
            .find(|(_, l)| l.trim_start().starts_with("role:"))
            .map(|(i, l)| (i + 1, l.trim().to_string()))
            .unwrap_or_else(|| panic!("{RESOURCES_REL}:{}: {marker} の role 行が無い", at + 1));
        assert_eq!(
            r.role, expect,
            "{RESOURCES_REL}:{}: {} ({} {}) の role が Issue #1999 の判断とずれている \
             （読むのは全役割 = Observe・優先度の変更と取り消しは Manage 以上）: `{}`",
            role_line.0, r.client_method, r.method, r.sample_path, role_line.1
        );
        assert_eq!(
            remote_resources::role_for(r.method, r.sample_path),
            Some(r.role),
            "{RESOURCES_REL}:{}: {} の role が表から引けない",
            role_line.0,
            r.client_method
        );
        // 変更の経路は監査に残す（読むだけの経路は残さない）
        assert_eq!(
            r.audit_event.is_some(),
            r.purpose != ResourcePurpose::Status,
            "{RESOURCES_REL}:{}: {} の監査イベントの有無が変更 / 読みと合っていない",
            role_line.0,
            r.client_method
        );
    }
    // 表に無い受け口（上限の書き換え・agent の起動待ちを起こす・wrapper 専用）は床の Manage
    for tail in ["limits", "jobs/7/start", "jobs/7/enqueue", "jobs/7/done"] {
        assert_eq!(
            remote_resources::role_for("POST", &format!("/api/resources/{tail}")),
            Some(DeviceRole::Manage),
            "{RESOURCES_REL}: `/api/resources/{tail}` が床の Manage へ落ちない"
        );
    }
    // 表は status / priority / cancel だけ（上限・wrapper 専用の action を表に載せない）
    let mut actions: Vec<&str> = RESOURCE_ROUTES.iter().map(|r| r.action).collect();
    actions.sort_unstable();
    actions.dedup();
    assert_eq!(
        actions,
        vec!["cancel", "priority", "status"],
        "{RESOURCES_REL}: 表の action が status / priority / cancel 以外を素通しする"
    );
}

// --------------------------------------------- 2. 認可とルータの配線

/// 注入 ②: `required_role` が表を引かなくなる / 受け口が表を通らない枝へ繋がる
#[test]
fn 認可とルータが表を通る() {
    let remote = production(REMOTE_REL);
    let (body, at) = fn_body(REMOTE_REL, &remote, "fn required_role(method:");
    assert!(
        joined(&body).contains("remote_resources::role_for("),
        "{REMOTE_REL}:{at}: required_role が `remote_resources::role_for` を引いていない \
         = 順番待ちの変更の role が表から外れる（#1999）"
    );
    if let Some(n) = line_in(&body, "\"/api/resources") {
        panic!(
            "{REMOTE_REL}:{n}: required_role が順番待ちの経路の判定を**直書き**している \
             = 表と実際の認可が 2 か所に割れる"
        );
    }
    // 表の分岐は「未知の POST = Manage」より**前**に居る
    let table_at = line_in(&body, "remote_resources::role_for(").unwrap_or(0);
    let post_floor = line_in(&body, "if *method == tiny_http::Method::Post").unwrap_or(usize::MAX);
    assert!(
        table_at < post_floor,
        "{REMOTE_REL}:{table_at}: 表の分岐が未知の POST の床（{REMOTE_REL}:{post_floor}）より後ろに居る"
    );

    let (routes, routes_at) = fn_body(REMOTE_REL, &remote, "fn handle_api_v2_routes(");
    let owns = line_in(&routes, "remote_resources::owns_path(");
    let call = line_in(&routes, "remote_resources::handle_resources_request(");
    let (owns, call) = match (owns, call) {
        (Some(o), Some(c)) if o < c => (o, c),
        _ => panic!(
            "{REMOTE_REL}:{routes_at}: `/api/resources*` の受け口が `owns_path` の枝で配線されていない \
             （owns={owns:?} call={call:?}）"
        ),
    };
    let role_pass = routes
        .iter()
        .any(|(n, l)| *n > owns && *n < call && l.contains("role: device.role,"));
    assert!(
        role_pass,
        "{REMOTE_REL}:{owns}: 受け口へ認可済みの `device.role` を渡していない \
         = 二重の門が何を見ているか分からない"
    );
    let (front, front_at) = fn_body(REMOTE_REL, &remote, "fn handle_request_v2(");
    assert!(
        line_in(&front, "remote_resources::").is_none(),
        "{REMOTE_REL}:{}: 順番待ちの受け口が層②（role 認可）より**前**に繋がっている",
        line_in(&front, "remote_resources::").unwrap_or(front_at)
    );
}

// --------------------------------------------- 3. 受け口の二重の門

/// 注入 ③: 受け口の門を外す / 門より先に dispatch を呼ぶ
#[test]
fn 受け口は表のroleに届かない端末を断る() {
    let src = production(RESOURCES_REL);
    let (body, at) = fn_body(RESOURCES_REL, &src, "pub fn handle_resources_request(");
    let gate = line_in(&body, "if deps.role < route.role {");
    let dispatch = line_in(&body, "dispatch_route(");
    match (gate, dispatch) {
        (Some(g), Some(d)) if g < d => {
            let text: String = body
                .iter()
                .filter(|(n, _)| *n >= g && *n < d)
                .map(|(_, l)| l.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains("403") && text.contains("return respond_json("),
                "{RESOURCES_REL}:{g}: 二重の門が 403 を返して止まっていない"
            );
        }
        _ => panic!(
            "{RESOURCES_REL}:{at}: 受け口の二重の門（`if deps.role < route.role {{`）が \
             dispatch より前に無い（gate={gate:?} dispatch={dispatch:?}）。\
             配線を誤ったときに変更が弱い端末へ開く（#1999）"
        ),
    }
}

// --------------------------------------------- 4. 上限・wrapper 専用の操作を受け取らない

/// 注入 ④: スマホから上限や wrapper の `job` を渡せるようにする
#[test]
fn スマホから上限とwrapper専用の操作を渡す口が無い() {
    let src = production(RESOURCES_REL);
    let (body, at) = fn_body(RESOURCES_REL, &src, "fn dispatch_route(");
    for fixed in ["job: None,", "limits: None,"] {
        assert!(
            line_in(&body, fixed).is_some(),
            "{RESOURCES_REL}:{at}: `Request::Queue` の `{fixed}` が固定されていない \
             = スマホから上限の書き換え / wrapper 専用の操作を渡せる"
        );
    }
    for forbidden in [
        "body[\"limits\"]",
        "get(\"limits\")",
        "body[\"job\"]",
        "get(\"job\")",
        "get(\"action\")",
        "\"limits\"",
        "\"enqueue\"",
        "\"start\"",
    ] {
        if let Some(n) = line_in(&body, forbidden) {
            panic!(
                "{RESOURCES_REL}:{n}: 受け口が `{forbidden}` を読んでいる \
                 = 表に無い順番待ちの操作をスマホから受け付ける（#1999）"
            );
        }
    }
    assert!(
        joined(&body).contains("action: Some(route.action.to_string())"),
        "{RESOURCES_REL}:{at}: action を表以外から組んでいる"
    );
}

// --------------------------------------------- 5. PWA の経路と門

/// 引用符・クエリ（`?`）・区切りまでを 1 本のパスとして取る（`${...}` は丸ごと含める）
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

/// `${...}` を `<id>` へ潰し、表の具体形の数字の段も `<id>` へ潰す
fn normalize(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        out.push_str("<id>");
        let end = rest[at..].find('}').map_or(rest.len(), |i| at + i + 1);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out.split('/')
        .map(|seg| {
            if !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()) {
                "<id>"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// PWA が表に無い `/api/resources…` を呼ぶ / 表の呼び口が api.js から消える
#[test]
fn pwaが呼ぶ順番待ちのapiはすべて表に在る() {
    let shapes: Vec<String> = RESOURCE_ROUTES
        .iter()
        .map(|r| normalize(r.sample_path))
        .collect();
    let mut stray = Vec::new();
    for rel in [PWA_API_REL, PWA_PAGE_REL, PWA_PANES_REL] {
        for (i, line) in read(rel).lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            let mut rest = line;
            while let Some(at) = rest.find("/api/resources") {
                let raw = take_path(&rest[at..]);
                if !shapes.contains(&normalize(&raw)) {
                    stray.push(format!("{rel}:{}: {raw}", i + 1));
                }
                rest = &rest[at + "/api/resources".len()..];
            }
        }
    }
    assert!(
        stray.is_empty(),
        "PWA が経路表（`remote_resources::RESOURCE_ROUTES`）に無い API を呼んでいる:\n{}",
        stray.join("\n")
    );
    let api = read(PWA_API_REL);
    for r in RESOURCE_ROUTES {
        assert!(
            api.contains(&format!("{}(", r.client_method)),
            "{PWA_API_REL}: 表の呼び口 {} が無い（{} {}）",
            r.client_method,
            r.method,
            r.sample_path
        );
    }
}

/// 注入 ⑤: 画面が manage 未満に変更の口を出す
#[test]
fn 画面はmanage未満に変更の口を出さない() {
    let pwa = read(PWA_PAGE_REL);
    let manage = RESOURCE_ROUTES
        .iter()
        .find(|r| r.purpose == ResourcePurpose::Cancel)
        .expect("Cancel の経路");
    let decl = format!("export const MANAGE_ROLE = '{}';", manage.role.as_str());
    let at = pwa
        .lines()
        .position(|l| l.contains("const MANAGE_ROLE ="))
        .map(|i| i + 1)
        .unwrap_or(0);
    assert!(
        pwa.contains(&decl),
        "{PWA_PAGE_REL}:{at}: 変更に要る role が daemon の表（{}）と一致していない（`{decl}` が無い）",
        manage.role.as_str()
    );
    let can = pwa
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("const canManage ="))
        .unwrap_or_else(|| panic!("{PWA_PAGE_REL}: canManage の宣言が無い"));
    assert!(
        can.1.contains("roleAtLeast(") && can.1.contains("MANAGE_ROLE"),
        "{PWA_PAGE_REL}:{}: canManage が role から作られていない: `{}`",
        can.0 + 1,
        can.1.trim()
    );
    // 段のチップ・取り消しのボタンは canManage の内側（`{canManage && (` の直後の塊）
    let lines: Vec<&str> = pwa.lines().collect();
    for testid in [
        "data-testid=\"prio-toggle\"",
        "data-testid=\"cancel-toggle\"",
    ] {
        let button = lines
            .iter()
            .position(|l| l.contains(testid))
            .unwrap_or_else(|| panic!("{PWA_PAGE_REL}: {testid} のボタンが無い"));
        let guard = lines[..button]
            .iter()
            .rposition(|l| l.contains("{canManage && ("));
        assert!(
            guard.is_some_and(|g| button - g <= 12),
            "{PWA_PAGE_REL}:{}: {testid} が `canManage &&` の内側に無い",
            button + 1
        );
    }
    assert!(
        pwa.contains("<PermissionRequest") && pwa.contains("need={MANAGE_ROLE}"),
        "{PWA_PAGE_REL}: 権限不足の導線が #1452 の `PermissionRequest` へ繋がっていない"
    );
    // ポーリングはタスクと同じ 1 実装（画面を離れたら止まる）
    assert!(
        pwa.contains("usePolling(")
            && !pwa
                .lines()
                .any(|l| l.contains("setInterval(") && !is_comment(l)),
        "{PWA_PAGE_REL}: ポーリングが `usePolling` の 1 実装を通っていない"
    );
    // `#/` のカードが一覧に組み込まれている（部品だけあって画面に出ない、を落とす）
    let panes = read(PWA_PANES_REL);
    assert!(
        panes.contains("<MachineCard") && panes.contains("'#/resources'"),
        "{PWA_PANES_REL}: `#/` に機のカード（と #/resources への入口）が無い"
    );
}

/// 絵文字の範囲は GUI 側の 1 定義（`ui_text::assert_no_emoji`）と同じにする
fn is_emoji(c: char) -> bool {
    let cp = c as u32;
    (0x1F000..=0x1FAFF).contains(&cp) || (0x2600..=0x27BF).contains(&cp) || cp == 0xFE0F
}

#[test]
fn 画面に絵文字が無い() {
    let hits: Vec<String> = [PWA_PAGE_REL, PWA_PANES_REL]
        .iter()
        .flat_map(|rel| {
            read(rel)
                .lines()
                .enumerate()
                .filter(|(_, l)| l.chars().any(is_emoji))
                .map(|(i, l)| format!("{rel}:{}: {}", i + 1, l.trim()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        hits.is_empty(),
        "UI に絵文字を使わない（印は CSS の矩形と SVG）:\n{}",
        hits.join("\n")
    );
}

// --------------------------------------------- 6. 診断ログに表示名を出さない

/// persist.log / 監査に載せるのは端末・操作・受付番号 / ペイン・段・結果だけ
#[test]
fn 診断ログと監査にコマンドの表示名を載せない() {
    let src = production(RESOURCES_REL);
    let (body, at) = fn_body(RESOURCES_REL, &src, "pub fn handle_resources_request(");
    let log_at = line_in(&body, "リモートから順番待ちを操作").unwrap_or_else(|| {
        panic!("{RESOURCES_REL}:{at}: 「どの端末から・何を」の persist.log が無い")
    });
    let args: Vec<&(usize, String)> = body
        .iter()
        .skip_while(|(n, _)| *n < log_at)
        .take_while(|(_, l)| !l.trim_start().starts_with("));"))
        .collect();
    for (n, line) in &args {
        let tokens: Vec<&str> = line
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|t| !t.is_empty())
            .collect();
        for forbidden in ["label", "title", "tab", "out", "value", "body"] {
            assert!(
                !tokens.contains(&forbidden),
                "{RESOURCES_REL}:{n}: persist.log に `{forbidden}` を載せている \
                 = コマンドの表示名やペインのタイトルが診断ログへ出る（AGENTS.md の絶対ルール）: `{}`",
                line.trim()
            );
        }
    }
    let audit_at = line_in(&body, "(deps.audit)(")
        .unwrap_or_else(|| panic!("{RESOURCES_REL}:{at}: 監査への記録が無い"));
    let audit: String = body
        .iter()
        .skip_while(|(n, _)| *n < audit_at)
        .take_while(|(_, l)| !l.trim_start().starts_with(");"))
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in ["label", "title", "\"tab\""] {
        assert!(
            !audit.contains(forbidden),
            "{RESOURCES_REL}:{audit_at}: 監査に `{forbidden}` を載せている（何をしたか以外）"
        );
    }
}
