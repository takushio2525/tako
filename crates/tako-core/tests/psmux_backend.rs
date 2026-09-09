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

/// psmux を叩く経路は**すべて期限つき**（#1271）。素の `Command` で待つと
/// 返らない回にテストプロセスごと固まり、`Fixture::drop` が 1 つも走らずに
/// 器（サーバー）と中の pwsh が残る
#[path = "common/psmux_ctl.rs"]
mod psmux_ctl;

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
    psmux_ctl::probe(bin)
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
        let mut all: Vec<&str> = vec!["-L", &self.socket];
        all.extend_from_slice(args);
        // **期限つき**（#1271）。`send-keys -X scroll-up` が返らない実測があり、
        // ここで無限に待つと後始末がまるごと走らなくなる
        let ran = psmux_ctl::psmux(&self.bin, &all, psmux_ctl::BASE);
        (ran.ok, ran.text)
    }

    /// セッションの接続クライアント数（`None` = そのセッションがもう無い）。
    ///
    /// 「クライアントが死んでも器は生きている」を**固定 sleep ではなく状態**で
    /// 見るための材料（#1114）。psmux 3.3.7 が `#{session_attached}` を持つことは
    /// 実機で確認済み
    fn attached(&self, name: &SessionRef) -> Option<u32> {
        let (ok, text) = self.raw(&[
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_attached}",
        ]);
        if !ok {
            return None;
        }
        text.lines()
            .filter_map(|l| l.split_once('\t'))
            .find(|(n, _)| n.trim() == name.as_str())
            .and_then(|(_, v)| v.trim().parse::<u32>().ok())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // **-L 必須**（省くと全ソケットのサーバーが死ぬ）。期限つきの待ちと
        // pid 指定の止めは `psmux_ctl::kill_server` の 1 実装（#1271）
        psmux_ctl::kill_server(&self.bin, &self.socket);
        let _ = std::fs::remove_dir_all(&self.owner_dir);
    }
}

fn session(name: &str) -> SessionRef {
    SessionRef::new(name).unwrap()
}

// ── 状態待ち（#1114）─────────────────────────────────────────────────────
//
// **固定回数で待たない**。旧実装は「100ms × N 回」を 1 単位に最大 6 周する形で、
// 予算が固定（プロンプト待ち 60 回 ≒ 6 秒 × 6 周）だった。実測（Windows 実機・
// このバイナリを 4 多重 + プロセス生成負荷 12）では器の中の pwsh がプロンプトを
// 出すまでに **20〜34 秒**かかり、6 周すべてをプロンプト待ちで使い切って FAILED に
// なる（9/16。落ちた画面は `PS …> Write-O` = 打鍵のエコーが途中で切れた形）。
//
// 直し方は `.agent/conventions.md`「セルフテストの待ち条件の書き方」と同じで、
// **状態で待つ + 上限を `state_wait_budget` で伸ばす**（伸ばすだけ・4 倍で打ち切り）。
// 予算切れの診断には「待っていたもの / 届いたもの」を必ず出す。

/// 画面のポーリング間隔（PTY のイベントを汲むだけなので安い）
const POLL: Duration = Duration::from_millis(100);

/// 器の状態のポーリング間隔。**1 回ごとに psmux の CLI プロセスが立つ**ので、
/// 画面と同じ 100ms で回すと 18 本並列のこのバイナリが自分で負荷を作ってしまう
/// （旧実装の 200〜500ms と同じ粒度に留める）
const STATE_POLL: Duration = Duration::from_millis(250);

/// マーカーが器の中のシェルから出るまでの素の上限。
/// 旧の実効上限（6 周 × (プロンプト 6 秒 + マーカー 6 秒) ≒ 72 秒）より**広く**採る
/// ＝ 空いている機でも新旧が同じにならず、A/B が取れる（#771 と同じ理由）
const MARKER_BASE: Duration = Duration::from_secs(90);

/// 打ち直しの間隔。旧の 1 周ぶん（≒6 秒）より少しだけ長く採る
const RESEND_BASE: Duration = Duration::from_secs(10);

/// クライアント切断が器へ届くまでの素の上限（実測は 1 秒未満）
const DETACH_BASE: Duration = Duration::from_secs(15);

/// 再 attach で画面が戻るまでの素の上限（旧 20 秒。実測は 1.3 秒未満）
const REATTACH_BASE: Duration = Duration::from_secs(30);

