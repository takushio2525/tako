//! 番犬: スマホからのコマンドカード実行を**権限外へ開かない**（#1724）。
//!
//! ## 何を止めたいのか
//!
//! #1724 は「PC のカードの『実行』と同じ dispatch を remote からも呼べるようにする」だけの
//! 変更で、実行ロジックは 1 行も足していない。だから壊れ方は実行の中身ではなく**門**にある:
//!
//! 1. 表の role が**黙って緩む**（Interact → Observe = 見るだけの端末が PC でコマンドを走らせる）
//! 2. `remote::required_role` が表を引かなくなる / 受け口が表を通らない枝へ繋がる
//! 3. 受け口の**二重の門**が外れる（配線を誤ったときの最後の網）
//! 4. スマホから**コマンドの本文**を渡せる口が生える（`show` / `commands` を素通しする）
//! 5. PWA が表に無い `/api/cards…` を呼ぶ / observe に「実行」を出す
//! 6. 実行記録の語彙が PC のカードとずれる・絵文字が混ざる
//! 7. persist.log / 監査にコマンド本文が載る（AGENTS.md の絶対ルール）
//!
//! の 7 つ。ここはそれをソース走査で押さえる。**落ちるときは file:line で名指し**。
//!
//! ## 相方（実挙動）
//!
//! 「observe / 降格直後 / 失効 / 未登録は 403・interact は通る」は実 HTTP の全経路テスト
//! `remote::tests::issue1724_権限外からのカード実行はapiで拒否される` が見る。
//! 画面の出し分けは e2e（`web/tako-remote/e2e/command-cards-1724.spec.js`）、PC 側の
//! カードまで含めた実経路は `scripts/test-remote-command-card-1724.sh`。
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① `remote_cards.rs` の Run の `role: DeviceRole::Interact` を `Observe` へ /
//! ② `remote.rs` の `required_role` から `remote_cards::role_for(` の分岐を消す /
//! ③ `handle_cards_request` の `if deps.role < route.role {` の門を消す /
//! ④ `dispatch_route` の `commands: Vec::new()` を `body["commands"]` 由来へ変える

use std::path::{Path, PathBuf};

use tako_control::remote_auth::DeviceRole;
use tako_control::remote_cards::{self, CardPurpose, CARD_ROUTES};

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

const CARDS_REL: &str = "crates/tako-control/src/remote_cards.rs";
const REMOTE_REL: &str = "crates/tako-control/src/remote.rs";
const UI_TEXT_REL: &str = "crates/tako-app/src/ui_text/command_card.rs";
const PWA_API_REL: &str = "web/tako-remote/src/api.js";
const PWA_CARDS_REL: &str = "web/tako-remote/src/components/command-cards.jsx";
const PWA_TERMINAL_REL: &str = "web/tako-remote/src/pages/terminal.jsx";

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

