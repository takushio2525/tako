//! 背景シェルが生き残った idle ペインへの送達 E2E（Issue #1259）
//!
//! #1259 の本番は「worker が最終報告を出して入力欄 `❯` が空・けれど Bash の子
//! （3 時間の `ssh …` / 対話待ちの `cp -i`）が生き残っていて `has_running_children=true`」
//! という状態だった。Issue の見立ては「busy と判定したペインへ送らないのでは」だが、
//! **送達フローは `has_running_children` を一度も参照しない**（`worker_status` 専用）。
//! ここでは実 tmux 上の模擬 TUI で「本当に送れる」ことを実測して固定する。
//!
//! 模擬 TUI は #1259 の実採取画面と同じ形を描く:
//! - 会話ログに**番号つきの並び**（worker の報告 `2.` / `3.` / `4.`）が残っている
//! - `✻ Crunched for … · 1 shell still running`
//! - 罫線に挟まれた**空の入力欄** `❯`
//! - モデル / ctx / 5h / 7d / `⏵⏵ auto mode on · 1 shell · ⏎ for agents` のフッター
//! - そして **`sleep 3600 &` の実プロセス**を子として抱える（= 生き残った背景シェル）
//!
//! 本物の tmux が無い環境（psmux しか無い Windows 等）ではスキップする。

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[path = "common/tmux_e2e.rs"]
mod tmux_e2e;

/// 本番のバックエンドや**他プロセスのテスト**と混ざらない専用の器（#1300）。
/// 固定名だと `cargo test --workspace` が 2 本走った瞬間に同名セッションを
/// 取り合って `duplicate session` で落ちる（実測は `tmux_e2e` のモジュール doc）
fn socket() -> &'static str {
    tmux_e2e::socket_for("1259")
}

/// 送る本文（実採取物は貼らない = #927。到達判定に使うので一意にする）
const PAYLOAD: &str = "E2E1259 please answer with the word pineapple";

