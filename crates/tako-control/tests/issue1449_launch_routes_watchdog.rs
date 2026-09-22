//! 番犬: 「+」から立てる経路を**宣言表から外さない**（#1449）。
//!
//! ## 何を止めたいのか
//!
//! #1449 は「操作を新設しない」ことが設計の中身で、PWA の 3 択は既存の
//! `POST /api/tabs`（#1078）と `POST /api/ssh`（#1080 / #1006）を呼ぶだけ。
//! だから壊れ方は「新しい操作が生えること」ではなく、
//!
//! 1. PWA が**表に無い API** を呼び始める（= role の判断からこぼれる。#1405 の懸念。
//!    未知の GET は `required_role` の最後で `Observe` へ落ちるので、
//!    画面を見るだけの端末に配ってはいけないものが配られる）
//! 2. 表の role が**黙って緩む**（Manage → Interact / Observe）
//! 3. 起動したことが**監査ログに残らなくなる**
//! 4. 表と `api.js` の**実際の呼び先がずれる**（表が飾りになる）
//!
//! の 4 つ。ここはその 4 つをソース走査で押さえる。**落ちるときは file:line で名指し**
//! （直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 「observe に 3 択を出さない」「押した先が 403」は実 DOM の e2e
//! （`web/tako-remote/e2e/launch-menu-1449.spec.js`）と実経路テスト
//! （`scripts/test-remote-launch-1449.sh`）が見る。ここは**宣言と実装の一致**だけ。

use std::path::{Path, PathBuf};

use tako_control::remote_auth::DeviceRole;
use tako_control::remote_launch::{self, LaunchKind, LAUNCH_ROUTES};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリのルートを解決できる")
}

fn pwa_src() -> PathBuf {
    repo_root().join("web/tako-remote/src")
}

#[path = "common/code_view.rs"]
mod code_view;

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// **肯定の存在確認**（「経路表を引いている」）が見る眺め = コメントを落とした本文。
///
/// 全文へ `contains` すると、走査先の doc コメントに書いた同じ綴りで真になり、
/// 実体が消えても緑のままになる（#1609）。PWA（`.jsx`）は Rust ではないので
/// この眺めを通さず全文のまま見る
fn read_code(path: &Path) -> String {
    let rel = path.display().to_string();
    code_view::without_comments_checked(&read(path), &rel)
}

/// 「+」から辿り着けるすべての画面（ここが呼ぶ API は全部 role の判断を通る必要がある）
fn launcher_files() -> Vec<PathBuf> {
    let src = pwa_src();
    vec![
        src.join("components/launch-sheet.jsx"),
        src.join("components/master-launcher.jsx"),
        src.join("components/ssh.jsx"),
    ]
}

/// `client.foo(` / `createClient().foo(` の `foo` を、行番号つきで拾う
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
                // 呼び出しだけを拾う（`client.base` のような参照は対象外）
                if !name.is_empty() && after[name.len()..].starts_with('(') {
                    out.push((name, i + 1));
                }
                rest = &rest[at + prefix.len()..];
            }
        }
    }
    out
}

/// `api.js` の `<name>() { ... request('<METHOD>', '<path>' ...` を読む。
/// テンプレートリテラル（`` `/api/tabs/${id}/master` ``）は `${...}` を代表値へ潰す
fn api_js_routes() -> Vec<(String, String, String, usize)> {
    let path = pwa_src().join("api.js");
    let text = read(&path);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        // メソッド定義の行（`    createTab(cwd) {`）を探す
        let Some(open) = trimmed.find('(') else {
            continue;
        };
        let name: String = trimmed[..open].to_string();
        if name.is_empty()
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || !trimmed.trim_end().ends_with('{')
        {
            continue;
        }
        // 定義の直後（数行以内）にある `request('METHOD', '<path>'` を読む
        for probe in lines.iter().skip(i).take(8) {
            let Some(at) = probe.find("request(") else {
                continue;
            };
            let args = &probe[at + "request(".len()..];
            let Some((method, rest)) = quoted(args) else {
                continue;
            };
            let Some(sep) = rest.find(',') else { continue };
            let Some((raw_path, _)) = quoted(&rest[sep + 1..]) else {
                continue;
            };
            out.push((name.clone(), method, collapse_template(&raw_path), i + 1));
            break;
        }
    }
    out
}

/// 先頭の引用符（`'` / `"` / `` ` ``）で囲まれた 1 つ目の文字列と残りを返す
fn quoted(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let quote = s.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') {
        return None;
    }
    let body = &s[quote.len_utf8()..];
    let end = body.find(quote)?;
    Some((body[..end].to_string(), &body[end + quote.len_utf8()..]))
}

/// `` `/api/tabs/${encodeURIComponent(tabId)}/master` `` → `/api/tabs/7/master`
fn collapse_template(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        out.push('7');
        let Some(close) = rest[at..].find('}') else {
            break;
        };
        rest = &rest[at + close + 1..];
    }
    out.push_str(rest);
    out
}