/// 注入 ①: Run の role を緩める。**表のその行**を名指す
#[test]
fn 実行はinteract以上で一覧はobserve() {
    let src = production(CARDS_REL);
    let lines: Vec<&str> = src.lines().collect();
    for r in CARD_ROUTES {
        let (expect, marker) = match r.purpose {
            CardPurpose::List => (DeviceRole::Observe, "purpose: CardPurpose::List"),
            CardPurpose::Run => (DeviceRole::Interact, "purpose: CardPurpose::Run"),
        };
        // 表の中でその経路の `role:` 行を引く（file:line で名指すため）
        let at = lines
            .iter()
            .position(|l| l.contains(marker))
            .unwrap_or_else(|| panic!("{CARDS_REL}: 表に {marker} が無い"));
        let role_line = lines
            .iter()
            .enumerate()
            .skip(at)
            .find(|(_, l)| l.trim_start().starts_with("role:"))
            .map(|(i, l)| (i + 1, l.trim().to_string()))
            .unwrap_or_else(|| panic!("{CARDS_REL}:{}: {marker} の role 行が無い", at + 1));
        assert_eq!(
            r.role, expect,
            "{CARDS_REL}:{}: {} ({} {}) の role が Issue #1724 の判断とずれている \
             （一覧は Observe・実行は Interact = ペインへ打鍵できる役割と同じ）: `{}`",
            role_line.0, r.client_method, r.method, r.sample_path, role_line.1
        );
        assert_eq!(
            remote_cards::role_for(r.method, r.sample_path),
            Some(r.role),
            "{CARDS_REL}:{}: {} の role が表から引けない",
            role_line.0,
            r.client_method
        );
    }
    // 表に無い受け口（カードを作る / PC のクリップボードを書く / 閉じる）は床の Manage
    for verb in ["show", "copy", "dismiss"] {
        assert_eq!(
            remote_cards::role_for("POST", &format!("/api/cards/7/{verb}")),
            Some(DeviceRole::Manage),
            "{CARDS_REL}: `/api/cards/7/{verb}` が床の Manage へ落ちない"
        );
    }
    // 表は list / run の 2 本だけ（本文を受け取る action を表に載せない）
    let actions: Vec<&str> = CARD_ROUTES.iter().map(|r| r.action).collect();
    assert_eq!(
        actions,
        vec!["list", "run"],
        "{CARDS_REL}: 表の action が list / run 以外を素通しする"
    );
}

// --------------------------------------------- 2. 認可とルータの配線

/// 注入 ②: `required_role` が表を引かなくなる / 受け口が表を通らない枝へ繋がる
#[test]
fn 認可とルータが表を通る() {
    let remote = production(REMOTE_REL);
    let (body, at) = fn_body(REMOTE_REL, &remote, "fn required_role(method:");
    let text = joined(&body);
    assert!(
        text.contains("remote_cards::role_for("),
        "{REMOTE_REL}:{at}: required_role が `remote_cards::role_for` を引いていない \
         = カード実行の role が表から外れる（#1724）"
    );
    assert!(
        !text.contains("\"/api/cards"),
        "{REMOTE_REL}:{}: required_role がカード経路の判定を**直書き**している \
         = 表と実際の認可が 2 か所に割れる",
        line_in(&body, "\"/api/cards").unwrap_or(at)
    );
    // 表の分岐は「未知の POST = Manage」より**前**に居る（後ろだと interact が通れない）
    let table_at = line_in(&body, "remote_cards::role_for(").unwrap_or(0);
    let post_floor = line_in(&body, "if *method == tiny_http::Method::Post").unwrap_or(usize::MAX);
    assert!(
        table_at < post_floor,
        "{REMOTE_REL}:{table_at}: 表の分岐が未知の POST の床（{REMOTE_REL}:{post_floor}）より後ろに居る"
    );

    // ルータ: 受け口は `owns_path` の枝の中で、認可済みの role を渡される
    let (routes, routes_at) = fn_body(REMOTE_REL, &remote, "fn handle_api_v2_routes(");
    let owns = line_in(&routes, "remote_cards::owns_path(");
    let call = line_in(&routes, "remote_cards::handle_cards_request(");
    match (owns, call) {
        (Some(o), Some(c)) if o < c => {}
        _ => panic!(
            "{REMOTE_REL}:{routes_at}: `/api/cards*` の受け口が `owns_path` の枝で配線されていない \
             （owns={owns:?} call={call:?}）"
        ),
    }
    // カードの枝の中（owns_path の行から受け口の呼び出しまで）で探す。
    // 他の受け口（タスク・ファイル）も同じ行を持つので、最初の一致では判定しない
    let role_pass = routes
        .iter()
        .find(|(n, l)| {
            *n > owns.unwrap_or(0) && *n < call.unwrap_or(0) && l.contains("role: device.role,")
        })
        .map(|(n, _)| *n);
    assert!(
        role_pass.is_some(),
        "{REMOTE_REL}:{routes_at}: 受け口へ認可済みの `device.role` を渡していない \
         = 二重の門が何を見ているか分からない"
    );
    // 受け口は `authorize_device` を通った後段（`handle_api_v2_routes`）にしか居ない
    let (front, front_at) = fn_body(REMOTE_REL, &remote, "fn handle_request_v2(");
    assert!(
        line_in(&front, "remote_cards::").is_none(),
        "{REMOTE_REL}:{}: カードの受け口が層②（role 認可）より**前**に繋がっている",
        line_in(&front, "remote_cards::").unwrap_or(front_at)
    );
}

