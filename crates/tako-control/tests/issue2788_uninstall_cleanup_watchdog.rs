//! アンインストールの片付けの配線の番犬（Issue #2788）
//!
//! 片付けの中身は `tests/issue2788_setup_undo.rs` が砂場で確かめる。ここは「どこから呼ばれ、
//! どこからは呼ばれないか」と「cask の zap が実際の data dir を消すか」を、ソースの字面で縛る。
//!
//! 1. Homebrew の cask（`distribution/homebrew-cask-draft.rb`）の `zap trash:` が tako-core の
//!    data dir（[`tako_core::paths::MACOS_DATA_DIR_REL`]）を指し、残りは .app の CFBundleIdentifier の
//!    置き場だけ。#2788 の前は `~/Library/Application Support/dev.takushio.tako`（存在しない置き場）だった
//! 2. cask に uninstall の段（`uninstall_preflight` / `uninstall_postflight` / `uninstall script:`）を
//!    置かない。Homebrew は `brew upgrade` でも旧版の uninstall の段を走らせるので、置くと更新のたびに
//!    連携が外れる。代わりに caveats が `tako setup undo` を案内する
//! 3. Windows のインストーラー（`installer/windows/tako.iss`）は、アンインストールの `usUninstall`
//!    （ファイルを消す前）でだけ `{app}\tako.exe setup undo` を起こす。インストール側の段
//!    （`CurStepChanged` / `InitializeSetup` / `DeinitializeSetup` / `[Run]`）からは起こさない
//!    = 上書きインストール・アプリ内更新（#2434 / #2443）では走らない
//! 4. Windows の CI がインストール → setup → 上書き → アンインストールの一巡を回している

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// 改行は LF にそろえて読む（Windows の checkout で CRLF になっても同じ判定）
fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
        .replace("\r\n", "\n")
}

/// Ruby のコメント（`#` 以降）を落とした行
fn ruby_code_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| match l.find('#') {
            // 文字列の中の `#{...}` は残す（cask の url / binary が使う）
            Some(i) if !l[i..].starts_with("#{") => l[..i].to_string(),
            _ => l.to_string(),
        })
        .collect()
}

/// cask の `zap trash: [ ... ]` の置き場
fn zap_entries(cask: &str) -> Vec<String> {
    let lines = ruby_code_lines(cask);
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("zap trash: ["))
        .expect("cask に zap trash: [ が無い");
    lines[start + 1..]
        .iter()
        .take_while(|l| l.trim() != "]")
        .filter_map(|l| {
            let t = l.trim().trim_end_matches(',');
            t.strip_prefix('"')?.strip_suffix('"').map(str::to_string)
        })
        .collect()
}

fn bundle_id() -> String {
    let script = read("scripts/build-app.sh");
    let at = script
        .find("<key>CFBundleIdentifier</key>")
        .expect("build-app.sh に CFBundleIdentifier が無い");
    let rest = &script[at..];
    let open = rest
        .find("<string>")
        .expect("CFBundleIdentifier の値が無い")
        + "<string>".len();
    let close = rest[open..]
        .find("</string>")
        .expect("CFBundleIdentifier の値が閉じない");
    rest[open..open + close].to_string()
}

#[test]
fn caskのzapはtako_coreのdata_dirとバンドルidの置き場だけを消す() {
    let cask = read("distribution/homebrew-cask-draft.rb");
    let entries = zap_entries(&cask);
    // cask の書き方（`~` + `/` 区切り）で組む（どの OS の CI でも同じ字面）
    let data_dir = ["~", tako_core::paths::MACOS_DATA_DIR_REL].join("/");
    assert!(
        entries.contains(&data_dir),
        "cask の zap に data dir {data_dir} が無い（tako-core::paths と食い違う）: {entries:?}"
    );
    let id = bundle_id();
    for entry in &entries {
        if *entry == data_dir {
            continue;
        }
        assert!(
            !entry.contains("Application Support"),
            "zap の Application Support は data dir だけ（{entry} は tako の置き場ではない。#2788 の前の形）"
        );
        let name = entry.rsplit('/').next().unwrap_or_default();
        assert!(
            name == id || name.starts_with(&format!("{id}.")),
            "zap の {entry} が .app のバンドル ID（{id}）の置き場でも data dir でもない"
        );
    }
    // macOS 上では data dir の既定の解決そのものと同じ形
    #[cfg(target_os = "macos")]
    assert_eq!(
        tako_core::paths::default_data_dir_under_home(Path::new("~")),
        PathBuf::from(&data_dir)
    );
}

#[test]
fn caskはuninstallの段を持たずcaveatsでtako_setup_undoを案内する() {
    let cask = read("distribution/homebrew-cask-draft.rb");
    let code = ruby_code_lines(&cask).join("\n");
    for stanza in [
        "uninstall_preflight",
        "uninstall_postflight",
        "uninstall script:",
        "uninstall early_script:",
        "uninstall delete:",
    ] {
        assert!(
            !code.contains(stanza),
            "cask に {stanza} がある。brew upgrade でも旧版の uninstall の段が走り、更新のたびに連携が外れる（#2788）"
        );
    }
    assert!(
        code.contains("tako setup undo"),
        "cask の caveats が tako setup undo を案内していない"
    );
}

