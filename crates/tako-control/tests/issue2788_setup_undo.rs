//! アンインストールの片付け（`tako setup undo`）を砂場で通しに回す（Issue #2788）
//!
//! 使い捨ての HOME / data dir / `%LOCALAPPDATA%` に setup が書く連携を置き、`setup_undo::run` で
//! 外す。確かめるのは受け入れ条件の 4 点:
//!
//! 1. MCP 登録・指示ファイルのブロック・mod の skills・PATH（unix のブロックと symlink・Windows の
//!    ユーザー環境変数 Path）・以前の版のシェル統合・WebView2 のキャッシュが外れる
//! 2. data dir の中身が 1 バイトも変わらない（ファイルの並びも）
//! 3. 2 回目は何もしない（砂場全体がバイト単位で同じ）
//! 4. 利用者が自分で書いた本文と、tako 以外の MCP 登録が残る
//!
//! 置き場は `Targets` に直に入れる（本物の HOME・レジストリ・agent の設定には一切触れない）。
//! Windows のユーザー環境変数 Path は偽の読み書き（`FakeUserPath`）で、値の算術は本物と同じ
//! `user_path::remove_entry` を通る

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use tako_control::claude_mod_install::{Source, Target};
use tako_control::setup_undo::{self, KeepReason, Kind, Outcome, Targets, UserPathTarget};
use tako_core::claude_mod_install as mod_core;
use tako_core::platform::user_path::UserPathValue;
use tako_core::shell_profile::{self, ShellKind};
use tako_core::test_residue::ScratchDir;

const SYNC_BEGIN: &str = "<!-- BEGIN SYNCED COMMON RULES -->";
const SYNC_END: &str = "<!-- END SYNCED COMMON RULES -->";
const USER_RULES: &str = "# 自分のルール\n\n- 日本語で答える\n";

