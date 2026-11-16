//! 記録の無い利用者へ「リモートを自動で起動できません」を出さないことの番犬（#2929）
//!
//! # なぜ要るか
//!
//! Windows で tako を起動するたびに、リモートを一度も使っていない利用者でもサイドバーの
//! 通知欄へ赤字で「リモートを自動で起動できません: この OS ではリモート daemon を
//! 起動できません（#971）」が出ていた（2026-11-16 の Windows 実機の確認。まっさらな data dir の
//! 初回起動でも毎回）。原因は GUI 起動時の判断 `remote_autostart::autostart_decision` が
//! 意図（`desired` = `tako remote start` の記録）より先に OS（`supported`）を見ていたこと。
//! `Unsupported` は通知欄へ出す理由（`AutostartSkip::is_noteworthy`）なので、記録が無くても
//! 未対応 OS では必ず出る。
//!
//! # 何を縛るか
//!
//! 1. `autostart_decision` の中で、A/B の門の付かない `!input.supported` の行は
//!    `!input.desired` の行より**後ろ**にある（意図 → OS の順）
//! 2. A/B（`TAKO_2929_LEGACY`）の門の行が意図より前に在り、`probe` が env の読み口
//!    `legacy_2929_mode` を渡し、env を読むのはその 1 箇所だけ
//! 3. 振る舞い: 記録なし・未対応 OS → `not-desired` で通知しない / 記録あり・未対応 OS →
//!    `unsupported` で通知する（純関数を直に呼ぶ = 両 OS の CI で走る）
//! 4. 実経路: まっさらな state の `probe()`（材料集めも本物）は OS によらず `not-desired`。
//!    Windows の CI では `platform_supported()` が本当に false なので、Issue の症状そのものを
//!    引く形になる。A/B は #2929 前の答えへ戻る
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1・2 は無意味に緑になるので、[`走査が空振りしていない`] で窓が採れている
//! ことを固定し、[`逆戻りを名指しできる`] で**修正前を再現した注入**が file:line で
//! 名指しされることを確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const AUTOSTART: &str = "crates/tako-control/src/remote_autostart.rs";

/// A/B の env の読み手を数える範囲（判断・status の読み手・GUI の呼び手と通知・CLI）
const AB_SCOPE: &[&str] = &[
    AUTOSTART,
    "crates/tako-control/src/remote.rs",
    "crates/tako-app/src/remote_panel.rs",
    "crates/tako-app/src/sidebar.rs",
    "crates/tako-cli/src/main.rs",
];

/// A/B の env の名前（数えるのはコードの行だけ。注釈で名前を挙げるのは自由）
const AB_ENV: &str = "TAKO_2929_LEGACY";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &'static str) -> String {
    let raw =
        std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::production(&raw, rel)
}

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（#1485 の番犬と同じ取り方）
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

/// 窓の**コードの**行（注釈を落とす）と、窓の先頭からの相対位置（0 始まり）
fn code_lines(window: &str) -> Vec<(usize, &str)> {
    window
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .collect()
}