fn real_tmux() -> bool {
    Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| tako_core::tmux::announces_only_tmux(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

struct EmuGuard {
    session: String,
    dir: PathBuf,
}

impl Drop for EmuGuard {
    fn drop(&mut self) {
        // 器はこのプロセス専用。セッションを畳み、**このプロセスの最後の 1 本**なら
        // サーバーごと退役させてソケットファイルまで消す（tmux は残す）
        tmux_e2e::release_session(socket(), &self.session);
        // 模擬 TUI が抱えていた背景の子（sleep 3600）はセッションごと落ちるが、
        // 取りこぼしがあっても pid ファイル経由で確実に片付ける
        if let Ok(pid) = std::fs::read_to_string(self.dir.join("child.pid")) {
            let _ = Command::new("kill").arg(pid.trim()).output();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// #1259 の実採取画面と同じ形を描く模擬 claude TUI。
///
/// - **背景の子**（`sleep 3600 &`）を抱えたまま入力欄で待つ
/// - 貼り付け（bracketed paste でも素の連続入力でも）を入力欄へ反映する
/// - Enter で入力欄を空へ戻し、受け取った本文を `GOT=…` としてログへ書く
const EMULATOR: &str = r#"#!/bin/bash
# tako の送達フロー（#1259）の実検証用。実 claude の画面の形だけを真似る
LOG="$1"
PIDFILE="$2"
WITH_CHILD="$3"
: > "$LOG"
# 生き残った背景シェル（本番は 3 時間の ssh / 対話待ちの cp -i だった）。
# 第 3 引数で有無を切り替える = 子の有無で経路が変わらないことの A/B
if [ "$WITH_CHILD" = "1" ]; then
  sleep 3600 &
  echo $! > "$PIDFILE"
fi
rule="────────────────────────────────────────────────────────────────────────"
buf=""
draw() {
  printf '\033[2J\033[H'
  echo "  2. Evidence per acceptance criterion"
  echo
  echo "  #0001 - the fixture returned two options and highlighted=0."
  echo
  echo "  3. What could not be verified"
  echo
  echo "  - the product path on real hardware is still pending."
  echo
  echo "  4. commit / PR"
  echo
  echo "  - abc1234 [fix] something (#0001)"
  echo
  echo "✻ Crunched for 1h 25m 10s · done 10:47 AM · 1 shell still running"
  echo
  echo "$rule"
  echo "❯ $buf"
  echo "$rule"
  echo "  [model placeholder]  worker: placeholder task"
  echo "  ctx  49% ████░░░░░░"
  echo "  5h   26% ██░░░░░░░░"
  echo "  7d   86% ████████░░"
  echo "  ⏵⏵ auto mode on · 1 shell · ⏎ for agents"
}
# 出力処理（onlcr）は残したまま 1 文字ずつ読む。icrnl を切るのが要点（#1236 の実測）
stty -echo -icanon -icrnl min 1 time 0 2>/dev/null
draw
while :; do
  IFS= read -rsn1 c || break
  case "$c" in
    $'\033')
      # CSI …~（bracketed paste の 200~ / 201~）は読み捨てる
      IFS= read -rsn1 -t 1 b
      if [ "$b" = "[" ]; then
        while IFS= read -rsn1 -t 1 d; do
          case "$d" in ~|A|B|C|D) break ;; esac
        done
      fi
      ;;
    ''|$'\r'|$'\n')
      if [ -n "$buf" ]; then
        echo "GOT=$buf" >> "$LOG"
        buf=""
      else
        echo "ENTER=empty" >> "$LOG"
      fi
      draw
      ;;
    *)
      buf="$buf$c"
      draw
      ;;
  esac
done
stty sane 2>/dev/null
exec sleep 120
"#;

fn launch_emulator(tag: &str) -> (EmuGuard, PathBuf) {
    launch_emulator_with(tag, true)
}

/// `with_child` = 生き残った背景シェル（`sleep 3600`）を抱えさせるか
fn launch_emulator_with(tag: &str, with_child: bool) -> (EmuGuard, PathBuf) {
    let dir = PathBuf::from(format!(
        "/private/tmp/tako-e2e-1259-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("作業ディレクトリを作れる");
    let script = dir.join("idle-emu.sh");
    std::fs::write(&script, EMULATOR).expect("模擬 TUI を書ける");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&script)
            .expect("stat できる")
            .permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&script, perm).expect("実行権を付けられる");
    }
    let log = dir.join("input.log");
    let pidfile = dir.join("child.pid");
    let session = format!("tako1259{tag}");
    // 起動より**先に**ガードを作る。起動が落ちたときも作業ディレクトリと器が残らない
    let guard = EmuGuard {
        session: session.clone(),
        dir: dir.clone(),
    };
    if let Err(diag) = tmux_e2e::new_session(
        socket(),
        &[
            "-d",
            "-s",
            &session,
            "-x",
            "100",
            "-y",
            "30",
            "-c",
            dir.to_str().expect("テストパスは UTF-8"),
            &format!(
                "{} {} {} {}",
                script.to_str().expect("UTF-8"),
                log.to_str().expect("UTF-8"),
                pidfile.to_str().expect("UTF-8"),
                if with_child { "1" } else { "0" },
            ),
        ],
    ) {
        panic!("{diag}");
    }
    // 入力欄が描かれるまで待つ（描画前に送ると材料が無い）
    let drawn = wait_until(Duration::from_secs(15), || {
        tako_core::tmux::capture_session(Some(socket()), &session)
            .map(|l| tako_control::claude_tui::input_line(&l).is_some())
            .unwrap_or(false)
    });
    assert!(
        drawn,
        "模擬 TUI の入力欄が検知されない:\n{}",
        dump(&session)
    );
    (guard, log)
}

fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn dump(session: &str) -> String {
    tako_core::tmux::capture_session(Some(socket()), session)
        .map(|l| l.join("\n"))
        .unwrap_or_else(|e| format!("<capture 失敗: {e}>"))
}

