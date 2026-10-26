//! 検証スクリプトの器の後片付けの 1 実装 `scripts/lib/tmux-socket.sh` の振る舞い（Issue #2399）
//!
//! tmux は `kill-server` で落としても、最後のセッションが閉じて自分で抜けても
//! `$TMUX_TMPDIR/tmux-<uid>/<名前>` のソケットファイルを残す。検証スクリプトは
//! `tmux_socket_kill <名前>` で器とファイルを一緒に片付ける（どこで呼ぶべきかの番犬は
//! `shell_scripts.rs` の `器を起こすスクリプトはソケットファイルまで消す`）。
//!
//! - 偽の tmux（終了コードと文言を env で決める）で、消す / 残すの分かれ目を確かめる
//!   （tmux の無い CI のランナーでも走る）
//! - 本物の tmux（門 = `tako_core::tmux_test_gate::real_tmux`）で、生きた器・自分で抜けた器・
//!   途中で落ちたスクリプトの EXIT の trap（失敗の終了・TERM）のどれでも 0 個になること、
//!   旧い形（`kill-server` だけ）では 1 個残ること（検出力）を確かめる
//!
//! 器はすべて一時の `TMUX_TMPDIR` の中に立てる（利用者の `/tmp/tmux-<uid>/` に触らない）。
//! 器の中身は `sleep 20` なので、テストが途中で殺されても器は 20 秒で自分で抜ける。
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn uid() -> String {
    let out = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .expect("id -u");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 一時の `TMUX_TMPDIR`。unix ソケットのパス長の上限に当たらないよう `/tmp` の直下に短く作る。
/// Drop で立てた器を落とし、置き場ごと消す（失敗の経路の巻き戻しでも走る）
struct Sandbox {
    dir: PathBuf,
    tmux: Option<String>,
    servers: Vec<&'static str>,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = PathBuf::from(format!("/tmp/tk2399-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).expect("一時 dir を作る");
        Sandbox {
            dir,
            tmux: None,
            servers: Vec::new(),
        }
    }

    fn socket_dir(&self) -> PathBuf {
        self.dir.join(format!("tmux-{}", uid()))
    }

    /// ソケットの置き場に残っているファイルの名前
    fn files(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(self.socket_dir())
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    /// 偽の器のファイル（中身は空）を置く
    fn touch(&self, name: &str) {
        std::fs::create_dir_all(self.socket_dir()).expect("置き場を作る");
        std::fs::write(self.socket_dir().join(name), "").expect("ファイルを置く");
    }

    /// 呼ぶたびに引数を 1 行ずつ記録し、`STUB_OUT` を stderr へ出して `STUB_RC` で抜ける偽の tmux
    fn stub_tmux(&self) -> PathBuf {
        let path = self.dir.join("bin/tmux");
        std::fs::write(
            &path,
            "#!/bin/sh\necho \"$*\" >> \"$STUB_CALLS\"\nprintf '%s\\n' \"$STUB_OUT\" >&2\nexit \"${STUB_RC:-0}\"\n",
        )
        .expect("偽の tmux を書く");
        let _ = Command::new("/bin/chmod").arg("+x").arg(&path).status();
        path
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(self.dir.join("calls")).unwrap_or_default()
    }

    /// 1 実装を読んだ `/bin/bash`（macOS 同梱の 3.2）で `script` を走らせる
    fn run(&self, script: &str, env: &[(&str, &str)]) -> Output {
        Command::new("/bin/bash")
            .arg("-c")
            .arg(format!(". \"$TMUX_SOCKET_LIB\"\n{script}"))
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", &self.dir)
            .env("TMUX_TMPDIR", &self.dir)
            .env(
                "TMUX_SOCKET_LIB",
                repo_root().join("scripts/lib/tmux-socket.sh"),
            )
            .env("STUB_CALLS", self.dir.join("calls"))
            .envs(env.iter().copied())
            .output()
            .expect("/bin/bash を起こす")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Some(tmux) = &self.tmux {
            for s in &self.servers {
                let _ = Command::new(tmux)
                    .args(["-L", s, "kill-server"])
                    .env("TMUX_TMPDIR", &self.dir)
                    .output();
                let _ = std::fs::remove_file(self.socket_dir().join(s));
            }
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn 落とせた器ともう居ない器のファイルだけを消し_落とせない器のファイルは残す() {
    let sb = Sandbox::new("stub");
    let stub = sb.stub_tmux();
    let stub = stub.to_str().expect("utf-8 のパス");
    let case = |rc: &str, msg: &str| -> (Vec<String>, String) {
        sb.touch("w1");
        sb.touch("other");
        let out = sb.run(
            "set -euo pipefail\ntmux_socket_kill w1\necho done",
            &[("REAL_TMUX", stub), ("STUB_RC", rc), ("STUB_OUT", msg)],
        );
        assert!(
            out.status.success(),
            "set -e の下でも抜けずに 0 で返る: {}",
            stderr(&out)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout), "done\n");
        let files = sb.files();
        let _ = std::fs::remove_file(sb.socket_dir().join("w1"));
        (files, stderr(&out))
    };
    // 落とせた（0）/ もう居ない（no server running）= 消す。他の名前のファイルは触らない
    assert_eq!(case("0", "").0, vec!["other"]);
    assert_eq!(
        case("1", "no server running on /tmp/x/tmux-1/w1").0,
        vec!["other"]
    );
    // 繋げない（器が生きているかもしれない）= 残して 1 行出す
    let (files, err) = case(
        "1",
        "error connecting to /tmp/x/tmux-1/w1 (Permission denied)",
    );
    assert_eq!(files, vec!["other", "w1"]);
    assert!(
        err.contains("器 w1 を落とせないのでソケットファイルを残す"),
        "{err}"
    );
    let (files, err) = case("1", "protocol version mismatch (client 8, server 7)");
    assert_eq!(files, vec!["other", "w1"]);
    assert!(err.contains("ソケットファイルを残す"), "{err}");
    // ファイルが無い = 黙って抜ける
    let (_, err) = case(
        "1",
        "error connecting to /tmp/x/tmux-1/w1 (No such file or directory)",
    );
    assert!(!err.contains("tmux_socket_kill"), "{err}");
    assert!(
        sb.calls().lines().all(|l| l == "-L w1 kill-server"),
        "{}",
        sb.calls()
    );
}

#[test]
fn 空の名前と本番や既定の器の名前には触らない() {
    let sb = Sandbox::new("names");
    let stub = sb.stub_tmux();
    for name in ["tako", "default", "w1"] {
        sb.touch(name);
    }
    let out = sb.run(
        "set -euo pipefail\nEMPTY=\ntmux_socket_kill \"$EMPTY\" tako default a/b .. && echo done",
        &[("REAL_TMUX", stub.to_str().expect("utf-8 のパス"))],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "done\n");
    assert_eq!(sb.calls(), "", "tmux を 1 度も呼ばない");
    assert_eq!(sb.files(), vec!["default", "tako", "w1"]);
    let err = stderr(&out);
    for name in ["'tako'", "'default'", "'a/b'", "'..'"] {
        assert!(
            err.contains(&format!("{name} は検証用の器の名前ではないので触らない")),
            "{err}"
        );
    }
    // 空の名前は正常（器を起こす前に後片付けが走った）なので何も言わない
    assert_eq!(err.lines().count(), 4, "{err}");
}

#[test]
fn tmuxが無ければ黙ってファイルを残す() {
    let sb = Sandbox::new("notmux");
    sb.touch("w1");
    let missing = sb.dir.join("bin/no-such-tmux");
    let out = sb.run(
        "tmux_socket_kill w1; echo \"rc=$?\"",
        &[("REAL_TMUX", missing.to_str().expect("utf-8 のパス"))],
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "rc=0\n");
    assert!(
        !stderr(&out).contains("tmux_socket_kill"),
        "{}",
        stderr(&out)
    );
    assert_eq!(sb.files(), vec!["w1"]);
}

/// 本物の tmux の器の生死（`-L <名前> ls` が通るか）
fn alive(sb: &Sandbox, name: &str) -> bool {
    let tmux = sb.tmux.as_deref().expect("本物の tmux");
    Command::new(tmux)
        .args(["-L", name, "ls"])
        .env("TMUX_TMPDIR", &sb.dir)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// 本物の tmux で器を立て、ソケットファイルができたのを確かめる
fn start_server(sb: &mut Sandbox, name: &'static str, command: &str) {
    let tmux = sb.tmux.clone().expect("本物の tmux");
    sb.servers.push(name);
    let out = Command::new(&tmux)
        .args(["-f", "/dev/null", "-L", name, "new-session", "-d", command])
        .env("TMUX_TMPDIR", &sb.dir)
        .env("HOME", &sb.dir)
        .output()
        .expect("tmux を起こす");
    assert!(out.status.success(), "器を立てられない: {}", stderr(&out));
    assert!(
        sb.files().iter().any(|f| f == name),
        "器 {name} のソケットファイルができない: {:?}",
        sb.files()
    );
}

/// 本物の tmux の絶対パス。子の bash は PATH を `/usr/bin:/bin` に絞るので、
/// `tmux_bin()` が名前だけ（PATH で見つかった）を返したときはこちらの PATH で引き直す
fn real_tmux_path() -> String {
    let bin = tako_core::tmux::tmux_bin();
    if Path::new(bin).is_absolute() {
        return bin.to_string();
    }
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(bin))
        .find(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| panic!("門は通ったのに {bin} の置き場を PATH から引けない"))
}

fn real_sandbox(tag: &str) -> Option<Sandbox> {
    if !tako_core::tmux_test_gate::real_tmux() {
        return None;
    }
    let mut sb = Sandbox::new(tag);
    sb.tmux = Some(real_tmux_path());
    Some(sb)
}

#[test]
fn 本物のtmuxで生きた器も自分で抜けた器もファイルまで片付く() {
    let Some(mut sb) = real_sandbox("real") else {
        return;
    };
    let tmux = sb.tmux.clone().expect("本物の tmux");
    // 旧い形（kill-server だけ）は 1 個残す = この検査の検出力
    start_server(&mut sb, "old", "sleep 20");
    let out = sb.run("\"$REAL_TMUX\" -L old kill-server", &[("REAL_TMUX", &tmux)]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!alive(&sb, "old"));
    assert_eq!(
        sb.files(),
        vec!["old"],
        "tmux は kill-server の後もファイルを残す"
    );
    std::fs::remove_file(sb.socket_dir().join("old")).expect("旧い形の残りを消す");

    // 生きた器（落として消す）と自分で抜けた器（もう居ないので消す）を 1 回で
    start_server(&mut sb, "live", "sleep 20");
    start_server(&mut sb, "gone", "true");
    for _ in 0..50 {
        if !alive(&sb, "gone") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!alive(&sb, "gone"), "最後のセッションが閉じた器が抜けない");
    assert_eq!(
        sb.files(),
        vec!["gone", "live"],
        "自分で抜けた器もファイルを残す"
    );
    let out = sb.run("tmux_socket_kill live gone", &[("REAL_TMUX", &tmux)]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "", "落とせた器について何も言わない");
    assert!(!alive(&sb, "live"));
    assert_eq!(sb.files(), Vec::<String>::new());
}

#[test]
fn 本物のtmuxで途中で落ちたスクリプトもexitのtrapで片付く() {
    let Some(mut sb) = real_sandbox("trap") else {
        return;
    };
    let tmux = sb.tmux.clone().expect("本物の tmux");
    sb.servers.extend(["fail", "term"]);
    // set -e の途中の失敗（終了コードは失敗のまま）
    let out = sb.run(
        "set -euo pipefail\nS=fail\ntrap 'tmux_socket_kill \"$S\"' EXIT\n\
         \"$REAL_TMUX\" -L \"$S\" -f /dev/null new-session -d 'sleep 20'\n\
         [ -S \"$TMUX_TMPDIR/tmux-$(id -u)/$S\" ] && echo started\nfalse\necho unreachable",
        &[("REAL_TMUX", &tmux)],
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "started\n");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(!alive(&sb, "fail"));
    assert_eq!(sb.files(), Vec::<String>::new());
    // TERM で止められた（シグナルの trap が exit して EXIT の trap が走る形）
    let out = sb.run(
        "S=term\ntrap 'tmux_socket_kill \"$S\"' EXIT\ntrap 'exit 143' TERM\n\
         \"$REAL_TMUX\" -L \"$S\" -f /dev/null new-session -d 'sleep 20'\n\
         [ -S \"$TMUX_TMPDIR/tmux-$(id -u)/$S\" ] && echo started\nkill -TERM $$\nsleep 5\necho unreachable",
        &[("REAL_TMUX", &tmux)],
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "started\n");
    assert_eq!(out.status.code(), Some(143), "{}", stderr(&out));
    assert!(!alive(&sb, "term"));
    assert_eq!(sb.files(), Vec::<String>::new());
}
