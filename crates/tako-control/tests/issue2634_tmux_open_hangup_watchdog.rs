//! **#2634 の番犬**: 器なしの `tako tmux open` のペインを閉じるときも、attach クライアントを先に帰してから
//! PTY の親側を閉じる（#2625 の閉じ方を利用者の tmux サーバーへ広げる）。
//!
//! ## なぜ縛るのか
//!
//! `tako tmux open`（#1185 / #1190）のペインは、器なし（persist OFF）だと PTY 直下の子が**利用者の**
//! tmux サーバーの attach クライアントになる（ログインシェルの `-c` が exec する）。その tty は利用者の
//! サーバーも開いているので、#2625 前の閉じ方（`Msg::Shutdown` → `Pty` の drop = 読むのをやめる → SIGHUP →
//! 子を wait → 親側を閉じる）では tty の最後の close が利用者のサーバーの中で起き、閉じるたびに
//! **利用者の tmux サーバー全体**が約 600ms 止まった（macOS の隔離 GUI で外から打った `tmux ls` が
//! 592〜606ms・5/5 回。器に包まれたビュー = persist ON は 6〜7ms で、外側が器のクライアントなので #2625 で
//! 既に止まらない）。いまは器のペインと同じ印（`TerminalSession::hang_up_client_on_close`）を立て、
//! PTY のスレッドが子の側が全部閉じるまで親側を読み捨ててから閉じる。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. `spawn_session` の印を立てる条件に、器なしのビューペイン（`backend_session.is_none()` かつ
//!    `self.tmux_view_panes.contains_key(&pane_id)`）の枝があり、判断は `tmux_backend::hang_up_view_client_on_close()`
//!    の 1 か所。この判断は `spawn_session` の外から呼ばない（経路ごとに書かない）
//! 2. dispatch の `TmuxOpen` は端末を起こす依頼（`host.attach_session(`）より先にビューを登録する
//!    （`host.track_tmux_view(`）。逆だと `spawn_session` の時点でビューと分からず印が立たない
//! 3. 判断は unix・A/B（`TAKO_2634_LEGACY=1`）でない の 2 つを見る（Windows = psmux / ConPTY では順を変えない）
//!
//! 閉じる知らせ（`Msg::HangUpAndDrain`）と PTY のスレッドの読み切りそのものは #2625 の番犬
//! （`issue2625_backend_client_hangup_watchdog.rs`）が縛る。実経路は `bash scripts/test-tmux-open-quit-2634.sh`

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
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
    // tmux_backend.rs / dispatch.rs はテストが本番より厚い（本番の割合は 2 割台）ので下限を下げて読む
    // （`production_range::MIN_COVERAGE` の注記と同じ扱い）
    let floor = if rel == TMUX_BACKEND || rel == DISPATCH {
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

/// 1: 器なしのビューペインも、起こした直後に閉じ方の印を立てる（判断は core の 1 か所・呼ぶのは spawn_session だけ）
#[test]
fn issue2634_器なしのtmux_openのペインも閉じ方の印を立てて起こす() {
    let src = read(MAIN);
    let (start, end) = span_of(&src, "    fn spawn_session(", MAIN);
    let body = &src[start..end];
    let call = body
        .find("session.hang_up_client_on_close();")
        .unwrap_or_else(|| {
            panic!(
                "{MAIN}:{} の `spawn_session` が閉じ方の印（`session.hang_up_client_on_close()`）を立てていない（#2625 / #2634）",
                line_at(&src, start)
            )
        });
    let cond = body[..call].rfind("if ").expect("印を立てる枝の `if`");
    let cond_text = &body[cond..call];
    for needle in [
        "backend_session.is_none()",
        "self.tmux_view_panes.contains_key(&pane_id)",
        "tako_core::tmux_backend::hang_up_view_client_on_close()",
    ] {
        assert!(
            cond_text.contains(needle),
            "{MAIN}:{} の印を立てる条件に `{needle}` が無い（器なしの `tako tmux open` のペインは PTY 直下の子が利用者の \
             tmux サーバーの attach クライアントなので、印が無いと閉じるたびに利用者の tmux サーバー全体が約 600ms 止まる。\
             器に包まれたビューは器の枝が立てる = `backend_session.is_none()` で #2625 の A/B を混ぜない。#2634）",
            line_at(&src, start + cond)
        );
    }
    // 判断を呼ぶのは spawn_session の印の枝だけ（閉じる経路ごと・ビューの経路ごとに書かない）
    let mut offenders = Vec::new();
    for (at, _) in src.match_indices("hang_up_view_client_on_close()") {
        if !(start + cond..start + call).contains(&at) {
            offenders.push(format!("{MAIN}:{}", line_at(&src, at)));
        }
    }
    assert!(
        offenders.is_empty(),
        "`hang_up_view_client_on_close()` を spawn_session の印の枝の外から呼んでいる（印は端末を起こした直後の 1 か所で\
         立てる。閉じる経路はすべて端末の drop を通る。#2634）:\n{}",
        offenders.join("\n")
    );
}

/// 2: dispatch の TmuxOpen は端末を起こす依頼より先にビューを登録する
#[test]
fn issue2634_tmux_openはビューを登録してから端末を起こす() {
    let src = read(DISPATCH);
    // 1 行 1 欄の形が本体の腕（#1989 の先取りの腕は `socket, session, ..` の 1 行）
    let sig = "        Request::TmuxOpen {\n            socket,\n            session,\n";
    let start = src.find(sig).unwrap_or_else(|| {
        panic!("{DISPATCH} に `TmuxOpen` の腕が無い（構造が変わったらこの番犬も見直すこと）")
    });
    // 最初の `{` はパターンの波括弧なので、腕の本体（`=> {`）から括弧を数える
    let arm = src[start..]
        .find("} => {")
        .map(|i| start + i + "} => ".len())
        .expect("`TmuxOpen` の腕の `=> {`");
    let end = block_end(&src, arm);
    let body = &src[start..end];
    let track = body.find("host.track_tmux_view(").unwrap_or_else(|| {
        panic!(
            "{DISPATCH}:{} の `TmuxOpen` がビューを登録していない（`host.track_tmux_view(`）",
            line_at(&src, start)
        )
    });
    let attach = body.find("host.attach_session(").unwrap_or_else(|| {
        panic!(
            "{DISPATCH}:{} の `TmuxOpen` が端末を起こす依頼（`host.attach_session(`）を出していない",
            line_at(&src, start)
        )
    });
    assert!(
        track < attach,
        "{DISPATCH}:{} の `TmuxOpen` が端末を起こす依頼の後にビューを登録している（`spawn_session` は \
         `tmux_view_panes` を見て閉じ方の印を立てるので、登録が後だと器なしのビューペインに印が立たず、閉じるたびに \
         利用者の tmux サーバー全体が約 600ms 止まる = #2634）",
        line_at(&src, start + track)
    );
}

/// 3: 判断は unix・A/B でない の 2 つ
#[test]
fn issue2634_判断はunixとab_の2つを見る() {
    let src = read(TMUX_BACKEND);
    let (start, end) = span_of(
        &src,
        "pub fn hang_up_view_client_on_close() -> bool {",
        TMUX_BACKEND,
    );
    let body = &src[start..end];
    for needle in ["cfg!(unix)", "!issue2634_legacy()"] {
        assert!(
            body.contains(needle),
            "{TMUX_BACKEND}:{} の `hang_up_view_client_on_close` に `{needle}` が無い（Windows = psmux / ConPTY では閉じ方を \
             変えない・`TAKO_2634_LEGACY=1` で修正前へ戻せる、を保つ。#2634）",
            line_at(&src, start)
        );
    }
    let (gate_start, gate_end) = span_of(&src, "pub fn issue2634_legacy() -> bool {", TMUX_BACKEND);
    assert!(
        src[gate_start..gate_end].contains("\"TAKO_2634_LEGACY\""),
        "{TMUX_BACKEND}:{} の A/B のゲートが `TAKO_2634_LEGACY` を読んでいない",
        line_at(&src, gate_start)
    );
}
