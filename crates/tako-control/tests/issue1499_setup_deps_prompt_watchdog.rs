//! **#1499 の番犬**: 標準 `tako setup` が未検出の CLI 依存を「聞かずに素通り」へ戻らない。
//!
//! ## 何が起きていたか（実測で確定した真因）
//!
//! #88 の「その場導入」は #262 の質問ゼロ化で死に、#1057（PR #1064）が復活させた。
//! ただし復活したのは **`--review` 経路だけ**で、`run_setup` の呼び出しは
//!
//! ```ignore
//! let (agents, missing) = run_dependency_check(review_mode && !setup_bootstrap::legacy_mode());
//! ```
//!
//! のまま。`interactive` が false だと `offer_dep_install` は案内 1 行で return するので、
//! **素の `tako setup` では実端末でも `[y/N]` が 1 つも出ない**（#1499 の実測。
//! パイプでも実 PTY でも出力が 1 文字も変わらないことで TTY 判定説を潰した）。
//!
//! 壊れ方が「落ちる」ではなく「機能が静かに到達不能」なので、テストは緑のまま残る。
//! しかも当時の `標準setupは依存の質問をしない` が**旧呼び出し形を文字列で固定**して
//! いたため、直そうとすると番犬に止められる状態だった。同じ形を二度作らないために、
//! ①呼び出しの形 ②判断そのもの（純粋関数の表）の両方をここで縛る。
//!
//! ## 何を縛るか
//!
//! 1. 依存チェック段の対話の可否を `review` **だけ**で決めていない（ソース走査）
//! 2. 標準 `tako setup` 相当の文脈で [`offer_for`] が `Ask` を返す（全文脈の表）
//! 3. 導入の実行が `setup_deps::install` の 1 実装を通る（呼び手の棚卸し）
//!
//! ## #1524 で動いた置き場
//!
//! #1499 当時は表示と入力が CLI（`setup.rs` の `offer_dep_install`）にあったので、
//! ①の入口検査も②の「計画を見せてから進む」も CLI を見ていた。#1524 でそのひと続きを
//! `setup_deps::offer_and_install` へ寄せたので、**検査先だけ寄せ先へ移してある**
//! （縛る中身は同じ）。CLI 側に判断や `[y/N]` が生え直したら
//! `issue1524_setup_prompt_single_impl_watchdog` が落とす

use std::path::{Path, PathBuf};
use tako_control::setup_deps::{offer_for, DepOffer, DepOfferContext, GuideReason};

// テスト領域を**潰して**本番コードだけを見る（切ると以降が視界から消える = #1420）。
// このファイル自身も、CLI 側の番犬も、禁止パターンを文字列として持っている
#[path = "common/production_range.rs"]
mod production_range;

const CLI_SETUP: &str = "crates/tako-cli/src/setup.rs";
const SETUP_DEPS: &str = "crates/tako-control/src/setup_deps.rs";

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

/// 本番コードだけの眺め（行番号は保たれるので `file:line` の名指しに使える）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

/// 違反行を `file:line`（囲んでいる関数名つき）で名指しする。
/// 関数名の追跡は共有ヘルパを通す（#1496。直書きは `pub(crate) fn` を見落とす）
fn locate(rel: &str, src: &str, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::from("(ファイル先頭)");
    for (i, line) in src.lines().enumerate() {
        if let Some(name) = tako_core::source_scan::fn_head_name(line) {
            current = name.to_string();
        }
        if line.contains(needle) {
            out.push(format!("{rel}:{} fn {current}: {}", i + 1, line.trim()));
        }
    }
    out
}