/// 砂場の全ファイル（相対パス → 中身。symlink は指す先の文字列）
fn snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let path = e.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.file_type().is_symlink() {
                let to = std::fs::read_link(&path).unwrap();
                out.insert(
                    format!("{rel} -> link"),
                    to.to_string_lossy().into_owned().into_bytes(),
                );
            } else if meta.is_dir() {
                out.insert(format!("{rel}/"), Vec::new());
                stack.push(path);
            } else {
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// Windows のユーザー環境変数 Path の偽物（値は共有し、テストから読める）
#[derive(Clone)]
struct FakeUserPath(Rc<RefCell<String>>);

impl setup_undo::UserPathStore for FakeUserPath {
    fn read(&self) -> Result<UserPathValue, String> {
        Ok(UserPathValue::new(self.0.borrow().clone()))
    }
    fn write(&self, value: &UserPathValue) -> Result<(), String> {
        *self.0.borrow_mut() = value.raw.clone();
        Ok(())
    }
}

/// setup が書く連携一式を置いた砂場
struct Sandbox {
    _root: ScratchDir,
    root: PathBuf,
    home: PathBuf,
    data: PathBuf,
    local: PathBuf,
    acct: PathBuf,
    tako_dir: PathBuf,
    user_path: Rc<RefCell<String>>,
    /// 外した後に期待する本文
    claude_json_after: String,
    codex_before: String,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let scratch = ScratchDir::new(&format!("issue2788-{tag}"));
        let root = scratch.path().to_path_buf();
        let home = root.join("home");
        let data = root.join("data");
        let local = root.join("local").join("tako");
        let acct = root.join("acct");
        let tako_dir = root.join("Programs").join("tako");
        let tako_exe = tako_dir.join(if cfg!(windows) { "tako.exe" } else { "tako" });
        write(&tako_exe, "fake tako");
        let tako_cmd = tako_exe.display().to_string().replace('\\', "\\\\");

        // --- data dir（守るもの） ---
        write(&data.join("settings.json"), "{\"lang\": \"ja\"}\n");
        write(&data.join("layout.json"), "{\"tabs\": []}\n");
        write(&data.join("persist.log"), "2026-11-08 起動\n");
        write(&data.join("sessions.yaml"), "sessions: []\n");
        write(
            &data.join("orchestrator/config.yaml"),
            "setup:\n  completed: true\n",
        );
        write(
            &data.join("shell-integration/zshenv.zsh"),
            "# integration\n",
        );

        // --- MCP 登録 ---
        let claude_json_after = "{\n  \"firstStartTime\": \"2026-11-08T00:00:00.000Z\",\n  \"mcpServers\": {\n    \"mine\": {\n      \"type\": \"stdio\",\n      \"command\": \"npx\",\n      \"args\": [\n        \"my-server\"\n      ]\n    }\n  },\n  \"userID\": \"u\"\n}".to_string();
        write(
            &home.join(".claude.json"),
            &format!("{{\n  \"firstStartTime\": \"2026-11-08T00:00:00.000Z\",\n  \"mcpServers\": {{\n    \"mine\": {{\n      \"type\": \"stdio\",\n      \"command\": \"npx\",\n      \"args\": [\n        \"my-server\"\n      ]\n    }},\n    \"tako\": {{\n      \"type\": \"stdio\",\n      \"command\": \"{tako_cmd}\",\n      \"args\": [\n        \"mcp\",\n        \"serve\"\n      ],\n      \"env\": {{}}\n    }}\n  }},\n  \"userID\": \"u\"\n}}"),
        );
        // アカウントの設定 dir（CLAUDE_CONFIG_DIR）にも登録がある
        write(
            &acct.join(".claude.json"),
            &format!("{{\n  \"mcpServers\": {{\n    \"tako\": {{\n      \"command\": \"{tako_cmd}\",\n      \"args\": [\"mcp\", \"serve\"]\n    }}\n  }}\n}}\n"),
        );
        // 以前の版が settings.json に書いた登録
        write(
            &home.join(".claude/settings.json"),
            "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"tako\": {\n      \"command\": \"tako\"\n    }\n  }\n}\n",
        );
        let codex_before = "# 自分の codex の設定\nmodel = \"gpt-5\"\n\n[mcp_servers.mine]\ncommand = \"npx\"\nargs = [\"my-server\"]\n".to_string();
        write(
            &home.join(".codex/config.toml"),
            &format!("{codex_before}\n[mcp_servers.tako]\nenv_vars = [\"TAKO_SOCKET\", \"TAKO_TOKEN\", \"TAKO_PANE_ID\"]\ncommand = \"{tako_cmd}\"\nargs = [\"mcp\", \"serve\"]\n"),
        );
        write(
            &home.join(".gemini/config/mcp_config.json"),
            &format!("{{\n  \"mcpServers\": {{\n    \"tako\": {{\n      \"command\": \"{tako_cmd}\",\n      \"args\": [\"mcp\", \"serve\"]\n    }},\n    \"other\": {{\n      \"command\": \"uvx\"\n    }}\n  }}\n}}\n"),
        );

        // --- 指示ファイル（同期ブロック = tako agents sync-rules の形） ---
        let block = format!("{SYNC_BEGIN}\n共通ルール\n{SYNC_END}");
        write(
            &home.join(".claude/CLAUDE.md"),
            &format!("{block}\n\n{USER_RULES}"),
        );
        // ファイルが無かったところへ同期が作った（ブロックだけ）
        write(&home.join(".codex/AGENTS.md"), &block);
        // 同期の対象外（利用者の本文だけ）
        write(&home.join(".gemini/GEMINI.md"), USER_RULES);

        // --- tako mod の skills（印つきの写し / 利用者のもの） ---
        mod_core::write_copy(&home.join(".claude"), &mod_core::Expected::current()).unwrap();
        write(
            &acct.join("skills/tako/SKILL.md"),
            "# 自分で作った tako という skill\n",
        );

        // --- PATH（unix のブロック + symlink） ---
        let bin = home.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&tako_exe, bin.join("tako")).unwrap();
        write(&home.join(".zprofile"), "export EDITOR=vim\n");
        shell_profile::ensure_dirs_on_path_in(&home, ShellKind::Zsh, &[bin.clone()], None).unwrap();
        // ログインシェルを変えた人の古いブロック（ファイルはブロックだけ = tako が作った）
        shell_profile::ensure_dirs_on_path_in(&home, ShellKind::Bash, &[bin.clone()], None)
            .unwrap();

        // --- Windows: ユーザー環境変数 Path・以前の版の $PROFILE・WebView2 ---
        let user_path = Rc::new(RefCell::new(format!(
            "C:\\Windows\\System32;{};{}",
            tako_dir.display(),
            bin.display()
        )));
        write(
            &home.join("Documents/PowerShell/profile.ps1"),
            // 以前の版の書き方（区切りの改行 1 個 + ブロック。text_block::apply の形）
            "Set-Alias ll Get-ChildItem\n\n# >>> tako shell integration >>>\n. 'x'\n# <<< tako shell integration <<<\n",
        );
        write(
            &local.join("WebView2/EBWebView/Default/Cache/data_0"),
            "cache",
        );
        write(
            &tako_dir.join("tako-app.exe.WebView2/EBWebView/Local State"),
            "{}",
        );

        Sandbox {
            _root: scratch,
            root,
            home,
            data,
            local,
            acct,
            tako_dir,
            user_path,
            claude_json_after,
            codex_before,
        }
    }

    fn targets(&self) -> Targets {
        let home = &self.home;
        let claude_dirs = vec![
            Target {
                dir: home.join(".claude"),
                sources: vec![Source::Default],
            },
            Target {
                dir: self.acct.clone(),
                sources: vec![Source::Account("univ".into())],
            },
        ];
        Targets {
            protected: vec![self.data.clone()],
            verification: true,
            claude_json: setup_undo::claude_json_paths(&claude_dirs),
            claude_settings: claude_dirs
                .iter()
                .map(|t| t.dir.join("settings.json"))
                .collect(),
            claude_config_dirs: claude_dirs,
            codex_config: Some(home.join(".codex/config.toml")),
            agy_config: Some(home.join(".gemini/config/mcp_config.json")),
            instructions: vec![
                ("claude", home.join(".claude/CLAUDE.md")),
                ("codex", home.join(".codex/AGENTS.md")),
                ("agy", home.join(".gemini/GEMINI.md")),
            ],
            cli_link: cfg!(unix).then(|| home.join(".local/bin/tako")),
            profiles: shell_profile::PATH_BLOCK_PROFILES
                .iter()
                .map(|(shell, rel)| (*shell, home.join(rel)))
                .collect(),
            path_dirs: vec![home.join(".local/bin")],
            user_path: Some(UserPathTarget {
                tako_dir: self.tako_dir.clone(),
                store: Box::new(FakeUserPath(self.user_path.clone())),
            }),
            ps_profiles: vec![home.join("Documents/PowerShell/profile.ps1")],
            caches: vec![
                self.local.join("WebView2"),
                self.tako_dir.join("tako-app.exe.WebView2"),
            ],
            cache_parent: Some(self.local.clone()),
        }
    }
}

