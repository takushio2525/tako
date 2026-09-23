//! **#1627 の番犬**: 本番コードに `Instant::now() - <Duration>` の直書きが無い。
//!
//! ## なぜ止めるのか
//!
//! `Instant` の起点はブートなので（Windows は QueryPerformanceCounter・macOS は
//! `CLOCK_UPTIME_RAW`）、**稼働時間より長く引くと panic する**
//! （`overflow when subtracting duration from instant`）。OS 依存ではない。
//!
//! 実物（#1627）: `PaneMapping::new()` が「最初は必ず期限切れ」を
//! `Instant::now() - Duration::from_secs(999)` で表していたので、**ブートから
//! 16 分 39 秒以内に `tako remote serve` が立つと panic**した。再起動直後の自動復帰
//! （#1485）は普通に起きる並びで、CI の Windows では **uptime が閾値を跨ぐかどうか
//! だけで結果が反転**していた（同じ head で 746 秒 = 3 件 FAILED / 1012 秒 = ok）。
//! 「速い CI ほど落ちる」ので、負荷や環境の揺れとして片付けると原因に届かない。
//!
//! ## 何を固定するか
//!
//! 直し方は 2 つあり、**どちらを選ぶかは意味で決まる**（規約は
//! `.agent/conventions.md`「`Instant` は巻き戻さない」節）:
//!
//! - 「まだ一度も起きていない」→ `Option<Instant>` の `None`（時刻を捏造しない）
//! - 「N 前に起きたことにする」→ `tako_core::monotonic::rewound(d)`（飽和する 1 実装）
//!
//! ここが止めるのは**書き方**で、1 行で戻せる。**走査は改行をまたぐ**:
//! Issue の初版は行単位の grep で数えたので `Instant::now()` と `- Duration` が
//! 別の行に割れた **3 箇所を見落としていた**（実際は 9 箇所）。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので、
// ファイル途中のテスト用ヘルパで走査範囲が消えない
#[path = "common/production_range.rs"]
mod production_range;

/// 寄せ先（飽和する巻き戻し）。綴りはこの 1 か所に持つ
const HELPER: &str = "monotonic::rewound";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 走査する本番コード（4 クレートの `src` 全部）
fn production_sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut out = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-app", "tako-cli"] {
        collect(
            &root.join("crates").join(crate_dir).join("src"),
            &root,
            &mut out,
        );
    }
    out.sort();
    assert!(
        out.len() > 50,
        "走査が壊れている（*.rs が {} 件しか見つからない）",
        out.len()
    );
    out
}

fn collect(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, root, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // 報告文は `/` 区切りへ正規化する（Windows の `strip_prefix` は `\` を返す）
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        // 2 段で潰す。どちらもバイト長と行番号が保たれるので `file:line` がそのまま使える:
        // ① `#[cfg(test)]` の付いた item（#1420 の作法。**切らずに**潰す）
        // ② コメントと文字列リテラル（`code_view`）。**この番犬にはこれが要る**:
        //    禁じている綴りは製品側の doc コメントに「こう書いてはいけない」として実在する
        let production = production_range::scan(&text).text;
        out.push((rel, production_range::code_view_of(&production)));
    }
}

/// `Instant::now()` の直後（空白・改行をまたいで）に `-` が続く箇所の行番号。
///
/// **改行をまたいで見る**のが要点（`rustfmt` が `Instant::now()\n    - d` へ割る）。
/// `checked_sub` は当たらない（`.` が続くので `-` の手前に別の字が入る）
fn raw_subtractions(source: &str) -> Vec<usize> {
    const HEAD: &str = "Instant::now()";
    let mut hits = Vec::new();
    let mut cursor = 0usize;
    while let Some(at) = source[cursor..].find(HEAD) {
        let at = cursor + at;
        cursor = at + HEAD.len();
        let rest = &source[cursor..];
        let trimmed = rest.trim_start();
        // 「引き算」だけを拾う。`->` / `-=` のような別の意味の字面は無い位置なので、
        // 直後の非空白が `-` かどうかだけを見る
        if trimmed.starts_with('-') {
            hits.push(source[..at].lines().count());
        }
    }
    hits
}

/// 検査本体（注入テストからも呼ぶので、読むソースは引数で受ける）
fn check_no_raw_subtraction(source: &str, rel: &str) {
    let hits = raw_subtractions(source);
    assert!(
        hits.is_empty(),
        "{rel}:{} `Instant::now() - <Duration>` の直書きがある（#1627）。\n\
         `Instant` の起点はブートなので、**稼働時間より長く引くと panic する**\n\
         （`overflow when subtracting duration from instant`）。書き換え先は意味で選ぶ:\n\
         ・「まだ一度も起きていない」= `Option<Instant>` の `None`（時刻を捏造しない）\n\
         ・「N 前に起きたことにする」= `tako_core::{HELPER}(d)`（飽和する 1 実装）",
        hits.iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(" / ")
    );
}

// --------------------------------------------------------------- テスト

#[test]
fn 本番コードにinstantの直引きが無い() {
    for (rel, source) in production_sources() {
        check_no_raw_subtraction(&source, &rel);
    }
}

