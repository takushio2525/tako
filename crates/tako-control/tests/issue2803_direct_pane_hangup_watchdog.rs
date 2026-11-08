//! **#2803 の番犬**: 器なしのペイン（PTY 直下の子がログインシェル）を閉じるときも、子へ SIGHUP を送って
//! 子の側が全部閉じるまで親側を読み捨ててから閉じる（#2625 の閉じ方を器なしの素のペインへ広げる）。
//! 読み捨てが上限を過ぎたら、`Pty` を落とす前に回線を切る。
//!
//! ## なぜ縛るのか
//!
//! 従来の閉じ方（`Msg::Shutdown` → `Pty` の drop = 読むのをやめる → SIGHUP → 子を wait → 親側を閉じる）では、
//! シェルが SIGHUP で終わる間際に端末へ書くと誰も読まない。zsh は前景のジョブ（`bash -c '…'`・bash の入口を
//! 持つ CLI）が残っていれば `zsh: warning: 1 jobs SIGHUPed` を書き、セッションリーダーの exit は端末の出力が
//! 読まれるまで待つ（macOS）。シェルは exit の途中（`ps` の STAT `?Es`）で、PTY のスレッドは `Pty` の drop の
//! `wait4` で止まり（隔離 GUI の sample で 17/17）、親側の fd（`/dev/ptmx`）も閉じない = GUI を終了するまで
//! プロセス 2 本・fd・スレッドが残った。シェルが `trap '' HUP` だと読み捨てても終わらないので、上限を
//! 過ぎたら親側を `/dev/null` に差し替えて回線を切る（子の側の read が EOF になって終わる）。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `spawn_session` の印を立てる条件に、器なしの素のペイン（`backend_session.is_none()` かつ
//!    `!self.tmux_view_panes.contains_key(&pane_id)`）の枝があり、判断は `pty_loop::hang_up_direct_on_close()`
//!    の 1 か所。この判断は `spawn_session` の外から呼ばない
//! 2. 判断は unix・A/B（`TAKO_2803_LEGACY=1`）でない の 2 つを見る（Windows = ConPTY には tty の close が無い）
//! 3. PTY のスレッドは読み捨てが上限を過ぎたとき（`hang_up_and_drain` が偽）だけ、監視から外した後
//!    （`deregister` の後）に回線を切る（`hang_up_line`）。回線を切るのは `dup2` で親側の番号の中身だけを
//!    差し替える形（番号を空けない = 他のスレッドが開いた fd と取り違えない）で、A/B では切らない
//! 4. 端末は PTY のスレッドへ親側の fd を渡す（`with_master_fd`。渡さないと回線を切る段が何もしない）
//!
//! 読み捨てそのもの（SIGHUP・EOF まで読む・上限）は #2625 の番犬
//! （`issue2625_backend_client_hangup_watchdog.rs`）が縛る。単体テストは `tako_core::terminal` の
//! `印を立てた端末は…` 2 件、実経路（Issue の対照表・境界・A/B）は `bash scripts/test-direct-close-2803.sh`

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const TERMINAL: &str = "crates/tako-core/src/terminal.rs";
const PTY_LOOP: &str = "crates/tako-core/src/pty_loop.rs";

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
    let src = std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
    production_range::code_view::without_comments(&production_range::production_with_floor(
        &src,
        rel,
        production_range::MIN_COVERAGE,
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

/// 1: 器なしの素のペインも、起こした直後に閉じ方の印を立てる（判断は core の 1 か所・呼ぶのは spawn_session だけ）
#[test]
fn issue2803_器なしの素のペインも閉じ方の印を立てて起こす() {
    let src = read(MAIN);
    let (start, end) = span_of(&src, "    fn spawn_session(", MAIN);
    let body = &src[start..end];
    let call = body
        .find("session.hang_up_client_on_close();")
        .unwrap_or_else(|| {
            panic!(
                "{MAIN}:{} の `spawn_session` が閉じ方の印（`session.hang_up_client_on_close()`）を立てていない（#2803）",
                line_at(&src, start)
            )
        });
    let cond = body[..call].rfind("if ").expect("印を立てる枝の `if`");
    let cond_text = &body[cond..call];
    for needle in [
        "!self.tmux_view_panes.contains_key(&pane_id)",
        "tako_core::pty_loop::hang_up_direct_on_close()",
    ] {
        assert!(
            cond_text.contains(needle),
            "{MAIN}:{} の印を立てる条件に `{needle}` が無い（器なしの素のペインに印が無いと、シェルが HUP で終わる間際に \
             端末へ書いた出力を誰も読まず、シェルの exit と PTY のスレッドの wait が互いに待って、プロセス・PTY の fd・\
             スレッドが GUI の終了まで残る。ビューペインは #2634 の枝 = `TAKO_2634_LEGACY` の A/B を混ぜない。#2803）",
            line_at(&src, start + cond)
        );
    }
    let direct = cond_text
        .find("tako_core::pty_loop::hang_up_direct_on_close()")
        .expect("上で確かめた");
    let branch_start = cond_text[..direct]
        .rfind("||")
        .expect("器なしの素のペインの枝は `||` で足す");
    assert!(
        cond_text[branch_start..direct].contains("backend_session.is_none()"),
        "{MAIN}:{} の器なしの素のペインの枝が `backend_session.is_none()` を見ていない（器のペインは #2625 の枝 = \
         `TAKO_2625_LEGACY` の A/B を混ぜない。#2803）",
        line_at(&src, start + cond + branch_start)
    );
    // 判断を呼ぶのは spawn_session の印の枝だけ
    let mut offenders = Vec::new();
    for (at, _) in src.match_indices("hang_up_direct_on_close()") {
        if !(start + cond..start + call).contains(&at) {
            offenders.push(format!("{MAIN}:{}", line_at(&src, at)));
        }
    }
    assert!(
        offenders.is_empty(),
        "`hang_up_direct_on_close()` を spawn_session の印の枝の外から呼んでいる（印は端末を起こした直後の 1 か所で\
         立てる。閉じる経路はすべて端末の drop を通る。#2803）:\n{}",
        offenders.join("\n")
    );
}

/// 2: 判断は unix・A/B でない の 2 つ
#[test]
fn issue2803_判断はunixとab_の2つを見る() {
    let src = read(PTY_LOOP);
    let (start, end) = span_of(&src, "pub fn hang_up_direct_on_close() -> bool {", PTY_LOOP);
    let body = &src[start..end];
    for needle in ["cfg!(unix)", "!issue2803_legacy()"] {
        assert!(
            body.contains(needle),
            "{PTY_LOOP}:{} の `hang_up_direct_on_close` に `{needle}` が無い（Windows = ConPTY では閉じ方を変えない・\
             `TAKO_2803_LEGACY=1` で修正前へ戻せる、を保つ。#2803）",
            line_at(&src, start)
        );
    }
    let (gate_start, gate_end) = span_of(&src, "pub fn issue2803_legacy() -> bool {", PTY_LOOP);
    assert!(
        src[gate_start..gate_end].contains("\"TAKO_2803_LEGACY\""),
        "{PTY_LOOP}:{} の A/B のゲートが `TAKO_2803_LEGACY` を読んでいない",
        line_at(&src, gate_start)
    );
}

/// 3: 読み捨てが上限を過ぎたときだけ、監視から外した後に `dup2` で回線を切る（A/B では切らない）
#[test]
fn issue2803_上限を過ぎたら監視から外した後に回線を切る() {
    let src = read(PTY_LOOP);
    // 読み捨ては上限で偽を返す（子の側が閉じたら真）
    let (drain_start, drain_end) = span_of(
        &src,
        "    #[cfg(unix)]\n    fn hang_up_and_drain(",
        PTY_LOOP,
    );
    let drain = &src[drain_start..drain_end];
    for needle in ["-> bool", "return false;", "Ok(0) => return true"] {
        assert!(
            drain.contains(needle),
            "{PTY_LOOP}:{} の `hang_up_and_drain` に `{needle}` が無い（上限を過ぎたことを呼び手へ返さないと、回線を切る段が\
             要るかどうかを決められない。#2803）",
            line_at(&src, drain_start)
        );
    }
    // 回線を切るのは dup2 で番号の中身だけを差し替える形・close-on-exec を付け直す
    let (cut_start, cut_end) = span_of(&src, "    #[cfg(unix)]\n    fn hang_up_line(", PTY_LOOP);
    let cut = &src[cut_start..cut_end];
    for needle in ["self.master_fd", "libc::dup2(", "libc::FD_CLOEXEC"] {
        assert!(
            cut.contains(needle),
            "{PTY_LOOP}:{} の `hang_up_line` に `{needle}` が無い（親側の番号を空けずに中身だけ差し替える形でないと、\
             `Pty` の drop が他のスレッドの開いた fd を閉じうる。#2803）",
            line_at(&src, cut_start)
        );
    }
    for needle in ["libc::close(master", "libc::close(self.master_fd"] {
        assert!(
            !cut.contains(needle),
            "{PTY_LOOP}:{} の `hang_up_line` が親側の番号そのものを閉じている（`{needle}`。番号が空くと `Pty` の drop が \
             二重に閉じ、間に他のスレッドが開いた fd を閉じうる。#2803）",
            line_at(&src, cut_start)
        );
    }
    // 呼ぶのはループの後: 読み捨ての結果 → deregister → 回線を切る（A/B のゲートつき）
    let (spawn_start, spawn_end) = span_of(
        &src,
        "    pub fn spawn(mut self) -> JoinHandle<()> {",
        PTY_LOOP,
    );
    let spawn = &src[spawn_start..spawn_end];
    let loop_label = spawn
        .find("'event_loop: loop {")
        .expect("`'event_loop: loop {` がある");
    let loop_end = block_end(spawn, loop_label);
    let tail = &spawn[loop_end..];
    let decide = tail
        .find("!self.hang_up_and_drain(")
        .unwrap_or_else(|| {
            panic!(
                "{PTY_LOOP}:{} の PTY のスレッドが読み捨ての結果（上限を過ぎたか）を見ていない（#2803）",
                line_at(&src, spawn_start + loop_end)
            )
        });
    let gate = tail.find("!issue2803_legacy()").unwrap_or_else(|| {
        panic!(
            "{PTY_LOOP}:{} の回線を切る判断が A/B のゲート（`!issue2803_legacy()`）を見ていない（#2803）",
            line_at(&src, spawn_start + loop_end + decide)
        )
    });
    let deregister = tail
        .find("self.pty.deregister(")
        .expect("ループの後の `deregister`");
    let cut_call = tail.find("self.hang_up_line();").unwrap_or_else(|| {
        panic!(
            "{PTY_LOOP}:{} の PTY のスレッドが回線を切っていない（`self.hang_up_line()`。HUP を無視するシェルのペインを閉じると \
             `Pty` の drop の wait が返らず、シェル・PTY の fd・スレッドが残る。#2803）",
            line_at(&src, spawn_start + loop_end)
        )
    });
    assert!(
        decide < deregister && gate < deregister && deregister < cut_call,
        "{PTY_LOOP}:{} の回線を切る段が「読み捨ての結果 → `deregister` → `hang_up_line`」の順に無い（監視に載ったまま \
         fd の中身を差し替えない・`Pty` を落とした後では遅い。#2803）",
        line_at(&src, spawn_start + loop_end + cut_call)
    );
    // 回線を切る呼び出しはこの 1 か所だけ
    let calls: Vec<usize> = src
        .match_indices("self.hang_up_line()")
        .map(|(at, _)| line_at(&src, at))
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "{PTY_LOOP} で回線を切る呼び出しが 1 か所でない（行 {calls:?}。閉じる知らせで抜けて読み捨てが上限を過ぎたときだけ。#2803）"
    );
}

/// 4: 端末は PTY のスレッドへ親側の fd を渡す
#[test]
fn issue2803_端末はptyのスレッドへ親側のfdを渡す() {
    let src = read(TERMINAL);
    let (start, end) = span_of(&src, "    pub fn spawn_in(", TERMINAL);
    let body = &src[start..end];
    let new_loop = body
        .find("PtyLoop::new(")
        .expect("`spawn_in` の `PtyLoop::new(`");
    let give = body.find(".with_master_fd(").unwrap_or_else(|| {
        panic!(
            "{TERMINAL}:{} の `spawn_in` が PTY のスレッドへ親側の fd を渡していない（`with_master_fd`。渡さないと回線を切る段が\
             何もしない = HUP を無視するシェルのペインが閉じても残る。#2803）",
            line_at(&src, start)
        )
    });
    let thread = body
        .find("event_loop.spawn()")
        .expect("`spawn_in` の `event_loop.spawn()`");
    assert!(
        new_loop < give && give < thread,
        "{TERMINAL}:{} の `with_master_fd` が PTY のスレッドを起こす前に無い（#2803）",
        line_at(&src, start + give)
    );
}
