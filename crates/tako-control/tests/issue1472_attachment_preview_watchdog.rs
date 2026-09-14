//! 番犬: 添付をその場で見せる経路を**増やさない**（#1472 B）。
//!
//! ## 何を止めたいのか
//!
//! #1472 B は「見せ方を足すだけで、配信経路は 1 本のまま」が設計の中身。
//! `<img>` / `<video>` の `src` は **`/api/files/download`（#1079）に
//! `disposition=inline` を足しただけ**で、認可は `resolve_in_root` の 1 実装のまま。
//! だから壊れ方は
//!
//! 1. PWA が**プレビュー専用の URL** を組み始める（= 認可の判断からこぼれる）
//! 2. daemon に**プレビュー専用の受け口**が生える（`FILE_ROUTES` の外 = #1405 の型）
//! 3. `Range` の解釈が**2 実装**に割れる（片方だけ直してオフバイワンが残る）
//! 4. MIME の表を**その場に写す**（`open_plan` とずれて `<video>` が鳴らなくなる）
//! 5. 画像 / 動画の判定を **PWA が拡張子で**やり直す（daemon の表とずれる）
//! 6. **一覧が添付を描き始める**（開いてもいないタスクの数百 MB を取りに行く）
//!
//! の 6 つ。ここはその 6 つをソース走査で押さえる。**落ちるときは file:line で名指し**。
//!
//! ## 相方
//!
//! 「実際に絵が出る / metadata が読める / 一覧では 1 本も飛ばない」は e2e
//! （`web/tako-remote/e2e/tasks-preview-1472.spec.js`）、
//! 「Range が 206 で返る / observe は 403 / 300 MB でも常駐量が増えない」は実経路テスト
//! （`scripts/test-remote-attachment-preview-1472.sh`）が見る。ここは**宣言と実装の一致**だけ。

use std::path::{Path, PathBuf};

use tako_control::remote_auth::DeviceRole;
use tako_control::remote_files::{self, FILE_ROUTES};

// 本番コードだけの眺めは 1 実装を通す（#1420 / #1445）
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const FILES_REL: &str = "crates/tako-control/src/remote_files.rs";
const TASKS_REL: &str = "crates/tako-control/src/remote_tasks.rs";
const PWA_REL: &str = "web/tako-remote/src/pages/tasks.jsx";
const OPEN_PLAN_REL: &str = "crates/tako-core/src/open_plan.rs";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("///") || t.starts_with('*') || t.starts_with("/*")
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

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// コメントを除いた「本文の行」（file:line で名指しできる形）
fn code_lines(rel: &str, src: &str) -> Vec<(String, usize, String)> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l))
        .map(|(i, l)| (rel.to_string(), i + 1, l.to_string()))
        .collect()
}

fn hits(rel: &str, src: &str, needle: &str) -> Vec<String> {
    code_lines(rel, src)
        .into_iter()
        .filter(|(_, _, l)| l.contains(needle))
        .map(|(r, n, l)| format!("{r}:{n}: {}", l.trim()))
        .collect()
}

// --------------------------------------------- 1. 添付の URL は 1 実装

/// 注入 ①: PWA がプレビュー専用の URL を組む / 2 か所目の `download` を書く
#[test]
fn 添付のurlはpwaの1実装だけが組む() {
    let pwa = read(PWA_REL);

    // (a) 組み立ては `attachmentUrl` の中だけ
    let (body, at) = fn_body(PWA_REL, &pwa, "export function attachmentUrl(");
    let text = joined(&body);
    assert!(
        text.contains("/api/files/download?"),
        "{PWA_REL}:{at}: 添付の URL が #1079 のファイル API を組んでいない"
    );
    assert!(
        text.contains("disposition") && text.contains("inline"),
        "{PWA_REL}:{at}: その場表示の指定（`disposition=inline`）が消えている \
         = `<video>` が MIME を見て鳴らなくなる（#1472）"
    );

    // (b) その 1 か所以外に配信経路の綴りが無い（コメントは除く）
    let found = hits(PWA_REL, &pwa, "/api/files/");
    assert_eq!(
        found.len(),
        1,
        "添付の配信経路が {} か所に散っている（認可の判断が割れる）:\n{}",
        found.len(),
        found.join("\n")
    );

    // (c) 画面が組む src / href は**必ずその 1 本の戻り**（生の文字列で組まない）
    for forbidden in ["src=\"", "src={`", "href=\"${", "href={`"] {
        let found = hits(PWA_REL, &pwa, forbidden);
        assert!(
            found.is_empty(),
            "画面が URL を生で組んでいる（`attachmentUrl` を迂回する経路）:\n{}",
            found.join("\n")
        );
    }
    // `<source>` を使うと src が 2 か所に増える（1 本の `src` 属性に閉じる）
    assert!(
        hits(PWA_REL, &pwa, "<source").is_empty(),
        "{PWA_REL}: `<source>` を使っている（src の組み立てが増える）"
    );

    // (d) 経路の role は表のまま（`FILE_ROUTES` から外れていない）
    assert_eq!(
        remote_files::required_role_for_method("GET", "/api/files/download"),
        Some(DeviceRole::Interact),
        "添付の配信に使う経路の role が `FILE_ROUTES` から外れた"
    );
}

