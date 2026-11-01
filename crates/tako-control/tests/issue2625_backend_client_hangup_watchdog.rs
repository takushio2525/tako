//! **#2625 の番犬**: 器（tmux）のペインを閉じるとき、attach クライアントを先に帰してから PTY の親側を閉じる。
//!
//! ## なぜ縛るのか
//!
//! 器のペインの PTY 直下の子は tmux の attach クライアントで、その tty は tmux サーバーも開いている。
//! 従来の閉じ方（`Msg::Shutdown` → `Pty` の drop = 読むのをやめる → SIGHUP → 子を wait → 親側を閉じる）では
//! tty の**最後の close が tmux サーバーの中**（`server_client_lost`）で起き、読まれない出力を抱えた
//! tty の close が約 600ms 戻らなかった。tmux サーバーは 1 本のスレッドなので、その間は器のサーバー全体
//! （他の器のペインの画面・tako が器へ打つ命令・`tako list` の器の一覧）が止まる（macOS の隔離 GUI で
//! 閉じるたびに外から打った `tmux ls` が 593〜606ms・tmux の sample で `server_client_lost → close` が 482/796）。
//! いまは閉じる知らせ `Msg::HangUpAndDrain` を受けた PTY のスレッドが、子へ SIGHUP を送り、子の側が
//! 全部閉じるまで親側を読み捨ててから閉じる（サーバーの close は待たない）。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. 端末の drop は、閉じ方の印が立っていれば `Msg::HangUpAndDrain` を送る（`Msg::Shutdown` 決め打ちに戻さない）。
//!    drop 自身は子へシグナルを送らない（子を回収した後の pid に当たりうる。送るのは PTY のスレッド）
//! 2. PTY のスレッドは閉じる知らせで抜けたときだけ、`Pty` を落とす前（`deregister` の前）に
//!    `hang_up_and_drain(` を通る。SIGHUP を送るのはその関数だけで、子の側が閉じた（`Ok(0)`）まで読み、上限を持つ
//! 3. 器のペインの端末は起こした直後に閉じ方の印を立てる（`spawn_session` の `backend_session.is_some()` の枝・
//!    判断は `tmux_backend::hang_up_client_on_close()` の 1 か所）
//! 4. 判断は unix・tmux の器・A/B（`TAKO_2625_LEGACY=1`）でない、の 3 つを見る（psmux = Windows では順を変えない）

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const TERMINAL: &str = "crates/tako-core/src/terminal.rs";
const PTY_LOOP: &str = "crates/tako-core/src/pty_loop.rs";
const TMUX_BACKEND: &str = "crates/tako-core/src/tmux_backend.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す）。下限つき = 黙って縮んだら落ちる
fn read(rel: &str) -> String {
    // tmux_backend.rs はテストが本番より厚い（本番の割合は 2 割台）ので下限を下げて読む
    // （`production_range::MIN_COVERAGE` の注記と同じ扱い）
    let floor = if rel == TMUX_BACKEND {
        0.20
    } else {
        production_range::MIN_COVERAGE
    };
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::code_view::without_comments(&production_range::production_with_floor(
        &src, rel, floor,
    ))
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置。文字列の中の括弧は数えない
fn block_end(src: &str, open: usize) -> usize {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let start = code[open..]
        .find('{')
        .map(|i| open + i)
        .expect("`{` がある");
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("対応する `}}` が無い");
}

/// `sig` で始まるブロックの範囲（シグネチャの位置〜閉じ括弧）
fn span_of(src: &str, sig: &str, rel: &str) -> (usize, usize) {
    let at = src.find(sig).unwrap_or_else(|| {
        panic!("{rel} に `{sig}` が無い（構造が変わったらこの番犬も見直すこと）")
    });
    (at, block_end(src, at))
}

fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// 1: 端末の drop は閉じ方の印を見て `Msg::HangUpAndDrain` を送り、自分ではシグナルを送らない
#[test]
fn issue2625_端末のdropは器のクライアントを先に帰す知らせを送る() {
    let src = read(TERMINAL);
    let (start, end) = span_of(&src, "impl Drop for TerminalSession {", TERMINAL);
    let body = &src[start..end];
    for needle in ["Msg::HangUpAndDrain", "self.hang_up_on_close"] {
        assert!(
            body.contains(needle),
            "{TERMINAL}:{} の端末の drop が `{needle}` を見ていない（閉じ方の印を無視して `Msg::Shutdown` を送ると、\
             器のクライアントの tty の最後の close が tmux サーバーの中で起き、器のサーバー全体が約 600ms 止まる = #2625）",
            line_at(&src, start)
        );
    }
    let mut offenders = Vec::new();
    for needle in ["libc::kill", "SIGHUP"] {
        for at in all_at(body, needle) {
            offenders.push(format!(
                "{TERMINAL}:{} `{needle}`",
                line_at(&src, start + at)
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "端末の drop が自分で子へシグナルを送っている（PTY のスレッドが子を回収した後なら pid が別のプロセスへ \
         再利用されうる。送るのは閉じる知らせを受けた PTY のスレッド = `hang_up_and_drain` だけ。#2625）:\n{}",
        offenders.join("\n")
    );
}

/// 2: SIGHUP は閉じる知らせで抜けたときの `hang_up_and_drain` だけ・`Pty` を落とす前・子の側が閉じるまで読む
#[test]
fn issue2625_ptyのスレッドは子の側が閉じるまで親側を読んでから閉じる() {
    let src = read(PTY_LOOP);
    let (drain_start, drain_end) = span_of(
        &src,
        "    #[cfg(unix)]\n    fn hang_up_and_drain(",
        PTY_LOOP,
    );
    let drain = &src[drain_start..drain_end];
    for needle in ["libc::SIGHUP", ".read(", "Ok(0) => return", "limit"] {
        assert!(
            drain.contains(needle),
            "{PTY_LOOP}:{} の `hang_up_and_drain` に `{needle}` が無い（子へ SIGHUP を送り、子の側が全部閉じる = \
             EOF まで親側を読み、上限で抜ける形でないと、tmux サーバーの close が読まれない出力を抱えて止まる = #2625）",
            line_at(&src, drain_start)
        );
    }
    // SIGHUP を送るのは hang_up_and_drain の中だけ（子の終了の腕の後で送ると回収済みの pid に当たる）
    let mut offenders = Vec::new();
    for at in all_at(&src, "SIGHUP") {
        if !(drain_start..drain_end).contains(&at) {
            offenders.push(format!("{PTY_LOOP}:{} `SIGHUP`", line_at(&src, at)));
        }
    }
    assert!(
        offenders.is_empty(),
        "pty_loop.rs で `hang_up_and_drain` の外から SIGHUP を送っている（#2625）:\n{}",
        offenders.join("\n")
    );
    // 印を立てるのは閉じる知らせの受け口だけ（立てたら即ループを抜ける = 子の終了の腕を通っていない）
    let (recv_start, recv_end) = span_of(&src, "fn drain_recv_channel(", PTY_LOOP);
    for at in all_at(&src, "self.hang_up = Some(") {
        assert!(
            (recv_start..recv_end).contains(&at),
            "{PTY_LOOP}:{} が閉じる知らせの受け口（`drain_recv_channel`）の外で印を立てている（子を回収した後に \
             SIGHUP を送りうる。#2625）",
            line_at(&src, at)
        );
    }
    // 呼ぶのはループを抜けた後・`deregister`（= `Pty` を落とす直前）の前
    let (spawn_start, spawn_end) = span_of(
        &src,
        "    pub fn spawn(mut self) -> JoinHandle<()> {",
        PTY_LOOP,
    );
    let spawn = &src[spawn_start..spawn_end];
    let call = spawn.find("self.hang_up_and_drain(").unwrap_or_else(|| {
        panic!(
            "{PTY_LOOP}:{} の PTY のスレッドが `hang_up_and_drain` を呼んでいない（閉じる知らせを受けても親側を読まずに \
             閉じる = #2625 が戻る）",
            line_at(&src, spawn_start)
        )
    });
    let take = spawn
        .find("self.hang_up.take()")
        .expect("`self.hang_up.take()` で閉じる知らせの印を読む");
    let loop_label = spawn
        .find("'event_loop: loop {")
        .expect("`'event_loop: loop {` がある");
    let loop_end = block_end(spawn, loop_label);
    let deregister = spawn[loop_end..]
        .find("self.pty.deregister(")
        .map(|i| loop_end + i)
        .expect("ループの後の `deregister`");
    assert!(
        loop_end < take && take < call && call < deregister,
        "{PTY_LOOP}:{} の `hang_up_and_drain` がループを抜けた後・`deregister` の前に無い（ループの中で呼ぶと読み書きの \
         途中で SIGHUP する・`Pty` を落とした後だと親側はもう閉じている = #2625）",
        line_at(&src, spawn_start + call)
    );
}

/// 3: 器のペインの端末は起こした直後に閉じ方の印を立てる（判断は core の 1 か所）
#[test]
fn issue2625_器のペインの端末は閉じ方の印を立てて起こす() {
    let src = read(MAIN);
    let (start, end) = span_of(&src, "    fn spawn_session(", MAIN);
    let body = &src[start..end];
    let call = body.find("session.hang_up_client_on_close();").unwrap_or_else(|| {
        panic!(
            "{MAIN}:{} の `spawn_session` が器のペインの端末に閉じ方の印を立てていない（閉じるたびに器の tmux サーバー \
             全体が約 600ms 止まる = #2625）。`session.hang_up_client_on_close()`",
            line_at(&src, start)
        )
    });
    let cond = body[..call].rfind("if ").expect("印を立てる枝の `if`");
    let cond_text = &body[cond..call];
    for needle in [
        "backend_session.is_some()",
        "tako_core::tmux_backend::hang_up_client_on_close()",
    ] {
        assert!(
            cond_text.contains(needle),
            "{MAIN}:{} の印を立てる条件に `{needle}` が無い（器でない直接のペインは閉じ方を変えない・判断は \
             `tmux_backend::hang_up_client_on_close` の 1 か所 = A/B と psmux の除外が効く。#2625）",
            line_at(&src, start + cond)
        );
    }
    let spawned = body
        .find("TerminalSession::spawn_in(")
        .expect("`spawn_session` の `TerminalSession::spawn_in(`");
    let inserted = body
        .find("self.terminals.insert(pane_id, session);")
        .expect("`spawn_session` の `self.terminals.insert(pane_id, session);`");
    assert!(
        spawned < call && call < inserted,
        "{MAIN}:{} の印を立てるのが端末を起こした後・`terminals` へ入れる前に無い（入れた後の経路は印の無い端末を \
         drop しうる。#2625）",
        line_at(&src, start + call)
    );
}

/// 4: 判断は unix・tmux の器・A/B でない の 3 つ
#[test]
fn issue2625_判断はunixとtmuxの器とab_の3つを見る() {
    let src = read(TMUX_BACKEND);
    let (start, end) = span_of(
        &src,
        "pub fn hang_up_client_on_close() -> bool {",
        TMUX_BACKEND,
    );
    let body = &src[start..end];
    for needle in [
        "cfg!(unix)",
        "crate::backend::Choice::Tmux",
        "!issue2625_legacy()",
    ] {
        assert!(
            body.contains(needle),
            "{TMUX_BACKEND}:{} の `hang_up_client_on_close` に `{needle}` が無い（Windows の psmux = ConPTY では閉じ方を \
             変えない・`TAKO_2625_LEGACY=1` で修正前へ戻せる、を保つ。#2625）",
            line_at(&src, start)
        );
    }
    let (gate_start, gate_end) = span_of(&src, "pub fn issue2625_legacy() -> bool {", TMUX_BACKEND);
    assert!(
        src[gate_start..gate_end].contains("\"TAKO_2625_LEGACY\""),
        "{TMUX_BACKEND}:{} の A/B のゲートが `TAKO_2625_LEGACY` を読んでいない",
        line_at(&src, gate_start)
    );
}
