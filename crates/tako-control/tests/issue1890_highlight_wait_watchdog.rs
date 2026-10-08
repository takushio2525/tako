//! **#1890 の番犬**: 大きいファイルの visual-test 節が、background の構文の塗りを
//! **回数の窓**で待つ形へ戻らないようにする。
//!
//! ## 何が起きていたのか（実測・修正前）
//!
//! visual-test 節 `large-file-decor`（#1660）は、10 MB のファイルを閲覧表示で開いたあと
//! 読み取り表示の塗り（background の syntect）が戻るのを `for _ in 0..3000 { 10ms 待って
//! 描く }` で待っていた。窓の長さは release（`scripts/test-large-file-edit-1660.sh`）の
//! 塗り（7.8 秒）だけを見て決めていたので、debug（visual-test 入り）では未最適化の正規表現で
//! 全文を塗り終える前に窓を使い切り、**節が入った `deecfc9` の時点から debug の単独実行では
//! 一度も通っていなかった**（main `7364bc0` も同じ。release は同じ時間帯に緑）。
//! 編集開始の全文の塗り（6000 回の窓）と、隣の節 `large-file-edit` の 2 か所も同じ形だった。
//!
//! ## ここで止める 4 つ
//!
//! 1. [`読み取り表示の塗りは戻ったら先に数を減らす`] — 走っている数
//!    （`view_highlights_running`）を、塗りを捨てる `return` より後で減らす形。捨てた塗りが
//!    「まだ走っている」に見え続け、塗られずに終わった回帰を上限まで待ってから落ちる
//! 2. [`大きいファイルの節は塗りの戻りを1実装で待つ`] — 節が `highlight_pending` や
//!    `s.color.is_some()` を自分で回す（= 回数の窓）形へ戻る
//! 3. [`塗りの待ちは走っている間だけ待ち予算で止める`] — 待ちの 1 実装が混み具合の予算
//!    （`state_wait_budget`）を使わなくなる / 読み取り表示の今を読む関数が走っている数を
//!    見なくなる（遅いビルドで即座に偽になる）形
//! 4. [`注入の遅れは旧の窓と新の上限のあいだにある`] — 実経路の A/B の遅れの注入が旧の窓より
//!    短い（旧でも通る）/ 新の release の素の上限より長い（新でも上限で諦める）形
//!
//! 落ちるときは **file:line で名指し**する。実 GUI での A/B は
//! `scripts/test-highlight-wait-1890.sh`（`TAKO_1890_INJECT=slow:<ミリ秒>` で塗りを遅らせ、
//! `TAKO_1890_LEGACY` で旧の回数の窓へ戻すと名指しで FAILED）。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments_checked};

const MAIN: &str = "crates/tako-app/src/main.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 関数 1 本の本体（`file:line` で名指しできるよう開始行も持つ）
struct Body {
    line: usize,
    text: String,
}

impl Body {
    /// 本体の中で `needle` が最初に現れる行（ファイルの行番号）
    fn line_of(&self, needle: &str) -> Option<usize> {
        let at = self.text.find(needle)?;
        Some(self.line + self.text[..at].bytes().filter(|b| *b == b'\n').count())
    }
}

/// 関数の本体を名前で引く（最初に見つかったもの。コメントは潰した眺め）。
/// 見つからなければ `rel:1` で落とす
fn body(rel: &str, name: &str) -> Body {
    let src = read(rel);
    let prod = production_range::production(&src, rel);
    let code = code_view(&prod);
    let view = without_comments_checked(&prod, rel);
    assert_eq!(
        code.len(),
        view.len(),
        "{rel}:1 2 つの眺めのバイト長が食い違う（範囲を使い回せない）"
    );
    let mut offset = 0;
    for line in code.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if fn_head_name(line.trim_start()) != Some(name) {
            continue;
        }
        let open = code[start..].find('{').expect("関数の本体") + start;
        let mut depth = 0usize;
        let mut end = open;
        for (i, byte) in code[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let head = code[..start].bytes().filter(|b| *b == b'\n').count() + 1;
        let open_line = head + code[start..open].bytes().filter(|b| *b == b'\n').count();
        return Body {
            line: open_line,
            text: view[open..end].to_string(),
        };
    }
    panic!("{rel}:1 `fn {name}` が見つからない（改名したならこの番犬も直す）");
}

