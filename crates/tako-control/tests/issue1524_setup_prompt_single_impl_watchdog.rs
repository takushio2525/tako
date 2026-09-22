//! **#1524 の番犬**: 依存の「案内 → `[y/N]` → 導入 → 再検出」は 1 実装だけ。
//!
//! ## 何が残っていたか
//!
//! #1499 は判断（`offer_for`）・実行（`install`）・再検出（`resolve`）を
//! `setup_deps` の 1 実装へ寄せたが、**表示と入力の読み取りは呼び手に残した**。
//! #1509 が `tako remote setup` を寄せる際に、そのひと続きを束ねた
//! `setup_deps::offer_and_install`（`DepPromptIo` で writer / reader を注入）を
//! 作ったものの、同時に #1501 の worker が `setup.rs` を触っていたため
//! **標準 `tako setup` 側は差分ゼロ**のまま着地した（PR #1523 の申し送り）。
//!
//! 結果、体験を組み立てる層が 2 か所（`setup.rs` の `offer_dep_install` と
//! `setup_deps::offer_and_install`）にあり、文面が同じになるよう手で揃えてある
//! だけの状態だった。**片方だけ直せば黙ってズレる**（#1509 が実測した
//! 「brew が無い」「非 TTY」「入れたのに引けない」の割れ方がそのまま戻る）。
//!
//! ## 何を止めるか
//!
//! 1. `tako setup` の CLI が判断（`offer_for` / `DepOffer` / `GuideReason`）を
//!    自分で分岐していない
//! 2. `[y/N]` の文面と、その答えで分かれる案内が**本番コード 1 ファイル**
//!    （`setup_deps.rs`）にしか無い
//! 3. `setup_deps::install` の直呼びは「利用者が導入を名指しで打った経路」だけ
//!    （理由つきの許可リスト）
//! 4. 寄せ先が空振りしていない（両方の呼び手が `offer_and_install` を実際に呼ぶ）

use std::path::{Path, PathBuf};

// テスト領域は**潰して**本番コードだけを見る（切ると以降が視界から消える = #1420）
#[path = "common/production_range.rs"]
mod production_range;

const CLI_SETUP: &str = "crates/tako-cli/src/setup.rs";
const SETUP_DEPS: &str = "crates/tako-control/src/setup_deps.rs";
const REMOTE_SETUP: &str = "crates/tako-control/src/remote_setup.rs";

/// CLI が持ってはいけないもの。**判断の分岐**と、`[y/N]` で分かれる**文面**
const CLI_BANNED: &[&str] = &[
    // 判断そのもの（正本は `offer_for`。呼ぶのは `offer_and_install` の中だけ）
    "setup_deps::offer_for(",
    "setup_deps::DepOffer::",
    "setup_deps::GuideReason::",
    // 確認と、その答えで分かれる案内（`DepOfferContext` は判断の**材料**なので対象外）
    "をインストールしますか？",
    "--yes のため確認を省略",
    "スキップしました（後から `tako setup deps install`",
];

/// `[y/N]` の文面。**本番コードでここにしか無い**ことを確かめる
const PROMPT_TEXT: &str = "をインストールしますか？";

/// 依存導入の直呼び。見つけたら理由つきで許可リストへ載せる
const DIRECT_INSTALL: &str = "setup_deps::install(";

/// 直呼びを許す場所（ファイル, 囲んでいる関数名, 理由）。**黙って増やさない**
const ALLOWED_DIRECT: &[(&str, &str, &str)] = &[
    (
        CLI_SETUP,
        "run_deps",
        "`tako setup deps install` = 利用者が導入そのものを名指しで打った経路。\
         案内も `[y/N]` も要らない（聞き直すのは #322 の逆）",
    ),
    (
        "crates/tako-control/src/dispatch.rs",
        "dispatch_inner",
        "MCP `tako_setup_deps`（非対話）。端末が無いので確認のしようがない",
    ),
];

