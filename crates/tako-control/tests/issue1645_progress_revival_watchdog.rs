//! 作業ログの「復活」を止める番犬（Issue #1645）
//!
//! 着地キューを 1 本ずつ rebase する運用では、`tako context-budget fix` の移送
//! （古いエントリを**消す**）と main 側の別 PR の移送が噛み合っても
//! **git は衝突を報告せず auto-merge する**ので、main で既にアーカイブ済みの
//! エントリが `.agent/progress.md` へ黙って戻る。9/23 だけで PR #1600 / #1604 /
//! #1610 / #1612 の 4 回起き、毎回 worker の目視で見つけて作り直していた。
//! **予算を超えなければ既存の番犬（`context_budget.rs`）では落ちない**ので、
//! 「戻っていないこと」そのものを検査する。
//!
//! 判定は `tako_core::context_budget::revivals` の 1 実装で、
//! `tako context-budget check` の violations / MCP `tako_context_budget` も
//! 同じものを通る（開発不変条件: UI でできることは AI からもできる）。
//!
//! # 偽の緑を作らないための作り
//!
//! - この番犬は**ソースを走査しない**（読むのは `.agent/progress.md` と
//!   `.agent/progress-archive.md` の 2 つだけ）ので、#1609 の型
//!   「自分の doc コメントの文字列を拾って緑になる」は起こらない
//! - 述語が壊れて**見出しを 1 件も拾えなくなる**と突き合わせは黙って素通りする。
//!   `見出しを1件も拾えない実装では緑にしない` が空振りを落とす

use std::path::{Path, PathBuf};
use tako_core::context_budget as budget;

const PROGRESS: &str = ".agent/progress.md";
const ARCHIVE: &str = ".agent/progress-archive.md";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let p = repo_root().join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{} が読めない: {e}", p.display()))
}

#[test]
fn アーカイブ済みのエントリが作業ログへ戻っていない() {
    let hits = budget::revivals(&read(PROGRESS), &read(ARCHIVE));
    // 表示言語に依らない出力にする（CI の環境設定で文言が変わらないように）
    let named: Vec<String> = hits
        .iter()
        .map(|r| r.locate_in(tako_core::i18n::Lang::Ja, PROGRESS, ARCHIVE))
        .collect();
    assert!(
        named.is_empty(),
        "アーカイブ済みのエントリが作業ログに戻っている（rebase の auto-merge で起きる）\n{}\n\
         直し方: git checkout origin/main -- {PROGRESS} {ARCHIVE} でこの 2 ファイルだけを\n\
         main の状態へ戻し、自分の 1 件を書き直してから push する\n\
         状態の確認: cargo run -p tako-cli -- context-budget（violations に同じ違反が出る）",
        named.join("\n")
    );
}

/// 日付を書こうとした見出しか（`#NNN` か 4 桁の数字の並びを持つ）
fn looks_dated(line: &str) -> bool {
    budget::has_issue_ref(line)
        || line
            .as_bytes()
            .windows(4)
            .any(|w| w.iter().all(|c| c.is_ascii_digit()))
}

