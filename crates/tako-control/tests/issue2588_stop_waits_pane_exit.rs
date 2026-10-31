//! **#2588 の番犬**: `stop_isolated_gui` は GUI のペインのプロセスが終わるまで待ってから返す。
//!
//! ## なぜ縛るのか
//!
//! 検証スクリプトの後片付けは `stop_isolated_gui` の直後に一時 dir を `rm -rf` する
//! （`launch_isolated_gui` を使う約 180 本のうち 162 本は HOME もそこへ置く）。GUI が終わると
//! PTY が切れてペインのシェルへ SIGHUP が届くが、シェルは**終わる前に書く**（zsh は HUP で
//! 履歴を保存する = 一時の HOME の `.zsh_history`）。GUI の pid だけを待って返すと、その書き込みが
//! rm の走査に割り込んで rmdir が「Directory not empty」で断られ、exit-guard の trap の中の
//! `set -e` で **PASS / FAIL 0 のまま rc=1** になる（#2588: `test-remote-launch-1449.sh`。
//! 残骸は `home/.zsh_history` だけだった）。
//!
//! ## 何を確かめるか
//!
//! 偽の GUI（zsh の `zpty` で PTY の master を自分で握ってペインのシェルを起こす = tako-app と
//! 同じ形。落ちると master が閉じてペインへ SIGHUP が届く）に、本物の `stop_isolated_gui` を当てる。
//!
//! 1. HUP の後 1 秒かけて終わり、終わり際に一時 dir へ書くペインのシェル: 返った時点で居ない・
//!    書き込みは返る前に済んでいる・直後の `rm -rf` が通る。**逆戻り（ペインを待たずに返す）は
//!    ここが `scripts/lib/isolated-gui.sh:<行>` を名指して落とす**
//! 2. HUP を無視して居座るペインのシェル: 控えた pid だけへ TERM が届いて落ちる
//! 3. 端末を持たない子（GUI が起こす remote daemon の役）: 巻き込まない（GUI の再起動をまたいで
//!    残すはずの daemon がある = #2518）
//!
//! 偽のペインと偽の GUI は 30 秒で自分で抜けるので、テストが途中で殺されても残らない。
#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use tako_core::test_residue::ScratchDir;

const HELPER_REL: &str = "scripts/lib/isolated-gui.sh";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// `stop_isolated_gui() {` の行番号（逆戻りを名指す先）
fn stop_line() -> usize {
    let src = std::fs::read_to_string(repo_root().join(HELPER_REL)).expect("ヘルパを読む");
    src.lines()
        .position(|l| l.starts_with("stop_isolated_gui() {"))
        .map(|i| i + 1)
        .expect("stop_isolated_gui の定義がある")
}

/// 偽の GUI を起こして `stop_isolated_gui` を当て、`key=value` の行を返す。
/// `pane` はペインで走らせる sh の本文（`$BOX` に一時 dir・`$PANE_PID` に自分の pid を書く先）
const DRIVER: &str = r#"
set -euo pipefail
. "$HELPER"
ISOLATED_GUI_PANE_EXIT_TRIES="$TRIES"
mkdir -p "$BOX"
# 偽の GUI: ペイン 1 枚（PTY の master は zsh が握る）+ 端末を持たない子（daemon 役）。30 秒で自分で抜ける
# 待ちは組み込みの zselect（外部の sleep を前面に置くと、GUI の死後も孤児として残る）。
# 最後のコマンドにもしない（zsh -c は最後を exec で置き換え、zpty の master が閉じてペインが即死する）
/bin/zsh -fc '
  zmodload zsh/zpty zsh/zselect || exit 3
  zpty pane /bin/sh -c ${(q)PANE}
  sleep 30 </dev/null >/dev/null 2>&1 &
  print -r -- $! > "$DAEMON_PID"
  print -r -- ready > "$READY"
  zselect -t 3000; exit 0
' </dev/null >/dev/null 2> "$HOME/gui.err" &
GUI=$!
# stop_isolated_gui より前に落ちたときだけ自分で起こした pid を片付ける（後は終わった pid を撃たない）
trap 'kill "$GUI" 2>/dev/null; [ -s "$PANE_PID" ] && kill -9 "$(cat "$PANE_PID")" 2>/dev/null; [ -s "$DAEMON_PID" ] && kill "$(cat "$DAEMON_PID")" 2>/dev/null; true' EXIT
for _ in $(seq 1 100); do
  [ -s "$READY" ] && [ -s "$PANE_PID" ] && break
  sleep 0.1