fn must_contain(rel: &str, body: &Body, name: &str, needle: &str, why: &str) {
    assert!(
        body.text.contains(needle),
        "{rel}:{} `fn {name}` が `{needle}` を通っていない（#1890）。\n{why}",
        body.line
    );
}

// --- 1) 走っている数の数え方 ----------------------------------------------------

/// 読み取り表示の塗りは起こすときに数を増やし、戻ったら**取り込むか捨てるかを決める前に**
/// 減らす。捨てる `return`（編集セッションがある・注入の `drop`）より後ろで減らすと、
/// 捨てた塗りが「まだ走っている」に見え続ける
#[test]
fn 読み取り表示の塗りは戻ったら先に数を減らす() {
    let name = "spawn_highlight";
    let f = body(SIDEBAR, name);
    must_contain(
        SIDEBAR,
        &f,
        name,
        "view_highlights_running.entry(pane)",
        "起こした塗りを数えないと、待つ側が「まだ塗っている」と「塗らずに終わった」を見分けられない",
    );
    let Some(decrement) = f.line_of("saturating_sub(1)") else {
        panic!(
            "{SIDEBAR}:{} `fn {name}` が戻った塗りの数を減らしていない（#1890）",
            f.line
        );
    };
    if let Some(early) = f.line_of("return") {
        assert!(
            decrement < early,
            "{SIDEBAR}:{early} `fn {name}` が走っている数を減らす前（{SIDEBAR}:{decrement}）に \
             塗りを捨てて戻る（#1890）。捨てた塗りが「まだ走っている」に見え続け、\
             塗られずに終わった回帰を上限まで待ってから落ちる"
        );
    }
}

// --- 2) 節は 1 実装で待つ ---------------------------------------------------------

/// 大きいファイルの 2 節は、読み取り表示（`view`）と編集開始（`seed`）の塗りの戻りを
/// `wait_for_background_highlight` の 1 実装で待つ。節の中で `highlight_pending` や
/// 色の有無を自分で回すのは回数の窓へ戻る形
#[test]
fn 大きいファイルの節は塗りの戻りを1実装で待つ() {
    for name in ["large_file_edit_visual", "large_file_decor_case"] {
        let f = body(MAIN, name);
        for stage in ["\"view\"", "\"seed\""] {
            assert!(
                f.text.contains("wait_for_background_highlight(") && f.text.contains(stage),
                "{MAIN}:{} `fn {name}` が {stage} の塗りの戻りを `wait_for_background_highlight` で \
                 待っていない（#1890）",
                f.line
            );
        }
        for hand_rolled in ["highlight_pending", "s.color.is_some()"] {
            if let Some(at) = f.line_of(hand_rolled) {
                panic!(
                    "{MAIN}:{at} `fn {name}` が塗りの戻りを `{hand_rolled}` で自分で回している（#1890）。\
                     回数の窓は debug や混んだ機で塗り終える前に使い切る。\
                     `view_highlight_probe` / `seed_highlight_probe` を通して \
                     `wait_for_background_highlight` で待つ"
                );
            }
        }
    }
}

// --- 3) 待ちの 1 実装 -------------------------------------------------------------

/// 待ちの 1 実装は「走っている間」だけ待ち、上限は混み具合の予算で決める。
/// 読み取り表示の今を読む関数は走っている数を見る（見ないと、塗りが長引くだけで
/// 「塗らずに終わった」と読んで即座に偽になる）
#[test]
fn 塗りの待ちは走っている間だけ待ち予算で止める() {
    let name = "wait_for_background_highlight";
    let f = body(MAIN, name);
    for (needle, why) in [
        (
            "state_wait_budget(",
            "上限は混み具合で伸ばすだけの予算（#1162）で決める。固定の回数ではビルドと混み具合に追従しない",
        ),
        (
            "HighlightWait::Settled",
            "塗りが戻ったのに揃っていないなら、上限まで待たずにその場で偽にする",
        ),
        (
            "legacy_1890(",
            "旧の回数の窓へ戻す A/B の腕（`TAKO_1890_LEGACY`）を残す",
        ),
    ] {
        must_contain(MAIN, &f, name, needle, why);
    }
    let name = "view_highlight_probe";
    let f = body(MAIN, name);
    must_contain(
        MAIN,
        &f,
        name,
        "view_highlights_running",
        "走っている数を見ないと、塗りが長引いただけで「塗らずに終わった」と読む",
    );
}

