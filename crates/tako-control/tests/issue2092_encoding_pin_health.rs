//! #2092: 文字コード固定を諦めたペインが **persist.log と診断（`tako check-health` /
//! MCP `tako_check_health`）の両方から分かる**ことを実経路で確かめる。
//!
//! - 全プラットフォーム: persist.log へ書く 1 実装（`diag::encoding_pin_log`）が
//!   ペイン ID・対象・理由を残し、送った文字を残さない。unix の check-health は
//!   `encoding_pin.needed=false`・`panes=[]`（固定が要らない = 門も記録も無い）
//! - Windows: 実際の ConPTY を注入で諦めさせ、ペインのイベント（GUI が persist.log へ
//!   書く材料）→ persist.log、dispatch の `CheckHealth` → `encoding_pin` 節と issues、
//!   の 2 経路で同じペインが名指しされる
//!
//! env（`TAKO_DATA_DIR` / 注入）はプロセス全体の状態なので、**専用の統合テストバイナリ**の
//! **1 本のテスト関数**の中で順に見る（`issue1259_legacy_ab.rs` と同じ作法）。

use std::path::{Path, PathBuf};

use tako_control::{
    PreviewHost, RemoteHost, SessionHost, SystemHost, TmuxHost, UiStateHost, WebViewHost,
    WorkspaceHost,
};
use tako_core::encoding_pin::{PinLogEvent, PinTarget};
use tako_core::{Pane, PaneId, PaneOrigin, SpawnOptions, TerminalSession, Workspace};

fn data_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tako-2092-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("一時 data dir を作れる");
    dir
}

fn persist_log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("persist.log")).unwrap_or_default()
}

/// GUI の代わり: ペイン 1 枚に端末（あれば）を持たせる
struct Host {
    workspace: Workspace,
    pane: PaneId,
    term: Option<TerminalSession>,
}

impl Host {
    fn new(term: Option<TerminalSession>) -> Self {
        let workspace = Workspace::new("1", Pane::new(PaneOrigin::User));
        let pane = workspace.active_tab().tree().focused();
        Self {
            workspace,
            pane,
            term,
        }
    }
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
    fn session(&self, pane: PaneId) -> Option<&TerminalSession> {
        (pane == self.pane).then_some(self.term.as_ref()).flatten()
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

impl PreviewHost for Host {}
impl SystemHost for Host {}
impl TmuxHost for Host {}
impl UiStateHost for Host {}
impl WebViewHost for Host {}
impl RemoteHost for Host {}

#[test]
fn 固定を諦めたペインはpersist_logと診断から分かる() {
    let dir = data_dir("health");
    std::env::set_var("TAKO_DATA_DIR", &dir);

    // --- persist.log へ書く 1 実装（GUI の `SessionNotice::EncodingPin` の腕が呼ぶ） ---
    let event = PinLogEvent::GaveUp {
        target: PinTarget::Container,
        pid: Some(4242),
        elapsed_ms: 15_001,
        reason: "器の中のペインの pid を 15 秒以内に取得できず（session=tako-0123456789ab）".into(),
    };
    tako_control::diag::encoding_pin_log(31, &event);
    let log = persist_log(&dir);
    assert!(
        log.contains("pane=31") && log.contains("対象=container") && log.contains("pid=4242"),
        "諦めが persist.log に出ない:\n{log}"
    );
    eprintln!("[2092] persist.log: {}", log.trim_end());

    // unix は固定が要らない = 門も記録も無い（check-health の節の形は dispatch の単体
    // `issue2092_諦めたペインは診断に名指しで出る` が見る。ここで check-health を叩くと
    // エージェント CLI への問い合わせ = 実測 10 秒級が走るので、実ペインのある Windows だけにする）
    #[cfg(not(windows))]
    {
        let (term, _rx) =
            TerminalSession::spawn(80, 24, SpawnOptions::default()).expect("PTY を張れる");
        assert!(term.encoding_pin().is_none(), "unix で門を作った");
        let _ = Host::new(Some(term));
    }

    #[cfg(windows)]
    windows::諦めたペインが2経路で名指しされる(&dir);

    // 一時 data dir を残さない（`tako test-residue` の掃除に頼らない）
    std::env::remove_var("TAKO_DATA_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::time::{Duration, Instant};
    use tako_core::terminal::{SessionNotice, SpawnCommand};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FreeConsole() -> i32;
    }

    pub(super) fn 諦めたペインが2経路で名指しされる(dir: &Path) {
        // 出荷構成（コンソールを持たない GUI プロセス）に合わせる
        unsafe {
            FreeConsole();
        }
        std::env::set_var("TAKO_2092_INJECT_PIN_GIVE_UP", "1");
        let (term, mut rx) = TerminalSession::spawn(
            80,
            24,
            SpawnOptions {
                command: Some(SpawnCommand {
                    program: "cmd.exe".into(),
                    args: vec!["/d".into(), "/q".into(), "/k".into(), "prompt $G".into()],
                }),
                cwd: Some(std::env::temp_dir()),
                env: Vec::new(),
                scrollback_lines: None,
            },
        )
        .expect("ConPTY を起動できる");
        std::env::remove_var("TAKO_2092_INJECT_PIN_GIVE_UP");
        let mut host = Host::new(Some(term));
        let pane = host.pane.as_u64();

        // ペインのイベント → GUI と同じ腕で persist.log へ
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut logged = false;
        while !logged && Instant::now() < deadline {
            while let Ok(event) = rx.try_recv() {
                let notice = host.term.as_mut().unwrap().process_event(event);
                if let Some(SessionNotice::EncodingPin(event)) = notice {
                    tako_control::diag::encoding_pin_log(pane, &event);
                    logged = true;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(logged, "諦めがペインのイベントで届かない");
        let log = persist_log(dir);
        let line = log
            .lines()
            .find(|l| l.contains(&format!("pane={pane} ")))
            .unwrap_or_else(|| panic!("ペイン {pane} の諦めが persist.log に無い:\n{log}"));
        assert!(line.contains("対象=direct"), "{line}");
        assert!(
            line.contains(tako_core::encoding_pin::INJECTED_GIVE_UP_REASON),
            "{line}"
        );
        eprintln!("[2092] persist.log（実ペイン）: {line}");

        // 診断（dispatch の CheckHealth = CLI / MCP と同じ 1 実装）
        let health = tako_control::dispatch::dispatch(
            &mut host,
            tako_control::protocol::Request::CheckHealth,
            PaneOrigin::Cli,
        )
        .expect("check-health が答える");
        let section = &health["encoding_pin"];
        eprintln!("[2092] check-health encoding_pin: {section}");
        assert_eq!(section["needed"], true, "{health}");
        assert_eq!(section["gave_up"], serde_json::json!([pane]), "{section}");
        assert_eq!(section["panes"][0]["state"], "gave_up", "{section}");
        assert_eq!(section["panes"][0]["targets"][0]["state"], "gave_up");
        // 諦めたペインのいまのコードページは UTF-8 ではない（読み直しが効いている）
        let live = &section["panes"][0]["targets"][0]["code_pages"];
        assert_eq!(live["utf8"], false, "{section}");
        let issue = health["issues"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["check"] == "encoding_pin")
            .unwrap_or_else(|| panic!("issues に出ない: {health}"));
        assert_eq!(issue["pane"], pane);
        assert_eq!(health["healthy"], false);
    }
}
