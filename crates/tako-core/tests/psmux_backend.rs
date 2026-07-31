//! psmux を器とするバックエンドの統合テスト（#519 M2。**実バイナリを使う**）
//!
//! 単体テスト（`backend/psmux.rs` 内）は「tako がどう組み立てるか」を固定する。
//! こちらは **psmux が実際にそう振る舞うか**を確かめる。適合検証レポートの
//! 受け入れ条件 1〜7 のうち、実バイナリでしか裏の取れないものを担当する。
//!
//! psmux が無い環境（macOS / CI）ではスキップする。バイナリの場所は
//! `TAKO_PSMUX_BIN` か PATH 上の `psmux`。
//!
//! **ソケットは必ず隔離する**（`tako-m2test-<pid>`）。psmux の `kill-server` は
//! **`-L` を落とすと全ソケットのサーバーを殺す**（実測）ので、後始末は
//! 必ずソケット指定つきで行う。

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use tako_core::backend::{PsmuxBackend, SessionBackend, SessionRef};
use tako_core::terminal::{SpawnCommand, SpawnOptions};

/// テスト用の psmux バイナリ。無ければ `None`（= スキップ）
fn psmux_bin() -> Option<String> {
    if let Some(bin) = std::env::var("TAKO_PSMUX_BIN")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return runnable(&bin).then_some(bin);
    }
    runnable("psmux").then(|| "psmux".to_string())
}