// --- 4) A/B の注入の幅 -------------------------------------------------------------

/// release の 1 周期（10ms の待ち + 1 フレームの描画）の実測の上限（#1890 の実測は
/// 12.5〜15.3ms）。旧の窓の長さ = 回数 × これ
const RELEASE_POLL_MS: u64 = 16;

/// `text` の中で `head` の後ろ（空白・改行を飛ばす）に続く数字（`_` 区切りを許す）
fn number_after(text: &str, head: &str) -> Option<u64> {
    let at = text.find(head)? + head.len();
    let digits: String = text[at..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '_')
        .filter(|c| *c != '_')
        .collect();
    digits.parse().ok()
}

/// 実経路の A/B（`scripts/test-highlight-wait-1890.sh`）の遅れの注入は
/// **旧の窓 < 注入の遅れ < 新の release の素の上限**に収まっていないと A/B にならない
/// （旧の窓より短いと旧でも通る / 新の上限より長いと新でも「まだ走っている」まま諦める。
/// 初版は上限 90 秒 × 混み具合 1.4 = 126 秒が注入 150 秒より短く、後者を実測で踏んだ）
#[test]
fn 注入の遅れは旧の窓と新の上限のあいだにある() {
    const SCRIPT: &str = "scripts/test-highlight-wait-1890.sh";
    let script = read(SCRIPT);
    const DELAY_HEAD: &str = "DELAY_MS=\"${DELAY_MS:-";
    let delay_line = script.find(DELAY_HEAD).map_or(1, |at| {
        script[..at].bytes().filter(|b| *b == b'\n').count() + 1
    });
    let delay_ms = number_after(&script, DELAY_HEAD)
        .unwrap_or_else(|| panic!("{SCRIPT}:1 遅れの既定値（DELAY_MS）を読めない"));
    let wait = body(MAIN, "wait_for_background_highlight");
    let base_line = wait.line_of("cfg!(debug_assertions)").unwrap_or(wait.line);
    let base_s = wait
        .text
        .find("cfg!(debug_assertions)")
        .and_then(|at| number_after(&wait.text[at..], "} else {"))
        .unwrap_or_else(|| {
            panic!(
                "{MAIN}:{} release の素の上限を読めない（形を変えたならこの番犬も直す）",
                wait.line
            )
        });
    let mut widest = 0u64;
    for name in ["large_file_edit_visual", "large_file_decor_case"] {
        let f = body(MAIN, name);
        for stage in ["\"view\",", "\"seed\","] {
            // 横並び（`"seed", 6000,`）でも fmt の縦並びでも、段の名前の次の引数が旧の回数
            let polls = number_after(&f.text, stage).unwrap_or_else(|| {
                panic!(
                    "{MAIN}:{} `fn {name}` の {stage} の旧の回数を読めない",
                    f.line
                )
            });
            widest = widest.max(polls);
        }
    }
    let legacy_ms = widest * RELEASE_POLL_MS;
    assert!(
        legacy_ms < delay_ms,
        "{SCRIPT}:{delay_line} 遅れの注入 {delay_ms}ms が旧の窓（{widest} 回 × {RELEASE_POLL_MS}ms = \
         {legacy_ms}ms）より短い（#1890）。旧の腕でも通ってしまい A/B にならない"
    );
    assert!(
        delay_ms < base_s * 1000,
        "{MAIN}:{} 新の release の素の上限 {base_s} 秒が遅れの注入 {delay_ms}ms より短い（#1890）。\
         新の腕でも「まだ走っている」まま上限で諦める",
        base_line
    );
}