// --------------------------------------------- 3. 受け口の二重の門

/// 注入 ③: 受け口の門を外す / 門より先に dispatch を呼ぶ
#[test]
fn 受け口は表のroleに届かない端末を断る() {
    let src = production(CARDS_REL);
    let (body, at) = fn_body(CARDS_REL, &src, "pub fn handle_cards_request(");
    let gate = line_in(&body, "if deps.role < route.role {");
    let dispatch = line_in(&body, "dispatch_route(");
    match (gate, dispatch) {
        (Some(g), Some(d)) if g < d => {
            // 門の中で 403 を返してから return している（素通しの枝になっていない）
            let text: String = body
                .iter()
                .filter(|(n, _)| *n >= g && *n < d)
                .map(|(_, l)| l.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains("403") && text.contains("return respond_json("),
                "{CARDS_REL}:{g}: 二重の門が 403 を返して止まっていない"
            );
        }
        _ => panic!(
            "{CARDS_REL}:{at}: 受け口の二重の門（`if deps.role < route.role {{`）が \
             dispatch より前に無い（gate={gate:?} dispatch={dispatch:?}）。\
             配線を誤ったときに実行が弱い端末へ開く（#1724）"
        ),
    }
}

// --------------------------------------------- 4. 本文を受け取らない

/// 注入 ④: スマホから本文を渡せるようにする / show を素通しする
#[test]
fn スマホからコマンドの本文を渡す口が無い() {
    let src = production(CARDS_REL);
    let (body, at) = fn_body(CARDS_REL, &src, "fn dispatch_route(");
    let text = joined(&body);
    let empty = line_in(&body, "commands: Vec::new(),");
    assert!(
        empty.is_some(),
        "{CARDS_REL}:{at}: `ShowCommand` の commands が空で固定されていない \
         = スマホから任意のコマンドを PC で走らせられる"
    );
    for forbidden in [
        "body[\"commands\"]",
        "get(\"commands\")",
        "\"show\"",
        "label: body",
    ] {
        if let Some(n) = line_in(&body, forbidden) {
            panic!(
                "{CARDS_REL}:{n}: 受け口が `{forbidden}` を読んでいる \
                 = カードの本文 / 作成をスマホから受け付ける（#1724）"
            );
        }
    }
    assert!(
        text.contains("action: Some(route.action.to_string())"),
        "{CARDS_REL}:{at}: action を表以外から組んでいる"
    );

    // PWA 側も番号しか送らない
    let api = read(PWA_API_REL);
    let run_at = api
        .lines()
        .position(|l| l.contains("runCommandCard(cardId, index)"))
        .unwrap_or_else(|| panic!("{PWA_API_REL}: runCommandCard の呼び口が無い"));
    let call = api.lines().nth(run_at + 1).unwrap_or_default();
    assert!(
        call.contains("/run`, { index })"),
        "{PWA_API_REL}:{}: 実行の本文が番号だけになっていない: `{}`",
        run_at + 2,
        call.trim()
    );
}

// --------------------------------------------- 5. PWA の経路と門

