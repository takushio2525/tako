//! 作業 dir が 8.3 の短い形（`C:\Users\WINUSE~1\…`）のときの事前信頼の鍵（#2843）。
//!
//! 2 本ある:
//!
//! - `issue2843_短い形の作業dirの鍵は長い形の鍵と同じ`（常設・Windows の全数で走る）: 実在する短い形・`/` 区切り・
//!   verbatim の短い形から作った鍵が長い形の鍵（#1886 の答え）と同じになること、製品の事前信頼
//!   （`claude_tui::ensure_trusted_in`）が短い形の cwd を受けても長い形の鍵で書くこと、戻せないパス
//!   （存在しない・`~` が長い名前の一部・空白と日本語を含む長い形）は従来の答えのままであること。
//! - `issue2843_windows_claude_trusts_short_cwd`（`#[ignore]`・**本物の claude を起こす**）:
//!   `scripts/windows/test-claude-mod-1886.ps1` が砂場へ claude を入れ偽の Messages API を立ててから名指しで回す
//!   （`agent_install` の門の中。#1886 / #2842 と同じ導入）。短い形の cwd の ConPTY のペインから claude を
//!   直起動 / ペインの PowerShell 5.1 から打つ形の両方で起こして、次の 3 つを見る:
//!   1. 信頼の確認を通したときに claude が自分で書く鍵（= claude が引く鍵）が長い形か短い形か
//!   2. 製品の事前信頼（短い形の cwd を渡す）を書いた設定 dir では確認が出ないこと
//!   3. 修正前の答え（短い形をそのまま鍵にした形）だけを書いた設定 dir では確認が出ること（2 の検出力）
//!
//! 結果は `TAKO2843_*` の行で CI のログへ残す（パスは `<scratch>` / `<scratch~>` に置き換える）。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::claude_tui::{claude_project_key, claude_project_key_in, ensure_trusted_in};
use tako_core::terminal::{SessionNotice, SpawnCommand, SpawnOptions, TerminalSession};
use tako_core::test_residue::ScratchDir;

/// 砂場に入れた本物の claude.exe（スクリプトが渡す）
const CLAUDE_EXE_ENV: &str = "TAKO_2843_CLAUDE_EXE";
/// claude を入れた砂場の USERPROFILE（claude の書き込みをここへ閉じ込める）
const HOME_ENV: &str = "TAKO_2843_HOME";
/// 偽の Messages API の URL
const API_URL_ENV: &str = "TAKO_2843_API_URL";
/// 別のボリュームの置き場（任意。8.3 の名前が作られないボリュームなら戻さない枝を実機で見る）
const OTHER_VOLUME_ENV: &str = "TAKO_2843_OTHER_VOLUME_DIR";

/// 偽の API へ渡すダミーの鍵（本物の認証は使わない。末尾 20 文字を事前承認に書く）
const DUMMY_KEY: &str = "sk-ant-tako2843-dummy-key-0000000000";
/// 状態を待つ上限（混んだランナーで claude の起動が遅い回を許す。尽きたら画面を添えて落ちる）
const STATE_WAIT: Duration = Duration::from_secs(90);

// --- 短い形 -------------------------------------------------------------------

unsafe extern "system" {
    fn GetShortPathNameW(lpszLongPath: *const u16, lpszShortPath: *mut u16, cchBuffer: u32) -> u32;
}

/// 実機の 8.3 の短い形（取れなければ `None`）
fn short_of(path: &Path) -> Option<String> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let len = unsafe { GetShortPathNameW(wide.as_ptr(), std::ptr::null_mut(), 0) };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), len) };
    if written == 0 || written >= len {
        return None;
    }
    buf.truncate(written as usize);
    std::ffi::OsString::from_wide(&buf).into_string().ok()
}

/// 長い形と短い形の作業 dir（空白と日本語を含む = 8.3 の名前が作られるボリュームなら短い名前を持つ）
struct Dirs {
    _scratch: ScratchDir,
    long_root: String,
    short_root: String,
    long_work: String,
    short_work: String,
}