fn runnable(bin: &str) -> bool {
    Command::new(bin)
        .arg("-V")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 隔離ソケット上の backend。Drop でサーバーごと落とす
struct Fixture {
    backend: PsmuxBackend,
    bin: String,
    socket: String,
    owner_dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Option<Self> {
        let bin = psmux_bin()?;
        let socket = format!("tako-m2test-{}-{tag}", std::process::id());
        let owner_dir =
            std::env::temp_dir().join(format!("tako-m2test-owners-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&owner_dir);
        Some(Self {
            backend: PsmuxBackend::with_parts(
                bin.clone(),
                "3.3.7".into(),
                socket.clone(),
                owner_dir.clone(),
            ),
            bin,
            socket,
            owner_dir,
        })
    }

    /// 隔離ソケット上での生 psmux 実行（psmux 側の生の挙動を観測する用）
    fn raw(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new(&self.bin)
            .args(["-L", &self.socket])
            .args(args)
            .output()
            .expect("psmux を実行できる");
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // **-L 必須**（省くと全ソケットのサーバーが死ぬ）
        let _ = Command::new(&self.bin)
            .args(["-L", &self.socket, "kill-server"])
            .output();
        let _ = std::fs::remove_dir_all(&self.owner_dir);
    }
}

fn session(name: &str) -> SessionRef {
    SessionRef::new(name).unwrap()
}

macro_rules! fixture {
    ($tag:expr) => {
        match Fixture::new($tag) {
            Some(f) => f,
            None => {
                eprintln!("skip: psmux が無い環境（TAKO_PSMUX_BIN / PATH）");
                return;
            }
        }
    };
}

/// **M2 の本命**（設計 §2.2 の B-1）: クライアント（tako 側の PTY）が死んでも
/// 器の中のシェルと画面内容が生き残り、同じ名前で開き直すと戻ってくる。
///
/// tmux 版の `セッションはクライアント切断後もattachで内容ごと戻る` と同じ形の検証を
/// psmux でやる。**これが成立しないと M2 に意味が無い**
#[test]
fn 器はクライアント切断後もattachで内容ごと戻る() {
    let f = fixture!("persist");
    let name = session("tako-m2persist01");
    let base = SpawnOptions {
        command: None,
        cwd: Some(std::env::temp_dir()),
        env: vec![],
    };

    // **rx を汲む**のが必須: psmux クライアントは端末クエリ（DA / DSR）への応答を待つ。
    // 捨てると応答待ちのまま描画が進まない（tmux の洪水テストと同じ理由）
    fn wait_for(
        session: &mut tako_core::TerminalSession,
        rx: &mut futures::channel::mpsc::UnboundedReceiver<tako_core::SessionEvent>,
        needle: &str,
        attempts: usize,
    ) -> bool {
        for _ in 0..attempts {
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
            if session
                .visible_lines()
                .iter()
                .any(|line| line.contains(needle))
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    // 入力のエコーと出力を区別するため、マーカーはシェルに組み立てさせる
    let marker_command: &[u8] = if cfg!(windows) {
        b"Write-Output ('TAKO-PSMUX' + '-OK')\r"
    } else {
        b"echo TAKO-PSMUX-'OK'\r"
    };

    // 1 回目: 器を作ってマーカーを出す
    let (mut first, mut rx1) =
        tako_core::TerminalSession::spawn(80, 24, f.backend.wrap_spawn(base.clone(), &name))
            .expect("psmux クライアントを spawn できる");
    // シェルが入力を受けられるようになるまで待つ。**固定 sleep では足りない**:
    // 並列テストで負荷がかかると pwsh の起動が遅れ、先頭の数文字が食われる
    // （実測: `Write-Output (AKO-PSMUX' …` のように打鍵が欠ける）。
    // プロンプトを待ってから打ち、それでも駄目なら打ち直す
    let prompt = if cfg!(windows) { "PS " } else { "$" };
    let mut delivered = false;
    for _ in 0..6 {
        if !wait_for(&mut first, &mut rx1, prompt, 60) {
            continue;
        }
        first.write(marker_command.to_vec());
        if wait_for(&mut first, &mut rx1, "TAKO-PSMUX-OK", 60) {
            delivered = true;
            break;
        }
    }
    assert!(
        delivered,
        "1 回目の器でマーカーが出力される。画面: {:?}",
        first.visible_lines().join("\n")
    );
    // クライアント破棄（tako 終了相当）。器はサーバー側に残る
    drop(first);
    std::thread::sleep(Duration::from_millis(800));
    assert!(
        f.backend.exists(&name),
        "クライアントが死んでも器は生きている"
    );

    // 2 回目: 同じ名前で開き直すと画面内容ごと戻る
    let (mut second, mut rx2) =
        tako_core::TerminalSession::spawn(80, 24, f.backend.wrap_spawn(base, &name))
            .expect("再 attach の psmux クライアントを spawn できる");
    assert!(
        wait_for(&mut second, &mut rx2, "TAKO-PSMUX-OK", 200),
        "再 attach で画面内容（scrollback）が復元される。画面: {:?}",
        second.visible_lines().join("\n")
    );
    drop(second);
    let _ = f.backend.kill(&name);
}

/// **要件 1**: `=`（exact-match 接頭辞）を付けない kill が効く。
///
/// tmux 流用が不可能だった直接の理由がこれ。`kill-session -t =name` は psmux で
/// **3/3 失敗し、各 5.1 秒ブロックする**（適合検証）。ペインを閉じるたびに
/// 器と pwsh がリークし、close が 5 秒固まる状態になる
#[test]
fn killはイコール無しで即座に効く() {
    let f = fixture!("kill");
    let name = session("tako-m2kill00001");
    f.backend
        .wrap_spawn(SpawnOptions::default(), &name)
        .command
        .expect("wrap_spawn が起動コマンドを返す");
    // クライアントを立てずに器だけ作る（PTY を挟まない純粋な CLI 検証）
    let (ok, out) = f.raw(&["new-session", "-d", "-s", name.as_str()]);
    assert!(ok, "器を作れる: {out}");
    assert!(f.backend.exists(&name));

    let started = Instant::now();
    f.backend.kill(&name).expect("kill が成功する");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "kill が {elapsed:?} かかった（`=` 付きターゲットは 5.1 秒ブロックして失敗する）"
    );
    assert!(!f.backend.exists(&name), "kill 後は器が消えている");
}

/// `=` を外しても取り違えは起きない（前方一致で別の器を巻き込まない）ことの実測。
/// tako が払い出す名前は固定長なので本来ぶつからないが、psmux のターゲット解決が
/// 緩い方向へ変わったら気づけるようにしておく
#[test]
fn killは前方一致する別の器を巻き込まない() {
    let f = fixture!("prefix");
    let short = session("tako-m2prefix001");
    let long = session("tako-m2prefix0011");
    for name in [&short, &long] {
        let (ok, out) = f.raw(&["new-session", "-d", "-s", name.as_str()]);
        assert!(ok, "器 {name} を作れる: {out}");
    }
    f.backend.kill(&short).expect("kill が成功する");
    assert!(!f.backend.exists(&short));
    assert!(
        f.backend.exists(&long),
        "前方一致する別の器まで殺している（ターゲット解決が緩い）"
    );
    let _ = f.backend.kill(&long);
}

/// **要件 5**: conf が実際に適用され、`warm off` が効いている。
/// あわせて **psmux が知らないオプションを持ち込んでいない**ことも確かめる
/// （知らない行が 1 つでもあると `psmux: N config warning(s):` がペインに出る＝
/// ユーザーの画面が汚れる）
#[test]
fn confは警告なしで受理されwarm_offが効く() {
    let f = fixture!("conf");
    let Some(conf) = tako_core::backend::psmux::ensure_conf() else {
        eprintln!("skip: data_dir が無い環境");
        return;
    };
    let out = Command::new(&f.bin)
        .args(["-L", &f.socket, "-f"])
        .arg(&conf)
        .args(["new-session", "-d", "-s", "tako-m2conf00001"])
        .output()
        .expect("psmux を実行できる");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !text.contains("warning") && !text.contains("unknown option"),
        "conf に psmux が知らないオプションがある（ペインへ警告が出る）: {text}"
    );
    let (ok, warm) = f.raw(&["show-options", "-g", "warm"]);
    assert!(ok, "show-options が動く: {warm}");
    assert!(
        warm.contains("warm off"),
        "warm off が適用されていない（1 セッションあたり pwsh が 1 本余計に常駐する）: {warm}"
    );
    let (_, limit) = f.raw(&["show-options", "-g", "history-limit"]);
    assert!(
        limit.contains("10000"),
        "history-limit が反映されない: {limit}"
    );
}

/// **要件 2**: `show-environment` が名前指定を無視して全変数を返しても、
/// 目的の 1 本を取り出せる（`-e` で注入した TAKO_PANE_ID が読める）
#[test]
fn セッション環境変数を全変数出力から取り出せる() {
    let f = fixture!("env");
    let name = session("tako-m2env000001");
    let (ok, out) = f.raw(&[
        "new-session",
        "-d",
        "-s",
        name.as_str(),
        "-e",
        "TAKO_PANE_ID=7",
        "-e",
        "TAKO_TAB_ID=3",
    ]);
    assert!(ok, "器を作れる: {out}");

    // psmux 側は名前指定を無視して全変数を返す（この前提が変わっても実装は通る）
    let (_, raw) = f.raw(&["show-environment", "-t", name.as_str(), "TAKO_PANE_ID"]);
    eprintln!("show-environment の生出力:\n{raw}");

    assert_eq!(
        f.backend.session_env(&name, "TAKO_PANE_ID"),
        Some("7".to_string())
    );
    assert_eq!(
        f.backend.session_env(&name, "TAKO_TAB_ID"),
        Some("3".to_string())
    );
    assert_eq!(f.backend.session_env(&name, "TAKO_NOT_SET"), None);

    // 書き戻し（orphan 復元後の pane ID 更新。#210）も効く
    f.backend.set_session_env(&name, "TAKO_PANE_ID", "42");
    assert_eq!(
        f.backend.session_env(&name, "TAKO_PANE_ID"),
        Some("42".to_string())
    );
    let _ = f.backend.kill(&name);
}

/// **要件 4**: psmux は実在しない Unix 風 tty を返すので、境界は `None` を返す。
/// 偽の tty を信じると listen ポート検知（FR-2.4.2）が誤った突き合わせをする
#[test]
fn pane_ttyは偽のttyを外へ出さない() {
    let f = fixture!("tty");
    let name = session("tako-m2tty000001");
    let (ok, out) = f.raw(&["new-session", "-d", "-s", name.as_str()]);
    assert!(ok, "器を作れる: {out}");

    let (_, raw) = f.raw(&["list-panes", "-t", name.as_str(), "-F", "#{pane_tty}"]);
    eprintln!("psmux が申告する tty: {}", raw.trim());
    assert!(
        f.backend.pane_tty(&name).is_none(),
        "境界は偽の tty を外へ出さない（psmux の申告: {}）",
        raw.trim()
    );
    let _ = f.backend.kill(&name);
}

/// **要件 3 のカナリア**: `#{history_bytes}` は空のまま返る。
/// この前提が変わったら（psmux が実装したら）役割 B の再検討ができる。
/// 実装はこの値に依存していない（`detached()` が `None`）ので、**失敗ではなく記録**にする
#[test]
fn history_bytesは空のままというカナリア() {
    let f = fixture!("hist");
    let name = session("tako-m2hist00001");
    let (ok, out) = f.raw(&["new-session", "-d", "-s", name.as_str()]);
    assert!(ok, "器を作れる: {out}");
    let (_, probe) = f.raw(&[
        "list-panes",
        "-a",
        "-F",
        "#{session_name}\t#{history_size}\t#{history_limit}\t#{history_bytes}\t#{alternate_on}",
    ]);
    let bytes_empty = probe
        .lines()
        .filter(|l| l.contains(name.as_str()))
        .any(|l| l.split('\t').nth(3).is_some_and(|b| b.trim().is_empty()));
    eprintln!(
        "history_bytes が空 = {bytes_empty}（true 想定。false になったら psmux が実装した \
         = pane_log の probe 経路を検討できる）。生出力: {}",
        probe.trim()
    );
    // 依存していないことの確認（役割 B を持たない）
    assert!(f.backend.detached().is_none());
    let _ = f.backend.kill(&name);
}

/// **要件 6**: 起動時プローブが実バイナリで通る（器の最小契約 3 つ）。
/// 未検証バージョンを掴んだときはこの結果で採否を決める
#[test]
fn 起動時プローブが実バイナリで通る() {
    let Some(bin) = psmux_bin() else {
        eprintln!("skip: psmux が無い環境");
        return;
    };
    tako_core::backend::psmux::behavior_probe(&bin).expect("器の最小契約を満たす");
}

/// 一覧・存在確認・cwd・残骸判定の往復（`=` なしターゲットで全部通ること）
#[test]
fn 一覧と存在確認とcwdが往復する() {
    let f = fixture!("list");
    let name = session("tako-m2list00001");
    let cwd = std::env::temp_dir();
    let (ok, out) = f.raw(&[
        "new-session",
        "-d",
        "-s",
        name.as_str(),
        "-c",
        &cwd.display().to_string(),
    ]);
    assert!(ok, "器を作れる: {out}");

    assert!(f.backend.exists(&name));
    let listed = f.backend.list();
    assert!(
        listed.iter().any(|i| i.session == name),
        "一覧に出る: {listed:?}"
    );
    let reported = f.backend.session_cwd(&name).expect("cwd を取れる");
    assert!(
        reported.to_lowercase().replace('/', "\\")
            == cwd.display().to_string().to_lowercase().replace('/', "\\")
            || reported.to_lowercase().starts_with(
                &cwd.display()
                    .to_string()
                    .to_lowercase()
                    .trim_end_matches('\\')
                    .to_string()
            ),
        "cwd が一致する: {reported} / {}",
        cwd.display()
    );

    // protected に入れた器は残骸扱いしない
    let protected: HashSet<SessionRef> = [name.clone()].into_iter().collect();
    assert!(f.backend.orphans(&protected).is_empty());
    assert!(f.backend.cleanup_orphans(&protected, None).is_empty());
    // protected から外れれば残骸として掃除される
    assert_eq!(
        f.backend.cleanup_orphans(&HashSet::new(), None),
        vec![name.clone()]
    );
    assert!(!f.backend.exists(&name), "cleanup で器が消える");
}

/// 明示コマンドつきの spawn が psmux の引数パーサを通る（`inner_command` の実地確認）。
/// バックスラッシュを含む Windows パスを引用してしまうと **起動に失敗する**ので、
/// 組み立て規則が壊れたらここで落ちる
#[test]
fn 明示コマンドつきの器が起動する() {
    let f = fixture!("cmd");
    let name = session("tako-m2cmd000001");
    let program = if cfg!(windows) {
        format!(
            "{}\\System32\\cmd.exe",
            std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())
        )
    } else {
        "/bin/sh".to_string()
    };
    let args = if cfg!(windows) {
        vec![
            "/c".to_string(),
            "echo TAKO-M2-CMD-OK & ping -n 30 127.0.0.1 > NUL".to_string(),
        ]
    } else {
        vec![
            "-c".to_string(),
            "echo TAKO-M2-CMD-OK; sleep 30".to_string(),
        ]
    };
    let options = SpawnOptions {
        command: Some(SpawnCommand { program, args }),
        cwd: None,
        env: vec![],
    };
    let wrapped = f.backend.wrap_spawn(options, &name);
    let cmd = wrapped.command.expect("起動コマンドが組まれる");
    // クライアントは PTY 無しでは即終了しうるので、器の中身だけを見る
    let child = Command::new(&cmd.program).args(&cmd.args).spawn();
    // 並列テストで負荷がかかると器の起動は数秒ずれる。固定 sleep ではなくポーリングで待つ
    let mut captured = String::new();
    for _ in 0..60 {
        std::thread::sleep(Duration::from_millis(500));
        captured = f.raw(&["capture-pane", "-t", name.as_str(), "-p"]).1;
        if captured.contains("TAKO-M2-CMD-OK") {
            break;
        }
    }
    if let Ok(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(
        captured.contains("TAKO-M2-CMD-OK"),
        "明示コマンドが器の中で起動していない（引数の組み立て規則が壊れた）: {captured}"
    );
    let _ = f.backend.kill(&name);
}

// --- 役割 B の読み側（採取。#519） ---------------------------------------

/// セッションの履歴が `want` 行を超えるまで待つ（並列テストの負荷でずれるので固定 sleep にしない）
fn wait_for_history(f: &Fixture, name: &SessionRef, want: usize) -> usize {
    let mut history = 0;
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        history = f
            .backend
            .detached_capture()
            .and_then(|c| c.history_probe(name))
            .map(|p| p.history)
            .unwrap_or(0);
        if history >= want {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    history
}

/// **#519 の本命**: tako-app が保持していないセッションから画面と履歴を採れる。
///
/// 長い出力（200 行）と日本語の折返しを含めるのは、この経路が
/// `orchestrator report` の材料そのものだから（CJK が壊れると報告が読めなくなる）。
/// **psmux は `-J` を無視する**ので折返しは行のまま残る。中身が落ちないことを確かめる
#[test]
fn 保持していないセッションの画面と履歴を採れる() {
    let f = fixture!("capture");
    let name = session("tako-m2cap000001");
    let (ok, out) = f.raw(&[
        "new-session",
        "-d",
        "-s",
        name.as_str(),
        "-x",
        "60",
        "-y",
        "20",
    ]);
    assert!(ok, "器を作れる: {out}");

    let capture = f
        .backend
        .detached_capture()
        .expect("psmux は採取の到達手段を持つ");
    assert!(
        f.backend.detached().is_none(),
        "送出の到達手段は持たない（信頼できないものを申告しない）"
    );

    // シェルが入力を受けられるようになるまで待ってから流し込む
    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        let screen = capture.capture_screen(&name).unwrap_or_default().join("\n");
        if screen.contains(if cfg!(windows) { "PS " } else { "$" }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let long_output = if cfg!(windows) {
        "1..200 | ForEach-Object { \"CAP-LINE-$_\" }"
    } else {
        "for i in $(seq 1 200); do echo CAP-LINE-$i; done"
    };
    let (ok, out) = f.raw(&["send-keys", "-t", name.as_str(), long_output, "Enter"]);
    assert!(ok, "出力を積める: {out}");
    let history = wait_for_history(&f, &name, 175);
    assert!(history >= 175, "履歴が積まれている: {history}");

    // 日本語の長行（幅 60 のペインなので必ず折り返す）。
    // **非 ASCII を psmux の argv へ渡さない**（Windows のコンソール既定コードページで
    // 化けるのは別件 #686 系の話で、ここで検証したいのは採取の側）ため、
    // 文字はシェルにコードポイントから組み立てさせる
    let jp = if cfg!(windows) {
        "Write-Output ((-join ([char]0x3042,[char]0x3044,[char]0x3046,[char]0x3048,[char]0x304A)) * 24)"
    } else {
        "printf '\\u3042\\u3044\\u3046\\u3048\\u304a%.0s' $(seq 1 24); echo"
    };
    f.raw(&["send-keys", "-t", name.as_str(), jp, "Enter"]);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if capture
            .capture_history_joined(&name, 400)
            .unwrap_or_default()
            .contains("あいうえお")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    // 可視画面: 画面 1 枚ぶんだけが返る
    let screen = capture.capture_screen(&name).expect("可視画面を採れる");
    assert!(
        (1..=25).contains(&screen.len()),
        "可視画面はペイン 1 枚ぶん: {} 行",
        screen.len()
    );

    // 履歴: `capture_history` は **履歴だけ**（`-E -1`）を返す。
    // 末尾 20 行前後は可視画面に残っているのでここには出ない（tmux と同じ意味論で、
    // `#{history_size}` の行数と 1:1 に保つため pane_log がこの形を要求する）
    let lines = capture.capture_history(&name, 400).expect("履歴を採れる");
    for needle in ["CAP-LINE-1", "CAP-LINE-100"] {
        assert!(
            lines.iter().any(|l| l.trim_end() == needle),
            "{needle} が履歴に無い（採取が途中で切れている）"
        );
    }
    assert!(
        !lines.iter().any(|l| l.trim_end() == "CAP-LINE-200"),
        "可視画面の行は履歴に混ざらない"
    );

    // 1 本のテキスト（report の第 1 層）: **履歴 + 可視画面**が端から端まで入る
    let joined = capture
        .capture_history_joined(&name, 400)
        .expect("結合テキストを採れる");
    assert!(joined.contains("CAP-LINE-1\n"), "履歴の先頭から入る");
    assert!(joined.contains("CAP-LINE-200"), "可視画面の末尾まで入る");
    // **日本語の折返し**: 幅 60 のペインで 120 文字（240 桁）ぶんを出しているので
    // 必ず複数行に割れる。psmux は `-J` を無視するので割れたまま出るが、
    // **中身は 1 文字も落ちない**ことをここで固定する
    let jp_lines: Vec<&str> = joined
        .lines()
        .filter(|l| l.contains('あ') && !l.contains("Write-Output"))
        .collect();
    assert!(
        jp_lines.len() >= 2,
        "折返しが行として割れている（psmux は -J で結合しない）: {jp_lines:?}"
    );
    let jp_chars: usize = jp_lines
        .iter()
        .map(|l| l.chars().filter(|c| *c == 'あ').count())
        .sum();
    assert_eq!(
        jp_chars, 24,
        "日本語 24 回ぶんが折返しをまたいで全部入っている:\n{joined}"
    );

    // probe: 行数は取れる。`#{history_bytes}` は空なので bytes は 0 に倒れる
    let probe = capture.history_probe(&name).expect("probe が返る");
    assert!(probe.history >= 175, "履歴行数: {}", probe.history);
    assert!(probe.limit > 0, "history-limit: {}", probe.limit);
    assert!(!probe.alternate, "alt screen ではない");
    assert_eq!(probe.bytes, 0, "psmux では観測できない（0 へ倒す）");

    // 一括 probe にも同じセッションが載る（#369 の probe 一括化）
    let batch = capture.history_probe_batch();
    let found = batch
        .iter()
        .find(|(s, _)| *s == name)
        .expect("一括 probe に載る");
    assert!(found.1.history >= 175);

    // スクロール位置（#687）: copy mode の外は 0 / 非モード
    let scroll = capture.scroll_probe(&name).expect("位置を読める");
    assert_eq!(scroll.position, 0, "copy mode の外は最下部");
    assert!(!scroll.in_mode);
    assert!(scroll.history >= 175);

    let _ = f.backend.kill(&name);
}

/// **#687 の読み側**: 器を copy mode へ入れると `scroll_probe` がその位置を返す。
///
/// ここは器の CLI（`copy-mode` / `send-keys -X`）で位置を作るが、それは
/// **テストが器を動かしているだけ**で、本番の tako は in-process の PTY へ
/// ホイール報告を書く（読み取り専用の約束は `backend/psmux.rs` の番犬テストが守る）
#[test]
fn copy_mode_の位置を読み戻せる() {
    let f = fixture!("scrollpos");
    let name = session("tako-m2scr000001");
    let (ok, out) = f.raw(&[
        "new-session",
        "-d",
        "-s",
        name.as_str(),
        "-x",
        "60",
        "-y",
        "20",
    ]);
    assert!(ok, "器を作れる: {out}");
    let capture = f.backend.detached_capture().expect("採取の到達手段");

    let deadline = Instant::now() + Duration::from_secs(45);
    while Instant::now() < deadline {
        let screen = capture.capture_screen(&name).unwrap_or_default().join("\n");
        if screen.contains(if cfg!(windows) { "PS " } else { "$" }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let long_output = if cfg!(windows) {
        "1..150 | ForEach-Object { \"SCR-$_\" }"
    } else {
        "for i in $(seq 1 150); do echo SCR-$i; done"
    };
    f.raw(&["send-keys", "-t", name.as_str(), long_output, "Enter"]);
    let history = wait_for_history(&f, &name, 125);
    assert!(history >= 125, "履歴が積まれている: {history}");

    f.raw(&["copy-mode", "-t", name.as_str()]);
    for _ in 0..7 {
        f.raw(&["send-keys", "-t", name.as_str(), "-X", "scroll-up"]);
    }
    let scrolled = capture.scroll_probe(&name).expect("位置を読める");
    assert!(
        scrolled.in_mode,
        "copy mode に入っていることが読める: {scrolled:?}"
    );
    assert_eq!(
        scrolled.position, 7,
        "遡った行数がそのまま読める（tako はこれを応答の offset に載せる）"
    );
    assert!(scrolled.history >= 125);

    // 抜ければ最下部へ戻る
    f.raw(&["send-keys", "-t", name.as_str(), "-X", "cancel"]);
    let back = capture.scroll_probe(&name).expect("位置を読める");
    assert_eq!(back.position, 0);
    assert!(!back.in_mode);

    let _ = f.backend.kill(&name);
}