#[test]
fn 起動画面が呼ぶapiはすべて経路表に載っている() {
    let mut missing = Vec::new();
    for file in launcher_files() {
        let text = read(&file);
        for (name, line) in client_calls(&text) {
            if remote_launch::route_for_client_method(&name).is_none() {
                missing.push(format!(
                    "{}:{line} — `client.{name}(` が `remote_launch::LAUNCH_ROUTES` に無い",
                    file.display()
                ));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "「+」から呼ぶ API が経路表から漏れている（role の判断をすり抜ける = #1405）。\n\
         `crates/tako-control/src/remote_launch.rs` の LAUNCH_ROUTES へ \
         role と監査イベントを宣言してから使うこと:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn 経路表のroleと呼び先がapi_jsと一致する() {
    let api = api_js_routes();
    let api_path = pwa_src().join("api.js");
    let mut problems = Vec::new();
    for route in LAUNCH_ROUTES {
        let Some((_, method, path, line)) =
            api.iter().find(|(name, ..)| name == route.client_method)
        else {
            problems.push(format!(
                "{}  — 経路表の `{}` に対応する呼び口が api.js に無い",
                api_path.display(),
                route.client_method
            ));
            continue;
        };
        if !method.eq_ignore_ascii_case(route.method) {
            problems.push(format!(
                "{}:{line} — `{}` の HTTP メソッドが表と違う（表 {} / api.js {}）",
                api_path.display(),
                route.client_method,
                route.method,
                method
            ));
        }
        if !route.matches(method, path) {
            problems.push(format!(
                "{}:{line} — `{}` の呼び先が表と違う（表 {} / api.js {}）",
                api_path.display(),
                route.client_method,
                route.sample_path,
                path
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "経路表と PWA の呼び口がずれている（表が飾りになる）:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn 新しいタブとプロセスを作る経路はmanage以上のまま() {
    let remote_rs = repo_root().join("crates/tako-control/src/remote.rs");
    for route in remote_launch::mutating_routes() {
        assert!(
            route.role >= DeviceRole::Manage,
            "{} ({} {}) の role が Manage 未満へ緩んでいる。\n\
             新しいタブとプロセスを作れる = 実質シェルアクセスなので、\
             close / resize と同じ強さで扱う（#1078 / #1080 / #1449）",
            route.client_method,
            route.method,
            route.sample_path
        );
    }
    // 認可が本当にこの表を通っているか（表だけ直して実装が別判断をしていたら意味が無い）
    let text = read_code(&remote_rs);
    assert!(
        text.contains("remote_launch::role_for("),
        "{} が `remote_launch::role_for` を引いていない。\
         経路表が飾りになると role の宣言と実際の認可がずれる",
        remote_rs.display()
    );
}

#[test]
fn 起動経路の監査イベントが実装に在る() {
    let sources = [
        repo_root().join("crates/tako-control/src/remote.rs"),
        repo_root().join("crates/tako-control/src/remote_ssh.rs"),
    ];
    let texts: Vec<(PathBuf, String)> = sources.iter().map(|p| (p.clone(), read(p))).collect();
    for route in remote_launch::mutating_routes() {
        let event = route
            .audit_event
            .expect("mutating_routes は監査イベントを持つ");
        let needle = format!("\"{event}\"");
        let found = texts.iter().any(|(_, t)| t.contains(&needle));
        assert!(
            found,
            "監査イベント `{event}`（{} {}）を記録する実装が見つからない。\
             スマホから立てたことが `<state_dir>/audit.log` に残らなくなる",
            route.method, route.sample_path
        );
    }
}

#[test]
fn pwaの3択と経路表の種別が一致する() {
    let file = pwa_src().join("components/launch-sheet.jsx");
    let text = read(&file);
    for kind in [LaunchKind::Master, LaunchKind::Terminal, LaunchKind::Ssh] {
        let needle = format!("id: '{}'", kind.as_str());
        assert!(
            text.contains(&needle),
            "{} の LAUNCH_KINDS に `{needle}` が無い。\
             PWA の 3 択と Rust の LaunchKind は 1:1（docs もこの表を引く）",
            file.display()
        );
    }
    // 逆向き: PWA 側に表の知らない種別が生えていないか
    let declared = text.matches("id: '").count();
    assert_eq!(
        declared,
        3,
        "{} の LAUNCH_KINDS が 3 種から増減している。\
         増やすなら `remote_launch::LaunchKind` と経路表にも足すこと",
        file.display()
    );
}

#[test]
fn 起動画面はroleの門を必ず通る() {
    let file = pwa_src().join("components/launch-sheet.jsx");
    let text = read(&file);
    assert!(
        text.contains("canLaunch(me)"),
        "{} が `canLaunch(me)` を見ていない。\
         observe 端末に 3 択を見せると、押してから 403 で断られる画面になる",
        file.display()
    );
    // 門の内側にしかメニューが無いこと（`!allowed ?` の分岐が KindMenu より前に在る）
    let gate = text
        .find("!allowed ?")
        .unwrap_or_else(|| panic!("{} に role の分岐が無い", file.display()));
    let menu = text
        .find("<KindMenu")
        .unwrap_or_else(|| panic!("{} に KindMenu が無い", file.display()));
    assert!(
        gate < menu,
        "{} の KindMenu が role の分岐より前に出ている（observe にも 3 択が見える）",
        file.display()
    );
}
