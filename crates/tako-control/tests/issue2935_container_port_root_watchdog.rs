//! **#2935 の番犬**: Windows の listen ポート検知（FR-2.4.2）は、器（psmux）のペインでは
//! 器の中のシェルをキーの根にする。
//!
//! ## なぜ縛るのか
//!
//! Windows には制御端末が無いので、ペイン配下は「根のプロセスの子孫」で決める（#524）。
//! #2935 の前は根が PTY 直下の子（器のペインでは器のクライアント）だった。psmux は器の
//! サーバーを**最初のクライアントの子**として起こすので、新しく立てたペインならサーバーと
//! その下の dev サーバーはクライアントの子孫になる。ところが tako を起こし直して器へ
//! 再 attach したペインでは、サーバーは死んだ旧クライアントの子のまま残り、新しい
//! クライアントは子を持たない。そのペインの dev サーバーはどのキーの子孫にも入らず、
//! `tako list` の `listen_ports` が `[]` のまま・提案チップも出なかった（Windows 実機・psmux 3.3.7 で実測）。
//!
//! いまは器の中のシェルの pid（起こした直後に器へ 1 回聞く `tmux_backend::pane_facts` の
//! `#{pane_pid}`。器への問い合わせは増やしていない）を端末へ写し、`ports::pane_key` が
//! それを根にする。判定そのもの（再 attach の形でも集まる・新しく立てたペインと器なしの
//! ペインは今どおり・器の内部ポートは落とす）は `procinfo.rs` の `portable_tests::issue2935_*`
//! が macOS でも Windows でも確かめる。ここが縛るのは**配線**（どれか 1 本外れると、
//! 判定の単体テストは緑のまま本番だけが #2935 の前へ戻る）:
//!
//! 1. tako-app の 3 秒の走査は、`ports::pane_key` へ端末の `container_shell_pid()` を渡す
//! 2. tako-app の `spawn_session` は、器へ聞いた `facts.pid` を `set_container_shell_pid` で
//!    端末へ写す（張り替え後の別の端末へは写さない = `serial` で絞る）
//! 3. `pane_key` の Windows の腕は `pid_tree_root(child_pid, container_shell_pid, issue2935_legacy())`
//!    を通る。A/B のゲート `issue2935_legacy` は `TAKO_2935_LEGACY` を読む
//!
//! 逆戻りは `file:line` を名指して落とす。実機の確かめ方は
//! `cargo test -p tako-core --test psmux_backend -- issue2935 --nocapture`（psmux がある Windows だけで走る）

use std::path::{Path, PathBuf};

const MAIN: &str = "crates/tako-app/src/main.rs";
const PROCINFO: &str = "crates/tako-core/src/platform/procinfo.rs";

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
    production_range::code_view::without_comments(&production_range::production(&src, rel))
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

/// `open` の位置から始まる `(` に対応する `)` の位置（呼び出しの引数の範囲）
fn paren_end(src: &str, open: usize) -> usize {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("対応する `)` が無い");
}

/// `sig` で始まるブロックの範囲（シグネチャの位置〜閉じ括弧）
fn span_of(src: &str, sig: &str, rel: &str) -> (usize, usize) {
    let at = src.find(sig).unwrap_or_else(|| {
        panic!("{rel} に `{sig}` が無い（構造が変わったらこの番犬も見直すこと）")
    });
    (at, block_end(src, at))
}

/// 1: 3 秒の走査は `pane_key` へ器の中のシェルの pid を渡す
#[test]
fn issue2935_ポート検知の走査はpane_keyへ器のシェルのpidを渡す() {
    let src = read(MAIN);
    let calls: Vec<usize> = src
        .match_indices("ports::pane_key(")
        .map(|(at, _)| at)
        .collect();
    assert!(
        !calls.is_empty(),
        "{MAIN} に `ports::pane_key(` の呼び出しが無い（ポート検知の走査の形が変わったらこの番犬も見直すこと）"
    );
    let mut offenders = Vec::new();
    for at in calls {
        let open = at + "ports::pane_key".len();
        let args = &src[open..=paren_end(&src, open)];
        if !args.contains(".container_shell_pid()") {
            offenders.push(format!("{MAIN}:{}", line_at(&src, at)));
        }
    }
    assert!(
        offenders.is_empty(),
        "`ports::pane_key(` へ端末の `container_shell_pid()` を渡していない呼び出しがある。Windows では器のペインの \
         キーの根が器のクライアントへ戻り、器へ再 attach したペインの dev サーバーのポートを検知しなくなる（#2935）:\n{}",
        offenders.join("\n")
    );
}