fn outcome_of<'a>(report: &'a setup_undo::Report, kind: Kind, path_end: &str) -> &'a Outcome {
    let found = report
        .items
        .iter()
        .find(|i| i.kind == kind && i.path.replace('\\', "/").ends_with(path_end))
        .unwrap_or_else(|| panic!("{kind:?} {path_end} の項目が無い: {:#?}", report.items));
    &found.outcome
}

#[test]
fn issue2788_連携が外れdataは1バイトも変わらず利用者の本文とtako以外の登録が残る() {
    let sb = Sandbox::new("full");
    let data_before = snapshot(&sb.data);
    let report = setup_undo::run(&sb.targets(), false);
    let json = report.to_json(tako_core::i18n::Lang::Ja);
    assert_eq!(report.failed(), 0, "失敗した項目がある: {json:#}");

    // 1. MCP 登録が外れ、tako 以外の登録・他の鍵は残る
    assert_eq!(read(&sb.home.join(".claude.json")), sb.claude_json_after);
    let acct: serde_json::Value =
        serde_json::from_str(&read(&sb.acct.join(".claude.json"))).unwrap();
    assert!(
        acct["mcpServers"].get("tako").is_none(),
        "アカウントの設定 dir の登録が残った"
    );
    let settings = read(&sb.home.join(".claude/settings.json"));
    assert!(
        !settings.contains("\"tako\"") && settings.contains("\"theme\": \"dark\""),
        "{settings}"
    );
    assert_eq!(read(&sb.home.join(".codex/config.toml")), sb.codex_before);
    let agy: serde_json::Value =
        serde_json::from_str(&read(&sb.home.join(".gemini/config/mcp_config.json"))).unwrap();
    assert!(agy["mcpServers"].get("tako").is_none());
    assert_eq!(agy["mcpServers"]["other"]["command"], "uvx");

    // 1. 指示ファイルの同期ブロックが外れ、利用者の本文はバイト単位で残る
    assert_eq!(read(&sb.home.join(".claude/CLAUDE.md")), USER_RULES);
    assert!(
        !sb.home.join(".codex/AGENTS.md").exists(),
        "ブロックだけのファイルが残った"
    );
    assert_eq!(read(&sb.home.join(".gemini/GEMINI.md")), USER_RULES);

    // 1. mod の skills: 印つきの写しだけを外し、利用者の tako は残す
    assert!(!mod_core::copy_dir(&sb.home.join(".claude")).exists());
    assert!(sb.acct.join("skills/tako/SKILL.md").is_file());
    assert_eq!(
        outcome_of(&report, Kind::ModSkills, "acct/skills/tako"),
        &Outcome::Kept(KeepReason::NotOurs)
    );

    // 1. PATH: symlink と、どのログインシェルのブロックも外れる（置き場が空になったので）
    if cfg!(unix) {
        assert!(std::fs::symlink_metadata(sb.home.join(".local/bin/tako")).is_err());
    }
    assert_eq!(read(&sb.home.join(".zprofile")), "export EDITOR=vim\n");
    assert!(
        !sb.home.join(".bash_profile").exists(),
        "ブロックだけの profile が残った"
    );
    // Windows のユーザー環境変数 Path: tako を入れた先だけを外し、共有の置き場は残す
    assert_eq!(
        *sb.user_path.borrow(),
        format!(
            "C:\\Windows\\System32;{}",
            sb.home.join(".local/bin").display()
        )
    );

    // 1. 以前の版のシェル統合と WebView2 のキャッシュ
    assert_eq!(
        read(&sb.home.join("Documents/PowerShell/profile.ps1")),
        "Set-Alias ll Get-ChildItem\n"
    );
    assert!(
        !sb.local.exists(),
        "空になった %LOCALAPPDATA%\\tako が残った"
    );
    assert!(!sb.tako_dir.join("tako-app.exe.WebView2").exists());

    // 2. data dir は 1 バイトも変わらない
    assert_eq!(snapshot(&sb.data), data_before, "data dir が変わった");
    assert_eq!(report.kept_data.as_deref(), Some(sb.data.as_path()));

    // 3. 2 回目は何もしない（砂場全体が同じ・外した項目が無い）
    let after_first = snapshot(&sb.root);
    let second = setup_undo::run(&sb.targets(), false);
    assert_eq!(snapshot(&sb.root), after_first, "2 回目に何かを書き換えた");
    let second_json = second.to_json(tako_core::i18n::Lang::Ja);
    assert_eq!(second_json["summary"]["removed"], 0, "{second_json:#}");
    assert_eq!(second_json["summary"]["failed"], 0, "{second_json:#}");
    assert_eq!(
        *sb.user_path.borrow(),
        format!(
            "C:\\Windows\\System32;{}",
            sb.home.join(".local/bin").display()
        )
    );
}