/// 走査が空振りしていないこと（#1627 が実在した 2 ファイルが視界に入っているか）。
///
/// ファイル移動・改名で黙って 0 件になるのを防ぐ。**潰したあとの眺め**で確かめるので、
/// テスト領域だけを潰す形が壊れたときもここで気づける
#[test]
fn 走査が空振りしていない() {
    let sources = production_sources();
    for (rel, needle) in [
        ("crates/tako-control/src/remote.rs", "fn is_stale"),
        ("crates/tako-app/src/main.rs", "let mut last_scan"),
        ("crates/tako-core/src/monotonic.rs", "pub fn rewound"),
    ] {
        let (_, source) = sources
            .iter()
            .find(|(r, _)| r == rel)
            .unwrap_or_else(|| panic!("{rel} が走査対象に入っていない"));
        assert!(
            source.contains(needle),
            "{rel} の本番コードに `{needle}` が無い: 走査先が間違っている"
        );
    }
}

/// 寄せ先が実在し、飽和する形で書かれていること（綴りだけ残って中身が
/// `Instant::now() - d` へ戻っていたら意味が無い）
#[test]
fn 寄せ先は飽和する形で書かれている() {
    let src = std::fs::read_to_string(repo_root().join("crates/tako-core/src/monotonic.rs"))
        .expect("monotonic.rs を読む");
    let code = production_range::code_view_of(&production_range::scan(&src).text);
    assert!(
        code.contains("checked_sub"),
        "monotonic.rs の巻き戻しが `checked_sub` を通っていない（#1627）"
    );
    // 実際に飽和すること（規則そのもの）は `monotonic` の単体テストが持つ。
    // ここでも 1 度だけ実物を呼ぶが、**実時間の絶対予算では測らない**
    // （負荷で反転する = #1220 / #962）。呼び出しの前後で挟んで「今を返した」を見る
    let before = std::time::Instant::now();
    let saturated = tako_core::monotonic::rewound(std::time::Duration::MAX);
    let after = std::time::Instant::now();
    assert!(
        saturated >= before && saturated <= after,
        "巻き戻しが飽和していない（#1627）"
    );
}

/// 番犬に検出力があること: **#1627 以前の形を実際に落とす**。
///
/// 製品ソースを書き換える代わりに、壊れた形の断片を同じ検査へ通す
/// （注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）
#[test]
fn 直引きの形を実際に落とす() {
    // ① #1627 そのもの（1 行）
    let legacy = "
fn new() -> Self {
    Self {
        updated_at: std::time::Instant::now() - std::time::Duration::from_secs(999),
    }
}
";
    assert!(
        std::panic::catch_unwind(|| check_no_raw_subtraction(legacy, "注入")).is_err(),
        "① 1 行の直引きを見逃した"
    );

    // ② rustfmt が改行へ割った形（**Issue の初版が 3 箇所見落とした形**）
    let wrapped = "
        app.last_term_notify = std::time::Instant::now()
            - std::time::Duration::from_secs(1);
";
    assert!(
        std::panic::catch_unwind(|| check_no_raw_subtraction(wrapped, "注入")).is_err(),
        "② 改行をまたいだ直引きを見逃した"
    );

    // ③ 二重の引き算（#749 の自己検査に在った形）
    let twice = "
        let old = std::time::Instant::now()
            - tako_core::handoff::NUDGE_GRACE
            - Duration::from_secs(10);
";
    assert!(
        std::panic::catch_unwind(|| check_no_raw_subtraction(twice, "注入")).is_err(),
        "③ 二重の引き算を見逃した"
    );

    // ④ 現行の 2 つの形は通る（常に落ちる無意味な番犬になっていないこと）
    let ok = "
        let mut last_scan: Option<std::time::Instant> = None;
        let triggered = last_scan.is_none_or(|t| t.elapsed() >= INTERVAL);
        last_scan = Some(std::time::Instant::now());
        app.last_term_notify = tako_core::monotonic::rewound(Duration::from_secs(1));
        let saturating = std::time::Instant::now().checked_sub(d).unwrap_or(now);
        let elapsed = std::time::Instant::now() - start_instant_is_not_a_duration();
";
    // 最後の 1 行は `Instant - Instant`（= Duration）で意味が違うが、
    // **この番犬は区別しない**（引き算そのものを禁じる）ので落ちるのが正しい。
    // 区別しない理由: `Instant - Instant` も左が右より前なら panic する
    assert!(
        std::panic::catch_unwind(|| check_no_raw_subtraction(ok, "注入")).is_err(),
        "④ `Instant::now() - <Instant>` も引き算なので落とす"
    );
    let ok_only = "
        let mut last_scan: Option<std::time::Instant> = None;
        let triggered = last_scan.is_none_or(|t| t.elapsed() >= INTERVAL);
        last_scan = Some(std::time::Instant::now());
        app.last_term_notify = tako_core::monotonic::rewound(Duration::from_secs(1));
        let saturating = std::time::Instant::now().checked_sub(d).unwrap_or(now);
";
    check_no_raw_subtraction(ok_only, "注入");
}