/// この機の混み具合 / 状態待ちの上限。**器を叩く経路と同じ 1 実装**を通す
/// （`psmux_ctl` 側が `tako_core::wait_budget` へ委譲する。政策を 2 か所に書かない）
use psmux_ctl::{budget_for, busy};

/// 旧の固定窓を再現するアーム（A/B 用）。`TAKO_1114_LEGACY=1`
fn legacy_1114() -> bool {
    std::env::var("TAKO_1114_LEGACY").is_ok_and(|v| v == "1")
}

/// 待ちの結果。診断に**待っていたもの / 届いたもの**を必ず出すための材料
#[derive(Debug)]
struct Waited {
    ok: bool,
    waited: Duration,
    budget: Duration,
    polls: u32,
    /// 打ち直した回数（打たない待ちでは 0）
    resends: u32,
}

impl Waited {
    /// 予算切れの 1 行。`got` は「実際に届いていたもの」（画面・器の状態）
    fn diag(&self, want: &str, got: &str) -> String {
        format!(
            "TAKO_1114_WAIT 待っていたもの={want:?} 届いたもの={got:?} \
             ok={} waited={:.1}s budget={:.1}s polls={} resends={} load={}",
            self.ok,
            self.waited.as_secs_f64(),
            self.budget.as_secs_f64(),
            self.polls,
            self.resends,
            busy()
                .map(|b| format!("{b:.2}"))
                .unwrap_or_else(|| "unknown".to_string()),
        )
    }
}

/// 器やファイルの状態が満ちるまで待つ（期限式）
fn wait_state(base: Duration, mut probe: impl FnMut() -> bool) -> Waited {
    let budget = budget_for(base);
    let started = Instant::now();
    let mut polls = 0;
    loop {
        let ok = probe();
        let waited = started.elapsed();
        if ok || waited >= budget {
            return Waited {
                ok,
                waited,
                budget,
                polls,
                resends: 0,
            };
        }
        std::thread::sleep(STATE_POLL);
        polls += 1;
    }
}

type Events = futures::channel::mpsc::UnboundedReceiver<tako_core::SessionEvent>;

/// 画面が条件を満たすまで PTY イベントを汲みながら待つ（期限式）。
///
/// **rx を汲むのが必須**: psmux クライアントは端末クエリ（DA / DSR）への応答を待つ。
/// 捨てると応答待ちのまま描画が進まない（tmux の洪水テストと同じ理由）
fn pump_until(
    term: &mut tako_core::TerminalSession,
    rx: &mut Events,
    base: Duration,
    hit: impl Fn(&str) -> bool,
) -> Waited {
    pump_inner(term, rx, base, hit, None)
}

/// 画面が条件を満たすまで待ち、**満たないあいだは同じ行を打ち直す**（期限式。#1114）。
///
/// 打ち直しが要るのは、器の中のシェルが起動しきる前の打鍵が食われることがあるため
/// （導入時の実測: `Write-Output (AKO-PSMUX' …` のように先頭が欠ける）。
/// ただし**打つのは「素のプロンプトで止まっている」ときだけ**にする: エコーの途中
/// （`PS …> Write-O`）へ重ねても行が壊れるだけで復旧しない。継続行（PowerShell の
/// `>>` = 引用符が閉じていない）へ落ちていたら Ctrl-C で素のプロンプトへ戻す
fn pump_until_typed(
    term: &mut tako_core::TerminalSession,
    rx: &mut Events,
    base: Duration,
    line: &[u8],
    hit: impl Fn(&str) -> bool,
) -> Waited {
    pump_inner(term, rx, base, hit, Some(line))
}

/// 画面末尾から読む「いま打ってよいか」
#[derive(PartialEq, Eq, Debug)]
enum Prompt {
    /// 素のプロンプトで止まっている（打ってよい）
    Fresh,
    /// 継続行に落ちている（Ctrl-C で戻す）
    Continuation,
    /// 起動途中 / 実行中 / エコーの途中（打たない）
    Busy,
}

fn prompt_state(lines: &[String]) -> Prompt {
    let Some(tail) = lines.iter().rev().find(|l| !l.trim().is_empty()) else {
        return Prompt::Busy;
    };
    // `visible_lines` は行末を trim 済み
    if cfg!(windows) {
        if tail.ends_with(">>") {
            return Prompt::Continuation;
        }
        if tail.ends_with('>') && tail.contains("PS ") {
            return Prompt::Fresh;
        }
    } else if tail.ends_with('$') || tail.ends_with('%') || tail.ends_with('#') {
        return Prompt::Fresh;
    }
    Prompt::Busy
}