#[test]
fn issue2788_dry_runは何も変えず外すものを並べる() {
    let sb = Sandbox::new("dry");
    let before = snapshot(&sb.root);
    let path_before = sb.user_path.borrow().clone();
    let report = setup_undo::run(&sb.targets(), true);
    assert_eq!(snapshot(&sb.root), before, "dry-run なのに書き換えた");
    assert_eq!(*sb.user_path.borrow(), path_before);
    assert!(report.dry_run);
    assert_eq!(
        outcome_of(&report, Kind::Mcp, "home/.claude.json"),
        &Outcome::WouldRemove
    );
    assert_eq!(
        outcome_of(&report, Kind::PathBlock, ".zprofile"),
        &Outcome::WouldRemove,
        "dry-run でも symlink は外した扱いで置き場の空きを見る"
    );
    assert!(report.items.iter().all(|i| i.outcome != Outcome::Removed));
    let lines = report.render_lines(tako_core::i18n::Lang::Ja).join("\n");
    assert!(lines.contains("外すなら: tako setup undo"), "{lines}");
}

#[test]
fn issue2788_置き場に他のコマンドが残るならpathのブロックは残しリンクだけ外す() {
    let sb = Sandbox::new("still-used");
    write(&sb.home.join(".local/bin/claude"), "#!/bin/sh\n");
    let zprofile_before = read(&sb.home.join(".zprofile"));
    let report = setup_undo::run(&sb.targets(), false);
    assert!(matches!(
        outcome_of(&report, Kind::PathBlock, ".zprofile"),
        Outcome::Kept(KeepReason::DirStillUsed { .. })
    ));
    assert_eq!(read(&sb.home.join(".zprofile")), zprofile_before);
    if cfg!(unix) {
        assert_eq!(
            outcome_of(&report, Kind::CliLink, ".local/bin/tako"),
            &Outcome::Removed
        );
    }
    // 英語の表示でも理由を名指す
    let en = report.render_lines(tako_core::i18n::Lang::En).join("\n");
    assert!(en.contains("other commands still live in"), "{en}");
    assert!(!en.contains("残し"), "英語の表示に日本語が混ざった: {en}");
}