/// **①真因の形**: 依存チェック段の対話を `review` だけで決めていない。
///
/// 旧実装そのもの（`run_dependency_check(review_mode …)`）が戻ったらここで落ちる
#[test]
fn issue1499_依存チェックの対話をreviewだけで決めない() {
    let src = production(CLI_SETUP);
    let banned = locate(CLI_SETUP, &src, "run_dependency_check(review");
    assert!(
        banned.is_empty(),
        "#1499 の退行: 依存チェック段の対話を review だけで決めている。\n\
         標準 `tako setup` でも未検出の CLI 依存は [y/N] を聞くこと。\n{}",
        banned.join("\n")
    );
    // 真偽値の直渡しも同じ退行（`run_dependency_check(false)` = #1057 前の形）
    for shape in ["run_dependency_check(false)", "run_dependency_check(true)"] {
        let hits = locate(CLI_SETUP, &src, shape);
        assert!(
            hits.is_empty(),
            "#1499 の退行: 依存チェック段へ真偽値を直接渡している（判断は DepCheckMode 経由）。\n{}",
            hits.join("\n")
        );
    }
    // 判断は純粋関数 1 本を通る（CLI へ条件分岐を書き戻していない）。
    // #1524 で表示と入力ごと `offer_and_install` の中へ移したので、CLI が呼ぶ入口は
    // そちら 1 本（中で `offer_for` を通る）。`offer_for` の直呼びが CLI へ戻ったら
    // `issue1524_setup_prompt_single_impl_watchdog` が落とす
    assert!(
        src.contains("setup_deps::offer_and_install("),
        "{CLI_SETUP}: その場導入は `setup_deps::offer_and_install`（中で `offer_for`）を\
         通すこと（#1499 / #1524）"
    );
    // 呼び出しの形（`--review` に依らない本体経路 / 読み取り専用の `--check`）
    for shape in [
        "run_dependency_check(DepCheckMode::for_setup(review_mode, assume_yes))",
        "run_dependency_check(DepCheckMode::check_only())",
        "stage_installs: true",
    ] {
        assert!(
            src.contains(shape),
            "{CLI_SETUP}: `{shape}` が無い（#1499 の依存チェック段の形）"
        );
    }
    // **#262 は壊さない**: 設定値（FDA / スリープ）の見直しは `--review` だけが対話になる
    for shape in [
        "run_fda_check(mode.review_settings)",
        "run_sleep_guard_check(mode.review_settings)",
    ] {
        assert!(
            src.contains(shape),
            "{CLI_SETUP}: 設定値の質問まで標準 setup へ増やしていないか（#262 / #1499）"
        );
    }
    // 代行できない / 聞けないときの案内は最簡形 1 本（#322）。
    // 文面の置き場は #1524 で `setup_deps`（`deps_install_hint`）へ揃った
    assert!(
        production(SETUP_DEPS).contains("いま入れる: tako setup deps install"),
        "{SETUP_DEPS}: 次の一手は最簡形 1 本で出すこと（#322 / #1524）"
    );
}

/// 聞く前・入れる前に必ず「何を・どの導入器で・どこへ入れるか」を見せる（#1499）。
///
/// 分岐の置き場は #1524 で `setup_deps::offer_and_install` の 1 組だけになった
/// （それまでは CLI 側にも同じ腕があり、片方だけ直る退行が起こりうる形だった）
#[test]
fn issue1499_導入計画を見せてから進む() {
    let src = production(SETUP_DEPS);
    for arm in ["DepOffer::Ask => {", "DepOffer::AutoInstall => {"] {
        let body = src.split(arm).nth(1).unwrap_or_else(|| {
            panic!("{SETUP_DEPS}: `{arm}` の腕が無い（#1499 の分岐が消えている）")
        });
        let head: String = body.lines().take(3).collect::<Vec<_>>().join("\n");
        assert!(
            head.contains("say_plan(io, state)"),
            "{SETUP_DEPS}: `{arm}` は導入計画を出してから進むこと（#1499）。実際の先頭:\n{head}"
        );
    }
}

/// 表を書きやすくするための文脈。`can_run` は `DepStatus::can_tako_install`
fn ctx(
    stage_installs: bool,
    review: bool,
    assume_yes: bool,
    tty: bool,
    legacy: bool,
) -> DepOfferContext {
    DepOfferContext {
        stage_installs,
        review,
        assume_yes,
        stdin_is_terminal: tty,
        legacy,
    }
}