// --------------------------------------------- 2. daemon の受け口

/// 注入 ②: `/api/files/preview` のような専用の受け口を足す
#[test]
fn 配信の受け口は表のぶんしか無い() {
    let src = production(FILES_REL);
    let (body, at) = fn_body(FILES_REL, &src, "pub fn handle_files_request(");
    let text = joined(&body);

    // ルーターが並べるパスと表が 1:1（片方だけ増えない）
    let declared: Vec<&str> = FILE_ROUTES.iter().map(|r| r.path).collect();
    let mut routed: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.split_once("\"/api/files") else {
            continue;
        };
        let path = rest.1.split('"').next().unwrap_or_default();
        routed.push(format!("/api/files{path}"));
    }
    routed.sort();
    routed.dedup();
    assert!(
        !routed.is_empty(),
        "{FILES_REL}:{at}: 受け口を 1 つも読めていない"
    );
    for p in &routed {
        assert!(
            declared.contains(&p.as_str()),
            "{FILES_REL}:{at}: 表（`FILE_ROUTES`）に無い受け口 {p} が生えている \
             = role の宣言からこぼれる（#1405 / #1472）"
        );
    }

    // 表そのものに配信の 2 本目が生えていない（見せ方は同じ経路のクエリで足す）
    for r in FILE_ROUTES {
        for banned in ["preview", "media", "stream", "thumb", "inline"] {
            assert!(
                !r.path.contains(banned),
                "`FILE_ROUTES` に配信の 2 本目 {} が生えている（#1472 は経路を増やさない）",
                r.path
            );
        }
    }

    // 見せ方の指定は download の分岐の中にある（別ハンドラへ割れていない）
    assert!(
        text.contains("\"disposition\"") && text.contains("respond_download("),
        "{FILES_REL}:{at}: `?disposition` の解釈が download の分岐から離れている"
    );
}

// --------------------------------------------- 3. Range は 1 実装

/// 注入 ③: 2 本目の Range 解釈を書く / タスク側が Range を読む
#[test]
fn rangeを読むのは1実装だけ() {
    let src = production(FILES_REL);

    // (a) 解釈の本体が在る（純粋関数）
    let (body, at) = fn_body(FILES_REL, &src, "pub fn parse_range(");
    let text = joined(&body);
    for token in ["bytes=", "Unsatisfiable", "Part {"] {
        assert!(
            text.contains(token),
            "{FILES_REL}:{at}: `parse_range` が {token} を扱っていない（解釈が痩せた）"
        );
    }

    // (b) ヘッダを読む口はここだけ（`respond_download` が拾って渡す）
    let readers = hits(FILES_REL, &src, "equiv(\"Range\")");
    assert_eq!(
        readers.len(),
        1,
        "`Range` ヘッダを読む口が {} か所ある（解釈が割れる）:\n{}",
        readers.len(),
        readers.join("\n")
    );
    let parsers = hits(FILES_REL, &src, "\"bytes=\"");
    assert!(
        parsers.len() <= 1,
        "`bytes=` の解釈が {} か所ある（`parse_range` の写し）:\n{}",
        parsers.len(),
        parsers.join("\n")
    );

    // (c) 応答を組むのも 1 本（`respond_file`）。206 / 416 が別のところで組まれない
    let (resp, resp_at) = fn_body(FILES_REL, &src, "fn respond_file(");
    let resp_text = joined(&resp);
    for token in [
        "parse_range(",
        "206",
        "416",
        "Content-Range",
        "Accept-Ranges",
    ] {
        assert!(
            resp_text.contains(token),
            "{FILES_REL}:{resp_at}: 部分応答の要素 {token} が `respond_file` から消えている"
        );
    }
    // **送出は 1 か所**（枝ごとに `respond` を書くと共通ヘッダの付け忘れが生える。
    // 実際 `remote_files_audit_watchdog` の「応答は no-store」が枝ぶんの付与を数える）
    let sends = resp_text.matches("request.respond(").count();
    assert_eq!(
        sends, 1,
        "{FILES_REL}:{resp_at}: 応答の送出が {sends} か所（1 か所へ畳んだはず）"
    );
    for token in ["206", "416"] {
        let found = resp_text.matches(token).count();
        assert_eq!(
            found, 1,
            "{FILES_REL}:{resp_at}: ステータス {token} を決める場所が {found} か所ある"
        );
    }

    // (d) タスク側は Range も配信も知らない（B3 の不変条件の延長）
    let tasks = production(TASKS_REL);
    for forbidden in ["Range", "Content-Range", "parse_range(", "respond_file("] {
        let found = hits(TASKS_REL, &tasks, forbidden);
        assert!(
            found.is_empty(),
            "タスク側が配信の事情を持ち始めた（認可と配信が 2 実装に割れる）:\n{}",
            found.join("\n")
        );
    }
}