/// 判断と材料集め（`remote_autostart.rs`）の検査。違反は `ファイル:行 — 理由`
fn scan(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: &str| out.push(format!("{AUTOSTART}:{line} — {why}"));

    // 1 と 2 の判断側: 意図 → OS の順と、#2929 前の順へ戻す門
    match fn_window(src, "pub fn autostart_decision(") {
        None => push(0, "`autostart_decision` が無い（走査の空振り）"),
        Some((at, window)) => {
            let lines = code_lines(&window);
            let desired = lines
                .iter()
                .find(|(_, l)| l.contains("!input.desired"))
                .map(|(i, _)| *i);
            let gate = lines
                .iter()
                .find(|(_, l)| l.contains("!input.supported") && l.contains("input.legacy_2929"))
                .map(|(i, _)| *i);
            let plain: Vec<usize> = lines
                .iter()
                .filter(|(_, l)| l.contains("!input.supported") && !l.contains("input.legacy_2929"))
                .map(|(i, _)| *i)
                .collect();
            match desired {
                None => push(
                    at,
                    "判断が意図（`input.desired`）を見ていない（記録の有無で答えが変わらない）",
                ),
                Some(d) => {
                    for p in plain.iter().filter(|p| **p < d) {
                        push(
                            at + p,
                            "意図（`input.desired`）より先に OS（`input.supported`）を見ている。\
                             記録の無い利用者にも未対応 OS（Windows）で `unsupported` が返り、\
                             GUI を起動するたびに通知欄へ赤字が出る（#2929）",
                        );
                    }
                    if !plain.iter().any(|p| *p > d) {
                        push(
                            at + d,
                            "意図の後ろに OS の枝が無い。記録があるのに未対応 OS のとき \
                             `unsupported` で知らせられない（#2929 の後も知らせる側）",
                        );
                    }
                    match gate {
                        None => push(
                            at,
                            "A/B（`TAKO_2929_LEGACY=1`）の門が無い（`input.legacy_2929 && \
                             !input.supported` の行）。同一バイナリで #2929 前の順を再現できない",
                        ),
                        Some(g) if g > d => push(
                            at + g,
                            "A/B の門が意図の後ろにある。立てても #2929 前の順（OS → 意図）に戻らない",
                        ),
                        Some(_) => {}
                    }
                }
            }
        }
    }

    // 2 の材料側: probe が A/B を渡し、読み口が env の名前を引く
    match fn_window(src, "pub fn probe()") {
        None => push(0, "`probe` が無い（走査の空振り）"),
        Some((at, window)) => {
            let found = code_lines(&window)
                .into_iter()
                .any(|(_, l)| l.contains("legacy_2929: legacy_2929_mode()"));
            if !found {
                push(
                    at,
                    "`probe` が A/B（`legacy_2929_mode()`）を判断へ渡していない。\
                     `TAKO_2929_LEGACY=1` を立てても GUI の判断が変わらない",
                );
            }
        }
    }
    match fn_window(src, "pub fn legacy_2929_mode()") {
        None => push(0, "`legacy_2929_mode` が無い（A/B の読み口が消えている）"),
        Some((at, window)) => {
            if !code_lines(&window)
                .iter()
                .any(|(_, l)| l.contains("LEGACY_2929_ENV"))
            {
                push(at, "`legacy_2929_mode` が `LEGACY_2929_ENV` を読んでいない");
            }
        }
    }
    out
}

/// 判断の宣言行（`file:line`）。振る舞いの失敗文に添える
fn decision_at(root: &Path) -> String {
    let src = read(root, AUTOSTART);
    let (at, _) = fn_window(&src, "pub fn autostart_decision(").expect("判断の窓");
    format!("{AUTOSTART}:{at}")
}

/// 1・2: 今のソースは意図 → OS の順で、A/B の門と読み口が揃っている
#[test]
fn 判断は意図をosより先に見る() {
    let root = workspace_root();
    let found = scan(&read(&root, AUTOSTART));
    assert!(found.is_empty(), "#2929 の逆戻り:\n{}", found.join("\n"));
}

/// 2: A/B の env を読むのは `remote_autostart.rs` の 1 箇所だけ（腕が割れない）
#[test]
fn abのenvを読むのは一箇所だけ() {
    let root = workspace_root();
    let mut hits = Vec::new();
    for rel in AB_SCOPE {
        let src = read(&root, rel);
        for (i, line) in src.lines().enumerate() {
            if !line.trim_start().starts_with("//") && line.contains(AB_ENV) {
                hits.push(format!("{rel}:{}", i + 1));
            }
        }
    }
    assert_eq!(
        hits.len(),
        1,
        "`{AB_ENV}` をコードで書く場所が 1 箇所でない（判断と表示で腕が割れる）: {hits:?}"
    );
    assert!(
        hits[0].starts_with(&format!("{AUTOSTART}:")),
        "`{AB_ENV}` の定義が判断のモジュールの外にある: {hits:?}"
    );
}

