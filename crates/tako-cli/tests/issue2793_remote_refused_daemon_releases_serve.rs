//! 実経路: remote daemon が tailscale の serve を張った後で断っても、張った serve の設定を残さない（Issue #2793）。
//!
//! 修正前（実測）: 偽の tailscale（serve の設定をファイルで持つ・ログイン済み）+ 壊れた `devices.json` で
//! `tako remote start` → `devices.json の解釈に失敗` で断られる → serve の設定は
//! `"Proxy":"http://127.0.0.1:<ポート>"` のまま残る（daemon はもう居ないので、tailnet の HTTPS は誰も待ち受けて
//! いないループバックのポートを向く）。トークンの書き出しの失敗（置き場がディレクトリ）でも同じだった。
//!
//! **ビルドした `tako` の本物の daemon** を起こす（`tako remote start` → `spawn_daemon` → `tako remote serve`）。
//! 見ること:
//! - serve を張った後の出口（壊れた `devices.json`・トークンの書き出しの失敗・注入 `TAKO_2793_INJECT` の
//!   `token` = 張った直後の最初の出口 / `self-check` = 自己疎通スレッドの起動）で断った後に、serve の設定が
//!   空に戻り、audit.log に外した記録（`serve_off`・`daemon_refused`）が残る
//! - 自分の到達先を向いていない設定は外さない: 張った後で別の daemon / 利用者が張り直したもの（偽の tailscale が
//!   `serve --bg` で別の到達先を書く = 注入）・起動前からある利用者の設定（tako 管理外として断る出口）
//! - A/B（`TAKO_2793_LEGACY=1`）では自分の到達先を向いたまま残る（このテストが残りを見分けられることの裏付け）
//!
//! daemon は器の門（#2771）を通らないと待ち受けより前で断るので、器（tmux）の無い環境では走らせない。
//! **走ったかどうかを必ず stderr に 1 行出し**、`I2793_REQUIRE_DAEMON=1` なら走らなかったことを失敗にする。
//! A/B: `TAKO_2793_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は残りの検査で落ちる。注入の出口は
//! 修正前に無いので起動してしまい「断られる」の検査で落ちる = 起きた daemon は自分の pid で畳む）。
//! 本番の data dir・GUI・tailscale へ届かないよう、`TAKO_*` を全部外して一時の置き場だけを渡す
//! （隔離中の serve は #2619 の門を偽の tailscale で通す）。
//!
//! unix だけで走らせる（偽の tailscale が sh のスクリプト）。Windows の実機は #842 で確かめる
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2793_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2793_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 偽の tailscale（ログイン済み）。serve の設定は隣の `serve.json` に持ち、呼び出しを `calls.log` に残す。
/// 隣に `bg-writes` があれば、`serve --bg` は頼まれた到達先ではなくその中身を書く（張った直後に別の daemon /
/// 利用者が張り直した = 注入）
const FAKE_TAILSCALE: &str = r#"#!/bin/sh
D="$(dirname "$0")"
while [ $# -gt 0 ]; do case "$1" in --socket) shift 2 ;; *) break ;; esac; done
NODE="fake-2793.tailfake.ts.net"
[ -f "$D/serve.json" ] || echo '{}' > "$D/serve.json"
echo "$*" >> "$D/calls.log"
case "$1" in
  --version) echo 1.0.0-fake ;;
  status) printf '{"BackendState":"Running","Version":"1.0.0-fake","CertDomains":["%s"],"Self":{"DNSName":"%s."}}\n' "$NODE" "$NODE" ;;
  serve)
    case "${2:-} ${3:-} ${4:-}" in
      "status "*) cat "$D/serve.json" ;;
      "--bg --https=443 "*)
        T="$4"; [ -f "$D/bg-writes" ] && T="$(cat "$D/bg-writes")"
        printf '{"TCP":{"443":{"HTTPS":true}},"Web":{"%s:443":{"Handlers":{"/":{"Proxy":"%s"}}}}}\n' "$NODE" "$T" > "$D/serve.json" ;;
      "--https=443 off ") echo '{}' > "$D/serve.json" ;;
      *) exit 1 ;;
    esac ;;
  *) exit 1 ;;
esac
"#;

