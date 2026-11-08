//! 実経路: remote daemon が待ち受けを開いた後で断っても、自分が書いた起動の痕跡（port ファイルほか）を残さず、
//! 次の `tako remote start` が「stale な待ち受け」を言わない（Issue #2781）。
//!
//! 修正前（実測）: 偽の tailscale に未ログイン（`NeedsLogin`）を返させて `tako remote start` → 断られる →
//! state の置き場に `tako-remote.port` が残る → もう一度 start → 「stale な待ち受けを検出しました。自動回収します:
//! 127.0.0.1:<ポート>」が毎回出る（そのポートを別のプロセスが取っていれば `DaemonListenerBusy` で起動ごと断る）。
//! serve の設定で断る出口（tako 管理外の設定）も同じだった。
//!
//! **ビルドした `tako` の本物の daemon** を起こす（`tako remote start` → `spawn_daemon` → `tako remote serve`）。
//! 見ること:
//! - Tailscale の不足 / serve の設定で断った後に、port・PID・トークン・URL のファイルが残らない
//! - 2 回目の start の stderr に stale の行が無い
//! - 断るまでの間に別の daemon が port ファイルを書き直していたら、それは消さない（偽の tailscale が
//!   `status` に答える前に書き直す = 注入。境界）
//! - A/B（`TAKO_2781_LEGACY=1`）では port ファイルが残る（このテストが残りを見分けられることの裏付け）
//!
//! daemon は器の門（#2771）を通らないと待ち受けより前で断るので、器（tmux）の無い環境では走らせない。
//! **走ったかどうかを必ず stderr に 1 行出し**、`I2781_REQUIRE_DAEMON=1` なら走らなかったことを失敗にする。
//! A/B: `TAKO_2781_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は残りの検査で落ちる）。
//! 本番の data dir・GUI・tailscale へ届かないよう、`TAKO_*` を全部外して一時の置き場だけを渡す
//! （隔離中の serve は #2619 の門を偽の tailscale で通す）。
//!
//! UDS（`TAKO_REMOTE_ENDPOINT=unix`）の socket は待ち受け（tiny_http の `Server`）の Drop が消すので修正前から
//! 残らない。ここで走らせないのは、砂場の置き場では socket のパスが unix の上限（100 バイト）を超えて
//! 待ち受けの前で断るため（手元の受け入れでは `/tmp` の短い置き場で確かめた = #2781 を閉じる PR の本文）。
//!
//! unix だけで走らせる（偽の tailscale が sh のスクリプト）。Windows の実機は #842 で確かめる
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2781_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2781_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 偽の tailscale。答え方は隣の `mode` ファイルで決める:
/// - `needs-login`: 未ログイン（Tailscale の不足で断る出口）
/// - `rewrite-port`: 答える前に state の port ファイルを別の daemon の中身（`1`）へ書き直してから未ログイン
/// - `serve-other`: ログイン済み・serve に tako 管理外の設定（パス分け）がある（serve の設定で断る出口）
const FAKE_TAILSCALE: &str = r#"#!/bin/sh
MODE="$(cat "$(dirname "$0")/mode")"
while [ $# -gt 0 ]; do case "$1" in --socket) shift 2 ;; *) break ;; esac; done
NODE="fake-2781.tailfake.ts.net"
case "$1 ${2:-}" in
  "--version "*) echo 1.0.0-fake ;;
  "status "*)
    case "$MODE" in
      needs-login) echo '{"BackendState":"NeedsLogin"}' ;;
      rewrite-port) echo 1 > "$TAKO_REMOTE_STATE_DIR/tako-remote.port"; echo '{"BackendState":"NeedsLogin"}' ;;
      *) printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE" ;;
    esac ;;
  "serve status") printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/a":{"Proxy":"http://127.0.0.1:1"},"/b":{"Proxy":"http://127.0.0.1:2"}}}}}\n' "$NODE" ;;
  *) exit 1 ;;
esac
"#;

/// daemon が書く起動の痕跡（断った後に残ってはいけないもの）
const DAEMON_FILES: [&str; 4] = [
    "tako-remote.port",
    "tako-remote.pid",
    "tako-remote.token",
    "tako-remote.url",
];

/// 1 ケース分の砂場: data（表示言語）・発見の置き場・remote の state・ホーム・偽の tailscale・器のソケット名
struct Sandbox {
    dir: ScratchDir,
    tmux: String,
}