/// PWA が表に無い `/api/cards…` を呼ぶ / 表の呼び口が api.js から消える
#[test]
fn pwaが呼ぶカードapiはすべて表に在る() {
    let shapes: Vec<String> = CARD_ROUTES
        .iter()
        .map(|r| r.sample_path.replace("/7/", "/<id>/"))
        .collect();
    let mut stray = Vec::new();
    for rel in [PWA_API_REL, PWA_CARDS_REL, PWA_TERMINAL_REL] {
        for (i, line) in read(rel).lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            let mut rest = line;
            while let Some(at) = rest.find("/api/cards") {
                let raw = take_path(&rest[at..]);
                if !shapes.contains(&normalize_template(&raw)) {
                    stray.push(format!("{rel}:{}: {raw}", i + 1));
                }
                rest = &rest[at + "/api/cards".len()..];
            }
        }
    }
    assert!(
        stray.is_empty(),
        "PWA が経路表（`remote_cards::CARD_ROUTES`）に無い API を呼んでいる:\n{}",
        stray.join("\n")
    );
    let api = read(PWA_API_REL);
    for r in CARD_ROUTES {
        assert!(
            api.contains(&format!("{}(", r.client_method)),
            "{PWA_API_REL}: 表の呼び口 {} が無い（{} {}）",
            r.client_method,
            r.method,
            r.sample_path
        );
    }
}

/// 引用符・クエリ（`?`）・区切りまでを 1 本のパスとして取る。
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

/// `${...}` を `<id>` へ潰す（`/` の直後 = 可変の id。それ以外は落とす）
fn normalize_template(raw: &str) -> String {
    let mut out = String::new();
    let mut rest = raw;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        if out.ends_with('/') {
            out.push_str("<id>");
        }
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

/// PWA が observe の端末に「実行」を出さない（押せないボタンを並べない）
#[test]
fn 画面がobserveに実行を出さない() {
    let pwa = read(PWA_CARDS_REL);
    let run = CARD_ROUTES
        .iter()
        .find(|r| r.purpose == CardPurpose::Run)
        .expect("Run の経路");
    let decl = format!("const RUN_ROLE = '{}';", run.role.as_str());
    let at = pwa
        .lines()
        .position(|l| l.contains("const RUN_ROLE ="))
        .map(|i| i + 1)
        .unwrap_or(0);
    assert!(
        pwa.contains(&decl),
        "{PWA_CARDS_REL}:{at}: 実行に要る role が daemon の表（{}）と一致していない（`{decl}` が無い）",
        run.role.as_str()
    );
    let can_run = pwa
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("const canRun ="))
        .unwrap_or_else(|| panic!("{PWA_CARDS_REL}: canRun の宣言が無い"));
    assert!(
        can_run.1.contains("roleAtLeast(") && can_run.1.contains("RUN_ROLE"),
        "{PWA_CARDS_REL}:{}: canRun が role から作られていない: `{}`",
        can_run.0 + 1,
        can_run.1.trim()
    );
    // 実行ボタンは canRun の内側・権限不足の導線は #1452 の部品
    let button = pwa
        .lines()
        .position(|l| l.contains("data-testid=\"command-run\""))
        .unwrap_or_else(|| panic!("{PWA_CARDS_REL}: 実行ボタンが無い"));
    let lines: Vec<&str> = pwa.lines().collect();
    let guard = lines[..button]
        .iter()
        .rposition(|l| l.contains("{canRun && ("));
    // `{canRun && (` → `<button` → 属性 2〜3 行 → `data-testid` の並び（字下げの 1 塊）
    assert!(
        guard.is_some_and(|g| button - g <= 6),
        "{PWA_CARDS_REL}:{}: 実行ボタンが `canRun &&` の内側に無い",
        button + 1
    );
    assert!(
        pwa.contains("<PermissionRequest") && pwa.contains("need={RUN_ROLE}"),
        "{PWA_CARDS_REL}: 権限不足の導線が #1452 の `PermissionRequest` へ繋がっていない"
    );
    // ペイン画面へ組み込まれている（部品だけあって画面に出ない、を落とす）
    let terminal = read(PWA_TERMINAL_REL);
    assert!(
        terminal.contains("<CommandCards") && terminal.contains("onMeRefresh={onMeRefresh}"),
        "{PWA_TERMINAL_REL}: ペイン画面にカードが組み込まれていない（または権限の取り直しを渡していない）"
    );
}

// --------------------------------------------- 6. 語彙と絵文字

