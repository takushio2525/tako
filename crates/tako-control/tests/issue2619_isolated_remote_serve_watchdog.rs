//! 隔離中（`TAKO_ISOLATED`）の remote start が本物の tailscale の serve を張らないことの番犬（#2619）
//!
//! # なぜ要るか
//!
//! tailscale の serve の設定はこの機械のノード単位の状態で、隔離の砂場の外にある。砂場の CLI から
//! `tako remote start` を打つと、daemon は既定の tailscale へ serve を張っていた（11/01 の観察と段 1 の
//! 再現: 既定のノード = GUI 版の Tailscale に `serve_set`）。砂場の向くノードが本番の公開と同じなら、
//! 起動時の張り替えの判定が本番の待ち受けを「前回のポートの残骸」と読んで張り替える = 本番の公開を奪う。
//! 門は `remote_isolated_serve::check`（判定は純関数 `allows` の 1 実装）。
//!
//! # 何を縛るか
//!
//! 1. 起動の 1 実装 `spawn_daemon` は、頼んだ記録の後・自分の印（`StartingGuard::begin`）と子の起動
//!    （`Command::new`）より前に門を通り、断りは型（`RemoteRefusal::IsolatedServe`）の表示で返す
//! 2. serve を張る関数 `establish_tailscale_serve` は、tailscale に話しかける前に門を通る
//!    （daemon の中の最後の守り = `tako remote serve` を直に起こした場合）
//! 3. UDS の `remote_setup::configure_serve` も serve を張る前に門を通る
//! 4. **serve を書く呼び出し**（`serve_start_target(_on)` / `serve_stop(_on)` / 生の `&["serve", …]`）は
//!    既知の場所（上の 3 つ・稼働中の張り直し `assert_target`・tailscale.rs の包み）にしか無い。
//!    新しい書き手は file:line で名指す（門を通すか、ここへ理由つきで足すかを決める）
//! 5. 実際の `check` が env（隔離・検証モード・偽 tailscale・opt-in・A/B）で答えを変える（実経路。
//!    **tailscale は起こさない** = 判定だけを縛る）
//!
//! 後始末の `serve_stop_if_ours(_on)`（自分の待ち受けと完全一致のときだけ外す）は書き手に数えない。
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば全部が無意味に緑になるので [`走査が空振りしていない`] で窓を固定し、
//! [`修正前の形へ戻すと名指しする`] で**修正前を再現した注入**が file:line で名指しされることを確かめる

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const REMOTE: &str = "crates/tako-control/src/remote.rs";
const SETUP: &str = "crates/tako-control/src/remote_setup.rs";
const TAILSCALE: &str = "crates/tako-control/src/tailscale.rs";
/// 門の呼び出し（この形で書く）
const GATE: &str = "crate::remote_isolated_serve::check()";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    let raw =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::production(&raw, rel)
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（「宣言行から N 行」では隣の関数を巻き込む）
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 窓の中で `needle` を含む最初の**コードの**行の 1-based 行番号（無ければ None）
fn code_line(at: usize, window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|i| at + i)
}

/// 門が `before` のどれよりも前にあるか（`before` の行は窓の中で最初に現れたもの）
fn gate_first(
    file: &str,
    src: &str,
    func: &str,
    before: &[&str],
    why: &str,
    out: &mut Vec<String>,
) -> Option<usize> {
    let Some((at, w)) = fn_window(src, func) else {
        out.push(format!("{file}:0 — `{func}` が無い（走査の空振り）"));
        return None;
    };
    let Some(gate) = code_line(at, &w, GATE) else {
        out.push(format!(
            "{file}:{at} — `{func}` が隔離の門（`{GATE}`）を通らずに serve へ向かう。{why}（#2619）"
        ));
        return None;
    };
    for b in before {
        match code_line(at, &w, b) {
            Some(line) if line < gate => out.push(format!(
                "{file}:{line} — `{func}` が隔離の門より前に `{b}` へ進む。{why}（#2619）"
            )),
            Some(_) => {}
            None => out.push(format!(
                "{file}:{at} — `{func}` に `{b}` が見つからない（走査の空振り）"
            )),
        }
    }
    Some(gate)
}