impl Dirs {
    fn new() -> Self {
        let scratch = ScratchDir::new("issue2843");
        let long_root = tako_core::platform::path::canonicalize(scratch.path())
            .expect("砂場の長い形")
            .display()
            .to_string();
        let work = Path::new(&long_root)
            .join("tako 2843 データ")
            .join("work dir");
        std::fs::create_dir_all(&work).unwrap();
        let long_work = work.display().to_string();
        let short_root =
            short_of(Path::new(&long_root)).expect("砂場の短い形を問い合わせられること");
        let short_work = short_of(&work).expect("作業 dir の短い形を問い合わせられること");
        Self {
            _scratch: scratch,
            long_root,
            short_root,
            long_work,
            short_work,
        }
    }

    /// 短い形が作れたか（TEMP が短い形の利用者名を持つ・8.3 の名前が作られるボリューム）
    fn has_short(&self) -> bool {
        self.short_work != self.long_work
    }

    /// CI のログへ出す前に砂場の根を伏せる（`\` と `/` の両方の綴り）
    fn mask(&self, text: &str) -> String {
        let mut out = text.to_string();
        // 長いほうから置き換える（短い形が長い形の頭と同じになることがある）
        let mut pairs = vec![
            (self.long_root.clone(), "<scratch>"),
            (self.long_root.replace('\\', "/"), "<scratch>"),
            (self.short_root.clone(), "<scratch~>"),
            (self.short_root.replace('\\', "/"), "<scratch~>"),
        ];
        pairs.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        for (from, to) in pairs {
            if !from.is_empty() {
                out = out.replace(&from, to);
            }
        }
        out
    }

    fn long_key(&self) -> String {
        claude_project_key_in(&self.long_work, true, false)
    }

    /// 修正前（#2843 より前）の答え = 短い形をそのまま `/` 区切りにした鍵
    fn short_key(&self) -> String {
        claude_project_key_in(&self.short_work, true, false)
    }

    fn form_of(&self, key: &str) -> &'static str {
        if key == self.long_key() {
            "long"
        } else if key == self.short_key() {
            "short"
        } else {
            "other"
        }
    }
}

// --- 常設（claude を起こさない） -------------------------------------------------

#[test]
fn issue2843_短い形の作業dirの鍵は長い形の鍵と同じ() {
    let dirs = Dirs::new();
    println!(
        "TAKO2843_PATHS short_differs={} short_root_differs={} long=[{}] short=[{}]",
        dirs.has_short(),
        dirs.short_root != dirs.long_root,
        dirs.mask(&dirs.long_work),
        dirs.mask(&dirs.short_work)
    );
    let long_key = dirs.long_key();
    // 長い形は #1886 の答えのまま
    assert_eq!(claude_project_key(&dirs.long_work), long_key);
    if !dirs.has_short() {
        // 8.3 の名前が作られない環境では短い形の入力を作れない（戻す対象が無い = 何も変わらない）
        println!("TAKO2843_SKIP 短い形を作れない（このボリュームで 8.3 の名前が作られない）");
        return;
    }
    // 短い形・`/` 区切りの短い形・verbatim の短い形のどれも長い形の鍵になる
    for (label, cwd) in [
        ("short", dirs.short_work.clone()),
        ("short-slash", dirs.short_work.replace('\\', "/")),
        ("short-verbatim", format!(r"\\?\{}", dirs.short_work)),
    ] {
        assert_eq!(
            claude_project_key(&cwd),
            long_key,
            "[#2843 {label}] 短い形の作業 dir {} の鍵が長い形の鍵 {} と違う（短い形を長い形へ戻す段 = 境界 B34 が効いていない）",
            dirs.mask(&cwd),
            dirs.mask(&long_key)
        );
    }
    // 製品の事前信頼（短い形の cwd を渡す）が長い形の鍵で書く
    let cfg = Path::new(&dirs.long_root).join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    ensure_trusted_in(Some(&cfg.display().to_string()), &dirs.short_work)
        .expect("事前信頼を書けること");
    let keys = trusted_keys(&cfg);
    assert_eq!(
        keys,
        vec![long_key.clone()],
        "[#2843 ensure_trusted_in] 短い形の cwd を渡した事前信頼の鍵が長い形でない: {:?}",
        keys.iter().map(|k| dirs.mask(k)).collect::<Vec<_>>()
    );
}

