//! 実経路: `tako mod`（status / on / off / band on|off）の応答と断りが、1 つの応答の中で表示言語に
//! そろう（Issue #2365 の続き。`tako mod ui` の枠は #2417）。
//!
//! 修正前（実測。英語表示）: `band-on` / `band-off` / `on` / `off` の `applies_to`・注入しない理由・
//! 失効した行の理由・skills の状態の文・mod を扱わない tako への断りが日本語のままだった。
//!
//! ここでは GUI の dispatch が `tako_mod` で呼ぶ 1 本（`claude_mod::run`）を GUI の代わりのホストへ渡す。
//! 応答（MCP の `tako_mod` と CLI の `--json` が返す JSON）・断り（`DispatchError` の表示 = IPC の受け口が
//! 返す文）を、英語表示では日本語（かな・漢字）が 1 字も無いこと、日英で別の文になることで見る。
//! 応答の JSON は `DUMP <言語> <ケース> <JSON>` の行で標準出力へ出す（修正前後の CLI の表示を偽の受け口で
//! 比べる A/B が読む）。表示言語はプロセス全体の状態なので、**専用の統合テストバイナリの 1 本の
//! テスト関数**の中で順に切り替える。修正前の木でもそのままコンパイルできる口だけを使う

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::claude_mod::{Injection, ModHub, OffReason};
use tako_core::i18n::{self, Lang};
use tako_core::test_residue::ScratchDir;
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

/// GUI の代わり: ペインの木・mod の状態・ui.json の置き場だけを持つ
struct Host {
    workspace: Workspace,
    hub: Option<ModHub>,
    ui_path: PathBuf,
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

impl SystemHost for Host {
    fn claude_mod(&self) -> Option<&ModHub> {
        self.hub.as_ref()
    }
    fn claude_mod_mut(&mut self) -> Option<&mut ModHub> {
        self.hub.as_mut()
    }
    fn set_claude_mod_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if let Some(hub) = self.hub.as_mut() {
            hub.enabled = enabled;
        }
        Ok(())
    }
    fn claude_mod_ui_path(&self) -> Option<PathBuf> {
        Some(self.ui_path.clone())
    }
}

impl TmuxHost for Host {}
impl UiStateHost for Host {}
impl PreviewHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}

fn has_japanese(s: &str) -> Option<char> {
    s.chars()
        .find(|c| matches!(*c as u32, 0x3000..=0x30FF | 0x4E00..=0x9FFF | 0xFF00..=0xFFEF))
}

/// 注入できる状態の mod（版は下限ちょうど・CLI と展開先あり）
fn healthy() -> ModHub {
    let mut hub = ModHub::new(true);
    hub.plugin_dir = Some(PathBuf::from("/d/claude-mod/tako"));
    hub.cli = Some(PathBuf::from("/a/tako"));
    hub.claude_version = Some(tako_core::claude_mod::MIN_CLAUDE_VERSION.into());
    hub
}

fn report(turn: &str) -> tako_core::claude_mod::ModReport {
    tako_core::claude_mod::parse_report(json!({
        "schema": 1, "mod_version": "t", "at": 1, "turn": turn,
        "context": {"tokens": 10, "window": 100, "percent": 10}, "rate_limits": [],
    }))
    .unwrap()
}

/// 1 ケース: mod の状態を作り、action を 1 回通した応答（成功は JSON・断りは表示の文）
fn run_case(dir: &std::path::Path, name: &str, action: &str, hub: Option<ModHub>) -> String {
    let workspace = Workspace::new("1", Pane::new(PaneOrigin::User));
    let pane = workspace.active_tab().tree().focused().as_u64();
    let hub = hub.map(|mut hub| {
        // 注入の記録（`not_injected` の行は止めた理由つき）と、失効した報告の行
        let injection = match name {
            "pane_not_injected" => Injection::Off(OffReason::Disabled),
            _ => hub.decide(false),
        };
        hub.record_spawn(pane, &injection);
        if name == "pane_stale" {
            hub.accept(
                pane,
                report("idle"),
                Instant::now() - Duration::from_secs(120),
            );
        }
        hub
    });
    let sub = dir.join(name);
    std::fs::create_dir_all(&sub).unwrap();
    let mut host = Host {
        workspace,
        hub,
        ui_path: sub.join("ui.json"),
    };
    match tako_control::claude_mod::run(&mut host, Some(action), None, None, None, false) {
        Ok(v) => v.to_string(),
        Err(e) => e.to_string(),
    }
}

/// 代表のケース（名前・action・mod の状態。`None` = mod を扱わない tako）
fn cases() -> Vec<(&'static str, &'static str, Option<ModHub>)> {
    let with = |f: fn(&mut ModHub)| {
        let mut hub = healthy();
        f(&mut hub);
        Some(hub)
    };
    vec![
        ("on", "on", Some(healthy())),
        ("off", "off", Some(healthy())),
        ("band_on", "band-on", Some(healthy())),
        ("band_off", "band-off", Some(healthy())),
        ("status", "status", Some(healthy())),
        ("band_on_disabled", "band-on", with(|h| h.enabled = false)),
        (
            "band_on_too_old",
            "band-on",
            with(|h| h.claude_version = Some("2.1.200".into())),
        ),
        (
            "band_off_unknown_claude",
            "band-off",
            with(|h| h.claude_version = None),
        ),
        ("band_on_no_cli", "band-on", with(|h| h.cli = None)),
        (
            "band_on_not_installed",
            "band-on",
            with(|h| h.plugin_dir = None),
        ),
        ("pane_stale", "band-on", Some(healthy())),
        ("pane_not_injected", "band-off", Some(healthy())),
        ("no_hub", "band-on", None),
        ("unknown_action", "bogus", Some(healthy())),
    ]
}

#[test]
fn mod_の状態と帯の応答は1つの応答の中で表示言語にそろう() {
    let scratch = ScratchDir::new("issue2365-mod-band");
    let dir = tako_core::platform::path::canonicalize(scratch.path()).unwrap();
    let base = dir.display().to_string();
    let original = i18n::lang();
    let mut by_lang: Vec<(Lang, Vec<(&str, String)>)> = Vec::new();
    for lang in [Lang::Ja, Lang::En] {
        i18n::set_lang(lang);
        let seen = cases()
            .into_iter()
            .map(|(name, action, hub)| {
                let text = run_case(&dir.join(format!("{lang:?}")), name, action, hub)
                    .replace(&base, "<DIR>");
                (name, text)
            })
            .collect();
        by_lang.push((lang, seen));
    }
    i18n::set_lang(original);

    let mut failures = Vec::new();
    for (lang, seen) in &by_lang {
        for (name, text) in seen {
            println!("DUMP {lang:?} {name} {text}");
            if *lang == Lang::En {
                if let Some(c) = has_japanese(text) {
                    failures.push(format!(
                        "[En] {name}: 英語表示の応答に日本語（{c}）が混ざる: {text}"
                    ));
                }
            }
        }
    }
    let (ja, en) = (&by_lang[0].1, &by_lang[1].1);
    for ((name, j), (_, e)) in ja.iter().zip(en) {
        // 注入できる状態の status は枠の文を持たない（skills の行の文は置く先の設定 dir の様子で
        // 変わる）ので、日英の違いは applies_to・理由・断りを持つケースだけで見る
        if j == e && *name != "status" {
            failures.push(format!("{name}: 日英が同じ文: {j}"));
        }
        // 成功の応答は JSON（MCP / `--json`）。日本語表示の応答も JSON として読める
        if let Ok(v) = serde_json::from_str::<Value>(j) {
            assert!(v.is_object(), "{name}: {j}");
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
