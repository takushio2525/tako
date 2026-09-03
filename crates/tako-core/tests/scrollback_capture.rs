//! scrollback 採取の統合テスト（#972。**実バイナリを使う**）
//!
//! 単体テスト（`tmux.rs` 内）は「tako がどう argv を組むか」を固定する。
//! こちらは **器が実際にそう答えるか**を確かめる。
//!
//! #972 の核心は「境界を通らない呼び方は器が違うと壊れる」で、macOS では
//! **tmux 3.6 が裸の `=session` を target-pane として解決できない**という形で現れる
//! （実測 3.6b: `can't find pane: =<name>`）。ここはその経路を実 tmux で押さえる。
//!
//! 本物の tmux が無い環境（psmux しか無い Windows 等）ではスキップする。
//! psmux 側の同じ検査は `psmux_backend.rs` が持つ。
//!
//! **ソケットは必ず隔離する**（`tako-972test-<pid>`）。後始末は
//! 必ずソケット指定つきの `kill-server` で行う（本番の器に触らない）。

use std::process::Command;

use tako_core::backend::{DetachedCapture, SessionRef, TmuxBackend};

/// 本物の tmux（tmux だけを名乗る実装）が使えるか
fn real_tmux() -> Option<String> {
    let bin = tako_core::tmux::tmux_bin().to_string();
    let out = Command::new(&bin).arg("-V").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let version = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    tako_core::tmux::announces_only_tmux(&version).then_some(bin)
}

struct Fixture {
    bin: String,
    socket: String,
    session: String,
}

impl Fixture {
    /// 履歴へ押し出す行と、現画面に残る行の両方を作る
    fn new(tag: &str) -> Option<Self> {
        let bin = real_tmux()?;
        let socket = format!("tako-972test-{}-{tag}", std::process::id());
        let session = format!("tako972{tag}");
        let script = "for i in $(seq 1 120); do echo LINE_$i; done; echo TAIL_MARKER; sleep 300";
        let ok = Command::new(&bin)
            .args([
                "-L",
                &socket,
                "new-session",
                "-d",
                "-x",
                "80",
                "-y",
                "24",
                "-s",
                &session,
                "sh",
                "-c",
                script,
            ])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            return None;
        }
        // 出力が落ち着くまで待つ（固定待ちにしない = #796 の作法）
        let fixture = Self {
            bin,
            socket,
            session,
        };
        for _ in 0..100 {
            if fixture
                .capture(500)
                .is_some_and(|l| l.iter().any(|x| x.contains("TAIL_MARKER")))
            {
                return Some(fixture);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        Some(fixture)
    }

    fn backend(&self) -> TmuxBackend {
        TmuxBackend::with_socket(self.socket.clone())
    }

    fn session_ref(&self) -> SessionRef {
        SessionRef::new(&self.session).expect("セッション名が不正")
    }

    fn capture(&self, lines: usize) -> Option<Vec<String>> {
        tako_core::tmux::capture_scrollback_plain(Some(&self.socket), &self.session, lines).ok()
    }

    /// #972 以前の呼び方（裸の `=session`）を実 tmux へそのまま投げる
    fn legacy_capture(&self) -> (bool, String) {
        let out = Command::new(&self.bin)
            .args([
                "-L",
                &self.socket,
                "capture-pane",
                "-t",
                &tako_core::tmux::exact_target(&self.session),
                "-p",
                "-S",
                "-500",
            ])
            .output()
            .expect("tmux を実行できない");
        (
            out.status.success() && !String::from_utf8_lossy(&out.stdout).trim().is_empty(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // **必ず `-L` 付き**（落とすのは隔離ソケットのサーバーだけ）
        let _ = Command::new(&self.bin)
            .args(["-L", &self.socket, "kill-server"])
            .output();
    }
}

/// 境界（`DetachedCapture::capture_scrollback`）が履歴と現画面の両方を返すこと
#[test]
fn 器の境界経由で履歴と現画面が採れる() {
    let Some(fx) = Fixture::new("main") else {
        eprintln!("SKIP: 本物の tmux が無い（psmux 等は psmux_backend.rs が見る）");
        return;
    };
    let lines = fx
        .backend()
        .capture_scrollback(&fx.session_ref(), 500)
        .expect("境界経由の採取が失敗した");
    assert!(
        lines.iter().any(|l| l.contains("TAIL_MARKER")),
        "現画面の行が入っていない: {:?}",
        lines.last_chunk::<5>()
    );
    assert!(
        lines.iter().any(|l| l.contains("LINE_1")),
        "履歴の行が入っていない（-E が付いていないか / 履歴が浅い）: {} 行",
        lines.len()
    );
}

/// 行数指定が効く（履歴の遡り幅）
#[test]
fn 行数指定で遡り幅が変わる() {
    let Some(fx) = Fixture::new("lines") else {
        eprintln!("SKIP: 本物の tmux が無い");
        return;
    };
    let few = fx.capture(5).expect("採取が失敗した");
    let many = fx.capture(500).expect("採取が失敗した");
    assert!(
        many.len() > few.len(),
        "遡り幅が効いていない: few={} many={}",
        few.len(),
        many.len()
    );
}

/// **#972 の機序そのもの**: 旧実装のターゲット（裸の `=session`）は
/// この tmux では解決できない。ここが「成功」に転じたら、
/// 旧経路が偶然通る tmux で回っている（= この検査は無効）ことの通知になる
#[test]
fn 裸の完全一致ターゲットではペインを解決できない() {
    let Some(fx) = Fixture::new("legacy") else {
        eprintln!("SKIP: 本物の tmux が無い");
        return;
    };
    if tako_core::tmux::target_syntax() != tako_core::tmux::TmuxTargetSyntax::Exact {
        eprintln!("SKIP: この CLI は `=` を使わない（psmux 等）");
        return;
    }
    let (ok, stderr) = fx.legacy_capture();
    assert!(
        !ok,
        "旧経路が通ってしまった（この tmux では #972 の macOS 側の症状が再現しない）: stderr={stderr}"
    );
    assert!(
        stderr.contains("can't find pane"),
        "想定と違う失敗の仕方: {stderr}"
    );
}