#[test]
fn issue2788_壊れた設定は触らず残りは進める() {
    let sb = Sandbox::new("broken");
    write(
        &sb.home.join(".claude.json"),
        "{ \"mcpServers\": { \"tako\": ",
    );
    let broken_md = format!("{SYNC_BEGIN}\n終わりの印が無い\n{USER_RULES}");
    write(&sb.home.join(".claude/CLAUDE.md"), &broken_md);
    // 名前は tako でも別のコマンド（利用者が自分で付けた）
    write(
        &sb.home.join(".gemini/config/mcp_config.json"),
        "{\"mcpServers\": {\"tako\": {\"command\": \"npx\", \"args\": [\"tako-like\"]}}}",
    );
    let report = setup_undo::run(&sb.targets(), false);
    assert!(matches!(
        outcome_of(&report, Kind::Mcp, "home/.claude.json"),
        Outcome::Kept(KeepReason::Broken {
            line: 1,
            column: 26
        })
    ));
    assert_eq!(
        read(&sb.home.join(".claude.json")),
        "{ \"mcpServers\": { \"tako\": "
    );
    assert_eq!(
        outcome_of(&report, Kind::Instructions, ".claude/CLAUDE.md"),
        &Outcome::Kept(KeepReason::UnterminatedBlock)
    );
    assert_eq!(read(&sb.home.join(".claude/CLAUDE.md")), broken_md);
    assert_eq!(
        outcome_of(&report, Kind::Mcp, "mcp_config.json"),
        &Outcome::Kept(KeepReason::NotTakoCommand {
            command: "npx".into()
        })
    );
    // 残りは進んだ
    assert_eq!(read(&sb.home.join(".codex/config.toml")), sb.codex_before);
    assert_eq!(report.failed(), 0);
}