/// 3: Windows の形（記録なし・未対応 OS）を純関数で直に引く。macOS の CI でも同じ答え
#[test]
fn 記録が無い未対応osの見送りは画面へ出さない() {
    use tako_control::remote_autostart::{autostart_decision, AutostartInput, AutostartSkip};
    let at = decision_at(&workspace_root());
    let fresh = AutostartInput {
        desired: false,
        daemon_running: false,
        daemon_stale: false,
        supported: false,
        legacy: false,
        legacy_2929: false,
    };
    let got = autostart_decision(&fresh);
    assert_eq!(
        got,
        Err(AutostartSkip::NotDesired),
        "{at}: 記録の無い未対応 OS で {got:?}（#2929 の症状 = 起動のたびに通知欄の赤字）"
    );
    assert!(!got.unwrap_err().is_noteworthy());

    let desired = AutostartInput {
        desired: true,
        ..fresh
    };
    let got = autostart_decision(&desired);
    assert_eq!(
        got,
        Err(AutostartSkip::Unsupported),
        "{at}: 記録があるのに未対応 OS で {got:?}（戻すつもりだったのに戻せないことは知らせる）"
    );
    assert!(got.unwrap_err().is_noteworthy());
}

/// 4: まっさらな state で本物の `probe()` を引く（材料集めも本物。GUI 起動時と同じ呼び方）。
///
/// Windows の CI では `platform_supported()` が本当に false なので、修正前は `unsupported`
/// （= Issue の症状）、修正後は `not-desired` になる。macOS では対応 OS 側の答えを確かめる。
/// tailscale は存在しないパスを渡す（`TAKO_TAILSCALE_BIN` は指定のパスだけを試す = 本物を叩かない）
#[test]
fn まっさらなstateのprobeはosによらず黙る() {
    // env を書き換えるので、この 1 本だけの子で走らせる（#2400）
    if tako_core::run_alone!(まっさらなstateのprobeはosによらず黙る) {
        return;
    }
    use tako_control::remote_autostart::{self as autostart, probe, AutostartSkip};
    let root = workspace_root();
    let src = read(&root, AUTOSTART);
    let probe_at = format!(
        "{AUTOSTART}:{}",
        fn_window(&src, "pub fn probe()").expect("probe の窓").0
    );

    let scratch = tako_core::test_residue::ScratchDir::new("tako-2929-probe");
    let state = scratch.path().join("state");
    let data = scratch.path().join("data");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::env::set_var("TAKO_REMOTE_STATE_DIR", &state);
    std::env::set_var("TAKO_DATA_DIR", &data);
    std::env::set_var(
        "TAKO_TAILSCALE_BIN",
        scratch.path().join("no-such-tailscale"),
    );
    for k in [autostart::LEGACY_ENV, autostart::LEGACY_2929_ENV] {
        std::env::remove_var(k);
    }

    let supported = autostart::platform_supported();
    let fresh = probe();
    std::env::set_var(autostart::LEGACY_2929_ENV, "1");
    let legacy = probe();
    std::env::remove_var(autostart::LEGACY_2929_ENV);
    tako_control::remote::set_desired();
    let desired = probe();

    assert_eq!(
        fresh,
        Err(AutostartSkip::NotDesired),
        "{probe_at}: まっさらな state（記録なし）で {fresh:?}（supported={supported}）。\
         未対応 OS ではこれが GUI 起動のたびの通知欄の赤字になる（#2929）"
    );
    let want_legacy = if supported {
        Err(AutostartSkip::NotDesired)
    } else {
        Err(AutostartSkip::Unsupported)
    };
    assert_eq!(
        legacy,
        want_legacy,
        "{probe_at}: A/B（{}=1）が #2929 前の答えへ戻っていない（supported={supported}）",
        autostart::LEGACY_2929_ENV
    );
    let want_desired = if supported {
        Ok(())
    } else {
        Err(AutostartSkip::Unsupported)
    };
    assert_eq!(
        desired, want_desired,
        "{probe_at}: 記録を作った後の答えが #2929 の前後で変わった（supported={supported}）"
    );
}