/// **②判断そのもの**: 「標準 `tako setup` を端末で叩いたら聞く」を先頭に、
/// 全 32 文脈（5 つの真偽値）の答えを表で固定する。
///
/// #1499 の真因は先頭の 1 行（`Ask` であるべき文脈が `Guide` だった）。
/// 表にしてあるので、1 マスでも意味が変わったらここで落ちる
#[test]
fn issue1499_標準setupの文脈では聞く() {
    // (stage_installs, review, assume_yes, tty, legacy) -> 期待
    let table: &[(bool, bool, bool, bool, bool, DepOffer)] = &[
        // --- #1499 の本体: 素の `tako setup` を端末で叩いた文脈 ---
        (true, false, false, true, false, DepOffer::Ask),
        // --review も同じ（#1057 の体験は据え置き）
        (true, true, false, true, false, DepOffer::Ask),
        // --yes は同意扱い（端末の有無に依らない）
        (true, false, true, true, false, DepOffer::AutoInstall),
        (true, false, true, false, false, DepOffer::AutoInstall),
        (true, true, true, true, false, DepOffer::AutoInstall),
        (true, true, true, false, false, DepOffer::AutoInstall),
        // 端末が無ければ聞かずに案内（止まらない）
        (
            true,
            false,
            false,
            false,
            false,
            DepOffer::Guide(GuideReason::NoTerminal),
        ),
        (
            true,
            true,
            false,
            false,
            false,
            DepOffer::Guide(GuideReason::NoTerminal),
        ),
        // A/B: legacy は標準経路だけを #1499 前へ戻す（--review は聞くまま）
        (
            true,
            false,
            false,
            true,
            true,
            DepOffer::Guide(GuideReason::Legacy),
        ),
        (
            true,
            false,
            false,
            false,
            true,
            DepOffer::Guide(GuideReason::Legacy),
        ),
        (true, true, false, true, true, DepOffer::Ask),
        (
            true,
            true,
            false,
            false,
            true,
            DepOffer::Guide(GuideReason::NoTerminal),
        ),
        (
            true,
            false,
            true,
            true,
            true,
            DepOffer::Guide(GuideReason::Legacy),
        ),
        (
            true,
            false,
            true,
            false,
            true,
            DepOffer::Guide(GuideReason::Legacy),
        ),
        (true, true, true, true, true, DepOffer::AutoInstall),
        (true, true, true, false, true, DepOffer::AutoInstall),
    ];
    for &(stage, review, yes, tty, legacy, expected) in table {
        let got = offer_for(true, ctx(stage, review, yes, tty, legacy));
        assert_eq!(
            got, expected,
            "文脈 stage_installs={stage} review={review} assume_yes={yes} tty={tty} legacy={legacy}"
        );
    }
    // `--check` は読み取り専用: 他がどうであれ何も導入しない（32 文脈の残り半分）
    for &review in &[false, true] {
        for &yes in &[false, true] {
            for &tty in &[false, true] {
                for &legacy in &[false, true] {
                    assert_eq!(
                        offer_for(true, ctx(false, review, yes, tty, legacy)),
                        DepOffer::Guide(GuideReason::CheckOnly),
                        "`tako setup --check` は何も導入しない（review={review} yes={yes} tty={tty} legacy={legacy}）"
                    );
                }
            }
        }
    }
    // 代行できないものは、どの文脈でも聞かない（聞いても入らないので嘘になる）
    for &stage in &[false, true] {
        for &yes in &[false, true] {
            for &tty in &[false, true] {
                assert_eq!(
                    offer_for(false, ctx(stage, true, yes, tty, false)),
                    DepOffer::Guide(GuideReason::CannotRun),
                    "代行できない依存は聞かない（stage={stage} yes={yes} tty={tty}）"
                );
            }
        }
    }
}

/// 「聞けたはずなのに聞かなかった」理由は人へ出す（無言にしない）
#[test]
fn issue1499_聞かなかった理由を人へ出す() {
    assert!(
        GuideReason::NoTerminal.note().is_some(),
        "端末が無い理由は出す"
    );
    assert!(
        GuideReason::Legacy.note().is_some(),
        "A/B で止めた理由は出す"
    );
    // 直前の表示が理由を語っている 2 つは重ねて出さない
    assert!(GuideReason::CannotRun.note().is_none());
    assert!(GuideReason::CheckOnly.note().is_none());
    // 機械可読の理由は重複しない
    let slugs: Vec<&str> = [
        GuideReason::CannotRun,
        GuideReason::CheckOnly,
        GuideReason::Legacy,
        GuideReason::NoTerminal,
    ]
    .iter()
    .map(|r| r.slug())
    .collect();
    let mut sorted = slugs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        slugs.len(),
        "理由の slug が重複している: {slugs:?}"
    );
}

/// **③導入経路は 1 本**: `setup_deps::install` を呼ぶ箇所を棚卸しする。
///
/// 本体フロー・`tako setup deps install`・dispatch（MCP）がすべて同じ関数を通ること。
/// 別の場所で `DepInstaller::args()` からコマンドを組み直したら落ちる
#[test]
fn issue1499_導入は1実装を通る() {
    // #1524 以降、`setup.rs` に残る直呼びは `tako setup deps install`（`run_deps`）
    // だけ。本体フローは `setup_deps::offer_and_install` の中で同じ `install` を通る
    let expected: &[(&str, &str)] = &[
        (CLI_SETUP, "run_deps"),
        ("crates/tako-control/src/dispatch.rs", "dispatch_inner"),
    ];
    for (rel, _) in expected {
        let src = production(rel);
        assert!(
            src.contains("setup_deps::install(") || src.contains("crate::setup_deps::install("),
            "{rel}: 導入は `setup_deps::install` を通すこと（#1057 / #1499）"
        );
    }
    // 依存の導入コマンドを自前で組み立てていない（正本は setup_deps::run_installer）
    let mut offenders = Vec::new();
    for rel in [CLI_SETUP, "crates/tako-control/src/dispatch.rs"] {
        let src = production(rel);
        offenders.extend(locate(rel, &src, "DepInstaller::args"));
        offenders.extend(locate(rel, &src, "Command::new(\"brew\")"));
    }
    assert!(
        offenders.is_empty(),
        "#1499: 依存の導入を `setup_deps` の外で組み立てている。\n{}",
        offenders.join("\n")
    );
}