fn read_log(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// 背景の子が本当に生きているか（`sleep 3600` の pid を kill -0 で確かめる）
fn child_alive(dir: &Path) -> bool {
    let Ok(pid) = std::fs::read_to_string(dir.join("child.pid")) else {
        return false;
    };
    Command::new("kill")
        .args(["-0", pid.trim()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// #1259 受け入れ条件 1: 生き残った子プロセスを抱えた idle ペインへも送達が成立する。
///
/// ついでに「画面そのものは原因ではない」ことも実測で固定する:
/// 会話ログに番号つきの並びが残っていても `is_choice_dialog` は偽・`input_line` は真
#[test]
fn 背景シェルが生き残ったidleペインへも送達が成立する() {
    if !real_tmux() {
        eprintln!("[e2e-1259] 本物の tmux が無いのでスキップ");
        return;
    }
    let (guard, log) = launch_emulator("deliver");
    let session = guard.session.clone();
    let dir = guard.dir.clone();

    // ① 前提: 背景の子が生きている（= #1259 の状態）
    assert!(
        child_alive(&dir),
        "背景の子（sleep 3600）が生きていない = 再現条件を満たしていない"
    );

    // ② 画面の判定: 番号つきの並びが会話ログに残っていても入力欄は読める。
    //    #1259 の調査で「画面が原因で `input_line` が None だった」説を否定した根拠
    let lines = tako_core::tmux::capture_session(Some(socket()), &session).expect("capture できる");
    assert!(
        !tako_control::claude_tui::is_choice_dialog(&lines),
        "会話ログの番号つきの並びを選択肢ダイアログと誤検知している:\n{}",
        lines.join("\n")
    );
    let input = tako_control::claude_tui::input_line(&lines).expect("入力欄が読める");
    assert!(
        tako_control::claude_tui::input_content_is_empty(input),
        "入力欄が空でない（人間の下書きがある状態を再現してしまっている）: {input:?}"
    );

    // ③ 送達（キー操作経路。peer は実 claude が要るのでここでは通らない）
    let report =
        tako_control::claude_tui::deliver_via_tmux(Some(socket()), &session, PAYLOAD, true)
            .expect("送達フローが完走する");
    assert!(
        report.verified,
        "送達を検証できない（report={report:?}）:\n{}",
        dump(&session)
    );

    // ④ 相手が本文を受け取っている（画面だけでなく模擬 TUI 側の記録で確かめる）
    let got = wait_until(Duration::from_secs(10), || {
        read_log(&log)
            .iter()
            .any(|l| l == &format!("GOT={PAYLOAD}"))
    });
    assert!(
        got,
        "模擬 TUI が本文を受け取っていない: {:?}\n{}",
        read_log(&log),
        dump(&session)
    );

    // ⑤ 送達のあいだも背景の子は生きたまま（子の有無で経路を変えていない）
    assert!(child_alive(&dir), "背景の子が送達の途中で消えている");
}

/// #1259 受け入れ条件 4（不変）: 人間の下書きがある入力欄へは本文を貼らない。
///
/// 「子プロセスの有無に関わらず送る」を入れても、**人間が打ちかけの行**を
/// 壊してはいけない。ここでは下書きを打ってから Enter 単独送達（#95）を掛け、
/// 貼り付けが起きないこと（`GOT=` が下書きそのまま）を確かめる
#[test]
fn 人間の下書きは送達で壊れない() {
    if !real_tmux() {
        eprintln!("[e2e-1259] 本物の tmux が無いのでスキップ");
        return;
    }
    let (guard, log) = launch_emulator("draft");
    let session = guard.session.clone();

    // 人間が打ちかけの行を作る
    tako_core::tmux::send_keys(Some(socket()), &session, "draft by human").expect("打てる");
    let shown = wait_until(Duration::from_secs(10), || {
        tako_core::tmux::capture_session(Some(socket()), &session)
            .map(|l| l.iter().any(|x| x.contains("draft by human")))
            .unwrap_or(false)
    });
    assert!(shown, "下書きが画面に出ない:\n{}", dump(&session));

    // Enter 単独送達（text 空）= 「入力欄に残っているものを送れ」。
    // 貼り付けは起きないので、相手が受け取るのは**下書きそのもの**
    let report = tako_control::claude_tui::deliver_via_tmux(Some(socket()), &session, "", true)
        .expect("Enter 単独送達が完走する");
    assert!(
        report.verified,
        "Enter 単独送達を検証できない（{report:?}）"
    );
    let got = wait_until(Duration::from_secs(10), || {
        read_log(&log).iter().any(|l| l == "GOT=draft by human")
    });
    assert!(
        got,
        "下書きがそのまま送られていない: {:?}\n{}",
        read_log(&log),
        dump(&session)
    );
    assert!(
        !read_log(&log).iter().any(|l| l.contains(PAYLOAD)),
        "下書きのある入力欄へ本文を貼っている: {:?}",
        read_log(&log)
    );
}

/// エッジ: **子プロセスが無い通常の idle** でも従来どおり送達が成立する（#1259）。
///
/// 「背景シェルの有無で経路を変えない」の対照。上の 1 本と合わせて A/B になり、
/// どちらでも `verified` かつ相手が本文を受け取ることを実測する
#[test]
fn 子プロセスが無い通常のidleでも送達が成立する() {
    if !real_tmux() {
        eprintln!("[e2e-1259] 本物の tmux が無いのでスキップ");
        return;
    }
    let (guard, log) = launch_emulator_with("nochild", false);
    let session = guard.session.clone();
    assert!(
        !child_alive(&guard.dir),
        "対照なのに背景の子が居る（fixture の切り替えが効いていない）"
    );
    let report =
        tako_control::claude_tui::deliver_via_tmux(Some(socket()), &session, PAYLOAD, true)
            .expect("送達フローが完走する");
    assert!(
        report.verified,
        "送達を検証できない（report={report:?}）:\n{}",
        dump(&session)
    );
    let got = wait_until(Duration::from_secs(10), || {
        read_log(&log)
            .iter()
            .any(|l| l == &format!("GOT={PAYLOAD}"))
    });
    assert!(
        got,
        "模擬 TUI が本文を受け取っていない: {:?}\n{}",
        read_log(&log),
        dump(&session)
    );
}
