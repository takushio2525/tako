//! 実経路: 接続元検証が全部を断る環境で、`tako remote start` / `status` が成功だけを名乗らず、理由と次の一手を
//! 表示言語で言う（Issue #2786）。
//!
//! 修正前は起動時の自己疎通が接続元検証（#841）の 403 を「到達した」と数え、start の応答は
//! `"self_check": {"ok": true, "status": 403}`・stderr は QR と URL の案内だけだった（スマホからは全部 403）。
//!
//! **ビルドした `tako` の本物の daemon** を起こす（`tako remote start` → `spawn_daemon` → `tako remote serve`）。
//! 全部を断る環境は注入 `TAKO_2786_INJECT=direct` で作る: 自己疎通を serve を通さず daemon のループバックへ直に
//! 打ち、serve と同じ転送ヘッダを付ける。接続元は daemon 自身（tailscale ではない）なので、**本物の接続元検証**が
//! `not_tailscale_daemon` で断る（判定は差し替えない）。見ること:
//! - start は成功（daemon は止めない）し、応答の `self_check` が `peer_rejected` = 理由コードを持ち、stderr が
//!   表示言語で理由と次の一手を言う（英語表示の行に日本語が混ざらない）
//! - status の `peer_verification.self_check_rejected` と `warnings`・stderr の 1 件
//! - stop は従来どおり止まり、serve の設定が空に戻る
//! - A/B（`TAKO_2786_LEGACY=1`）では修正前の形（`ok: true` / `status: 403`・何も言わない）
//!
//! daemon は器の門（#2771）を通らないと待ち受けより前で断るので、器（tmux）の無い環境では走らせない。
//! **走ったかどうかを必ず stderr に 1 行出し**、`I2786_REQUIRE_DAEMON=1` なら走らなかったことを失敗にする。
//! 本番の data dir・GUI・tailscale へ届かないよう、`TAKO_*` を全部外して一時の置き場だけを渡す
//! （隔離中の serve は #2619 の門を偽の tailscale で通す）。
//!
//! unix だけで走らせる（偽の tailscale が sh のスクリプト）。Windows の実機は #842 で確かめる
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`）
fn tako() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tako"))
}

/// 偽の tailscale（ログイン済み）。serve の設定は隣の `serve.json` に持つ
const FAKE_TAILSCALE: &str = r#"#!/bin/sh
D="$(dirname "$0")"
while [ $# -gt 0 ]; do case "$1" in --socket) shift 2 ;; *) break ;; esac; done
NODE="fake-2786.tailfake.ts.net"
[ -f "$D/serve.json" ] || echo '{}' > "$D/serve.json"
case "$1" in
  --version) echo 1.0.0-fake ;;
  status) printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE" ;;
  serve)
    case "${2:-} ${3:-} ${4:-}" in
      "status "*) cat "$D/serve.json" ;;
      "--bg --https=443 "*)
        printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' "$NODE" "$4" > "$D/serve.json" ;;
      "--https=443 off ") echo '{}' > "$D/serve.json" ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
"#;

/// 1 ケース分の砂場: data（表示言語）・発見の置き場・remote の state・ホーム・偽の tailscale・器のソケット名
struct Sandbox {
    dir: ScratchDir,
    tmux: String,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout が JSON でない（{e}）:\n{}", self.stdout))
    }
}

impl Sandbox {
    fn new(name: &str, lang: &str) -> Self {
        let dir = ScratchDir::new("i2786");
        for d in ["data", "disc", "state", "hm", "ts"] {
            let d = dir.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        std::fs::write(
            dir.path().join("data/settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を書ける");
        let ts = dir.path().join("ts/tailscale");
        std::fs::write(&ts, FAKE_TAILSCALE).expect("偽 tailscale を書ける");
        std::fs::set_permissions(&ts, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let tmux = format!("t2786-{}-{name}", std::process::id());
        Self { dir, tmux }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn state(&self) -> PathBuf {
        self.path().join("state")
    }

    /// `TAKO_*` を外して隔離の置き場だけを渡した `tako remote <sub>`（`extra` は足す env）
    fn remote(&self, sub: &str, extra: &[(&str, &str)]) -> Out {
        let mut command = Command::new(tako());
        command
            .args(["remote", sub])
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
            .env("TAKO_TAILSCALE_BIN", self.path().join("ts/tailscale"))
            .env("TAKO_TMUX_SOCKET", &self.tmux)
            .env("TAKO_REMOTE_NO_NOTIFY", "1")
            .env("HOME", self.path().join("hm"));
        for (k, v) in extra {
            command.env(k, v);
        }
        let out = command.output().expect("tako が終わる");
        Out {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).to_string(),
            stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        }
    }

    fn serve(&self) -> String {
        std::fs::read_to_string(self.path().join("ts/serve.json")).unwrap_or_default()
    }

    /// 起こした daemon を自分の pid だけで畳む（stop が成功した後は PID ファイルが無いので何もしない）
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
        tako_core::tmux_backend::kill_server(&self.tmux);
    }
}

/// 器（tmux）が使えるか。daemon の器の門（#2771）は画面の読み取りを求める
fn daemon_can_start() -> bool {
    let ran = tako_core::backend::capabilities().detached_capture;
    if ran {
        eprintln!("[issue2786] 本物の daemon のケース: 走った");
    } else {
        eprintln!("[issue2786] 本物の daemon のケース: 器（tmux）が無いので走らせない");
        assert!(
            std::env::var_os("I2786_REQUIRE_DAEMON").is_none(),
            "I2786_REQUIRE_DAEMON=1 なのに本物の daemon のケースを走らせられなかった"
        );
    }
    ran
}

/// 日本語の字を含むか
fn has_japanese(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x9fff | 0xff00..=0xffef | 0x3000..=0x303f)
    })
}

const DIRECT: (&str, &str) = ("TAKO_2786_INJECT", "direct");

/// start → status → stop を通し、表示言語ごとの理由の行を確かめる
fn rejected_round(lang: &str, reason_marker: &str, next_marker: &str) {
    let sb = Sandbox::new(lang, lang);
    let start = sb.remote("start", &[DIRECT]);
    assert_eq!(
        start.code, 0,
        "[{lang}] 接続元検証で断られても start は daemon を止めない: {}",
        start.stderr
    );
    let info = start.json();
    assert_eq!(info["running"], Value::Bool(true), "{}", start.stdout);
    assert_eq!(
        info["self_check"]["peer_rejected"].as_str(),
        Some("not_tailscale_daemon"),
        "[{lang}] start の応答が接続元検証の断りを名指さない（修正前は ok: true / status: 403）: {}",
        start.stdout
    );
    assert_eq!(info["self_check"]["ok"], Value::Bool(false));
    let line = start
        .stderr
        .lines()
        .find(|l| l.contains(reason_marker))
        .unwrap_or_else(|| panic!("[{lang}] start の stderr に理由が無い:\n{}", start.stderr));
    let next = start
        .stderr
        .lines()
        .find(|l| l.contains(next_marker))
        .unwrap_or_else(|| {
            panic!(
                "[{lang}] start の stderr に次の一手が無い:\n{}",
                start.stderr
            )
        });
    if lang == "en" {
        assert!(
            !has_japanese(line) && !has_japanese(next),
            "英語表示の行に日本語が混ざる:\n{line}\n{next}"
        );
    }

    let status = sb.remote("status", &[]);
    assert_eq!(status.code, 0, "{}", status.stderr);
    let st = status.json();
    assert_eq!(
        st["peer_verification"]["self_check_rejected"].as_str(),
        Some("not_tailscale_daemon"),
        "[{lang}] status が自己疎通の断りを持たない: {}",
        status.stdout
    );
    assert!(
        st["warnings"].as_array().is_some_and(|w| w
            .iter()
            .any(|x| x.as_str().is_some_and(|s| s.contains("接続元検証")))),
        "[{lang}] status の warnings に断りの 1 行が無い: {}",
        status.stdout
    );
    assert!(
        status.stderr.contains(reason_marker) && status.stderr.contains(next_marker),
        "[{lang}] status の stderr が理由と次の一手を言わない:\n{}",
        status.stderr
    );

    let stop = sb.remote("stop", &[]);
    assert_eq!(stop.code, 0, "[{lang}] stop は従来どおり: {}", stop.stderr);
    assert_eq!(
        sb.serve(),
        "{}\n",
        "[{lang}] stop の後に serve の設定が残る"
    );
}

#[test]
fn 接続元検証で全部断られる環境ではstartとstatusが理由と次の一手を言う() {
    if !daemon_can_start() {
        return;
    }
    rejected_round("ja", "接続元検証で断られました", "次の一手: ");
    rejected_round("en", "refused by the peer verification", "next step: ");
}

#[test]
fn 修正前の分類ではstartが403を到達と数えて何も言わない() {
    if !daemon_can_start() {
        return;
    }
    let sb = Sandbox::new("legacy", "ja");
    let start = sb.remote("start", &[DIRECT, ("TAKO_2786_LEGACY", "1")]);
    assert_eq!(start.code, 0, "{}", start.stderr);
    let info = start.json();
    assert_eq!(
        info["self_check"],
        serde_json::json!({ "ok": true, "status": 403 }),
        "A/B は修正前の形（403 を到達と数える）: {}",
        start.stdout
    );
    assert!(
        !start.stderr.contains("接続元検証"),
        "A/B では断りを言わない:\n{}",
        start.stderr
    );
    let status = sb.remote("status", &[]);
    assert!(
        status.json()["peer_verification"]["self_check_rejected"].is_null(),
        "A/B では status に断りを残さない: {}",
        status.stdout
    );
    let stop = sb.remote("stop", &[]);
    assert_eq!(stop.code, 0, "{}", stop.stderr);
}