/// 1〜3 の違反
fn gate_violations(remote: &str, setup: &str) -> Vec<String> {
    let mut out = Vec::new();

    // 1. spawn_daemon: 頼んだ記録の後・自分の印と子の起動の前
    if let Some(gate) = gate_first(
        REMOTE,
        remote,
        "pub fn spawn_daemon()",
        &[
            "StartingGuard::begin(",
            "Command::new(",
            "start_blocks_request()",
        ],
        "隔離の砂場から機械全体の serve を張る daemon を起こす",
        &mut out,
    ) {
        let (at, w) = fn_window(remote, "pub fn spawn_daemon()").expect("上で見つけた");
        match code_line(at, &w, "audit_daemon_request(\"daemon_start_requested\"") {
            Some(l) if l < gate => {}
            _ => out.push(format!(
                "{REMOTE}:{gate} — 隔離の門が頼んだ記録（`daemon_start_requested`）より前にある。\
                 断った起動が audit.log に残らない（#2533 / #2619）"
            )),
        }
        match code_line(at, &w, "audit_daemon_request(\"isolated_serve_refused\"") {
            Some(l) if l > gate => {}
            _ => out.push(format!(
                "{REMOTE}:{gate} — 隔離の門で断ったことを audit.log（`isolated_serve_refused`）へ残していない（#2619）"
            )),
        }
        match code_line(at, &w, "return Err(refusal.to_string())") {
            Some(l) if l > gate => {}
            _ => out.push(format!(
                "{REMOTE}:{gate} — 隔離の門の断りが型（`RemoteRefusal::IsolatedServe`）の表示で返っていない。\
                 英語表示に日本語が混ざる（#2365 / #2619）"
            )),
        }
    }

    // 2. establish_tailscale_serve: tailscale に話しかける前
    gate_first(
        REMOTE,
        remote,
        "fn establish_tailscale_serve(",
        &[
            "crate::tailscale::setup_status()",
            "crate::tailscale::serve_state_on(",
            "crate::tailscale::serve_start_target_on(",
        ],
        "daemon を直に起こした隔離の砂場が既定の tailscale へ serve を張る",
        &mut out,
    );

    // 3. configure_serve（UDS の setup）
    gate_first(
        SETUP,
        setup,
        "pub fn configure_serve(",
        &["tailscale::serve_state(", "tailscale::serve_start_target("],
        "隔離の砂場の setup が既定の tailscale へ serve を張る",
        &mut out,
    );
    out
}

/// serve を書く呼び出しが書かれてよい関数（ファイル, 関数名）。
/// 門を通る 3 つ + 起動時の門を通った daemon の中でだけ回る張り直し + tailscale.rs の包み
const KNOWN_WRITERS: &[(&str, &str)] = &[
    (REMOTE, "establish_tailscale_serve"),
    (REMOTE, "assert_target"),
    (SETUP, "configure_serve"),
    (TAILSCALE, "serve_start_target"),
    (TAILSCALE, "serve_start_target_on"),
    (TAILSCALE, "serve_stop"),
    (TAILSCALE, "serve_stop_on"),
    (TAILSCALE, "serve_stop_if_ours_on"),
];

/// 行が serve を書く呼び出しか（後始末の `serve_stop_if_ours` は数えない）
fn is_writer_call(line: &str) -> bool {
    let code = line.split("//").next().unwrap_or("");
    let calls = [
        "serve_start_target(",
        "serve_start_target_on(",
        "serve_stop(",
        "serve_stop_on(",
    ];
    let named = calls.iter().any(|c| {
        code.match_indices(c).any(|(i, _)| {
            // 宣言（`fn serve_stop(`）は呼び出しではない
            !code[..i].trim_end().ends_with("fn")
                && code[..i]
                    .chars()
                    .last()
                    .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '_'))
        })
    });
    let raw = code.contains("&[\"serve\",") && !code.contains("&[\"serve\", \"status\"");
    named || raw
}

