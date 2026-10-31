//! 実経路: エージェント CLI の PATH 通し（`tako_setup_bootstrap` の path / undo-path / status）が、
//! 種別の分からない `$SHELL`（nu・tcsh 等）を zsh と見なさない（Issue #2550）。
//!
//! 修正前（実測。砂場の `tako setup bootstrap path`）: `SHELL` が偽の tcsh / nu・`/usr/bin/false`・存在しない
//! `…/tcsh` のどれでも `"shell": "zsh"`・`change: installed`・`wrote: true` で成功が返り、そのシェルが読まない
//! `~/.zprofile` へ書いていた = `setup_bootstrap::shell_target` が判定できない `$SHELL` を OS の既定（zsh）へ
//! 落としていた。「使っているシェル（…）の設定ファイルが分かりません」の断りは unix では届かなかった。
//!
//! 見るもの（GUI の代わりのホストへ CLI / MCP と同じ dispatch を日英で通す。CLI の `tako setup bootstrap` が
//! 呼ぶ日本語の口 `ensure_path_for` / `undo_path_for` も同じ答えか）:
//!
//! - 未知のシェル: path は足すディレクトリ（`$HOME/…`・重複なし・構文は包まない = #2557）つきの断り、
//!   undo-path は断り、どちらもホームのファイルを 1 つも書き換えない（tako CLI の symlink だけは
//!   PATH より先に張る = #1502 の順のまま）。status の `shell` / `profile` は null
//! - `SHELL` が空・未設定: 従来どおり `~/.zprofile`。zsh / bash / sh / fish（存在しないパスの zsh を含む）:
//!   従来どおりのファイル
//!
//! **本物の環境を 1 つも書き換えない**: `HOME` / `TAKO_DATA_DIR` / `TAKO_ORCHESTRATOR_DIR` /
//! `CLAUDE_CONFIG_DIR` / `TMPDIR` は一時の置き場の下、`PATH` は空の dir、`SHELL` は何も答えずに終わる
//! 偽のログインシェル（名前で種別が決まる）か存在しないパス。インストーラは取得も実行もしない
//! （path / undo-path / status だけを叩く）。表示言語と env はプロセス全体の状態なので、1 本のテスト関数を
//! `run_alone!` の子で走らせる（#2400）。unix だけ: Windows の PATH 通しはユーザー環境変数の経路で
//! `$SHELL` を見ない
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_control::protocol::Request;
use tako_control::setup_bootstrap;
use tako_control::{
    DispatchError, PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost,
    WebViewHost, WorkspaceHost,
};
use tako_core::i18n::{self, Lang};
use tako_core::platform::agent_install::AgentKind;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木だけを持つ（どの要求もペインを作らない）
struct Host {
    workspace: Workspace,
}

impl WorkspaceHost for Host {
    fn workspace(&self) -> &Workspace {
        &self.workspace
    }
    fn workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspace
    }
}

impl SessionHost for Host {
    fn session(&self, _pane: PaneId) -> Option<&TerminalSession> {
        None
    }
    fn attach_session(&mut self, _pane: PaneId, _options: SpawnOptions) {}
    fn detach_session(
        &mut self,
        _pane: PaneId,
        _origin: tako_core::pane_log::CloseOrigin,
        _caller: Option<&str>,
    ) {
    }
}

impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl TmuxHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}
impl SystemHost for Host {}

/// 偽のログインシェル: PATH の読み取りにも `command -v` にも何も答えない
const FAKE_SHELL: &str = "#!/bin/sh\n# #2550 の偽のログインシェル: 何も答えない\nexit 1\n";

/// tako が profile へ置くブロックの 1 行目（`shell_profile` の目印）
const BLOCK_BEGIN: &str = "# >>> tako PATH >>>";