/// 寄せ先（ファイル, その中で呼んでいるはずの関数, 入口）
const MIGRATED: &[(&str, &str, &str)] = &[
    (
        CLI_SETUP,
        "run_dependency_check",
        "setup_deps::offer_and_install(",
    ),
    (
        REMOTE_SETUP,
        "install_tailscale",
        "setup_deps::offer_and_install(",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 本番コードだけの眺め（行番号は保たれるので `file:line` の名指しに使える）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

/// `crates/*/src/**/*.rs`（本番コードの置き場）を集める
fn production_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let crates = repo_root().join("crates");
    for entry in std::fs::read_dir(&crates)
        .expect("crates/ を読めない")
        .flatten()
    {
        let src = entry.path().join("src");
        if src.is_dir() {
            collect(&src, &mut out);
        }
    }
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(reader) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in reader.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn rel_of(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// 違反行を**囲んでいる関数名**で名指しする（追跡は共有ヘルパ = #1496）
fn offenders_in(
    src: &str,
    rel: &str,
    needles: &[&str],
    allowed: &[(&str, &str, &str)],
) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::from("(ファイル先頭)");
    for (i, line) in src.lines().enumerate() {
        if let Some(name) = tako_core::source_scan::fn_head_name(line) {
            current = name.to_string();
        }
        // 説明文（コメント）は対象外。禁止するのは**コードが組み立てる体験**
        if line.trim_start().starts_with("//") {
            continue;
        }
        let Some(hit) = needles.iter().find(|n| line.contains(**n)) else {
            continue;
        };
        if allowed
            .iter()
            .any(|(f, func, _)| *f == rel && *func == current)
        {
            continue;
        }
        out.push(format!("{rel}:{} （{current} の中）: {hit}", i + 1));
    }
    out
}

/// その needle を含む行が、どの関数の中にあるか（最初の 1 件）
fn enclosing_fn(src: &str, needle: &str) -> Option<String> {
    let mut current = String::from("(ファイル先頭)");
    for line in src.lines() {
        if let Some(name) = tako_core::source_scan::fn_head_name(line) {
            current = name.to_string();
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        if line.contains(needle) {
            return Some(current);
        }
    }
    None
}

/// **本体 1**: CLI は判断も `[y/N]` も持たない（渡すのは段組みの字下げだけ）。
#[test]
fn issue1524_setupは依存の案内と確認を組み立てない() {
    let src = production(CLI_SETUP);
    let offenders = offenders_in(&src, CLI_SETUP, CLI_BANNED, &[]);
    assert!(
        offenders.is_empty(),
        "#1524 の退行: `tako setup` の CLI が依存導入の体験を自分で組み立てている:\n  {}\n\
         → 案内 → `[y/N]` → 導入 → 再検出のひと続きは \
         `tako_control::setup_deps::offer_and_install` を通してください。\
         2 か所に置くと `tako remote setup` と文面・挙動が黙ってズレます（#1509 / #1524）",
        offenders.join("\n  ")
    );
}

/// **本体 2**: `[y/N]` の文面は本番コードに 1 か所だけ（第 3 の呼び手を止める）。
#[test]
fn issue1524_確認の文面は1ファイルだけが持つ() {
    let mut holders = Vec::new();
    let mut scanned = 0usize;
    for path in production_sources() {
        let rel = rel_of(&path);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        scanned += 1;
        let body = production_range::scan(&src).text;
        if !offenders_in(&body, &rel, &[PROMPT_TEXT], &[]).is_empty() {
            holders.push(rel);
        }
    }
    assert!(
        scanned > 100,
        "走査が当たっていない（{scanned} ファイルしか読めていない）"
    );
    assert_eq!(
        holders,
        vec![SETUP_DEPS.to_string()],
        "`{PROMPT_TEXT}` を持つ本番ファイルは `{SETUP_DEPS}` だけであること（#1524）。\
         実際: {holders:?}"
    );
}

/// **本体 3**: `install` の直呼びは名指しで打たれた経路だけ。
#[test]
fn issue1524_導入の直呼びは名指しの経路だけ() {
    let mut offenders = Vec::new();
    let mut scanned = 0usize;
    for path in production_sources() {
        let rel = rel_of(&path);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        scanned += 1;
        let body = production_range::scan(&src).text;
        // `setup_deps.rs` の中（`offer_and_install` → `install`）は正本そのもの
        if rel == SETUP_DEPS {
            continue;
        }
        offenders.extend(offenders_in(
            &body,
            &rel,
            &[DIRECT_INSTALL, "crate::setup_deps::install("],
            ALLOWED_DIRECT,
        ));
    }
    assert!(scanned > 100, "走査が当たっていない（{scanned} ファイル）");
    assert!(
        offenders.is_empty(),
        "#1524: 依存導入を `install` の直呼びで済ませている（勧める側は \
         `offer_and_install` を通す）:\n  {}\n\
         → 直呼びしてよいのは**利用者が導入を名指しで打った経路**だけです。\
         増やすときは ALLOWED_DIRECT へ理由つきで宣言してください",
        offenders.join("\n  ")
    );
}

/// **本体 4**: 寄せ先が空振りしていない（許可リストだけ残して中身を戻した /
/// 改名で当たらなくなった、を検出する）。
#[test]
fn issue1524_寄せ先が空振りしていない() {
    for (rel, func, needle) in MIGRATED {
        let src = production(rel);
        let found = enclosing_fn(&src, needle);
        assert_eq!(
            found.as_deref(),
            Some(*func),
            "{rel}: `{needle}` が `{func}` の中から呼ばれていない（実際: {found:?}）。\
             導入を自前で組み直したか、寄せ先を改名したまま（#1524）"
        );
    }
    // 許可リストの免除先が実在すること（改名で穴が開いたまま気づかない、を防ぐ）
    for (rel, func, why) in ALLOWED_DIRECT {
        let src = read(rel);
        assert!(
            src.lines()
                .any(|l| tako_core::source_scan::fn_head_name(l) == Some(*func)),
            "{rel} に `{func}` が無い（免除の理由: {why}）。\
             改名したら許可リストも直すこと（#1524）"
        );
    }
}