/// 行を囲む関数の名前（字下げが浅い直近の `fn 名前(`）
fn enclosing_fn(lines: &[&str], idx: usize) -> Option<String> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let here = indent(lines[idx]);
    lines[..idx].iter().rev().find_map(|l| {
        if indent(l) >= here {
            return None;
        }
        let t = l.trim_start();
        let pos = t.find("fn ")?;
        let head = &t[..pos];
        if !head.is_empty()
            && !head
                .split_whitespace()
                .all(|w| matches!(w, "pub" | "pub(crate)" | "async" | "const" | "unsafe"))
        {
            return None;
        }
        let rest = &t[pos + 3..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    })
}

/// 4. 既知の場所の外の書き手
fn writer_violations(files: &[(&str, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for (file, src) in files {
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") || !is_writer_call(line) {
                continue;
            }
            let func = enclosing_fn(&lines, i).unwrap_or_default();
            if !KNOWN_WRITERS.iter().any(|(f, n)| f == file && *n == func) {
                out.push(format!(
                    "{file}:{} — `{func}` が tailscale の serve を書く。隔離の砂場から機械全体の serve を\
                     書き換えうるので、隔離の門（`{GATE}`）を通すか、通さない理由を番犬の既知の書き手へ足す（#2619）",
                    i + 1
                ));
            }
        }
    }
    out
}

/// `crates/*/src` の本番コード（rs）を全部読む（テスト専用のファイルは外し、テスト領域は空白へ潰す。
/// 小さいファイルもあるので下限の確かめ（`production`）は通さない = 行番号は原文のまま）
fn production_sources(root: &Path) -> Vec<(String, String)> {
    let files = production_range::product_sources();
    let test_only = production_range::test_only_files(&files);
    files
        .into_iter()
        .filter(|rel| !test_only.contains(rel))
        .map(|rel| {
            let raw = std::fs::read_to_string(root.join(&rel))
                .unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
            let text = production_range::scan(&raw).text;
            (rel, text)
        })
        .collect()
}

fn all_violations(root: &Path) -> Vec<String> {
    let remote = read(root, REMOTE);
    let setup = read(root, SETUP);
    let mut out = gate_violations(&remote, &setup);
    let files = production_sources(root);
    let borrowed: Vec<(&str, String)> =
        files.iter().map(|(f, s)| (f.as_str(), s.clone())).collect();
    out.extend(writer_violations(&borrowed));
    out
}