/// 2: `spawn_session` は器へ聞いた `facts.pid` を端末へ写す（この端末だけ）
#[test]
fn issue2935_器へ聞いたシェルのpidを端末へ写す() {
    let src = read(MAIN);
    let (start, end) = span_of(&src, "    fn spawn_session(", MAIN);
    let body = &src[start..end];
    let facts = body
        .find("tmux_backend::pane_facts(")
        .unwrap_or_else(|| {
            panic!(
                "{MAIN}:{} の `spawn_session` が器へ `pane_facts` を聞いていない（構造が変わったらこの番犬も見直すこと）",
                line_at(&src, start)
            )
        });
    let set = body.find(".set_container_shell_pid(").unwrap_or_else(|| {
        panic!(
            "{MAIN}:{} の `spawn_session` が器の中のシェルの pid を端末へ写していない（`set_container_shell_pid`）。\
             Windows のポート検知の根が無くなり、器へ再 attach したペインの dev サーバーを検知しない（#2935）",
            line_at(&src, start + facts)
        )
    });
    assert!(
        facts < set,
        "{MAIN}:{} の `set_container_shell_pid` が器への問い合わせ（`pane_facts`）より前にある（器が答えた pid を写すこと。#2935）",
        line_at(&src, start + set)
    );
    // 写す値は器の答え（`facts.pid`）で、写す先はこの端末（`serial` が一致する端末）だけ
    let guard = body[..set]
        .rfind("if let Some(pid) = facts.pid")
        .unwrap_or_else(|| {
            panic!(
                "{MAIN}:{} の `set_container_shell_pid` が `facts.pid` の枝の中に無い（#2935）",
                line_at(&src, start + set)
            )
        });
    let branch = &body[guard..set];
    assert!(
        branch.contains(".serial() == serial"),
        "{MAIN}:{} の `set_container_shell_pid` を端末の `serial` で絞っていない（再 attach で張り替えた後の別の端末へ、\
         古い器のシェルの pid を写しうる。#2935）",
        line_at(&src, start + set)
    );
    assert!(
        body[set..]
            .trim_start_matches(".set_container_shell_pid(")
            .starts_with("Some(pid)"),
        "{MAIN}:{} の `set_container_shell_pid` が器の答えた `pid` を写していない（#2935）",
        line_at(&src, start + set)
    );
}

/// 3: `pane_key` の Windows の腕は `pid_tree_root` を通り、A/B のゲートは `TAKO_2935_LEGACY` を読む
#[test]
fn issue2935_windowsのpane_keyは器のシェルを根にする判定を通る() {
    let src = read(PROCINFO);
    let (start, end) = span_of(&src, "pub fn pane_key(", PROCINFO);
    let body = &src[start..end];
    let arm = body.find("#[cfg(windows)]").unwrap_or_else(|| {
        panic!(
            "{PROCINFO}:{} の `pane_key` に Windows の腕（`#[cfg(windows)]`）が無い（構造が変わったらこの番犬も見直すこと）",
            line_at(&src, start)
        )
    });
    let arm_end = block_end(&src, start + arm) - start;
    let arm_text = &body[arm..arm_end];
    assert!(
        arm_text.contains("pid_tree_root(child_pid, container_shell_pid, issue2935_legacy())"),
        "{PROCINFO}:{} の `pane_key` の Windows の腕が `pid_tree_root(child_pid, container_shell_pid, issue2935_legacy())` \
         を通っていない。PTY 直下の子（器のペインでは器のクライアント）を根に戻すと、器へ再 attach したペインの \
         dev サーバーのポートを検知しなくなる（#2935）",
        line_at(&src, start + arm)
    );

    let (root_start, root_end) = span_of(&src, "fn pid_tree_root(", PROCINFO);
    let root = &src[root_start..root_end];
    let container = root.find("container_shell_pid").unwrap_or_else(|| {
        panic!(
            "{PROCINFO}:{} の `pid_tree_root` が器の中のシェルの pid を見ていない（#2935）",
            line_at(&src, root_start)
        )
    });
    let fallback = root.find(".or(child_pid").unwrap_or_else(|| {
        panic!(
            "{PROCINFO}:{} の `pid_tree_root` が器のシェルを聞けていないときに PTY 直下の子へ戻っていない（#524 の器なしのペイン）",
            line_at(&src, root_start)
        )
    });
    assert!(
        container < fallback && root.contains(".filter(|_| !legacy)"),
        "{PROCINFO}:{} の `pid_tree_root` が「器の中のシェルを先に・A/B では使わない・無ければ PTY 直下の子」の順になっていない（#2935）",
        line_at(&src, root_start)
    );

    let (gate_start, gate_end) =
        span_of(&src, "pub(crate) fn issue2935_legacy() -> bool {", PROCINFO);
    assert!(
        src[gate_start..gate_end].contains("\"TAKO_2935_LEGACY\""),
        "{PROCINFO}:{} の A/B のゲートが `TAKO_2935_LEGACY` を読んでいない",
        line_at(&src, gate_start)
    );
}
