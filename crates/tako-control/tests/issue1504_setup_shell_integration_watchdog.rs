//! 番犬: シェル統合の配置（FR-2.14.14 / #1504）が setup の段から外れたまま緑にならないようにする
//!
//! ## 何が起きていたのか
//!
//! `tako_core::shell_integration::install()` の呼び手は CLI `tako shell-integration` と
//! MCP だけで、**`tako setup` も installer も CI も呼んでいなかった**（#1500 の棚卸し Z9）。
//! macOS / Linux は spawn 時の環境変数注入で届くので人の手はゼロだが、**Windows は
//! `$PROFILE` へブロックを追記しないと 1 行も効かない**ので、OSC 7 / 133（ペインの
//! cwd 追従・コマンド実行状態）・入力予測・自動命名の素材が、人が
//! `tako shell-integration install` を自分で見つけて打つまで欠けたままだった。
//!
//! ## ここで止める 5 つ
//!
//! 1. **段の呼び忘れ**。配置は `tako setup` の 1 行と `tako setup --check` の 1 行で
//!    正本（tako-control）へ入る。**この 2 行は消えやすい**（`setup.rs` は #1499 /
//!    #1501 / #1503 / #1504… が同じ関数を順番に触る）ので、呼んでいることを縛る
//! 2. **配置ロジックの二重化**。`install()` を呼ぶ本番コードは
//!    `crates/tako-control/src/shell_integration.rs` の 1 ファイルだけ（CLI が自分で
//!    `$PROFILE` を触り始めたら、冪等性とマーカー 1 個の保証が 2 か所に散る）
//! 3. **質問の再生**。段は `[y/N]` を出さない（#262 の質問ゼロ）。聞かないので
//!    `--yes` / 非 TTY / GUI の初回起動で分岐する必要が無い = 入力を読む口を持たない
//! 4. **`cfg!(windows)` での分岐**。配置が要るかは [`Delivery`] が宣言している。
//!    cfg で分けると **Windows でだけ通る経路が macOS のテストから消える**
//!    （実機を持たない CI では永久に検査できなくなる）
//! 5. **失敗で setup が止まること**。段は `Result` を返さない（`?` で上へ投げられない）。
//!    配置に失敗しても「残り 1 件」として脇に置き、setup は完走する（#1501 の契約）
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! 実経路（隔離 HOME で `tako setup` → 表示・冪等・失敗時の完走）は
//! `scripts/test-setup-shell-integration-1504.sh`。Windows 形の表示・残りの判断は
//! `tako_control::shell_integration` の `stage_tests`（Profile 経路の `Status` を
//! 組んで macOS 上で全分岐を通す）。ここは**配線が外れていないこと**だけを見る。

use std::path::{Path, PathBuf};

use tako_core::source_scan::{fn_head_name, is_top_level_fn_head};

// 本番コードだけの眺めは共有部品の 1 実装を通す（#1420。`#[cfg(test)]` で
// **切る**とテスト用ヘルパ 1 つで以降の本番コードが番犬の視界から消える）
#[path = "common/production_range.rs"]
mod production_range;

const STAGE_RS: &str = "crates/tako-control/src/shell_integration.rs";
const SETUP_RS: &str = "crates/tako-cli/src/setup.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリのルートを解決できる")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// **本番コードだけ**の眺め（テスト領域は空白へ潰す。行番号は保たれる）。
///
/// 段のテスト（`stage_tests`）は Windows 形の `Status` を組んで「`[y/N]` が出ないこと」を
/// 確かめる側なので、そこに出てくる文字列を本番の配線と混ぜると番犬が自分のテストを
/// 捕まえる。関数名の追跡も入れ子モジュールの中では効かない（トップレベルの `fn` しか
/// 見ない）ので、テスト領域は落としてから走査する。**切らずに潰す**のと
/// 「黙って縮んだ」の検出は `common/production_range.rs` の 1 実装（#1420）
fn production_part(rel: &str, text: &str) -> String {
    production_range::production(text, rel)
}

/// 走査だけ（下限を課さない）。ファイルまるごとテストのものも通るので
/// クレート全体の掃き出しに使う（`common/production_range.rs` の注記どおり）
fn production_loose(text: &str) -> String {
    production_range::scan(text).text
}