#[test]
fn issue2843_戻せないパスは従来の答えのまま() {
    let dirs = Dirs::new();
    // 存在しない（GetLongPathNameW が答えない）
    let missing = format!(r"{}\NOEXIS~1\work", dirs.long_root);
    assert_eq!(
        claude_project_key(&missing),
        claude_project_key_in(&missing, true, false)
    );
    // `~` が長い名前の一部（8.3 の名前が無いボリュームと同じく OS が同じ形を返す）
    let literal = Path::new(&dirs.long_root).join("a~1 データ");
    std::fs::create_dir_all(&literal).unwrap();
    let literal = literal.display().to_string();
    assert_eq!(
        claude_project_key(&literal),
        claude_project_key_in(&literal, true, false)
    );
    // 空白と日本語を含む長い形
    assert_eq!(claude_project_key(&dirs.long_work), dirs.long_key());
    // 別のボリューム（スクリプトが渡したときだけ。8.3 の名前が作られないなら短い形 = 長い形）
    if let Some(other) = std::env::var_os(OTHER_VOLUME_ENV) {
        let other = PathBuf::from(other);
        let dir = other.join("tako 2843 データ").join("work dir");
        std::fs::create_dir_all(&dir).unwrap();
        let long = tako_core::platform::path::canonicalize(&dir)
            .unwrap()
            .display()
            .to_string();
        let short = short_of(Path::new(&long)).unwrap_or_else(|| long.clone());
        println!(
            "TAKO2843_OTHER_VOLUME drive={} short_differs={}",
            long.chars().take(2).collect::<String>(),
            short != long
        );
        assert_eq!(
            claude_project_key(&short),
            claude_project_key_in(&long, true, false)
        );
        assert_eq!(
            claude_project_key(&long),
            claude_project_key_in(&long, true, false)
        );
    }
}

/// `.claude.json` の `projects` のうち信頼済みの鍵
fn trusted_keys(cfg: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(cfg.join(".claude.json")).unwrap_or_default();
    let root: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    root["projects"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, v)| v["hasTrustDialogAccepted"] == json!(true))
        .map(|(k, _)| k.clone())
        .collect()
}

// --- 本物の claude ------------------------------------------------------------

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!(
            "{name} が無い。このテストは scripts/windows/test-claude-mod-1886.ps1 が砂場を用意してから --ignored で回す"
        )
    })
}

struct Given {
    claude: PathBuf,
    home: PathBuf,
    api_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Launch {
    /// ペインの command = claude.exe
    Direct,
    /// ペインの PowerShell 5.1 から打つ（worker の spawn と同じく、シェルへ起動のコマンドを送る形）
    ViaShell,
}

/// オンボーディング済み・ダミーの鍵を承認済みにした `.claude.json`（信頼は書かない）
fn onboarded_config() -> Value {
    let tail: String = DUMMY_KEY
        .chars()
        .rev()
        .take(20)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    json!({
        "hasCompletedOnboarding": true,
        "theme": "dark",
        "customApiKeyResponses": { "approved": [tail], "rejected": [] },
    })
}

fn fresh_config(dirs: &Dirs, name: &str) -> PathBuf {
    let cfg = Path::new(&dirs.long_root).join(format!("cfg {name}"));
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join(".claude.json"),
        serde_json::to_string_pretty(&onboarded_config()).unwrap(),
    )
    .unwrap();
    cfg
}

fn windows_powershell_module_path() -> String {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let program_files =
        std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    format!(
        "{program_files}\\WindowsPowerShell\\Modules;{root}\\system32\\WindowsPowerShell\\v1.0\\Modules"
    )
}

/// claude を砂場へ閉じ込める env（書き込み先・偽の API）
fn sandbox_env(given: &Given, cfg: &Path, pwd_out: &Path) -> Vec<(String, String)> {
    let home = given.home.display().to_string();
    vec![
        ("CLAUDE_CONFIG_DIR".into(), cfg.display().to_string()),
        ("USERPROFILE".into(), home.clone()),
        ("HOME".into(), home),
        (
            "APPDATA".into(),
            given
                .home
                .join("AppData")
                .join("Roaming")
                .display()
                .to_string(),
        ),
        (
            "LOCALAPPDATA".into(),
            given
                .home
                .join("AppData")
                .join("Local")
                .display()
                .to_string(),
        ),
        ("ANTHROPIC_API_KEY".into(), DUMMY_KEY.into()),
        ("ANTHROPIC_BASE_URL".into(), given.api_url.clone()),
        ("DISABLE_AUTOUPDATER".into(), "1".into()),
        (
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into(),
        ),
        (
            "TAKO_2843_CLAUDE".into(),
            given.claude.display().to_string(),
        ),
        ("TAKO_2843_PWD_OUT".into(), pwd_out.display().to_string()),
        // テストは pwsh 7 から起きるので、ペインの PowerShell 5.1 の標準モジュールの置き場へ戻す（#1886 と同じ）
        ("PSModulePath".into(), windows_powershell_module_path()),
    ]
}