/// 空振り検査。**述語が壊れると突き合わせは黙って素通りする**ので、
/// リポジトリの実ファイルを述語が読めていることをここで固定する
#[test]
fn 見出しを1件も拾えない実装では緑にしない() {
    let progress = read(PROGRESS);
    let archive = read(ARCHIVE);

    // 1) 日付を書いた `## ` 行はすべて日付見出しとして読めること。
    //    見出しの書式が崩れた行（`## 2026/09/23（#1504 …）` など）は
    //    エントリとして拾われず、そのぶん突き合わせが素通りする。
    //    冒頭の説明（`## 追記フォーマット` とテンプレの `## YYYY-MM-DD（#Issue 一言）`）は
    //    日付も番号も持たないので、ここでは「日付らしさ」で選り分ける
    let bad: Vec<String> = progress
        .lines()
        .enumerate()
        .filter(|(_, l)| {
            l.starts_with("## ") && budget::heading_date(l).is_none() && looks_dated(l)
        })
        .map(|(i, l)| format!("  {PROGRESS}:{} {l}", i + 1))
        .collect();
    assert!(
        bad.is_empty(),
        "日付見出しとして読めない `## ` 行がある（書式は `## YYYY-MM-DD（#NNN 一言）`）\n{}",
        bad.join("\n")
    );

    // 2) 実際に拾えていること（0 件なら突き合わせは何もしていない）
    let log = budget::parse_log(&progress);
    assert!(
        log.entries.len() >= 3,
        "{PROGRESS} の見出しを {} 件しか拾えていない。\
         `heading_date` か見出しの書式が壊れると、復活の検査は黙って素通りする",
        log.entries.len()
    );
    assert_eq!(
        budget::heading_line_numbers(&progress).len(),
        log.entries.len(),
        "見出しの行番号とエントリの件数が食い違う（名指しの行がズレる）"
    );

    // 3) 突き合わせの鍵に Issue 番号が載っていること
    let lines = budget::heading_line_numbers(&progress);
    let no_ref: Vec<String> = log
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !budget::has_issue_ref(&e.archive_line()))
        .map(|(i, e)| format!("  {PROGRESS}:{} {}", lines[i], e.heading))
        .collect();
    assert!(
        no_ref.is_empty(),
        "見出しにも本文にも `#NNN` が無いエントリがある（アーカイブの 1 行と突き合わせられない）\n{}",
        no_ref.join("\n")
    );

    // 4) アーカイブ側の 1 行形式も読めていること
    let archived = archive
        .lines()
        .filter(|l| budget::one_line_date(l).is_some())
        .count();
    assert!(
        archived >= 50,
        "{ARCHIVE} の 1 行エントリを {archived} 件しか拾えていない。\
         `one_line_date` か `- YYYY-MM-DD …` の書式が壊れている"
    );
    let bad_archive: Vec<String> = archive
        .lines()
        .enumerate()
        .filter(|(_, l)| l.starts_with("- 2") && budget::one_line_date(l).is_none())
        .map(|(i, l)| format!("  {ARCHIVE}:{} {l}", i + 1))
        .collect();
    assert!(
        bad_archive.is_empty(),
        "アーカイブの 1 行として読めない行がある（書式は `- YYYY-MM-DD #NNN 一言`）\n{}",
        bad_archive.join("\n")
    );
}

/// 検出力そのものの検査（合成素材。実ファイルには触れない）
#[test]
fn 戻したエントリと重複したエントリを名指しで落とす() {
    let progress = concat!(
        "# Progress Log\n",
        "\n",
        "## 2026-09-22（#1501: 未認証でも setup が完走する）\n",
        "- 何を / どこを / 結果\n",
        "\n",
        "## 2026-09-23（#1504: Windows のシェル統合を段にした）\n",
        "- 何を / どこを / 結果\n",
    );
    let moved = budget::parse_log(progress).entries[0].archive_line();
    let archive = format!("# Progress Archive\n\n- 2026-09-21 #1493: 番犬で縛った\n{moved}\n");

    // (a) アーカイブ済みが戻っている
    let hits = budget::revivals(progress, &archive);
    assert_eq!(hits.len(), 1, "復活を 1 件名指しする");
    let named = hits[0].locate_in(tako_core::i18n::Lang::Ja, PROGRESS, ARCHIVE);
    assert!(
        named.starts_with(&format!("{PROGRESS}:3 ")) && named.contains(&format!("{ARCHIVE}:4")),
        "両側を file:line で名指しする: {named}"
    );

    // (b) 同じエントリが 2 回
    let dup = format!(
        "{progress}\n{}",
        budget::parse_log(progress).entries[1].render()
    );
    let hits = budget::revivals(&dup, "# Progress Archive\n");
    assert_eq!(hits.len(), 1, "重複を 1 件名指しする");
    assert_eq!(hits[0].kind, budget::RevivalKind::Duplicated);
    assert!(
        hits[0]
            .locate_in(tako_core::i18n::Lang::Ja, PROGRESS, ARCHIVE)
            .contains(&format!("{PROGRESS}:6")),
        "先に出た見出しの行を指す"
    );

    // (c) 同じ日・同じ Issue の別エントリは落とさない（アーカイブに実在する形）
    let same_day = concat!(
        "# Progress Log\n",
        "\n",
        "## 2026-09-14（#1450: tasks ビューを右パネルへ出した）\n",
        "- 何を / どこを / 結果\n",
        "\n",
        "## 2026-09-14（#1450: tasks を PWA から片付けられるようにした）\n",
        "- 何を / どこを / 結果\n",
    );
    assert!(
        budget::revivals(
            same_day,
            "# Progress Archive\n\n- 2026-09-14 #1450: 受け口を足した\n"
        )
        .is_empty(),
        "同じ日・同じ Issue の別エントリを復活と読まない（誤検出すると書けなくなる）"
    );
}