/// 未知のシェルへの path の断りが示す「足すディレクトリ」の誤り（#2557）。
///
/// 案内は字下げ 2 の行に 1 個ずつ。`$HOME/…` の式（書く行と同じ = `shell_profile::dirs_for_unknown_shell`）で
/// 重複なし・そのシェルの構文を推測して包まない。修正前は `  export PATH="~/.local/bin:$PATH"` が
/// ランチャーの数だけ並んだ（sh 系は二重引用符の中の `~` を展開しない・tcsh / nu では sh の構文が通らない）。
/// 組み立ての場所は [`hint_sources`] が失敗の文へ file:line で載せる
fn hint_problems(text: &str) -> Vec<String> {
    let dirs: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("  "))
        .collect();
    let mut problems = Vec::new();
    if dirs.is_empty() {
        problems.push("足すディレクトリが 1 つも載っていない".to_string());
    }
    for (i, dir) in dirs.iter().enumerate() {
        if dirs[..i].contains(dir) {
            problems.push(format!("同じディレクトリが重ねて並ぶ: {dir}"));
        }
        if dir.contains('~') {
            problems.push(format!("`~` 短縮の表示（書く行は `$HOME/…`）: {dir}"));
        }
        for syntax in ["export", "setenv", "$env", "PATH", "\"", "'", "=", ";"] {
            if dir.contains(syntax) {
                problems.push(format!(
                    "シェルの構文（{syntax}）を推測して包んでいる: {dir}"
                ));
            }
        }
        if !dir.starts_with("$HOME/") && !dir.starts_with('/') {
            problems.push(format!("`$HOME/…` の式でも絶対パスでもない: {dir}"));
        }
    }
    if !dirs.contains(&"$HOME/.local/bin") {
        problems.push("ランチャーの置き場所（$HOME/.local/bin）が載っていない".to_string());
    }
    problems
}

/// 未知のシェルへの案内を組む 3 か所（失敗の文に file:line で載せる = どこを戻したかを名指す。#2557）
fn hint_sources() -> String {
    let at = locate;
    format!(
        "並び = {} → {}、文 = {}",
        at(
            "crates/tako-control/src/setup_bootstrap.rs",
            include_str!("../src/setup_bootstrap.rs"),
            "fn unknown_shell_refusal(",
        ),
        at(
            "crates/tako-core/src/shell_profile.rs",
            include_str!("../../tako-core/src/shell_profile.rs"),
            "pub fn dirs_for_unknown_shell(",
        ),
        at(
            "crates/tako-control/src/setup_refusal.rs",
            include_str!("../src/setup_refusal.rs"),
            "fn dir_lines(",
        ),
    )
}

/// `needle` が最初に出る行を `path:line` で返す（失敗の文で直す場所を名指す）
fn locate(path: &str, src: &str, needle: &str) -> String {
    match src.lines().position(|line| line.contains(needle)) {
        Some(i) => format!("{path}:{}", i + 1),
        None => format!("{path}（{needle} が見つからない）"),
    }
}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

fn bootstrap(action: &str) -> Value {
    json!({ "method": "setup_bootstrap", "params": { "action": action, "agent": "claude" } })
}

fn dispatch(request: &Value) -> Result<Value, String> {
    let mut host = Host {
        workspace: Workspace::new("1", Pane::new(PaneOrigin::User)),
    };
    let request: Request = serde_json::from_value(request.clone()).expect("要求の形");
    let result: Result<Value, DispatchError> =
        tako_control::dispatch(&mut host, request, PaneOrigin::Cli);
    result.map_err(|e| e.to_string())
}

/// `$SHELL` を差し替える（`None` は外す）
fn set_shell(shell: Option<&str>) {
    match shell {
        Some(value) => std::env::set_var("SHELL", value),
        None => std::env::remove_var("SHELL"),
    }
}

/// 新しい空のホームへ切り替える
fn fresh_home(root: &Path, label: &str) -> PathBuf {
    let home = root.join("homes").join(label);
    std::fs::create_dir_all(&home).expect("一時のホーム");
    std::env::set_var("HOME", &home);
    home
}