// --------------------------------------------- 4. MIME の表

/// 注入 ④: 拡張子 → MIME の表をその場に写す
#[test]
fn mimeの表はopen_planの1本だけ() {
    let files = production(FILES_REL);
    let (body, at) = fn_body(FILES_REL, &files, "fn respond_file(");
    let text = joined(&body);
    assert!(
        text.contains("open_plan::media_type("),
        "{FILES_REL}:{at}: インライン配信の `Content-Type` が `open_plan` の表を通っていない \
         = `<video>` が鳴らない綴りが混ざる（#1472）"
    );

    // 型のリテラルは正本（`open_plan.rs`）にだけ在る
    for rel in [FILES_REL, TASKS_REL] {
        let src = production(rel);
        for banned in ["image/png", "image/jpeg", "video/mp4", "video/webm"] {
            let found = hits(rel, &src, banned);
            assert!(
                found.is_empty(),
                "MIME の表が {rel} へ写っている（{OPEN_PLAN_REL} とずれる）:\n{}",
                found.join("\n")
            );
        }
    }
    // 正本には在る（走査対象が空になっていないことの裏返し）
    let plan = read(OPEN_PLAN_REL);
    assert!(
        plan.contains("pub fn media_type_for_extension(") && plan.contains("\"video/mp4\""),
        "{OPEN_PLAN_REL}: MIME の正本が消えている"
    );
}

// --------------------------------------------- 5. 画像 / 動画の判定

/// 注入 ⑤: PWA が拡張子で判定し直す / daemon が `open_plan` を通さなくなる
#[test]
fn 画像か動画かの判定はdaemonの1実装() {
    // (a) daemon 側は `open_plan` の表を通る
    let tasks = production(TASKS_REL);
    let (body, at) = fn_body(TASKS_REL, &tasks, "fn preview_kind(");
    let text = joined(&body);
    assert!(
        text.contains("open_plan::preview_route("),
        "{TASKS_REL}:{at}: 添付の種別が `open_plan` の表を通っていない \
         = cmd+クリックや `tako open` と別の判断が生える"
    );
    let (dec, dec_at) = fn_body(TASKS_REL, &tasks, "fn decorate_attachment(");
    assert!(
        joined(&dec).contains("preview_kind("),
        "{TASKS_REL}:{dec_at}: 添付に種別が載らなくなった（PWA が判定を持つしかなくなる）"
    );

    // (b) PWA は `att.preview` を見るだけ（拡張子の表を持たない）
    let pwa = read(PWA_REL);
    assert!(
        pwa.contains("att.preview === 'image'") && pwa.contains("att.preview === 'video'"),
        "{PWA_REL}: 画面が daemon の宣言（`attachments[].preview`）を見ていない"
    );
    for banned in [
        ".endsWith('.mp4",
        ".endsWith('.png",
        "'mp4'",
        "'png'",
        "'jpeg'",
        "'webm'",
        "split('.').pop",
    ] {
        let found = hits(PWA_REL, &pwa, banned);
        assert!(
            found.is_empty(),
            "画面が拡張子で判定し直している（daemon の表とずれる）:\n{}",
            found.join("\n")
        );
    }
}

// --------------------------------------------- 6. 一覧は取りに行かない

/// 注入 ⑥: 一覧の行にサムネを足す
#[test]
fn 一覧は添付を描かない() {
    let pwa = read(PWA_REL);
    let (row, at) = fn_body(PWA_REL, &pwa, "function TaskRow(");
    let text = joined(&row);
    for banned in [
        "<img",
        "<video",
        "AttachmentPreview",
        "attachmentUrl(",
        "attachments",
    ] {
        assert!(
            !text.contains(banned),
            "{PWA_REL}:{at}: 一覧の行が添付（{banned}）に触っている \
             = 開いてもいないタスクの数百 MB を取りに行く（#1472 の受け入れ条件）"
        );
    }

    // 描く側は詳細の中だけ（`AttachmentPreview` を呼ぶのは `Attachment` の 1 か所）
    let callers = hits(PWA_REL, &pwa, "<AttachmentPreview");
    assert_eq!(
        callers.len(),
        1,
        "プレビューを描く場所が {} か所ある（一覧へ漏れうる）:\n{}",
        callers.len(),
        callers.join("\n")
    );
    // 動画は必ず `preload="metadata"`（本体を先読みしない）
    let (prev, prev_at) = fn_body(PWA_REL, &pwa, "function AttachmentPreview(");
    let prev_text = joined(&prev);
    assert!(
        prev_text.contains("preload=\"metadata\""),
        "{PWA_REL}:{prev_at}: `<video>` の `preload=\"metadata\"` が外れた（本体を丸ごと読み始める）"
    );
    assert!(
        prev_text.contains("controls"),
        "{PWA_REL}:{prev_at}: `<video>` から `controls` が消えた（再生も停止もできない）"
    );
}