/// `needle` を含む**コメントでない**行を、それを囲むトップレベル関数名つきで拾う。
/// 関数名の追跡は `source_scan` の 1 実装（#1496。`pub fn` / `async fn` を取りこぼさない）
fn hits_in_fn(text: &str, needle: &str) -> Vec<(usize, String)> {
    let mut current = String::new();
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if is_top_level_fn_head(line) {
            current = fn_head_name(line).unwrap_or("").to_string();
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        if line.contains(needle) {
            out.push((i + 1, current.clone()));
        }
    }
    out
}

fn where_(rel: &str, hits: &[(usize, String)]) -> String {
    if hits.is_empty() {
        return "（無し）".to_string();
    }
    hits.iter()
        .map(|(line, f)| format!("{rel}:{line}（{f}）"))
        .collect::<Vec<_>>()
        .join(" / ")
}

/// 1: `tako setup` の本体が配置の段を呼んでいる（#1504 の配線）
#[test]
fn setupがシェル統合の配置を呼んでいる() {
    let text = production_part(SETUP_RS, &read(SETUP_RS));
    let hits = hits_in_fn(&text, "run_setup_stage");
    assert!(
        !hits.is_empty(),
        "{SETUP_RS}: `tako setup` がシェル統合の配置を呼んでいない（FR-2.14.14 / #1504）。\n\
         `run_setup` の中で \
         `setup_shell_integration::run_setup_stage()` を呼び、行を出して \
         `remaining.extend(…)` すること。\n\
         これが無いと Windows では OSC 7 / 133・cwd 追従・入力予測が \
         人が `tako shell-integration install` を打つまで効かない"
    );
    assert!(
        hits.iter().any(|(_, f)| f == "run_setup"),
        "{SETUP_RS}: 配置の呼び出しが `run_setup` の外に居る（見つかった場所: {}）。\n\
         `tako setup` の本流から必ず通る位置へ置くこと（#1504）",
        where_(SETUP_RS, &hits)
    );
    // 段が返した「残り」を捨てていない（捨てると失敗が末尾の一覧から消える）
    let remaining = hits_in_fn(&text, "shell_integration.remaining");
    assert!(
        remaining.iter().any(|(_, f)| f == "run_setup"),
        "{SETUP_RS}: 段が返した残り作業を `remaining` へ積んでいない（#1501 の契約）。\n\
         配置に失敗しても末尾の「残り N 件」に出ないと、人は次に打つ 1 行を知れない。\n\
         見つかった場所: {}",
        where_(SETUP_RS, &remaining)
    );
}

/// 1': `tako setup --check` が配置状況を報告している
#[test]
fn setup_checkがシェル統合を報告している() {
    let text = production_part(SETUP_RS, &read(SETUP_RS));
    for needle in ["setup_shell_integration::check_line", "check_remaining"] {
        let hits = hits_in_fn(&text, needle);
        assert!(
            hits.iter().any(|(_, f)| f == "run_check"),
            "{SETUP_RS}: `tako setup --check` が `{needle}` を `run_check` から呼んでいない\
             （#1504 / Z19）。\n\
             `tako setup` と同じ 1 実装で状況と残りを出すこと。見つかった場所: {}",
            where_(SETUP_RS, &hits)
        );
    }
}