/// 対話のペイン（ConPTY）
struct Pane {
    session: TerminalSession,
    rx: futures::channel::mpsc::UnboundedReceiver<tako_core::terminal::SessionEvent>,
    exited: bool,
    screens: Vec<String>,
}

impl Pane {
    fn spawn(given: &Given, cwd: &str, launch: Launch, env: Vec<(String, String)>) -> Self {
        let command = match launch {
            Launch::Direct => SpawnCommand {
                program: given.claude.display().to_string(),
                args: vec!["--permission-mode".into(), "default".into()],
            },
            Launch::ViaShell => SpawnCommand {
                program: PathBuf::from(
                    std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()),
                )
                .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
                .display()
                .to_string(),
                args: vec!["-NoLogo".into(), "-NoProfile".into()],
            },
        };
        let (session, rx) = TerminalSession::spawn(
            120,
            40,
            SpawnOptions {
                command: Some(command),
                cwd: Some(PathBuf::from(cwd)),
                env,
                scrollback_lines: None,
            },
        )
        .expect("ConPTY のペインを起動できること");
        Self {
            session,
            rx,
            exited: false,
            screens: Vec::new(),
        }
    }

    fn pump(&mut self) -> String {
        while let Ok(ev) = self.rx.try_recv() {
            if let Some(SessionNotice::Exited) = self.session.process_event(ev) {
                self.exited = true;
            }
        }
        let screen = self.session.visible_lines().join("\n");
        if !screen.trim().is_empty() && self.screens.last() != Some(&screen) {
            self.screens.push(screen.clone());
            if self.screens.len() > 6 {
                self.screens.remove(0);
            }
        }
        screen
    }

    fn dump(&self, label: &str, dirs: &Dirs) {
        println!(
            "TAKO2843_PANE {label} exited={} screens={}",
            self.exited,
            self.screens.len()
        );
        for (i, screen) in self.screens.iter().enumerate() {
            let body: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
            println!(
                "TAKO2843_SCREEN {label} #{i}:\n{}",
                dirs.mask(&body.join("\n"))
            );
        }
    }

    fn wait(&mut self, span: Duration) {
        let until = Instant::now() + span;
        while Instant::now() < until {
            self.pump();
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    /// 文字を打ってから Enter を分けて送る（claude_tui の作法）
    fn type_line(&mut self, text: &str) {
        self.session.write(text.as_bytes().to_vec());
        self.wait(Duration::from_millis(400));
        self.session.write(b"\r".to_vec());
    }
}

/// 起動の後に見えたもの
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// 信頼の確認が出た
    TrustDialog,
    /// 確認なしで入力欄まで来た
    Input,
    /// どちらも見えないまま尽きた・終わった
    Nothing,
}

fn is_trust_dialog(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    lower.contains("trust") && (lower.contains("folder") || lower.contains("files"))
}

fn is_input(screen: &str) -> bool {
    screen.contains('\u{276F}') && screen.to_lowercase().contains("claude code v")
}