/// Inno の `[Code]` の 1 手続きの本文（`procedure <name>` から次の `end;` の行まで）
fn pascal_procedure<'a>(iss: &'a str, name: &str) -> &'a str {
    let head = format!("procedure {name}(");
    let start = iss
        .find(&head)
        .or_else(|| iss.find(&format!("function {name}(")))
        .unwrap_or_else(|| panic!("tako.iss に {name} が無い"));
    let rest = &iss[start..];
    let end = rest
        .find("\nend;")
        .map(|i| i + "\nend;".len())
        .unwrap_or(rest.len());
    &rest[..end]
}

/// `[Code]` 節（Pascal Script）の本文
fn code_section(iss: &str) -> &str {
    iss.split("\n[Code]\n")
        .nth(1)
        .expect("tako.iss に [Code] 節が無い")
}

/// Pascal Script の `{ ... }` と `//` のコメントを落とす（`'...'` の文字列の中の波括弧 =
/// `ExpandConstant('{app}')` は残す）
fn without_pascal_comments(code: &str) -> String {
    let mut out = String::new();
    let mut chars = code.chars().peekable();
    let (mut in_brace, mut in_str) = (false, false);
    while let Some(c) = chars.next() {
        if in_brace {
            match c {
                '}' => in_brace = false,
                '\n' => out.push('\n'),
                _ => {}
            }
            continue;
        }
        if in_str {
            out.push(c);
            in_str = c != '\'';
            continue;
        }
        match c {
            '{' => in_brace = true,
            '\'' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                while chars.peek().is_some_and(|n| *n != '\n') {
                    chars.next();
                }
            }
            c => out.push(c),
        }
    }
    out
}

#[test]
fn windowsのアンインストーラーはファイルを消す前にだけtako_setup_undoを起こす() {
    let iss = read("installer/windows/tako.iss");
    let iss = code_section(&iss);
    let undo = pascal_procedure(iss, "UndoTakoIntegrations");
    assert!(
        undo.contains("{#CliExeName}") && undo.contains("'setup undo'"),
        "UndoTakoIntegrations が {{app}}\\tako.exe setup undo を起こしていない:\n{undo}"
    );
    let uninstall = pascal_procedure(iss, "CurUninstallStepChanged");
    let code = without_pascal_comments(uninstall);
    let at = code.find("usUninstall then").unwrap_or_else(|| {
        panic!("CurUninstallStepChanged に usUninstall の段が無い:\n{uninstall}")
    });
    assert!(
        code[at..]
            .lines()
            .nth(1)
            .is_some_and(|l| l.contains("UndoTakoIntegrations()")),
        "usUninstall（ファイルを消す前）で UndoTakoIntegrations を呼んでいない:\n{uninstall}"
    );
    // インストール側の段・起動の段からは起こさない（上書きインストール・アプリ内更新で走らない）
    for name in ["CurStepChanged", "InitializeSetup", "DeinitializeSetup"] {
        let body = without_pascal_comments(pascal_procedure(iss, name));
        assert!(
            !body.contains("UndoTakoIntegrations") && !body.contains("setup undo"),
            "{name}（インストール・更新の経路）から片付けを起こしている。アプリ内更新のたびに連携が外れる"
        );
    }
    let whole = read("installer/windows/tako.iss");
    let run_section = whole
        .split("\n[Run]")
        .nth(1)
        .and_then(|r| r.split("\n[").next())
        .unwrap_or_default();
    assert!(
        !run_section.contains("setup undo"),
        "[Run]（インストールの後）から片付けを起こしている"
    );
    // 呼び出しは usUninstall の 1 か所だけ
    let calls = without_pascal_comments(iss)
        .matches("UndoTakoIntegrations()")
        .count();
    assert_eq!(
        calls, 2,
        "UndoTakoIntegrations の定義 1 + 呼び出し 1 のはず（{calls}）"
    );
}

#[test]
fn windowsのciがインストールからアンインストールまでの片付けの一巡を回す() {
    let ci = read(".github/workflows/ci.yml");
    assert!(
        ci.contains("scripts/windows/test-uninstall-cleanup-2788.ps1"),
        "ci.yml が #2788 の一巡（scripts/windows/test-uninstall-cleanup-2788.ps1）を回していない"
    );
    let script = read("scripts/windows/test-uninstall-cleanup-2788.ps1");
    for must in [
        // 上書きインストール（アプリ内更新と同じ引数）の後も連携が残る
        "/TAKOWAITPID=",
        // アンインストーラーで連携が外れ、data dir は残る
        "UninstallString",
        "WebView2",
    ] {
        assert!(script.contains(must), "一巡の検査に {must} の段が無い");
    }
}