/// ホームの下のファイル（symlink を含む）を相対パスで並べる。tako CLI の symlink は除く
/// （PATH より先に張る = #1502 の順。書き先のシェルの判定とは関係しない）
fn written_files(home: &Path) -> Vec<String> {
    fn walk(dir: &Path, home: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).expect("メタデータ");
            if meta.is_dir() {
                walk(&path, home, out);
            } else {
                let rel = path
                    .strip_prefix(home)
                    .expect("ホームの下")
                    .display()
                    .to_string();
                if rel != ".local/bin/tako" {
                    out.push(rel);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(home, home, &mut out);
    out.sort();
    out
}

#[test]
fn 種別の分からないシェルはzshと見なさずprofileへ書かずに断る() {
    if tako_core::run_alone!(種別の分からないシェルはzshと見なさずprofileへ書かずに断る)
    {
        return;
    }
    let root = std::env::temp_dir().join(format!("tako-2550-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("一時の置き場");
    let root = root.canonicalize().expect("正規化");
    for dir in [
        "homes",
        "data",
        "orch",
        "ccd",
        "tmp",
        "empty-path",
        "fakesh",
    ] {
        std::fs::create_dir_all(root.join(dir)).expect("一時の dir");
    }
    // 本番の設定・データを読み書きしない（どの置き場も一時の場所の下）
    std::env::set_var("TAKO_DATA_DIR", root.join("data"));
    std::env::set_var("TAKO_ORCHESTRATOR_DIR", root.join("orch"));
    std::env::set_var("CLAUDE_CONFIG_DIR", root.join("ccd"));
    std::env::set_var("TMPDIR", root.join("tmp"));
    std::env::remove_var("CODEX_HOME");
    std::env::remove_var("TAKO_SELF_TEST");
    std::env::set_var("PATH", root.join("empty-path"));
    let fake = |name: &str| -> String {
        let at = root.join("fakesh").join(name);
        std::fs::write(&at, FAKE_SHELL).expect("偽のシェル");
        std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o755)).expect("実行ビット");
        at.display().to_string()
    };
    // status は claude を引けたら認証の問い合わせで起こすので、偽のシェルの下で本物を引けないことを
    // 先に確かめる（引けたら何も叩かずに止まる。#2365 の実経路テストと同じ門）
    std::env::set_var("SHELL", fake("tcsh"));
    assert!(
        tako_core::platform::exe::find("claude").is_none(),
        "claude を引けてしまう（注入が効いていない）ので status が本物を起こしうる。先へ進まない"
    );
    let original = i18n::lang();
    let mut failures: Vec<String> = Vec::new();

    // ---- 種別の分からないシェル: 書かずに断る ----
    let unknown = [
        ("tcsh", fake("tcsh")),
        ("nu", fake("nu")),
        ("false", "/usr/bin/false".to_string()),
        ("missing-tcsh", "/nonexistent/tako-2550/tcsh".to_string()),
    ];
    let mut refused = 0;
    for (label, shell) in &unknown {
        let home = fresh_home(&root, label);
        set_shell(Some(shell));
        // 修正前の版が置いたブロック（読まれない `.zprofile`）が残っていても触らない
        let leftover = home.join(".zprofile");
        let leftover_body = format!("{BLOCK_BEGIN}\n# #2550 の修正前に置かれたブロック\n");
        if *label == "missing-tcsh" {
            std::fs::write(&leftover, &leftover_body).expect("残りのブロック");
        }
        let before = written_files(&home);

        let mut texts = Vec::new();
        for lang in [Lang::Ja, Lang::En] {
            i18n::set_lang(lang);
            for action in ["path", "undo-path"] {
                match dispatch(&bootstrap(action)) {
                    Ok(v) => failures.push(format!(
                        "[{lang:?}] {label} の {action}: 断られずに通った（zsh と見なした）: {v}"
                    )),
                    Err(text) => texts.push((lang, action, text)),
                }
            }
        }
        i18n::set_lang(Lang::Ja);
        let cli_path = setup_bootstrap::ensure_path_for(AgentKind::Claude);
        let cli_undo = setup_bootstrap::undo_path_for(AgentKind::Claude);
        for (lang, action, text) in &texts {
            println!("[{lang:?}] {label} の {action}: {text}");
            let want_head = match (lang, *action) {
                (Lang::Ja, "path") => {
                    format!("使っているシェル（{shell}）の設定ファイルが分かりません。")
                }
                (Lang::En, "path") => {
                    format!("cannot tell which settings file your shell ({shell}) uses.")
                }
                (Lang::Ja, _) => "使っているシェルの設定ファイルが分かりません".to_string(),
                (Lang::En, _) => "cannot tell which settings file your shell uses".to_string(),
            };
            if !text.starts_with(&want_head) {
                failures.push(format!(
                    "[{lang:?}] {label} の {action}: 既存の断りではない\n  want: {want_head}…\n  got:  {text}"
                ));
            }
            if *action == "path" {
                for problem in hint_problems(text) {
                    failures.push(format!(
                        "[{lang:?}] {label} の path: 足すディレクトリの案内が誤り（#2557。{}）: {problem}\n  got: {text}",
                        hint_sources()
                    ));
                }
            }
            if *lang == Lang::En {
                if let Some(ch) = has_japanese(text) {
                    failures.push(format!(
                        "[En] {label} の {action}: 英語表示に日本語（{ch}）が混ざる: {text}"
                    ));
                }
            }
            if *lang == Lang::Ja {
                let cli = if *action == "path" {
                    &cli_path
                } else {
                    &cli_undo
                };
                if cli.as_ref().err() != Some(text) {
                    failures.push(format!(
                        "{label} の {action}: CLI の口と dispatch の日本語が違う\n  dispatch: {text}\n  CLI:      {cli:?}"
                    ));
                }
            }
            refused += 1;
        }
        let after = written_files(&home);
        if after != before {
            failures.push(format!(
                "{label}: ホームのファイルが変わった（書いた）: {before:?} → {after:?}"
            ));
        }
        if *label == "missing-tcsh"
            && std::fs::read_to_string(&leftover).ok().as_deref() != Some(leftover_body.as_str())
        {
            failures.push(format!("{label}: 残っていた .zprofile を書き換えた"));
        }
        // status は書き先を名乗らない（zsh / `.zprofile` と言わない）
        match dispatch(&bootstrap("status")) {
            Ok(status) => {
                println!(
                    "{label} の status: shell={} profile={} profile_has_block={}",
                    status["shell"], status["profile"], status["profile_has_block"]
                );
                if !status["shell"].is_null() || !status["profile"].is_null() {
                    failures.push(format!(
                        "{label} の status: 書き先を名乗った: shell={} profile={}",
                        status["shell"], status["profile"]
                    ));
                }
            }
            Err(e) => failures.push(format!("{label} の status が断られた: {e}")),
        }
    }

    // ---- #2564: 案内どおり足し終えた未知のシェルは断らない ----
    // 断りが示した一覧（`$HOME/…`）をそのまま PATH へ足すと、path は書かずに `already_on_path` を返す。
    // 修正前は足し終えても同じ断り = `tako setup` が「足してください」を出し続けた（形を変えた堂々巡り）。
    // 偽のシェルは何も答えないので、見え方はプロセスの PATH（本物の tcsh も `-l -c` を受け付けずこちら）
    let mut closed = 0;
    for (label, shell) in &unknown[..2] {
        let home = fresh_home(&root, &format!("{label}-added"));
        set_shell(Some(shell));
        i18n::set_lang(Lang::Ja);
        let listed: Vec<PathBuf> = match dispatch(&bootstrap("path")) {
            Ok(v) => {
                failures.push(format!("{label}（足す前）: 断られずに通った: {v}"));
                continue;
            }
            Err(text) => text
                .lines()
                .filter_map(|line| line.strip_prefix("  "))
                .map(|dir| match dir.strip_prefix("$HOME/") {
                    Some(rel) => home.join(rel),
                    None => PathBuf::from(dir),
                })
                .collect(),
        };
        if listed.is_empty() {
            failures.push(format!("{label}: 断りに足すディレクトリが無い"));
            continue;
        }
        let path = std::env::join_paths(listed.iter().chain([&root.join("empty-path")]))
            .expect("PATH を組める");
        std::env::set_var("PATH", &path);
        let before = written_files(&home);
        for (via, result) in [
            ("dispatch", dispatch(&bootstrap("path"))),
            (
                "CLI の口",
                setup_bootstrap::ensure_path_for(AgentKind::Claude),
            ),
        ] {
            println!("{label}（足し終えた）の {via}: {result:?}");
            match result {
                Ok(v) if v["change"] == "already_on_path"
                    && v["wrote"] == false
                    && v["verified"] == true
                    && v["shell"].is_null()
                    && v["profile"].is_null() =>
                {
                    closed += 1;
                }
                other => failures.push(format!(
                    "{label}（足し終えた）の {via}: 断らずに already_on_path を返さない（#2564。判定は {}）: {other:?}",
                    locate(
                        "crates/tako-control/src/setup_bootstrap.rs",
                        include_str!("../src/setup_bootstrap.rs"),
                        "fn unknown_shell_path(",
                    )
                )),
            }
        }
        if written_files(&home) != before {
            failures.push(format!("{label}（足し終えた）: ホームのファイルを書いた"));
        }
        // undo-path は従来どおり断る（そのシェルの設定ファイルは分からないまま）
        if let Ok(v) = dispatch(&bootstrap("undo-path")) {
            failures.push(format!("{label}（足し終えた）の undo-path が通った: {v}"));
        }
        std::env::set_var("PATH", root.join("empty-path"));
    }

    // ---- 既存の利用者（空・未設定 = OS の既定・zsh / bash / sh / fish）: 従来どおりのファイル ----
    i18n::set_lang(Lang::Ja);
    let known: [(&str, Option<String>, &str, &str); 7] = [
        ("unset", None, "zsh", ".zprofile"),
        ("empty", Some(String::new()), "zsh", ".zprofile"),
        ("zsh", Some(fake("zsh")), "zsh", ".zprofile"),
        (
            "missing-zsh",
            Some("/nonexistent/tako-2550/zsh".to_string()),
            "zsh",
            ".zprofile",
        ),
        ("bash", Some(fake("bash")), "bash", ".bash_profile"),
        ("sh", Some(fake("sh")), "bash", ".bash_profile"),
        (
            "fish",
            Some(fake("fish")),
            "fish",
            ".config/fish/config.fish",
        ),
    ];
    let mut kept = 0;
    for (label, shell, kind, profile) in &known {
        let home = fresh_home(&root, label);
        set_shell(shell.as_deref());
        // 空・未設定では本物のログインシェル（`exe::fallback_login_shell`。macOS は `/bin/zsh -l` = #2583）に
        // 尋ねる。そこで本物の claude を引ける環境なら status は
        // 認証の問い合わせで本物を起こすので叩かず、書き先の答えは path の応答だけで見る
        let status = if tako_core::platform::exe::find("claude").is_some() {
            println!("{label}: 本物の claude を引けるので status は叩かない");
            json!({ "shell": kind, "profile": home.join(profile).display().to_string() })
        } else {
            dispatch(&bootstrap("status")).unwrap_or_else(|e| json!({ "error": e }))
        };
        let result = dispatch(&bootstrap("path"));
        let files = written_files(&home);
        println!(
            "{label}: status shell={} profile={} / path {} / 書いたファイル {files:?}",
            status["shell"],
            status["profile"],
            match &result {
                Ok(v) => format!("shell={} change={}", v["shell"], v["change"]),
                Err(e) => format!("断り: {e}"),
            }
        );
        let want_profile = home.join(profile).display().to_string();
        let ok = matches!(&result, Ok(v) if v["shell"] == *kind && v["profile"] == want_profile.as_str() && v["wrote"] == true)
            && status["shell"] == *kind
            && status["profile"] == want_profile.as_str()
            && files == vec![profile.to_string()]
            && std::fs::read_to_string(home.join(profile))
                .is_ok_and(|text| text.starts_with(BLOCK_BEGIN));
        if ok {
            kept += 1;
        } else {
            failures.push(format!(
                "{label}: 従来の書き先（{kind} / ~/{profile}）と違う: status={status} path={result:?} files={files:?}"
            ));
        }
    }

    i18n::set_lang(original);
    let _ = std::fs::remove_dir_all(&root);
    println!(
        "種別の分からないシェルで断った: {refused} / {}（4 シェル × 日英 × path / undo-path）",
        unknown.len() * 4
    );
    println!(
        "足し終えた未知のシェルで断らなかった: {closed} / 4（tcsh / nu × dispatch / CLI の口）"
    );
    println!("既存の書き先を保った: {kept} / {}", known.len());
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
