//! **#1509 の番犬**: 依存ツールの導入経路は `setup_deps` の 1 本だけ。
//!
//! ## 何が壊れていたか
//!
//! `tako setup` の依存チェック段は `setup_deps::install`（判断 `offer_for` →
//! 実行 `install` → 再検出 `resolve`）を通るのに、`tako remote setup` の [1/5] は
//! **`brew install tailscale` を自前で提案・実行**していた（#1500 の棚卸し Z6）。
//! 同じ「依存を入れる」体験が 2 か所で組み上がっていたので、片方だけ直る退行が
//! 実際に残っていた:
//!
//! | 実測（隔離環境・出荷版 v0.8.17） | `remote setup`（旧） | `tako setup`（#1499 後） |
//! |---|---|---|
//! | brew が無い | **聞いてから起動に失敗**して中断 | 聞かずに `要 Homebrew` を案内 |
//! | 非 TTY | EOF を N と読むだけ（理由を言わない） | 案内 + 理由（`端末が無いので…`） |
//! | 入れたのに引けない | `検出できません` で中断 | 理由 + `いま入れる:` の次の一手 |
//! | どこへ入るか | 出さない | `導入器 … → … へ入ります` |
//!
//! ## 何を止めるか
//!
//! 1. 導入器（brew / winget）の**プロセス起動**が `setup_deps::run_installer` 以外に無い
//!    （tako 自身の配布物を扱う `update_checker` は別目的なので理由つきで許可）
//! 2. `remote_setup.rs` の本番領域に導入器の知識（`brew` / `winget` の語）が無い
//! 3. 寄せ先が空振りしていない（`setup_deps::offer_and_install` を実際に呼んでいる）

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

/// 導入器のプロセス起動。見つけたら `setup_deps::install` を通すこと
const INSTALLER_LAUNCH: &[&str] = &[
    "Command::new(\"brew\")",
    "Command::new(\"winget\")",
    "Command::new(\"brew.exe\")",
    "Command::new(\"winget.exe\")",
];

/// 起動を許す場所（ファイル, 囲んでいる関数名, 理由）。**黙って増やさない**
const ALLOWED_LAUNCH: &[(&str, &str, &str)] = &[
    (
        "crates/tako-control/src/setup_deps.rs",
        "run_installer",
        "依存ツール導入の**唯一の実行点**（#88 / #1057 / #1499 / #1509）。\
         判断は `offer_for`・再検出は `resolve` が同じファイルで担う",
    ),
    (
        "crates/tako-app/src/update_checker.rs",
        "is_brew_available",
        "tako 自身が cask で入っているかの判定（#50）。依存ツールの導入ではない",
    ),
    (
        "crates/tako-app/src/update_checker.rs",
        "is_brew_cask_registered",
        "同上（台帳に tako の cask が載っているかの判定）",
    ),
    (
        "crates/tako-app/src/update_checker.rs",
        "repair_brew",
        "broken-brew（tako 自身の cask 台帳）の修復（#50）",
    ),
    (
        "crates/tako-app/src/update_checker.rs",
        "update_via_homebrew",
        "tako 自身のアップデート（#36）。入れる対象が tako なので依存表には載らない",
    ),
];

/// `remote_setup.rs` の本番領域に出てはいけない語（導入器の知識は持たない）
const REMOTE_SETUP_BANNED: &[&str] = &["brew", "winget"];

/// 寄せ先（ファイル, 呼んでいるはずの入口）。巻き戻し・改名の空振りを止める
const MIGRATED: &[(&str, &str)] = &[
    (
        "crates/tako-control/src/remote_setup.rs",
        "setup_deps::offer_and_install",
    ),
    (
        "crates/tako-control/src/remote_setup.rs",
        "setup_deps::status_of",
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// `crates/*/src/**/*.rs`（本番コードの置き場）を集める
fn production_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let crates = repo_root().join("crates");
    let Ok(reader) = std::fs::read_dir(&crates) else {
        panic!("crates/ を読めない");
    };
    for entry in reader.flatten() {
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
        // 説明文（コメント）は対象外。禁止するのは**コードが起こす導入器**
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

/// **本体 1**: 導入器のプロセス起動は `setup_deps::run_installer` だけ。
#[test]
fn 依存導入の実行点は1か所() {
    let mut offenders = Vec::new();
    let mut scanned = 0usize;
    for path in production_sources() {
        let rel = rel_of(&path);
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        scanned += 1;
        // テスト領域は潰す（本番コードは**切らない** = #1420）
        let body = production_range::scan(&src).text;
        offenders.extend(offenders_in(&body, &rel, INSTALLER_LAUNCH, ALLOWED_LAUNCH));
    }
    assert!(
        scanned > 100,
        "走査が当たっていない（{scanned} ファイルしか読めていない）"
    );
    assert!(
        offenders.is_empty(),
        "依存ツールの導入器を直に起こしている箇所がある:\n  {}\n\
         → `tako_control::setup_deps::install`（判断は `offer_for`・\
         案内と確認ごと寄せるなら `offer_and_install`）を通してください。\
         直叩きだと「brew が無い」「非 TTY」「入れたのに引けない」の扱いが\
         経路ごとに割れます（#1509）",
        offenders.join("\n  ")
    );
}

/// **本体 2**: `remote setup` は導入器の知識を持たない（#1509 で寄せた先が正）。
#[test]
fn remote_setupは導入器を知らない() {
    let rel = "crates/tako-control/src/remote_setup.rs";
    let src = std::fs::read_to_string(repo_root().join(rel)).expect("remote_setup.rs");
    let body = production_range::production(&src, rel);
    let offenders = offenders_in(&body, rel, REMOTE_SETUP_BANNED, &[]);
    assert!(
        offenders.is_empty(),
        "`remote setup` が導入器を自分で扱っている:\n  {}\n\
         → 導入の判断・実行・再検出は `setup_deps`（`offer_and_install`）が持ちます。\
         手段の名前（brew / winget）は `setup_deps::deps()` の依存表の中だけ（#1509）",
        offenders.join("\n  ")
    );
}

/// **本体 3**: 寄せ先が実際に呼ばれていること（許可リストだけ残して
/// 中身を書き戻した / 改名で空振りした、を検出する）
#[test]
fn 寄せ先が空振りしていない() {
    for (rel, needle) in MIGRATED {
        let src = std::fs::read_to_string(repo_root().join(rel))
            .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
        assert!(
            src.contains(needle),
            "{rel} が `{needle}` を呼んでいない。導入を自前で組み直したか、\
             寄せ先を改名したまま（#1509）"
        );
    }
    // 許可リストの免除先が実在すること（改名で穴が開いたまま気づかない、を防ぐ）
    for (rel, func, why) in ALLOWED_LAUNCH {
        let src = std::fs::read_to_string(repo_root().join(rel))
            .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"));
        assert!(
            src.lines()
                .any(|l| tako_core::source_scan::fn_head_name(l) == Some(*func)),
            "{rel} に `{func}` が無い（免除の理由: {why}）。\
             改名したら許可リストも直すこと（#1509）"
        );
    }
}