fn pump_inner(
    term: &mut tako_core::TerminalSession,
    rx: &mut Events,
    base: Duration,
    hit: impl Fn(&str) -> bool,
    line: Option<&[u8]>,
) -> Waited {
    let budget = budget_for(base);
    let resend_after = budget_for(RESEND_BASE);
    let started = Instant::now();
    let mut polls = 0;
    let mut resends = 0;
    let mut typed_at: Option<Instant> = None;
    loop {
        while let Ok(event) = rx.try_recv() {
            term.process_event(event);
        }
        let lines = term.visible_lines();
        let ok = lines.iter().any(|l| hit(l));
        let waited = started.elapsed();
        if ok || waited >= budget {
            return Waited {
                ok,
                waited,
                budget,
                polls,
                resends,
            };
        }
        if let Some(line) = line {
            let due = typed_at.is_none_or(|t| t.elapsed() >= resend_after);
            if due {
                match prompt_state(&lines) {
                    Prompt::Fresh => {
                        term.write(line.to_vec());
                        typed_at = Some(Instant::now());
                        resends += 1;
                    }
                    Prompt::Continuation => {
                        term.write(vec![0x03]);
                        typed_at = Some(Instant::now());
                    }
                    Prompt::Busy => {}
                }
            }
        }
        std::thread::sleep(POLL);
        polls += 1;
    }
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
        scrollback_lines: None,
    };

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
    // 器の中のシェルが**素のプロンプトで止まる**まで待ってから打ち、出力が出るまで
    // 期限で待つ（起動前の打鍵は先頭が食われるので、素のプロンプトでだけ打ち直す）
    let delivered = if legacy_1114() {
        legacy_deliver_marker(&mut first, &mut rx1, marker_command)
    } else {
        pump_until_typed(&mut first, &mut rx1, MARKER_BASE, marker_command, |line| {
            line.contains("TAKO-PSMUX-OK")
        })
    };
    assert!(
        delivered.ok,
        "1 回目の器でマーカーが出力される。{}",
        delivered.diag("TAKO-PSMUX-OK", &first.visible_lines().join("\n"))
    );
    // クライアント破棄（tako 終了相当）。器はサーバー側に残る
    drop(first);
    // **固定 sleep で「もう切断済み」を仮定しない**（#1114）。器が切断を認識した
    // ことを状態で待ってから生存を見る。器ごと死んでいれば `attached` は `None` に
    // なってこの待ちは即座に抜け、判定は下の `exists` が行う ＝ 検出力は落ちない
    let detached = wait_state(DETACH_BASE, || f.attached(&name) != Some(1));
    assert!(
        f.backend.exists(&name),
        "クライアントが死んでも器は生きている。{}",
        detached.diag(
            "attached != Some(1)",
            &format!("attached={:?}", f.attached(&name))
        )
    );

    // 2 回目: 同じ名前で開き直すと画面内容ごと戻る
    let (mut second, mut rx2) =
        tako_core::TerminalSession::spawn(80, 24, f.backend.wrap_spawn(base, &name))
            .expect("再 attach の psmux クライアントを spawn できる");
    let restored = pump_until(&mut second, &mut rx2, REATTACH_BASE, |line| {
        line.contains("TAKO-PSMUX-OK")
    });
    assert!(
        restored.ok,
        "再 attach で画面内容（scrollback）が復元される。{}",
        restored.diag("TAKO-PSMUX-OK", &second.visible_lines().join("\n"))
    );
    drop(second);
    let _ = f.backend.kill(&name);
}