#[test]
fn 隔離中のremote_startはserveを張る前に門を通る() {
    let found = all_violations(&workspace_root());
    assert!(found.is_empty(), "{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let remote = read(&root, REMOTE);
    let setup = read(&root, SETUP);
    for (src, file, needle) in [
        (&remote, REMOTE, "pub fn spawn_daemon()"),
        (&remote, REMOTE, "fn establish_tailscale_serve("),
        (&setup, SETUP, "pub fn configure_serve("),
    ] {
        let (_, w) = fn_window(src, needle).unwrap_or_else(|| panic!("{file}: `{needle}` が無い"));
        assert!(
            w.lines().count() >= 5,
            "{file}: `{needle}` の窓が短すぎる（走査の空振り）"
        );
    }
    // 書き手の走査は既知の書き手を全部見つけている（見つけられないなら判定が空振り）
    let files = production_sources(&root);
    assert!(
        files.len() >= 100,
        "本番コードの走査が少なすぎる: {}",
        files.len()
    );
    for (file, func) in KNOWN_WRITERS {
        let src = &files
            .iter()
            .find(|(f, _)| f == file)
            .unwrap_or_else(|| panic!("{file} が走査に無い"))
            .1;
        let lines: Vec<&str> = src.lines().collect();
        let hit = lines.iter().enumerate().any(|(i, l)| {
            !l.trim_start().starts_with("//")
                && is_writer_call(l)
                && enclosing_fn(&lines, i).as_deref() == Some(*func)
        });
        assert!(
            hit,
            "{file}: 既知の書き手 `{func}` に serve を書く呼び出しが見つからない（走査の空振りか、書き手が消えた = 表を直す）"
        );
    }
}

/// 注入を当てる（対象が無ければ注入そのものが空振りしている）
fn inject(base: &str, from: &str, to: &str, what: &str) -> String {
    let out = base.replacen(from, to, 1);
    assert!(out != base, "注入「{what}」の対象が見つからない: {from:?}");
    out
}

#[test]
fn 修正前の形へ戻すと名指しする() {
    let root = workspace_root();
    let remote = read(&root, REMOTE);
    let setup = read(&root, SETUP);
    let named = |found: Vec<String>, file: &str, what: &str| {
        assert!(
            found.iter().any(
                |o| o.starts_with(&format!("{file}:")) && !o.starts_with(&format!("{file}:0 "))
            ),
            "注入「{what}」を {file} の行で名指しできていない: {found:?}"
        );
    };
    let gate_in_spawn =
        "    if let Err(refusal) = crate::remote_isolated_serve::check() {\n        audit_daemon_request(\"isolated_serve_refused\"";

    // 注入 1: spawn_daemon が門を通らない（#2619 前）
    named(
        gate_violations(
            &inject(
                &remote,
                gate_in_spawn,
                "    if let Err(refusal) = Ok::<(), crate::RemoteRefusal>(()) {\n        audit_daemon_request(\"isolated_serve_refused\"",
                "spawn_daemon が門を通らない",
            ),
            &setup,
        ),
        REMOTE,
        "spawn_daemon が門を通らない",
    );

    // 注入 2: 門を自分の印の後へずらす
    {
        let moved = inject(
            &remote,
            "    let _starting = StartingGuard::begin(renewing);\n",
            "",
            "印を外す",
        );
        let moved = inject(
            &moved,
            gate_in_spawn,
            &format!("    let _starting = StartingGuard::begin(None);\n{gate_in_spawn}"),
            "門を印の後へ",
        );
        named(gate_violations(&moved, &setup), REMOTE, "門を印の後へ");
    }

    // 注入 3: 断りを日本語の固定文で返す（#2365 前の形）
    named(
        gate_violations(
            &inject(
                &remote,
                "return Err(refusal.to_string())",
                "return Err(format!(\"隔離中なので serve を張りません: {refusal:?}\"))",
                "日本語の固定文",
            ),
            &setup,
        ),
        REMOTE,
        "日本語の固定文",
    );

    // 注入 4: establish_tailscale_serve が門を通らない（daemon を直に起こす経路）
    named(
        gate_violations(
            &inject(
                &remote,
                "    crate::remote_isolated_serve::check().map_err(|r| io::Error::other(r.to_string()))?;\n",
                "",
                "establish が門を通らない",
            ),
            &setup,
        ),
        REMOTE,
        "establish が門を通らない",
    );

    // 注入 5: configure_serve が門を通らない
    named(
        gate_violations(
            &remote,
            &inject(
                &setup,
                "    crate::remote_isolated_serve::check().map_err(|r| r.to_string())?;\n",
                "",
                "configure_serve が門を通らない",
            ),
        ),
        SETUP,
        "configure_serve が門を通らない",
    );

    // 注入 6: 門の外に新しい書き手（daemon_status の中で serve を張り直す）
    let injected = inject(
        &remote,
        "pub fn spawn_daemon() -> Result<Value, String> {\n",
        "pub fn spawn_daemon() -> Result<Value, String> {\n    let _ = crate::tailscale::serve_start_target_on(\"tailscale\", None, \"http://127.0.0.1:1\");\n",
        "門の外の書き手",
    );
    named(
        writer_violations(&[(REMOTE, injected)]),
        REMOTE,
        "門の外の書き手（名前つき）",
    );
    // 注入 7: 生の引数で書く
    let injected = inject(
        &setup,
        "pub fn configure_serve(",
        "fn sneaky(cli: &str) {\n    let _ = std::process::Command::new(cli).args(&[\"serve\", \"--bg\", \"--https=443\", \"http://127.0.0.1:1\"]);\n}\n\npub fn configure_serve(",
        "生の引数の書き手",
    );
    named(
        writer_violations(&[(SETUP, injected)]),
        SETUP,
        "生の引数の書き手",
    );
}

/// 実経路: 実際の `check` が env で答えを変える（**tailscale は起こさない**。判定だけを縛る）。
///
/// - 隔離でない（本番）は常に通す
/// - 隔離中: 理由が無ければ日英の断り（英語に日本語が混ざらない）。検証モード・偽 tailscale・opt-in・A/B で通す
/// - 素の名前・本物の候補を名指す `TAKO_TAILSCALE_BIN` は偽物と見なさない
#[test]
fn 実際の門はenvで答えを変える() {
    // env と表示言語を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(実際の門はenvで答えを変える) {
        return;
    }
    use tako_control::remote_isolated_serve::{check, LEGACY_ENV, OPT_IN_ENV};
    use tako_core::i18n::{set_lang, Lang};
    const NAMES: &[&str] = &[
        "TAKO_ISOLATED",
        "TAKO_REMOTE_TEST_MODE",
        "TAKO_TAILSCALE_BIN",
        OPT_IN_ENV,
        LEGACY_ENV,
    ];
    let with = |vars: &[(&str, &str)]| {
        for n in NAMES {
            std::env::remove_var(n);
        }
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        check()
    };
    let scratch = tako_core::test_residue::ScratchDir::new("issue2619-gate");
    let fake = scratch.path().join("fake-tailscale");
    let fake = fake.to_str().expect("utf-8");

    // 本番（隔離でない）は常に通す
    assert!(with(&[]).is_ok());
    assert!(with(&[("TAKO_ISOLATED", "0")]).is_ok());
    assert!(with(&[("TAKO_TAILSCALE_BIN", "tailscale")]).is_ok());

    // 隔離中で理由が無い → 断り（日英）
    for v in ["1", "true", "on"] {
        assert!(with(&[("TAKO_ISOLATED", v)]).is_err(), "TAKO_ISOLATED={v}");
    }
    set_lang(Lang::En);
    let en = with(&[("TAKO_ISOLATED", "1")])
        .expect_err("断る")
        .to_string();
    assert!(
        en.contains("TAKO_REMOTE_ISOLATED_SERVE=1")
            && !en.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)),
        "{en}"
    );
    set_lang(Lang::Ja);
    let ja = with(&[("TAKO_ISOLATED", "1")])
        .expect_err("断る")
        .to_string();
    assert!(ja.contains("隔離中"), "{ja}");

    // 隔離中でも通す
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_REMOTE_TEST_MODE", "1")]).is_ok());
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_TAILSCALE_BIN", fake)]).is_ok());
    assert!(with(&[("TAKO_ISOLATED", "1"), (OPT_IN_ENV, "1")]).is_ok());
    assert!(with(&[("TAKO_ISOLATED", "1"), (LEGACY_ENV, "1")]).is_ok());

    // 偽物と見なさない差し替え・偽の値
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_TAILSCALE_BIN", "tailscale")]).is_err());
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_TAILSCALE_BIN", "")]).is_err());
    assert!(with(&[("TAKO_ISOLATED", "1"), ("TAKO_REMOTE_TEST_MODE", "0")]).is_err());
    assert!(with(&[("TAKO_ISOLATED", "1"), (OPT_IN_ENV, "0")]).is_err());
    assert!(with(&[("TAKO_ISOLATED", "1"), (LEGACY_ENV, "0")]).is_err());
    #[cfg(target_os = "macos")]
    for real in ["/opt/homebrew/bin/tailscale", "/usr/local/bin/tailscale"] {
        if Path::new(real).exists() {
            assert!(
                with(&[("TAKO_ISOLATED", "1"), ("TAKO_TAILSCALE_BIN", real)]).is_err(),
                "本物の候補 {real} を偽物と読んだ"
            );
        }
    }
    for n in NAMES {
        std::env::remove_var(n);
    }
}