/// 短い形の cwd のペインで claude を起こし、信頼の確認が出るか入力欄まで来るかを見る。
/// シェル経由の回はペインの PowerShell の位置（`$PWD` と プロセスの current directory）も控える
fn launch_and_observe(
    given: &Given,
    dirs: &Dirs,
    label: &str,
    cfg: &Path,
    launch: Launch,
) -> (Pane, Seen, Option<String>) {
    let pwd_out = Path::new(&dirs.long_root).join(format!("pwd {label}.txt"));
    let env = sandbox_env(given, cfg, &pwd_out);
    let mut pane = Pane::spawn(given, &dirs.short_work, launch, env);
    let mut pwd = None;
    if launch == Launch::ViaShell {
        let deadline = Instant::now() + STATE_WAIT;
        while !pane.pump().contains("PS ") {
            if Instant::now() >= deadline || pane.exited {
                pane.dump(label, dirs);
                panic!("[#2843 {label}] ペインの PowerShell のプロンプトが出ない");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // 打つのは ASCII だけ（値は env から読む）
        pane.type_line(
            "[IO.File]::WriteAllText($env:TAKO_2843_PWD_OUT, ($PWD.ProviderPath + [char]10 + [Environment]::CurrentDirectory), (New-Object Text.UTF8Encoding $false))",
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while !pwd_out.is_file() && Instant::now() < deadline {
            pane.wait(Duration::from_millis(100));
        }
        pwd = std::fs::read_to_string(&pwd_out).ok();
        pane.type_line("& $env:TAKO_2843_CLAUDE --permission-mode default");
    }
    let deadline = Instant::now() + STATE_WAIT;
    let seen = loop {
        let screen = pane.pump();
        if is_trust_dialog(&screen) {
            break Seen::TrustDialog;
        }
        if is_input(&screen) {
            break Seen::Input;
        }
        if Instant::now() >= deadline || pane.exited {
            pane.dump(label, dirs);
            break Seen::Nothing;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    (pane, seen, pwd)
}

/// 信頼の確認で「信頼する」を選んで確定する（選択の印 `❯` が Yes の行に来るまで ↓ を送る）
fn accept_trust(pane: &mut Pane, dirs: &Dirs, label: &str) -> bool {
    pane.wait(Duration::from_millis(500));
    for _ in 0..4 {
        let screen = pane.pump();
        let cursor = screen
            .lines()
            .find(|l| l.contains('\u{276F}'))
            .unwrap_or_default()
            .to_lowercase();
        if cursor.contains("yes") {
            pane.session.write(b"\r".to_vec());
            pane.wait(Duration::from_millis(800));
            return true;
        }
        pane.session.write(b"\x1b[B".to_vec());
        pane.wait(Duration::from_millis(400));
    }
    pane.dump(&format!("{label} accept"), dirs);
    false
}

/// 1. claude が引く鍵の実測: 信頼を書いていない設定 dir で起こし、確認を通したときに claude が書く鍵
fn claude_own_key(
    given: &Given,
    dirs: &Dirs,
    launch: Launch,
) -> Result<(String, Option<String>), String> {
    let label = format!("own-{launch:?}").to_lowercase();
    let cfg = fresh_config(dirs, &label);
    let (mut pane, seen, pwd) = launch_and_observe(given, dirs, &label, &cfg, launch);
    if seen != Seen::TrustDialog {
        return Err(format!(
            "信頼を書いていない設定 dir で確認が出ない（seen={seen:?}）"
        ));
    }
    let shown: Vec<String> = pane
        .pump()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| dirs.mask(l))
        .collect();
    println!("TAKO2843_DIALOG {label}:\n{}", shown.join("\n"));
    if !accept_trust(&mut pane, dirs, &label) {
        return Err("信頼の確認で Yes の行を選べない".into());
    }
    let deadline = Instant::now() + STATE_WAIT;
    let keys = loop {
        pane.pump();
        let keys = trusted_keys(&cfg);
        if !keys.is_empty() {
            break keys;
        }
        if Instant::now() >= deadline || pane.exited {
            pane.dump(&label, dirs);
            return Err("確認を通しても claude が信頼の鍵を書かない".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    drop(pane);
    for k in &keys {
        println!(
            "TAKO2843_CLAUDE_KEY launch={launch:?} form={} key=[{}]",
            dirs.form_of(k),
            dirs.mask(k)
        );
    }
    if keys.len() != 1 {
        return Err(format!("claude が信頼の鍵を {} 本書いた", keys.len()));
    }
    Ok((keys[0].clone(), pwd))
}

/// 2 / 3. 事前信頼を書いた設定 dir で起こし、確認が出るかを見る
fn dialog_with(given: &Given, dirs: &Dirs, label: &str, cfg: &Path, launch: Launch) -> Seen {
    let (pane, seen, _) = launch_and_observe(given, dirs, label, cfg, launch);
    drop(pane);
    println!("TAKO2843_TRUST arm={label} launch={launch:?} seen={seen:?}");
    seen
}

#[test]
#[ignore = "本物の claude を起こす。scripts/windows/test-claude-mod-1886.ps1 が砂場を用意してから名指しで回す"]
fn issue2843_windows_claude_trusts_short_cwd() {
    let given = Given {
        claude: PathBuf::from(required(CLAUDE_EXE_ENV)),
        home: PathBuf::from(required(HOME_ENV)),
        api_url: required(API_URL_ENV),
    };
    assert!(given.claude.is_file(), "claude.exe が無い");
    let dirs = Dirs::new();
    println!(
        "TAKO2843_PATHS short_differs={} long=[{}] short=[{}] long_key=[{}] short_key=[{}]",
        dirs.has_short(),
        dirs.mask(&dirs.long_work),
        dirs.mask(&dirs.short_work),
        dirs.mask(&dirs.long_key()),
        dirs.mask(&dirs.short_key())
    );
    assert!(
        dirs.has_short(),
        "[#2843] 短い形の作業 dir を作れない（TEMP が短い形でなく、このボリュームで 8.3 の名前も作られない）= 測れない"
    );
    let mut failures = Vec::new();
    let mut verdict = Vec::new();

    // 1. claude が引く鍵（直起動とシェル経由）
    for launch in [Launch::Direct, Launch::ViaShell] {
        match claude_own_key(&given, &dirs, launch) {
            Ok((key, pwd)) => {
                if let Some(pwd) = pwd {
                    let mut lines = pwd.lines();
                    let ps = lines.next().unwrap_or_default();
                    let cd = lines.next().unwrap_or_default();
                    println!(
                        "TAKO2843_SHELL_PWD provider=[{}] provider_form={} current_directory=[{}] current_directory_form={}",
                        dirs.mask(ps),
                        dirs.form_of(&claude_project_key_in(ps, true, false)),
                        dirs.mask(cd),
                        dirs.form_of(&claude_project_key_in(cd, true, false))
                    );
                }
                let form = dirs.form_of(&key);
                verdict.push(format!("claude_key_{launch:?}={form}").to_lowercase());
                if form != "long" {
                    failures.push(format!(
                        "[#2843 own-{launch:?}] 短い形の cwd から起こした claude が書いた鍵が長い形でない（form={form}）= tako の鍵を長い形へ寄せる前提が崩れている"
                    ));
                }
            }
            Err(why) => failures.push(format!("[#2843 own-{launch:?}] {why}")),
        }
    }

    // 2. 製品の事前信頼（短い形の cwd を渡す = spawn の cwd が短い形のとき）
    for launch in [Launch::Direct, Launch::ViaShell] {
        let label = format!("fixed-{launch:?}").to_lowercase();
        let cfg = fresh_config(&dirs, &label);
        ensure_trusted_in(Some(&cfg.display().to_string()), &dirs.short_work)
            .expect("製品の事前信頼を書けること");
        let written = trusted_keys(&cfg);
        println!(
            "TAKO2843_WRITTEN arm={label} keys={:?}",
            written.iter().map(|k| dirs.form_of(k)).collect::<Vec<_>>()
        );
        let seen = dialog_with(&given, &dirs, &label, &cfg, launch);
        verdict
            .push(format!("fixed_dialog_{launch:?}={}", seen == Seen::TrustDialog).to_lowercase());
        if seen != Seen::Input {
            failures.push(format!(
                "[#2843 {label}] 短い形の cwd を渡して事前信頼を書いたのに入力欄まで来ない（seen={seen:?}。TrustDialog なら鍵が claude の引く形とずれている = 短い形を長い形へ戻す段が効いていない）"
            ));
        }
    }

    // 3. 修正前の答え（短い形をそのまま鍵にした形）だけでは確認が出る = 2 に検出力がある
    for launch in [Launch::Direct, Launch::ViaShell] {
        let label = format!("legacy-{launch:?}").to_lowercase();
        let cfg = fresh_config(&dirs, &label);
        let mut config = onboarded_config();
        config["projects"] = json!({ dirs.short_key(): { "hasTrustDialogAccepted": true } });
        std::fs::write(
            cfg.join(".claude.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
        let seen = dialog_with(&given, &dirs, &label, &cfg, launch);
        verdict
            .push(format!("legacy_dialog_{launch:?}={}", seen == Seen::TrustDialog).to_lowercase());
        if seen != Seen::TrustDialog {
            failures.push(format!(
                "[#2843 {label}] 修正前の答え（短い形の鍵）だけでも確認が出ない（seen={seen:?}）= 2 の検出力が無い"
            ));
        }
    }

    // 長い形の鍵は #1886 の答えのまま
    if claude_project_key(&dirs.long_work) != dirs.long_key() {
        failures.push("[#2843 long] 長い形の作業 dir の鍵が #1886 の答えと違う".into());
    }

    println!("TAKO2843_VERDICT {}", verdict.join(" "));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