/// **旧の固定窓**（`TAKO_1114_LEGACY=1` の A/B アーム）。
///
/// 「プロンプト待ち 60 回 × 100ms → 打つ → マーカー待ち 60 回」を最大 6 周する形。
/// 予算が機の混み具合に追従しないので、器の起動が 20 秒を超える負荷では 6 周を
/// プロンプト待ちだけで使い切って偽 FAILED になる（#1114 の実測: 9/16）
fn legacy_deliver_marker(
    term: &mut tako_core::TerminalSession,
    rx: &mut Events,
    line: &[u8],
) -> Waited {
    fn fixed_window(
        term: &mut tako_core::TerminalSession,
        rx: &mut Events,
        needle: &str,
        attempts: usize,
    ) -> bool {
        for _ in 0..attempts {
            while let Ok(event) = rx.try_recv() {
                term.process_event(event);
            }
            if term.visible_lines().iter().any(|l| l.contains(needle)) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }
    let prompt = if cfg!(windows) { "PS " } else { "$" };
    let started = Instant::now();
    let mut resends = 0;
    for _ in 0..6 {
        if !fixed_window(term, rx, prompt, 60) {
            continue;
        }
        term.write(line.to_vec());
        resends += 1;
        if fixed_window(term, rx, "TAKO-PSMUX-OK", 60) {
            return Waited {
                ok: true,
                waited: started.elapsed(),
                budget: Duration::from_secs(72),
                polls: 0,
                resends,
            };
        }
    }
    Waited {
        ok: false,
        waited: started.elapsed(),
        budget: Duration::from_secs(72),
        polls: 0,
        resends,
    }
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
    let mut cmd = Command::new(&f.bin);
    cmd.args(["-L", &f.socket, "-f"]).arg(&conf).args([
        "new-session",
        "-d",
        "-s",
        "tako-m2conf00001",
    ]);
    let text = psmux_ctl::wait_bounded(&mut cmd, psmux_ctl::BASE).text;
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

/// #974: **本番の spawn 経路が実際に書く conf**（`tmux_backend::backend_conf` が
/// 器の能力で組むもの）を実 psmux に食わせて、警告が 1 行も出ないことを確かめる。
///
/// 上の `confは警告なしで受理されwarm_offが効く` は psmux 版 conf
/// （`backend::psmux::ensure_conf`）を見ているが、**tako-app のペイン spawn は今も
/// `tmux_backend::wrap_options` を直接呼んでいる**（#885）ので、そちらの中身を
/// 確かめないと #974 の再発を実機で検出できない
#[test]
fn 本番が書くconfも警告なしで受理される() {
    use tako_core::backend::SessionBackend;

    let f = fixture!("conf974");
    let conf_body = tako_core::tmux_backend::backend_conf(&f.backend.capabilities());
    let path = std::env::temp_dir().join(format!("tako-974-{}.conf", std::process::id()));
    std::fs::write(&path, &conf_body).expect("conf を書ける");

    let mut cmd = Command::new(&f.bin);
    cmd.args(["-L", &f.socket, "-f"]).arg(&path).args([
        "new-session",
        "-d",
        "-s",
        "tako-m2conf974x",
    ]);
    let text = psmux_ctl::wait_bounded(&mut cmd, psmux_ctl::BASE).text;
    let _ = std::fs::remove_file(&path);
    assert!(
        !text.contains("warning") && !text.contains("unknown option"),
        "本番が書く conf に psmux が知らないオプションがある\n出力: {text}\nconf:\n{conf_body}"
    );
    // 落としたのは 4 行だけで、器が解する設定はちゃんと届いている
    let (ok, limit) = f.raw(&["show-options", "-g", "history-limit"]);
    assert!(ok, "show-options が動く: {limit}");
    assert!(
        limit.contains("10000"),
        "history-limit が反映されない（削りすぎ）: {limit}"
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

/// **#728 の土台**: 器の全ペインを 1 回で `(session:window.pane, pane_pid)` に列挙できる。
///
/// セッションカタログ（#112）と worker の状態解決（#592）は「実プロセス → どのペインか」を
/// この列挙で逆引きする。psmux は `-J` を無視する・複合コマンドの後半を捨てる、といった
/// 非互換を持つので、**`list-panes -a` と `#{session_name}:#{window_index}.#{pane_index}` に
/// 本当に答えるのか**を実バイナリで固定しておく（答えなくなったら「器の中の claude が
/// 1 つも見えない」に化けて、症状はカタログが空という遠い場所に出る）
#[test]
fn 器の全ペインをidとpidで列挙できる() {
    let f = fixture!("panepids");
    let a = session("tako-m2pids00001");
    let b = session("tako-m2pids00002");
    for name in [&a, &b] {
        let (ok, out) = f.raw(&["new-session", "-d", "-s", name.as_str()]);
        assert!(ok, "器を作れる: {out}");
    }

    let all = f.backend.pane_pids_all();
    for name in [&a, &b] {
        let found: Vec<_> = all
            .iter()
            .filter(|(id, _)| id.starts_with(&format!("{name}:")))
            .collect();
        assert_eq!(found.len(), 1, "{name} のペインが 1 件出る: {all:?}");
        assert!(found[0].1 > 0, "pane_pid が 0 でない: {found:?}");
        // セッション単位版と同じ pid を指す（2 つの API が食い違わない）
        assert_eq!(
            vec![found[0].1],
            f.backend.pane_pids(name),
            "pane_pids と pane_pids_all が同じ pid を返す"
        );
    }

    // 器を畳めば列挙からも消える（stale な pid を返し続けない）
    f.backend.kill(&a).expect("kill できる");
    let after = f.backend.pane_pids_all();
    assert!(
        !after.iter().any(|(id, _)| id.starts_with(&format!("{a}:"))),
        "kill した器は消える: {after:?}"
    );
    assert!(
        after.iter().any(|(id, _)| id.starts_with(&format!("{b}:"))),
        "残った器は出続ける: {after:?}"
    );
    let _ = f.backend.kill(&b);
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
        scrollback_lines: None,
    };
    let wrapped = f.backend.wrap_spawn(options, &name);
    let cmd = wrapped.command.expect("起動コマンドが組まれる");
    // クライアントは PTY 無しでは即終了しうるので、器の中身だけを見る
    let child = Command::new(&cmd.program).args(&cmd.args).spawn();
    // 並列テストで負荷がかかると器の起動は数秒ずれる。**固定回数ではなく期限**で待つ（#1114）
    let mut captured = String::new();
    let started = wait_state(Duration::from_secs(30), || {
        captured = f.raw(&["capture-pane", "-t", name.as_str(), "-p"]).1;
        captured.contains("TAKO-M2-CMD-OK")
    });
    if let Ok(mut child) = child {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(
        started.ok,
        "明示コマンドが器の中で起動していない（引数の組み立て規則が壊れた）。{}",
        started.diag("TAKO-M2-CMD-OK", &captured)
    );
    let _ = f.backend.kill(&name);
}

// --- 役割 B の読み側（採取。#519） ---------------------------------------

/// セッションの履歴が `want` 行を超えるまで待つ（並列テストの負荷でずれるので固定 sleep にしない）
fn wait_for_history(f: &Fixture, name: &SessionRef, want: usize) -> usize {
    let mut history = 0;
    let deadline = Instant::now() + budget_for(Duration::from_secs(45));
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
    let deadline = Instant::now() + budget_for(Duration::from_secs(45));
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
    let deadline = Instant::now() + budget_for(Duration::from_secs(30));
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

    let deadline = Instant::now() + budget_for(Duration::from_secs(45));
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

// --- 器の中のペインの品質（#659 / #686。いずれも Windows 固有） -----------------

/// 器の中のシェルの pid が取れること。**#659 の要**で、これが取れないと
/// 器の内側の疑似コンソールに触れず、psmux ペインだけ CP932 のまま残る
/// （#655 の固定は tako の ConPTY 直下 = psmux クライアントにしか当たらない）
#[test]
fn 器の中のシェルのpidが取れる() {
    let f = fixture!("panepid");
    let name = session("tako-m2pid00000001");
    let (ok, out) = f.raw(&["new-session", "-d", "-s", name.as_str()]);
    assert!(ok, "器を作れる: {out}");

    let mut pids = Vec::new();
    let got = wait_state(Duration::from_secs(8), || {
        pids = f.backend.pane_pids(&name);
        !pids.is_empty()
    });
    assert!(
        got.ok,
        "器の中のペインの pid が取れない（#659 の再発。コードページ固定が届かなくなる）。{}",
        got.diag("pane_pids が空でない", &format!("{pids:?}"))
    );
    assert!(pids.iter().all(|pid| *pid != 0));
    // 存在しない器には答えない（`unwrap_or_default` が空を返す経路）
    assert!(f
        .backend
        .pane_pids(&session("tako-m2pidnothere"))
        .is_empty());
    let _ = f.backend.kill(&name);
}

/// **#659 の症状そのもの**: 器の中のシェルが吐く UTF-8 の日本語が化ける。
/// 器の中の pid へコードページ固定を当てれば直る（`force_` を使うのは
/// テストプロセスが自分のコンソールを持つため。出荷構成は GUI サブシステム）
#[cfg(windows)]
#[test]
fn 器の中のシェルのコードページをutf8へ固定できる() {
    use tako_core::platform::console::{force_pin_pane_to_utf8, PinOutcome};

    /// 化けの判定に使う文字列（CP932 にも UTF-8 にもある常用漢字）
    const JP: &str = "日本語テスト";
    /// UTF-8 を CP932 として解釈したときの既知の並び
    const MOJIBAKE: &str = "譌･譛ｬ隱";

    let f = fixture!("panecp");
    let name = session("tako-m2cp0000001");
    let fixture_path =
        std::env::temp_dir().join(format!("tako-psmux-enc-{}.txt", std::process::id()));
    std::fs::write(&fixture_path, format!("{JP}\r\n").as_bytes()).expect("fixture を書ける");

    // psmux の既定シェル（pwsh 7）は自分で UTF-8 にしてしまうので、
    // **自分では直さない** cmd.exe を明示して #659 の条件を作る
    let (ok, out) = f.raw(&[
        "new-session",
        "-d",
        "-s",
        name.as_str(),
        "cmd.exe /d /k prompt $G",
    ]);
    assert!(ok, "器を作れる: {out}");

    let capture = |f: &Fixture| f.raw(&["capture-pane", "-t", name.as_str(), "-p"]).1;
    // **固定回数ではなく期限**で待つ（#1114）
    let wait_for = |f: &Fixture, needle: &str| -> bool {
        wait_state(Duration::from_secs(10), || capture(f).contains(needle)).ok
    };
    let send = |f: &Fixture, line: &str| {
        f.raw(&["send-keys", "-t", name.as_str(), line, "Enter"]);
    };

    assert!(wait_for(&f, ">"), "プロンプトが出ない: {}", capture(&f));

    // 修正前の状態（器の内側は OEM コードページのまま）を先に見ておく
    send(&f, &format!("type {}", fixture_path.display()));
    let before_ok = wait_for(&f, JP);
    let before = capture(&f);

    // 器の中の pid へ固定を当てる（**ここが #659 の修正**）
    let pids = f.backend.pane_pids(&name);
    assert!(!pids.is_empty(), "器の中の pid が取れない: {}", capture(&f));
    for pid in &pids {
        assert_eq!(
            force_pin_pane_to_utf8(*pid),
            PinOutcome::Pinned,
            "器の中のペイン（pid {pid}）を固定できない"
        );
    }

    send(&f, "cls");
    send(&f, "chcp");
    assert!(
        wait_for(&f, "65001"),
        "器の中のコードページが UTF-8 になっていない: {}",
        capture(&f)
    );
    send(&f, &format!("type {}", fixture_path.display()));
    assert!(
        wait_for(&f, JP),
        "固定後も UTF-8 の日本語が化ける: {}",
        capture(&f)
    );
    assert!(
        !capture(&f).contains(MOJIBAKE),
        "CP932 誤解釈の化け方が残っている: {}",
        capture(&f)
    );

    // 前提（固定前は化けていた）が崩れていたら、このテストは #659 を検出できていない。
    // OEM コードページが元から UTF-8 の環境ではありえるので、失敗ではなく明示する
    if before_ok {
        eprintln!(
            "note: 固定前から化けていなかった（この機の OEM コードページが UTF-8 か、\
             シェルが自分で直している）。before の画面: {before}"
        );
    } else {
        assert!(
            before.contains(MOJIBAKE),
            "固定前は CP932 誤解釈で化けているはず（前提が崩れている）: {before}"
        );
    }

    let _ = std::fs::remove_file(&fixture_path);
    let _ = f.backend.kill(&name);
}

/// `needle` **ちょうど**の行が出るまで汲む（期限式。#1114）。
/// 計算結果（`42`）のように短いマーカーは、入力エコーや `LINE 42` と
/// 区別するために行全体の一致で見る
#[cfg(windows)]
fn pump_line(
    term: &mut tako_core::TerminalSession,
    rx: &mut Events,
    needle: &str,
    base: Duration,
) -> Waited {
    pump_until(term, rx, base, |line| line.trim() == needle)
}

/// 遡れるだけの履歴を持つ psmux ペインを開く（#686 の 2 本が共有）。
///
/// **打鍵の再送が要る**: 並列テストで負荷がかかると pwsh の起動が遅れ、
/// 先頭の数文字が食われる（既存テストと同じ実測。psmux_backend.rs の
/// `器はクライアント切断後もattachで内容ごと戻る` を参照）
#[cfg(windows)]
fn open_pane_with_history(f: &Fixture, name: &SessionRef) -> (tako_core::TerminalSession, Events) {
    let (mut term, mut rx) = tako_core::TerminalSession::spawn(
        100,
        30,
        f.backend.wrap_spawn(
            SpawnOptions {
                command: None,
                cwd: Some(std::env::temp_dir()),
                env: vec![],
                scrollback_lines: None,
            },
            name,
        ),
    )
    .expect("psmux クライアントを spawn できる");

    let made = pump_until_typed(
        &mut term,
        &mut rx,
        MARKER_BASE,
        b"1..80 | ForEach-Object { \"LINE $_\" }\r",
        |line| line.contains("LINE 80"),
    );
    assert!(
        made.ok,
        "遡るための履歴が作れない。{}",
        made.diag("LINE 80", &term.visible_lines().join("\n"))
    );
    (term, rx)
}

/// **#686 の症状と修正**: 器が copy mode に居るあいだ打鍵はシェルへ届かない。
/// 器へ確かめてから in-band 解除を仕込むと、同じ打鍵が届くようになる。
///
/// tako 本番と同じ経路（実 PTY のクライアント + `TerminalSession` のホイール転送 +
/// `write`）で測るので、ここが通れば GUI でも同じ結果になる
#[cfg(windows)]
#[test]
fn copy_mode滞在中の打鍵がin_band解除で届く() {
    let f = fixture!("copymode");
    let name = session("tako-m2copy000001");
    let (mut term, mut rx) = open_pane_with_history(&f, &name);

    // psmux クライアントはマウス報告を要求している（= ホイールは PTY へ転送される）
    assert!(
        term.mouse_reporting(),
        "psmux クライアントがマウス報告を要求していない（前提が崩れている）"
    );

    // 器を copy mode に置き直す（前の試行で抜けているため。ホイールは tako と同じ経路）
    let enter_copy_mode = |term: &mut tako_core::TerminalSession| -> bool {
        // 一度 cancel してから入れ直す（飲まれた打鍵が copy mode を中途半端な
        // 状態に残すことがあるので、毎回きれいな状態から測る）
        f.raw(&["send-keys", "-X", "-t", &format!("{name}:"), "cancel"]);
        std::thread::sleep(Duration::from_millis(300));
        term.scroll_wheel(3, 10, 10);
        wait_state(Duration::from_secs(5), || {
            f.backend.pane_in_mode(&name) == Some(true)
        })
        .ok
    };

    // --- 上へ遡る → 器が copy mode に入る ---
    assert!(
        enter_copy_mode(&mut term),
        "ホイール上で器が copy mode に入らない（前提が崩れている）"
    );
    assert!(term.wheel_scrolled_back(), "遡り量の勘定が付いていない");

    // 打鍵は**1 キー = 1 回の `write`** で送る（GUI の `handle_key` と同じ形）。
    // まとめて 1 回で書くと、この機の pwsh が負荷時に入力の途中数文字を落とす揺らぎ
    // （`器はクライアント切断後もattachで内容ごと戻る` が再送している理由と同じ）に
    // 巻き込まれ、#686 と無関係な理由で落ちる。
    // 式は短く: 入力エコーは `20+20` / `31+11`、実行結果は `40` / `42` で取り違えない
    let type_keys = |term: &mut tako_core::TerminalSession, keys: &str| {
        for ch in keys.chars() {
            let mut buf = [0u8; 4];
            term.write(ch.encode_utf8(&mut buf).as_bytes().to_vec());
            std::thread::sleep(Duration::from_millis(60));
        }
        term.write(b"\r".to_vec());
    };

    // --- before: 解除を仕込まずに打鍵 → copy mode に食われて届かない ---
    type_keys(&mut term, "20+20");
    // **否定検査**なので窓は「伸ばすだけ」で安全に効く（長く見るほど検出力が上がり、
    // 短くなることは無い ＝ 混んだ機で偽 PASS になりにくくなる）
    let leaked = pump_line(&mut term, &mut rx, "40", Duration::from_secs(2));
    assert!(
        !leaked.ok,
        "copy mode 中なのに打鍵が届いている（症状が再現していない）。{}",
        leaked.diag("40 が出ないこと", &term.visible_lines().join("\n"))
    );

    // --- after: 器へ確かめてから in-band 解除を仕込む → 同じ打鍵が届く ---
    let exit = f
        .backend
        .copy_mode_exit_bytes()
        .expect("psmux は in-band 解除キーを申告する");
    // **打鍵の再送が要る**: この機の pwsh は負荷がかかると入力の途中数文字を落とす
    // （`器はクライアント切断後もattachで内容ごと戻る` が同じ理由で再送している）。
    // #686 の検証はあくまで「copy mode を抜けて打鍵がシェルへ届くか」なので、
    // 化けたら遡り直して打ち直す
    let mut delivered = false;
    for _ in 0..6 {
        if !enter_copy_mode(&mut term) {
            continue;
        }
        term.arm_copy_mode_exit(exit);
        type_keys(&mut term, "31+11");
        if pump_line(&mut term, &mut rx, "42", Duration::from_secs(20)).ok {
            delivered = true;
            break;
        }
    }
    assert!(
        delivered,
        "in-band 解除を仕込んでも打鍵が届かない（器の状態: in_mode={:?}）: {}",
        f.backend.pane_in_mode(&name),
        term.visible_lines().join("\n")
    );
    assert_eq!(
        f.backend.pane_in_mode(&name),
        Some(false),
        "解除後も copy mode に居る"
    );
    assert!(!term.wheel_scrolled_back(), "解除後は遡り量も 0 に戻る");
    // 解除キーがシェルへ漏れていないこと（漏れると入力欄に q が残る）
    assert!(
        !term
            .visible_lines()
            .iter()
            .any(|l| l.contains("q31+11") || l.contains("q20+20")),
        "解除キーがシェルへ漏れている: {}",
        term.visible_lines().join("\n")
    );

    let _ = f.backend.kill(&name);
}

/// **#686 の誤射防止の前提**: 器の**ホイール**は上下対称で、最下部へ戻ると
/// copy mode を抜ける。だから tako は「転送したホイール報告の上下差」だけで
/// 「器へ聞かずに copy mode ではないと言い切れる」瞬間を判定できる。
/// ここが崩れると「下まで戻してから打鍵」でシェルへ解除キーが漏れる。
///
/// **ホイール限定の性質**である点が重要: 同じことをソケット側の
/// `send-keys -X scroll-down` でやると位置は 0 に戻るが copy mode は**抜けない**
/// （実測）。tako が使う経路（PTY へ転送する SGR 報告）で測らなければ意味が無い
#[cfg(windows)]
#[test]
fn 器のホイールは上下対称で最下部でcopy_modeを抜ける() {
    let f = fixture!("symmetry");
    let name = session("tako-m2sym0000001");
    let (term, _rx) = open_pane_with_history(&f, &name);

    let target = format!("{name}:");
    let pos = |f: &Fixture| -> i64 {
        f.raw(&["display-message", "-p", "-t", &target, "#{scroll_position}"])
            .1
            .trim()
            .parse()
            .unwrap_or(-1)
    };
    let settle = |f: &Fixture, want: Option<bool>| -> Option<bool> {
        let mut got = None;
        wait_state(Duration::from_secs(5), || {
            got = f.backend.pane_in_mode(&name);
            got == want
        });
        got
    };

    assert_eq!(f.backend.pane_in_mode(&name), Some(false), "初期状態");
    term.scroll_wheel(3, 10, 10);
    assert_eq!(
        settle(&f, Some(true)),
        Some(true),
        "ホイール上で copy mode へ"
    );
    assert!(term.wheel_scrolled_back(), "tako 側の勘定も遡り中になる");
    let up = pos(&f);
    assert!(up > 0, "遡れていない: pos={up}");

    // 同じ報告数だけ下げると最下部へ戻り copy mode を抜ける（= 上下対称）
    term.scroll_wheel(-3, 10, 10);
    assert_eq!(
        settle(&f, Some(false)),
        Some(false),
        "同数の下げで copy mode を抜けない（tako の即時判定の前提が崩れている）"
    );
    assert_eq!(pos(&f), 0, "最下部へ戻っていない");
    assert!(
        !term.wheel_scrolled_back(),
        "tako 側の勘定も最下部に戻る（ここがずれると解除キーがシェルへ漏れる）"
    );
    let _ = f.backend.kill(&name);
}