/// 実行記録の言い方が PC のカードと同じ（同じ記録を 2 つの画面が読む）
#[test]
fn 実行記録の語彙がpcのカードと同じ() {
    let ui = read(UI_TEXT_REL);
    let pwa = read(PWA_CARDS_REL);
    for (pc, phone) in [
        ("tr!(\"実行中\", \"Running\")", "text: '実行中'"),
        (
            "format!(\"実行済み（終了コード {code}）\")",
            "`実行済み（終了コード ${run.exit_code}）`",
        ),
        (
            "tr!(\"実行済み（ペインは閉じた）\"",
            "'実行済み（ペインは閉じた）'",
        ),
    ] {
        assert!(ui.contains(pc), "{UI_TEXT_REL}: PC 側の文言 `{pc}` が無い");
        assert!(
            pwa.contains(phone),
            "{PWA_CARDS_REL}: PWA 側の文言 `{phone}` が PC（{UI_TEXT_REL} の `{pc}`）とずれている"
        );
    }
}

/// 絵文字の範囲は GUI 側の 1 定義（`ui_text::assert_no_emoji`）と同じにする
fn is_emoji(c: char) -> bool {
    let cp = c as u32;
    (0x1F000..=0x1FAFF).contains(&cp) || (0x2600..=0x27BF).contains(&cp) || cp == 0xFE0F
}

#[test]
fn 画面に絵文字が無い() {
    let hits: Vec<String> = read(PWA_CARDS_REL)
        .lines()
        .enumerate()
        .filter(|(_, l)| l.chars().any(is_emoji))
        .map(|(i, l)| format!("{PWA_CARDS_REL}:{}: {}", i + 1, l.trim()))
        .collect();
    assert!(
        hits.is_empty(),
        "UI に絵文字を使わない（アイコンは SVG）:\n{}",
        hits.join("\n")
    );
}

// --------------------------------------------- 7. 診断ログに本文を出さない

/// persist.log / 監査に載せるのは端末・カード・番号・結果だけ
#[test]
fn 診断ログと監査にコマンド本文を載せない() {
    let src = production(CARDS_REL);
    let (body, at) = fn_body(CARDS_REL, &src, "pub fn handle_cards_request(");
    let log_at = line_in(&body, "リモートからコマンドカードを実行").unwrap_or_else(|| {
        panic!("{CARDS_REL}:{at}: 「どの端末から・どのカードを」の persist.log が無い")
    });
    // その行から `));` までが format! の引数
    let args: Vec<&(usize, String)> = body
        .iter()
        .skip_while(|(n, _)| *n < log_at)
        .take_while(|(_, l)| !l.trim_start().starts_with("));"))
        .collect();
    for (n, line) in &args {
        // 識別子として比べる（`outcome` を `out` と取り違えない）
        let tokens: Vec<&str> = line
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|t| !t.is_empty())
            .collect();
        for forbidden in ["command", "commands", "body", "out", "cwd", "value"] {
            assert!(
                !tokens.contains(&forbidden),
                "{CARDS_REL}:{n}: persist.log に `{forbidden}` を載せている \
                 = コマンド本文や応答が診断ログへ出る（AGENTS.md の絶対ルール）: `{}`",
                line.trim()
            );
        }
    }
    let audit_at = line_in(&body, "(deps.audit)(")
        .unwrap_or_else(|| panic!("{CARDS_REL}:{at}: 監査への記録が無い"));
    let audit_args = body
        .iter()
        .find(|(n, l)| *n > audit_at && l.contains("json!("))
        .map(|(n, l)| (*n, l.clone()))
        .unwrap_or_else(|| panic!("{CARDS_REL}:{audit_at}: 監査の中身が読めない"));
    assert!(
        audit_args.1.contains("\"card\"")
            && audit_args.1.contains("\"index\"")
            && !audit_args.1.contains("command"),
        "{CARDS_REL}:{}: 監査に何をしたか以外を載せている: `{}`",
        audit_args.0,
        audit_args.1.trim()
    );
}