/// 2: `install()` を呼ぶ本番コードは 1 ファイルだけ（配置の二重化を止める）
#[test]
fn 配置を呼ぶ本番コードは一箇所のまま() {
    // CLI が自分で tako_core のシェル統合へ手を伸ばしていない
    let setup = production_part(SETUP_RS, &read(SETUP_RS));
    let direct = hits_in_fn(&setup, "tako_core::shell_integration");
    assert!(
        direct.is_empty(),
        "{SETUP_RS}: CLI が `tako_core::shell_integration` を直に触っている（{}）。\n\
         配置の判断・表示・「残り」の正本は `tako_control::shell_integration` の\n\
         `run_setup_stage` / `check_line` で、CLI は呼んで出すだけにすること（#1504）",
        where_(SETUP_RS, &direct)
    );

    // 本番ソース全体で `si::install()` を呼ぶのは段の正本だけ
    let src = repo_root().join("crates");
    let mut callers: Vec<String> = Vec::new();
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("crates を辿れる") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                // テストは対象外（`tests/` は配置を検査する側）
                if path
                    .file_name()
                    .is_some_and(|n| n == "tests" || n == "target")
                {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().is_some_and(|e| e == "rs") {
                let text = production_loose(&std::fs::read_to_string(&path).unwrap_or_default());
                let rel = path
                    .strip_prefix(repo_root())
                    .unwrap_or(&path)
                    .display()
                    .to_string();
                if rel.ends_with("shell_integration.rs") {
                    continue; // 正本（tako-core の実装本体と tako-control の段）
                }
                for needle in ["si::install()", "shell_integration::install()"] {
                    if !hits_in_fn(&text, needle).is_empty() {
                        callers.push(rel.clone());
                    }
                }
            }
        }
    }
    assert!(
        callers.is_empty(),
        "シェル統合の配置（`install()`）を正本以外から呼んでいる: {callers:?}\n\
         冪等性・マーカー 1 個・`uninstall` で元へ戻せることは {STAGE_RS} の\n\
         1 実装が担保する（#1504）。呼び手は `run_setup_stage` を通すこと"
    );
}

/// 3: 段は聞かない（`--yes` / 非 TTY で分岐する必要が無い形を保つ）
#[test]
fn 段は入力を読まない() {
    let text = production_part(STAGE_RS, &read(STAGE_RS));
    for needle in ["[y/N]", "read_line", "is_terminal", "stdin"] {
        let hits = hits_in_fn(&text, needle);
        assert!(
            hits.is_empty(),
            "{STAGE_RS}: 段が `{needle}` を持っている（{}）。\n\
             ユーザーのファイルへ書く前の「何をどこへ」は**表示して同意扱いで続行**する\n\
             （#262 の質問ゼロ・#1502 の PATH 設置と同じ作法）。聞かないから\n\
             `--yes` / 非 TTY / GUI の初回起動で分岐せずに同じ結果になる（#1504）",
            where_(STAGE_RS, &hits)
        );
    }
}

/// 4: 配置が要るかの判断は `Delivery` で、`cfg!(windows)` で分けない
#[test]
fn 配置の判断はdeliveryで行い_cfgで分けない() {
    let text = production_part(STAGE_RS, &read(STAGE_RS));
    for needle in ["cfg!(windows)", "cfg(windows)", "cfg!(unix)", "cfg(unix)"] {
        let production = hits_in_fn(&text, needle);
        assert!(
            production.is_empty(),
            "{STAGE_RS}: 段が `{needle}` で分岐している（{}）。\n\
             配置が要るかは `Delivery`（automatic / profile）が宣言しているので\n\
             そちらで分けること。cfg で分けると Windows でだけ通る経路が\n\
             macOS のテストから消え、実機を持たない CI では永久に検査できない（#1504）",
            where_(STAGE_RS, &production)
        );
    }
    assert!(
        text.contains("fn needs_placement"),
        "{STAGE_RS}: `needs_placement`（配置が要るかの判断 1 か所）が消えている"
    );
    assert!(
        text.contains("si::Delivery::Profile"),
        "{STAGE_RS}: 判断が `Delivery` を見ていない"
    );
}

/// 5: 段は `Result` を返さない（失敗を `?` で上へ投げて setup を止められない形）
#[test]
fn 段は失敗してもsetupを止めない() {
    let text = production_part(STAGE_RS, &read(STAGE_RS));
    assert!(
        text.contains("pub fn run_setup_stage() -> StageOutcome"),
        "{STAGE_RS}: `run_setup_stage` が `StageOutcome` 以外を返している。\n\
         `Result` にすると呼び手が `?` で上へ投げられるようになり、\n\
         配置の失敗で `tako setup` 全体が exit 1 する（#1501 で畳んだ形へ逆戻り）"
    );
    // 呼び手も `?` を付けていない
    let hits = hits_in_fn(
        &production_part(SETUP_RS, &read(SETUP_RS)),
        "run_setup_stage()?",
    );
    assert!(
        hits.is_empty(),
        "{SETUP_RS}: 段の結果を `?` で投げている（{}）",
        where_(SETUP_RS, &hits)
    );
}