done
[ -s "$PANE_PID" ] || { echo "setup=no-pane $(tr '\n' ' ' < "$HOME/gui.err")"; exit 0; }
PANE=$(cat "$PANE_PID")
DAEMON=$(cat "$DAEMON_PID")
echo "snapshot=$(iso_pane_processes "$GUI" | awk '{ print $1 }' | tr '\n' ' ')"
stop_isolated_gui "$GUI" 2> "$ERR"
trap - EXIT
echo "pane=${PANE}"
echo "pane_alive=$(kill -0 "$PANE" 2>/dev/null && echo yes || echo no)"
echo "late=$(cat "$BOX/late" 2>/dev/null || echo none)"
if rm -rf "${BOX:?}"; then echo "rm=ok"; else echo "rm=failed"; fi
echo "daemon_alive=$(kill -0 "$DAEMON" 2>/dev/null && echo yes || echo no)"
# daemon 役はいま生きていると確かめた自分の子（30 秒で自分でも抜ける）
kill "$DAEMON" 2>/dev/null || true
echo "stderr=$(tr '\n' ' ' < "$ERR")"
"#;

/// HUP を受けたら 1 秒かけて終わり、終わり際に `$BOX/late` を書く（zsh の履歴保存の役）
const PANE_LATE_WRITER: &str = r#"echo $$ > "$PANE_PID"
trap 'sleep 1; echo late > "$BOX/late"; exit 0' HUP
i=0; while [ "$i" -lt 150 ]; do sleep 0.2; i=$((i + 1)); done"#;

/// HUP を無視して居座る
const PANE_IGNORES_HUP: &str = r#"echo $$ > "$PANE_PID"
trap '' HUP
i=0; while [ "$i" -lt 150 ]; do sleep 0.2; i=$((i + 1)); done"#;

fn run(tag: &str, pane: &str, tries: u32) -> HashMap<String, String> {
    let scratch = ScratchDir::new(&format!("2588-{tag}"));
    let dir = scratch.path();
    let out = Command::new("/bin/bash")
        .arg("-c")
        .arg(DRIVER)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", dir)
        .env("HELPER", repo_root().join(HELPER_REL))
        .env("TRIES", tries.to_string())
        .env("BOX", dir.join("box"))
        .env("PANE", pane)
        .env("PANE_PID", dir.join("pane.pid"))
        .env("DAEMON_PID", dir.join("daemon.pid"))
        .env("READY", dir.join("ready"))
        .env("ERR", dir.join("stop.err"))
        .output()
        .expect("/bin/bash を起こす");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "{tag}: 駆動のスクリプトが失敗した（rc={:?}）\nstdout:\n{stdout}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    let map: HashMap<String, String> = stdout
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.trim().to_string()))
        .collect();
    assert_eq!(
        map.get("setup").map(String::as_str),
        None,
        "{tag}: 偽の GUI がペインを起こせなかった\n{stdout}"
    );
    map
}

#[test]
fn ペインのシェルが終わり際に書き終えてから返るので直後のrmが通る() {
    let r = run("late", PANE_LATE_WRITER, 50);
    let pane = &r["pane"];
    assert!(
        r["snapshot"].split_whitespace().any(|p| p == pane),
        "ペインのシェル（pid {pane}）を控えていない: {:?}",
        r
    );
    assert_eq!(
        r["pane_alive"],
        "no",
        "{HELPER_REL}:{}: stop_isolated_gui がペインのプロセスの終了を待たずに返った（返った時点で \
         pid {pane} が生きていた）= 呼び手が直後に消す一時 dir と、SIGHUP を受けたシェルの書き込み \
         （zsh の履歴保存）が競る（#2588: test-remote-launch-1449.sh が PASS 38 / FAIL 0 のまま rc=1）: {:?}",
        stop_line(),
        r
    );
    assert_eq!(
        r["late"], "late",
        "終わり際の書き込みが返る前に済んでいない: {r:?}"
    );
    assert_eq!(r["rm"], "ok", "直後の rm -rf が落ちた: {r:?}");
}

#[test]
fn hupを無視して居座るペインは控えたpidだけへtermが届く() {
    let r = run("ignore-hup", PANE_IGNORES_HUP, 5);
    assert_eq!(
        r["pane_alive"],
        "no",
        "{HELPER_REL}:{}: HUP を無視するペインのプロセスが stop_isolated_gui の後も残った \
         （一時 dir へ後から書きうる・検証の残骸になる。#2588）: {r:?}",
        stop_line()
    );
    assert!(
        r["stderr"].contains("TERM") && r["stderr"].contains(&r["pane"]),
        "居座ったペインへ TERM を送った旨を pid つきで出していない: {r:?}"
    );
}

#[test]
fn 端末を持たない子は巻き込まない() {
    let r = run("daemon", PANE_LATE_WRITER, 50);
    assert_eq!(
        r["daemon_alive"],
        "yes",
        "{HELPER_REL}:{}: 端末を持たない GUI の子（remote daemon の役）まで落とした \
         = GUI の再起動をまたいで残すはずの daemon（#2518）を巻き込む: {r:?}",
        stop_line()
    );
    assert!(
        !r["stderr"].contains("TERM"),
        "素直に終わるペインへ TERM を送った: {r:?}"
    );
}