#[test]
fn issue2788_連携が最初から無ければ全部もともと無い() {
    let scratch = ScratchDir::new("issue2788-empty");
    let home = scratch.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let targets = Targets {
        verification: true,
        claude_json: vec![home.join(".claude.json")],
        codex_config: Some(home.join(".codex/config.toml")),
        agy_config: Some(home.join(".gemini/config/mcp_config.json")),
        instructions: vec![("claude", home.join(".claude/CLAUDE.md"))],
        cli_link: Some(home.join(".local/bin/tako")),
        profiles: vec![(ShellKind::Zsh, home.join(".zprofile"))],
        path_dirs: vec![home.join(".local/bin")],
        claude_config_dirs: vec![Target {
            dir: home.join(".claude"),
            sources: vec![Source::Default],
        }],
        ..Targets::default()
    };
    let before = snapshot(scratch.path());
    let report = setup_undo::run(&targets, false);
    assert_eq!(snapshot(scratch.path()), before);
    assert!(
        report.items.iter().all(|i| i.outcome == Outcome::Absent),
        "{:#?}",
        report.items
    );
    let lines = report.render_lines(tako_core::i18n::Lang::Ja).join("\n");
    assert!(lines.contains("もともと無かったもの"), "{lines}");
}

#[test]
fn issue2788_data_dirの下の書き先は断る() {
    let sb = Sandbox::new("protected");
    // data dir の中に claude の設定 dir と MCP 登録がある（守る側が勝つ）
    let inside = sb.data.join("agent-home/.claude");
    mod_core::write_copy(&inside, &mod_core::Expected::current()).unwrap();
    write(
        &sb.data.join("agent-home/.claude.json"),
        "{\"mcpServers\": {\"tako\": {\"command\": \"tako\"}}}",
    );
    let before = snapshot(&sb.data);
    let mut targets = sb.targets();
    targets
        .claude_json
        .push(sb.data.join("agent-home/.claude.json"));
    targets.claude_config_dirs.push(Target {
        dir: inside,
        sources: vec![Source::Override],
    });
    let report = setup_undo::run(&targets, false);
    assert_eq!(snapshot(&sb.data), before, "data dir の下を書き換えた");
    assert_eq!(
        outcome_of(&report, Kind::Mcp, "agent-home/.claude.json"),
        &Outcome::Kept(KeepReason::InsideDataDir)
    );
    assert_eq!(
        outcome_of(&report, Kind::ModSkills, "agent-home/.claude/skills/tako"),
        &Outcome::Kept(KeepReason::InsideDataDir)
    );
}

#[test]
fn issue2788_claudeの設定dirから引くclaude_jsonの置き場() {
    // 実測（claude 2.1.301）: 既定は ~/.claude.json・CLAUDE_CONFIG_DIR=X は X/.claude.json
    let paths = setup_undo::claude_json_paths(&[
        Target {
            dir: PathBuf::from("/h/.claude"),
            sources: vec![Source::Default, Source::Account("a".into())],
        },
        Target {
            dir: PathBuf::from("/h/.claude-univ"),
            sources: vec![Source::Env],
        },
    ]);
    assert_eq!(
        paths,
        vec![
            PathBuf::from("/h/.claude.json"),
            PathBuf::from("/h/.claude/.claude.json"),
            PathBuf::from("/h/.claude-univ/.claude.json"),
        ]
    );
}

/// tako CLI の symlink が一時 dir の外（.app の実体）を指していても、外すのはリンクだけ。
/// 番人はリンクの置き場で見る（指す先まで辿ると「外へ書く」と取り違えて残していた。macOS の実経路で発見）
#[cfg(unix)]
#[test]
fn issue2788_外を指すtakoのリンクも置き場で判定して外す() {
    let scratch = ScratchDir::new("issue2788-link-outside");
    let bin = scratch.path().join("home/.local/bin");
    std::fs::create_dir_all(&bin).unwrap();
    let outside = PathBuf::from("/bin/sh");
    assert!(outside.exists());
    std::os::unix::fs::symlink(&outside, bin.join("tako")).unwrap();
    let targets = Targets {
        verification: true,
        cli_link: Some(bin.join("tako")),
        ..Targets::default()
    };
    let report = setup_undo::run(&targets, false);
    assert_eq!(
        outcome_of(&report, Kind::CliLink, ".local/bin/tako"),
        &Outcome::Removed
    );
    assert!(std::fs::symlink_metadata(bin.join("tako")).is_err());
    assert!(outside.exists(), "指す先に触れた");
}