/// HTTPS:443 の "/" → `target` の serve の設定（偽の tailscale が書くのと同じ形）
fn serve_json_to(target: &str) -> String {
    format!(
        "{{\"TCP\":{{\"443\":{{\"HTTPS\":true}}}},\"Web\":{{\"fake-2793.tailfake.ts.net:443\":\
         {{\"Handlers\":{{\"/\":{{\"Proxy\":\"{target}\"}}}}}}}}}}\n"
    )
}

/// 1 ケース分の砂場: data（表示言語）・発見の置き場・remote の state・ホーム・偽の tailscale・器のソケット名
struct Sandbox {
    dir: ScratchDir,
    tmux: String,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let dir = ScratchDir::new("i2793");
        for d in ["data", "disc", "state", "hm", "ts"] {
            let d = dir.path().join(d);
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 断りの文は日本語の固定文なので、表示言語を固定して OS のロケールで答えを変えない
        std::fs::write(
            dir.path().join("data/settings.json"),
            "{\"language\":\"ja\"}",
        )
        .expect("settings.json を書ける");
        let ts = dir.path().join("ts/tailscale");
        std::fs::write(&ts, FAKE_TAILSCALE).expect("偽 tailscale を書ける");
        std::fs::set_permissions(&ts, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let tmux = format!("t2793-{}-{name}", std::process::id());
        Self { dir, tmux }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn state(&self) -> PathBuf {
        self.path().join("state")
    }

    fn ts(&self, name: &str) -> PathBuf {
        self.path().join("ts").join(name)
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
            .env("TAKO_TAILSCALE_BIN", self.ts("tailscale"))
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

    /// いまの serve の設定
    fn serve(&self) -> String {
        std::fs::read_to_string(self.ts("serve.json")).unwrap_or_default()
    }

    /// 偽の tailscale の呼び出し（1 行 1 回）
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.ts("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// daemon が張った到達先（`serve --bg --https=443 <到達先>` の到達先）
    fn set_target(&self) -> Option<String> {
        self.calls().iter().find_map(|c| {
            c.strip_prefix("serve --bg --https=443 ")
                .map(|t| t.trim().to_string())
        })
    }

    fn audit(&self) -> String {
        std::fs::read_to_string(self.state().join("audit.log")).unwrap_or_default()
    }

    /// 起きてしまった daemon（断るはずが起きた回 = 修正前のバイナリの注入の出口）を自分の pid だけで畳む
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

/// 器（tmux）が使えるか。daemon の器の門（#2771）は画面の読み取りを求める
fn daemon_can_start() -> bool {
    let ran = tako_core::backend::capabilities().detached_capture;
    if ran {
        eprintln!("[issue2793] 本物の daemon のケース: 走った");
    } else {
        eprintln!("[issue2793] 本物の daemon のケース: 器（tmux）が無いので走らせない");
        assert!(
            std::env::var_os("I2793_REQUIRE_DAEMON").is_none(),
            "I2793_REQUIRE_DAEMON=1 なのに本物の daemon のケースを走らせられなかった"
        );
    }
    ran
}

/// 張った後の出口を 1 つ通す: start が `why` で断られ、張った到達先を外し、外した記録が残る
fn refused_after_serve_releases(sb: &Sandbox, name: &str, extra: &[(&str, &str)], why: &str) {
    let out = sb.start(extra);
    assert_ne!(
        out.code, 0,
        "[{name}] start は断られる（{why}）: {}",
        out.stderr
    );
    assert!(
        out.stderr.contains(why),
        "[{name}] 断りの理由が「{why}」: {}",
        out.stderr
    );
    let target = sb.set_target();
    assert!(
        target
            .as_deref()
            .is_some_and(|t| t.starts_with("http://127.0.0.1:")),
        "[{name}] 断る前に serve を張っている（張った後の出口を通った）: {:?}",
        sb.calls()
    );
    assert_eq!(
        sb.serve(),
        "{}\n",
        "[{name}] 断った後に serve の設定が残っている（張った到達先 {target:?}）"
    );
    assert!(
        sb.calls().iter().any(|c| c == "serve --https=443 off"),
        "[{name}] serve を外していない: {:?}",
        sb.calls()
    );
    let audit = sb.audit();
    assert!(
        audit
            .lines()
            .any(|l| l.contains("\"serve_off\"") && l.contains("daemon_refused")),
        "[{name}] audit.log に外した記録（serve_off・daemon_refused）が無い:\n{audit}"
    );
}

#[test]
fn 壊れたdevices_jsonで断ってもserveの設定が残らない() {
    if !daemon_can_start() {
        return;
    }
    let sb = Sandbox::new("devices");
    std::fs::write(sb.state().join("devices.json"), "{").expect("devices.json を書ける");
    refused_after_serve_releases(&sb, "devices", &[], "devices.json の解釈に失敗");
}

#[test]
fn トークンの書き出しで断ってもserveの設定が残らない() {
    if !daemon_can_start() {
        return;
    }
    let sb = Sandbox::new("token-dir");
    // トークンの置き場がディレクトリ = 書き出し（temp → rename）が失敗する
    std::fs::create_dir_all(sb.state().join("tako-remote.token/x")).expect("ディレクトリを作れる");
    refused_after_serve_releases(&sb, "token-dir", &[], "トークンファイルの書き出しに失敗");
}

#[test]
fn 張った直後の最初の出口と自己疎通スレッドの起動で断ってもserveの設定が残らない() {
    if !daemon_can_start() {
        return;
    }
    for point in ["token", "self-check"] {
        let sb = Sandbox::new(point);
        refused_after_serve_releases(
            &sb,
            point,
            &[("TAKO_2793_INJECT", point)],
            &format!("TAKO_2793_INJECT={point}"),
        );
    }
}

#[test]
fn 張った後で別のdaemonや利用者が張り直した設定は外さない() {
    if !daemon_can_start() {
        return;
    }
    for (name, other) in [
        ("other-daemon", "http://127.0.0.1:1"),
        ("user-after", "http://192.168.1.5:8080"),
    ] {
        let sb = Sandbox::new(name);
        std::fs::write(sb.state().join("devices.json"), "{").expect("devices.json を書ける");
        std::fs::write(sb.ts("bg-writes"), other).expect("bg-writes を書ける");
        let out = sb.start(&[]);
        assert_ne!(out.code, 0, "[{name}] start は断られる: {}", out.stderr);
        assert!(
            sb.set_target().is_some(),
            "[{name}] 断る前に serve を張りにいっている: {:?}",
            sb.calls()
        );
        assert_eq!(
            sb.serve(),
            serve_json_to(other),
            "[{name}] 自分の到達先を向いていない設定を外した"
        );
        assert!(
            !sb.calls().iter().any(|c| c.contains("off")),
            "[{name}] 自分のものでない設定に off を打った: {:?}",
            sb.calls()
        );
    }
}

#[test]
fn 起動前からある利用者の設定は断った後も外さない() {
    if !daemon_can_start() {
        return;
    }
    let sb = Sandbox::new("user-before");
    let user = serve_json_to("http://192.168.1.5:8080");
    std::fs::write(sb.ts("serve.json"), &user).expect("serve.json を書ける");
    let out = sb.start(&[]);
    assert_ne!(out.code, 0, "start は断られる: {}", out.stderr);
    assert!(
        out.stderr.contains("既存の設定があります"),
        "tako 管理外の設定として断る: {}",
        out.stderr
    );
    assert_eq!(sb.serve(), user, "利用者の設定を外した");
    assert!(
        !sb.calls()
            .iter()
            .any(|c| c.starts_with("serve --bg") || c.contains("off")),
        "利用者の設定に書き込んだ: {:?}",
        sb.calls()
    );
}

#[test]
fn abでは張った後に断るとserveの設定が残る() {
    if !daemon_can_start() {
        return;
    }
    let sb = Sandbox::new("legacy");
    std::fs::write(sb.state().join("devices.json"), "{").expect("devices.json を書ける");
    let out = sb.start(&[("TAKO_2793_LEGACY", "1")]);
    assert_ne!(out.code, 0, "start は断られる: {}", out.stderr);
    let target = sb.set_target().expect("断る前に serve を張っている");
    assert_eq!(
        sb.serve(),
        serve_json_to(&target),
        "A/B（修正前）では張った到達先を向いたまま残る（このテストが残りを見分けられることの裏付け）"
    );
}