impl Sandbox {
    fn new(name: &str, mode: &str) -> Self {
        let dir = ScratchDir::new("i2781");
        for d in ["data", "disc", "state", "hm"] {
            let d = dir.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // stale の行は日本語の固定文なので、表示言語を固定して OS のロケールで答えを変えない
        std::fs::write(
            dir.path().join("data/settings.json"),
            "{\"language\":\"ja\"}",
        )
        .expect("settings.json を書ける");
        std::fs::write(dir.path().join("mode"), mode).expect("mode を書ける");
        let ts = dir.path().join("tailscale-fake");
        std::fs::write(&ts, FAKE_TAILSCALE).expect("偽 tailscale を書ける");
        std::fs::set_permissions(&ts, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let tmux = format!("t2781-{}-{name}", std::process::id());
        Self { dir, tmux }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn state(&self) -> PathBuf {
        self.path().join("state")
    }

    /// `TAKO_*` を外して隔離の置き場だけを渡した `tako remote start`（`extra` は足す env）
    fn start(&self, extra: &[(&str, &str)]) -> Out {
        let mut command = Command::new(tako());
        command
            .args(["remote", "start"])
            .current_dir(self.path().join("hm"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy().to_string();
            if key.starts_with("TAKO_") || key.starts_with("CLAUDE") || key == "CODEX_HOME" {
                command.env_remove(key);
            }
        }
        command
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.path().join("data"))
            .env("TAKO_DISCOVERY_DIR", self.path().join("disc"))
            .env("TAKO_REMOTE_STATE_DIR", self.state())
            .env("TAKO_TAILSCALE_BIN", self.path().join("tailscale-fake"))
            .env("TAKO_TMUX_SOCKET", &self.tmux)
            .env("HOME", self.path().join("hm"));
        for (k, v) in extra {
            command.env(k, v);
        }
        let out = command.output().expect("tako が終わる");
        Out {
            code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    /// state の置き場に残っている daemon の痕跡（名前と中身）
    fn leftovers(&self) -> Vec<String> {
        DAEMON_FILES
            .iter()
            .filter_map(|name| {
                let path = self.state().join(name);
                path.exists().then(|| {
                    let body = std::fs::read_to_string(&path).unwrap_or_default();
                    format!("{name}={}", body.trim())
                })
            })
            .collect()
    }

    /// 起きてしまった daemon（断るはずが起きた回）を自分の pid だけで畳む
    fn stop_own_daemon(&self) {
        let pid = std::fs::read_to_string(self.state().join("tako-remote.pid"))
            .ok()
            .and_then(|s| s.lines().next().map(|l| l.trim().to_string()));
        if let Some(pid) = pid.filter(|p| p.parse::<u32>().is_ok()) {
            let _ = Command::new("kill").args(["-TERM", &pid]).status();
        }
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.stop_own_daemon();
        // 器のソケットファイルまで消す 1 実装（daemon は断る前に器のサーバーを起こさないが、念のため）
        tako_core::tmux_backend::kill_server(&self.tmux);
    }
}

struct Out {
    code: i32,
    stderr: String,
}

/// 断る出口を 1 つ通す: 1 回目の start が断られ、痕跡が残らず、2 回目に stale の行が出ない
fn refused_twice_leaves_nothing(name: &str, mode: &str, extra: &[(&str, &str)], why: &str) {
    let sb = Sandbox::new(name, mode);
    let first = sb.start(extra);
    assert_ne!(
        first.code, 0,
        "[{name}] 1 回目の start は断られる（{why}）: {}",
        first.stderr
    );
    assert!(
        first.stderr.contains(why),
        "[{name}] 断りの理由が「{why}」: {}",
        first.stderr
    );
    assert_eq!(
        sb.leftovers(),
        Vec::<String>::new(),
        "[{name}] 断った後に daemon の痕跡が残っている"
    );
    let second = sb.start(extra);
    assert_ne!(
        second.code, 0,
        "[{name}] 2 回目も断られる: {}",
        second.stderr
    );
    assert!(
        !second.stderr.contains("stale"),
        "[{name}] 2 回目の start が stale の行を出した: {}",
        second.stderr
    );
    assert_eq!(
        sb.leftovers(),
        Vec::<String>::new(),
        "[{name}] 2 回目の後に daemon の痕跡が残っている"
    );
}

/// 器（tmux）が使えるか。daemon の器の門（#2771）は画面の読み取りを求める
fn daemon_can_start() -> bool {
    let ran = tako_core::backend::capabilities().detached_capture;
    if ran {
        eprintln!("[issue2781] 本物の daemon のケース: 走った");
    } else {
        eprintln!("[issue2781] 本物の daemon のケース: 器（tmux）が無いので走らせない");
        assert!(
            std::env::var_os("I2781_REQUIRE_DAEMON").is_none(),
            "I2781_REQUIRE_DAEMON=1 なのに本物の daemon のケースを走らせられなかった"
        );
    }
    ran
}

#[test]
fn tailscaleの不足で断ってもportファイルが残らず次のstartはstaleを言わない() {
    if !daemon_can_start() {
        return;
    }
    refused_twice_leaves_nothing(
        "needs-login",
        "needs-login",
        &[],
        "Tailscale にログインしていない",
    );
}

#[test]
fn serveの設定で断ってもportファイルが残らない() {
    if !daemon_can_start() {
        return;
    }
    refused_twice_leaves_nothing(
        "serve-other",
        "serve-other",
        &[],
        "tako 管理外の設定があります",
    );
}

#[test]
fn 断るまでに別のdaemonが書き直したportファイルは消さない() {
    if !daemon_can_start() {
        return;
    }
    let sb = Sandbox::new("rewrite", "rewrite-port");
    let out = sb.start(&[]);
    assert_ne!(out.code, 0, "start は断られる: {}", out.stderr);
    assert_eq!(
        sb.leftovers(),
        vec!["tako-remote.port=1".to_string()],
        "別の daemon が書き直した port ファイルだけが残る（自分のものは残さない・他人のものは消さない）"
    );
}

#[test]
fn abでは断った後にportファイルが残る() {
    if !daemon_can_start() {
        return;
    }
    let sb = Sandbox::new("legacy", "needs-login");
    let out = sb.start(&[("TAKO_2781_LEGACY", "1")]);
    assert_ne!(out.code, 0, "start は断られる: {}", out.stderr);
    let left = sb.leftovers();
    assert!(
        left.len() == 1 && left[0].starts_with("tako-remote.port="),
        "A/B（修正前）では port ファイルが残る（このテストが残りを見分けられることの裏付け）: {left:?}"
    );
    let again = sb.start(&[("TAKO_2781_LEGACY", "1")]);
    assert!(
        again.stderr.contains("stale"),
        "A/B（修正前）の 2 回目は stale の行を出す: {}",
        again.stderr
    );
}