/// 走査の窓が採れている（空振りで緑にならない）
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let src = read(&root, AUTOSTART);
    for needle in [
        "pub fn autostart_decision(",
        "pub fn probe()",
        "pub fn legacy_2929_mode()",
    ] {
        let (at, window) = fn_window(&src, needle)
            .unwrap_or_else(|| panic!("{AUTOSTART} の {needle} の窓が採れない（走査が空振り）"));
        assert!(at > 0, "{needle} の行番号が採れていない");
        assert!(
            window.lines().count() > 2,
            "{needle} の窓が 2 行以下（字下げの規約が変わって窓が切れている）"
        );
    }
    let (_, decision) = fn_window(&src, "pub fn autostart_decision(").unwrap();
    for needle in ["!input.desired", "!input.supported", "input.legacy_2929"] {
        assert!(
            decision.contains(needle),
            "判断の窓に `{needle}` が無い（検査の手がかりが変わった = 番犬を直す）"
        );
    }
}

/// 修正前を再現した注入を file:line で名指しできる
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let base = read(&root, AUTOSTART);
    let line_of = |src: &str, needle: &str| {
        src.lines()
            .position(|l| l == needle)
            .unwrap_or_else(|| panic!("注入した行 `{needle}` がある"))
            + 1
    };

    // 注入 1: #2929 前の順（OS → 意図）。Issue の症状そのもの
    let desired_then_os = "    if !input.desired {\n        return Err(AutostartSkip::NotDesired);\n    }\n    if !input.supported {\n        return Err(AutostartSkip::Unsupported);\n    }\n";
    let os_then_desired = "    if !input.supported {\n        return Err(AutostartSkip::Unsupported);\n    }\n    if !input.desired {\n        return Err(AutostartSkip::NotDesired);\n    }\n";
    let s = base.replace(desired_then_os, os_then_desired);
    assert!(s != base, "注入 1 の対象が見つからない");
    let want = format!("{AUTOSTART}:{}", line_of(&s, "    if !input.supported {"));
    let found = scan(&s);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(&format!("{want} ")) && o.contains("#2929")),
        "OS を先に見る順を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 2: A/B の門を外す（同一バイナリで修正前を再現できない）
    let s = base.replace(
        "    if input.legacy_2929 && !input.supported {",
        "    if false {",
    );
    assert!(s != base, "注入 2 の対象が見つからない");
    let found = scan(&s);
    assert!(
        found
            .iter()
            .any(|o| o.contains("A/B") && o.contains("門が無い")),
        "A/B の門が無い形を名指しできていない: {found:#?}"
    );

    // 注入 3: A/B の門を意図の後ろへ置く（立てても順が戻らない）
    let gate = "    // A/B（`TAKO_2929_LEGACY=1`）: #2929 前の順 = 意図より先に OS を見る\n    if input.legacy_2929 && !input.supported {\n        return Err(AutostartSkip::Unsupported);\n    }\n";
    let desired_only =
        "    if !input.desired {\n        return Err(AutostartSkip::NotDesired);\n    }\n";
    assert!(
        base.contains(gate),
        "注入 3 の対象（A/B の門）が見つからない"
    );
    let s = base
        .replacen(gate, "", 1)
        .replacen(desired_only, &format!("{desired_only}{gate}"), 1);
    assert!(s != base, "注入 3 が効いていない");
    let want = format!(
        "{AUTOSTART}:{}",
        line_of(&s, "    if input.legacy_2929 && !input.supported {")
    );
    let found = scan(&s);
    assert!(
        found
            .iter()
            .any(|o| o.starts_with(&format!("{want} ")) && o.contains("意図の後ろ")),
        "意図の後ろの A/B の門を名指しできていない（期待 {want}）: {found:#?}"
    );

    // 注入 4: probe が A/B を渡さない（env を立てても GUI の判断が変わらない）
    let s = base.replace(
        "        legacy_2929: legacy_2929_mode(),",
        "        legacy_2929: false,",
    );
    assert!(s != base, "注入 4 の対象が見つからない");
    let want = format!(
        "{AUTOSTART}:{}",
        line_of(&s, "pub fn probe() -> Result<(), AutostartSkip> {")
    );
    let found = scan(&s);
    assert!(
        found.iter().any(|o| o.starts_with(&format!("{want} "))),
        "A/B を渡さない probe を名指しできていない（期待 {want}）: {found:#?}"
    );
}
