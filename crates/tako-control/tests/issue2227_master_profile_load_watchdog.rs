//! master の起動で読めない `profiles/default.yaml` を黙って組み込み既定へ落とさない番犬（#2227）
//!
//! # なぜ要るか
//!
//! `tako master`（CLI）と `master_launch::plan`（スマホの「+ master」・返答の配送）は、
//! それぞれ `Err(_) if profile_name == "default" => Profile::default()` で読めない
//! default.yaml を握りつぶしていた。個人ルール（`prompt_blocks`）・モデルの抜けた master が
//! 警告なしに立ち、`-<key>` は #1825 で専用プロファイルが作られていないため直し方の無い
//! 「見つからない」で止まった（修正前の実測）。どちらも壊れ方が静かで、立てた master に
//! ルールが効いていないと気づくまで誰にも見えない。
//!
//! # 何を縛るか
//!
//! 1. CLI と `plan` がプロファイルを**同じ 1 実装**（`master_launch::load_profile_typed`。CLI と `plan` は
//!    断りを表示言語で組む口 `load_profile_in` = #2365）で読み、
//!    自前の `Profile::load` / `Profile::default()` の握りつぶしへ戻らない
//! 2. `load_default_at` が継承元と同じ読み方（`project_profile_base_at` = 退避つき）を通り、
//!    `Err` を握りつぶさず warning にする
//! 3. `load_profile_typed` が warning を persist.log へ出し、`-<key>` の「見つからない」に
//!    #1825 の理由（`missing_project_profile_reason`）を足す
//! 4. 理由の判定が継承元の読み取り（`check_project_profile_base`）と #1825 の理由文を通る
//! 5. warning が画面（CLI の `警告:` 行 = 見出しは #2365 から表示言語の `ReplyNote::WarningLabel`・
//!    応答の `warnings`）と master 本人の system prompt
//!    （`with_startup_warning`）へ届く
//!
//! # #2233 で広げた範囲（同じ根の残り）
//!
//! 6. 引き継ぎの後任（`dispatch_orchestrator_handoff`）が `load_successor_profile`
//!    （= `load_profile_typed`）で読み、警告を応答の `warnings` と後任の system prompt へ渡す
//! 7. `tako solo`（CLI）が `load_solo`（solo の default も `load_default_at` = 退避つき・
//!    persist.log）で読み、`警告:` 行と solo 本人の system prompt へ渡す
//! 8. 表示・判定だけの経路（`orchestrator self` / `supervisor status` / `tako setup` の表示）が
//!    `load_for_display`（**読むだけ** = 退避しない）を通り、理由を `warnings` / 画面の行へ出す。
//!    `handoff_policy_for`（GUI の周期判定）は出し先が無いので対象外（判断は doc と PR #2233）
//!
//! # #2256 で広げた範囲（#2233 の残り）
//!
//! 9. worker の spawn・コンフリクト解消エージェント（`resolve_caller_profile_with_role`）が
//!    `load_caller_profile`（default の起動と同じ退避つきの読み方）で読み、止めずに立てた理由を
//!    応答の `warnings` と persist.log（`[spawn]` / `[git-resolve]`）へ出す
//! 10. adopt の起動時プロファイル・`orchestrator guide`・master / solo 本人の自動復帰の既定が
//!     `read_for_judgment`（**読むだけ** = 退避しない）を通り、理由を応答の `warnings`
//!     （guide は `local-rules` の本文も「読めない」へ・CLI は `警告:` 行）/ persist.log へ出す
//!
//! # 見逃す側へ倒れないための作り
//!
//! 窓が採れていることを [`走査が空振りしていない`] で固定し、[`逆戻りを名指しできる`] で
//! **修正前を再現した注入**がそれぞれ注入した行の file:line で名指しされることを確かめる。
//! 範囲取りは #1420 の 1 実装（`production_range`）を通す。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const LAUNCH: &str = "crates/tako-control/src/orchestrator/master_launch.rs";
const ORCH: &str = "crates/tako-control/src/orchestrator/mod.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const SETUP: &str = "crates/tako-cli/src/setup.rs";
const GUIDE: &str = "crates/tako-control/src/orchestrator/guide.rs";

/// 読み込みで**使ってはいけない**握りつぶし
const SWALLOWS: &[&str] = &[
    "unwrap_or_default()",
    ".ok()",
    "unwrap_or(",
    "unwrap_or_else(",
    ".unwrap()",
    ".expect(",
];

#[derive(Debug)]
struct Offender {
    file: &'static str,
    line: usize,
    why: String,
}

impl Offender {
    fn report(&self) -> String {
        format!("{}:{} — {}", self.file, self.line, self.why)
    }
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

/// 本番コードだけの眺め（テスト領域は空白へ潰す。行番号は保たれる）
fn production(rel: &'static str, src: &str) -> String {
    production_range::production_with_floor(src, rel, 0.20)
}

#[derive(Clone)]
struct Sources {
    launch: String,
    orch: String,
    cli: String,
    dispatch: String,
    setup: String,
    guide: String,
}

fn sources(root: &Path) -> Sources {
    Sources {
        launch: production(LAUNCH, &read(root, LAUNCH)),
        orch: production(ORCH, &read(root, ORCH)),
        cli: production(CLI, &read(root, CLI)),
        dispatch: production(DISPATCH, &read(root, DISPATCH)),
        setup: production(SETUP, &read(root, SETUP)),
        guide: production(GUIDE, &read(root, GUIDE)),
    }
}

/// 関数の窓（宣言行の 1-based 行番号と、本文の行）。終わりは宣言行と同じ字下げの `}`
fn fn_window<'a>(src: &'a str, needle: &str) -> Option<(usize, Vec<&'a str>)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].to_vec()))
}

fn is_code(line: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with("//") && !t.is_empty()
}

fn window_has(window: &[&str], needle: &str) -> bool {
    window.iter().any(|l| is_code(l) && l.contains(needle))
}

/// 窓の中で `needle` を含むコード行の 1-based 行番号（すべて）
fn lines_with(at: usize, window: &[&str], needle: &str) -> Vec<usize> {
    window
        .iter()
        .enumerate()
        .filter(|(_, l)| is_code(l) && l.contains(needle))
        .map(|(i, _)| at + i)
        .collect()
}

/// 関数の窓に要るもの・あってはいけないものの宣言（#2256 の経路で使う）
struct Rule {
    fn_needle: &'static str,
    /// 窓に要るもの（欠けたら関数の宣言行を名指し）と、欠けたときの理由
    needs: &'static [(&'static str, &'static str)],
    /// 窓のコード行にあってはいけないもの（あればその行を名指し）と理由
    bans: &'static [(&'static str, &'static str)],
}

fn scan_rules(file: &'static str, src: &str, rules: &[Rule], out: &mut Vec<Offender>) {
    for rule in rules {
        let Some((at, window)) = fn_window(src, rule.fn_needle) else {
            out.push(Offender {
                file,
                line: 0,
                why: format!("`{}` が見つからない（走査が空振り）", rule.fn_needle),
            });
            continue;
        };
        for (need, why) in rule.needs {
            if !window_has(&window, need) {
                out.push(Offender {
                    file,
                    line: at,
                    why: (*why).to_string(),
                });
            }
        }
        for (ban, why) in rule.bans {
            for line in lines_with(at, &window, ban) {
                out.push(Offender {
                    file,
                    line,
                    why: (*why).to_string(),
                });
            }
        }
    }
}

/// master の起動の入口が直接使ってはいけない読み方（修正前の形）
const MASTER_BADS: &[&str] = &["Profile::load(", "Profile::default()"];

/// 起動の入口（CLI / `plan` / 引き継ぎの後任 / `tako solo`）が共有の読み込みを通り、
/// 自前の握りつぶしを持たない（縛る項目の 1. / 6. / 7.）
fn scan_entry(
    file: &'static str,
    src: &str,
    fn_needle: &str,
    loader: &str,
    bads: &[&str],
    out: &mut Vec<Offender>,
) {
    let Some((at, window)) = fn_window(src, fn_needle) else {
        out.push(Offender {
            file,
            line: 0,
            why: format!("`{fn_needle}` が見つからない（走査が空振り）"),
        });
        return;
    };
    if !window_has(&window, loader) {
        out.push(Offender {
            file,
            line: at,
            why: format!(
                "`{loader}` を通っていない = 起動の経路ごとに読み方が食い違う（#2227 / #2233）"
            ),
        });
    }
    for &bad in bads {
        for line in lines_with(at, &window, bad) {
            out.push(Offender {
                file,
                line,
                why: format!(
                    "起動の入口が `{bad}` を直接使っている = 読めない default.yaml を黙って \
                     組み込み既定へ落とす修正前の形（#2227 / #2233）"
                ),
            });
        }
    }
    if !window_has(&window, "with_startup_warning(") {
        out.push(Offender {
            file,
            line: at,
            why: "system prompt に起動時の警告を添えていない = 全画面の TUI に隠れた `警告:` 行・ \
                  前任への応答のほかに、立てた本人へ届く道が無い（#2227 / #2233）"
                .into(),
        });
    }
}

/// 2. / 3. / 5. 共有の読み込み（`master_launch.rs`）
fn scan_launch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    scan_entry(
        LAUNCH,
        src,
        "pub fn plan(",
        "load_profile_in(",
        MASTER_BADS,
        &mut out,
    );
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: LAUNCH,
            line,
            why,
        })
    };

    match fn_window(src, "pub fn load_default_at(") {
        None => push(0, "`load_default_at` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "profile_base_at(") {
                push(
                    at,
                    "default.yaml を継承元と同じ読み方（`profile_base_at` = 退避つき）で \
                     読んでいない = 壊れた中身が保全されない（#916 / #2227）"
                        .into(),
                );
            }
            if !window_has(&window, "warning: Some(") {
                push(
                    at,
                    "読めないときに warning を持たせていない = 黙って組み込み既定で立つ（#2227）"
                        .into(),
                );
            }
            for (i, line) in window.iter().enumerate() {
                if !is_code(line) {
                    continue;
                }
                if let Some(bad) = SWALLOWS.iter().find(|s| line.contains(**s)) {
                    push(
                        at + i,
                        format!(
                            "default.yaml の読み込みで `{bad}` を使っている = 読めない理由を \
                             握りつぶす（#2227）"
                        ),
                    );
                }
            }
        }
    }

    // #2365: 本体は断りを型で返す `load_profile_typed`。表示言語で組む口（CLI・`plan`）はそこへ委ねるだけ
    match fn_window(src, "pub fn load_profile_in(") {
        None => push(0, "`load_profile_in` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "load_profile_typed(") {
                push(
                    at,
                    "`load_profile_in` が本体 `load_profile_typed` へ委ねていない = 起動の経路ごとに \
                     読み方が食い違う（#2227 / #2365）"
                        .into(),
                );
            }
        }
    }
    match fn_window(src, "pub fn load_profile_typed(") {
        None => push(0, "`load_profile_typed` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "load_default_at(") {
                push(
                    at,
                    "default の読み込みが `load_default_at` を通っていない（#2227）".into(),
                );
            }
            if !window_has(&window, "missing_project_profile_reason(") {
                push(
                    at,
                    "`-<key>` の「見つからない」に #1825 の理由と直し方を足していない（#2227）"
                        .into(),
                );
            }
            if lines_with(at, &window, "persist_log(").len() < 2 {
                push(
                    at,
                    "default の警告と `-<key>` の理由の両方を persist.log へ残していない（#2227）"
                        .into(),
                );
            }
        }
    }

    // 6. 引き継ぎの後任は CLI / スマホと同じ `load_profile_typed` を通る（#2233）
    match fn_window(src, "pub fn load_successor_profile(") {
        None => push(
            0,
            "`load_successor_profile` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window_has(&window, "load_profile_typed(name)") {
                push(
                    at,
                    "後任の読み込みが `load_profile_typed` を通っていない = 後任だけ個人ルールの抜けた \
                     master が黙って立つ（#2233）"
                        .into(),
                );
            }
            // 読めなくても引き継ぎは止めないが、既定で立てた理由を warning と persist.log に持つ
            if !window_has(&window, "warning: Some(") || !window_has(&window, "persist_log(") {
                push(
                    at,
                    "読めない後任のプロファイルを既定で立てるのに warning / persist.log を持たない \
                     = 黙って組み込み既定の後任が立つ（#2233）"
                        .into(),
                );
            }
        }
    }

    // 7. `tako solo` の default も同じ形（退避つきの読み方・persist.log）
    match fn_window(src, "pub fn load_solo(") {
        None => push(0, "`load_solo` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "load_solo_in(") {
                push(
                    at,
                    "`load_solo` が本体 `load_solo_in` へ委ねていない（#2233 / #2365）".into(),
                );
            }
        }
    }
    match fn_window(src, "pub fn load_solo_in(") {
        None => push(0, "`load_solo_in` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "load_default_at(") || !window_has(&window, "ProfileKind::Solo")
            {
                push(
                    at,
                    "solo の default が `load_default_at(ProfileKind::Solo, ..)` を通っていない \
                     = 読めない中身が退避されず warning も持たない（#2233）"
                        .into(),
                );
            }
            if !window_has(&window, "persist_log(") {
                push(
                    at,
                    "solo の default の警告を persist.log へ残していない（#2233）".into(),
                );
            }
        }
    }

    // 9. master / solo 本人ではないエージェントを立てる経路の読み込み（#2256）
    let mut rules_out = Vec::new();
    scan_rules(
        LAUNCH,
        src,
        &[Rule {
            fn_needle: "pub fn load_caller_profile(",
            needs: &[
                (
                    "profile_base_at(",
                    "呼び出し元のプロファイルを default の起動と同じ読み方（`profile_base_at` = 退避つき）で \
                     読んでいない = 読めない中身が保全されず理由も無い（#2256）",
                ),
                (
                    "warning: Some(",
                    "読めないプロファイルを既定で立てるのに warning を持たない = worker / 解消エージェントが \
                     黙って組み込み既定で立つ（#2256）",
                ),
            ],
            bans: &[],
        }],
        &mut rules_out,
    );
    for o in rules_out {
        push(o.line, o.why);
    }

    // 5. 応答（スマホの「+ master」）に warnings を載せる
    match fn_window(src, "pub fn to_json(") {
        None => push(0, "`to_json` が見つからない（走査が空振り）".into()),
        Some((at, window)) => {
            if !window_has(&window, "\"warnings\"") {
                push(
                    at,
                    "起動の応答に `warnings` が無い = スマホから立てた master の警告が画面へ出ない \
                     （#2227）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 4. 理由の判定（`orchestrator/mod.rs`）
fn scan_orchestrator(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let mut push = |line: usize, why: String| {
        out.push(Offender {
            file: ORCH,
            line,
            why,
        })
    };
    match fn_window(src, "pub fn missing_project_profile_reason(") {
        None => push(
            0,
            "`missing_project_profile_reason` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window_has(&window, "check_project_profile_base()") {
                push(
                    at,
                    "継承元を `check_project_profile_base` で見ていない = `migrate status` / \
                     `projects add` と判定が食い違う（#1825 / #2227）"
                        .into(),
                );
            }
        }
    }
    // 8. 表示だけの経路の読み込みは読むだけ（退避しない）で、理由を返す
    match fn_window(src, "pub fn load_for_display(") {
        None => push(
            0,
            "`load_for_display` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window_has(&window, "load_for_display_at(") {
                push(
                    at,
                    "`load_for_display` が本体（`load_for_display_at`）を通っていない（#2233）"
                        .into(),
                );
            }
        }
    }
    match fn_window(src, "fn load_for_display_at(") {
        None => push(
            0,
            "`load_for_display_at` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window_has(&window, "read_profile_base_at(") {
                push(
                    at,
                    "表示用の読み込みが継承元と同じ判定（`read_profile_base_at`）を通っていない \
                     = 起動の経路・`migrate status` と「読めない」の基準が食い違う（#2233）"
                        .into(),
                );
            }
            for (i, line) in window.iter().enumerate() {
                if !is_code(line) {
                    continue;
                }
                let quarantines = line.contains("quarantine_unreadable(")
                    || (line.contains("profile_base_at(")
                        && !line.contains("read_profile_base_at("));
                if quarantines {
                    push(
                        at + i,
                        "表示だけの経路が退避つきの読み方を使っている = 表示のたびに書き込む \
                         （退避は起動の経路と `tako migrate` が持つ。#2233）"
                            .into(),
                    );
                }
                if let Some(bad) = SWALLOWS.iter().find(|s| line.contains(**s)) {
                    push(
                        at + i,
                        format!(
                            "表示用の読み込みで `{bad}` を使っている = 読めない理由を握りつぶす（#2233）"
                        ),
                    );
                }
            }
        }
    }
    // 10. 判定・表示の経路の読み込み（#2256）は読むだけで、理由を返す
    match fn_window(src, "pub fn read_for_judgment(") {
        None => push(
            0,
            "`read_for_judgment` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window_has(&window, "read_profile_base_at(") {
                push(
                    at,
                    "判定の経路の読み込みが継承元と同じ判定（`read_profile_base_at`）を通っていない（#2256）"
                        .into(),
                );
            }
            for (i, line) in window.iter().enumerate() {
                if !is_code(line) {
                    continue;
                }
                let quarantines = line.contains("quarantine_unreadable(")
                    || (line.contains("profile_base_at(")
                        && !line.contains("read_profile_base_at("));
                if quarantines {
                    push(
                        at + i,
                        "判定の経路が退避つきの読み方を使っている = 判定のたびに書き込む（#2256）"
                            .into(),
                    );
                }
                if let Some(bad) = SWALLOWS.iter().find(|s| line.contains(**s)) {
                    push(
                        at + i,
                        format!("判定の経路の読み込みで `{bad}` を使っている = 理由を握りつぶす（#2256）"),
                    );
                }
            }
        }
    }
    let mut rules_out = Vec::new();
    scan_rules(
        ORCH,
        src,
        &[Rule {
            fn_needle: "pub fn master_pane_limit_resume_decision(",
            needs: &[(
                "read_for_judgment(",
                "自動復帰の既定が判定の読み込み（`read_for_judgment`）を通っていない = 読めない \
                 プロファイルで全体の既定に落ちたことを黙る（#2256）",
            )],
            bans: &[
                (
                    "Profile::load(",
                    "自動復帰の既定が `Profile::load(..)` を直接使っている = 読めないプロファイルを \
                     黙って既定へ落とす修正前の形（#2256）",
                ),
                (
                    "load_solo_profile(",
                    "自動復帰の既定が `load_solo_profile(..)` を直接使っている = 修正前の形（#2256）",
                ),
            ],
        }],
        &mut rules_out,
    );
    for o in rules_out {
        push(o.line, o.why);
    }
    match fn_window(src, "fn missing_project_profile_reason_from(") {
        None => push(
            0,
            "`missing_project_profile_reason_from` が見つからない（走査が空振り）".into(),
        ),
        Some((at, window)) => {
            if !window_has(&window, "not_inherited_reason(") {
                push(
                    at,
                    "理由文が #1825 の `not_inherited_reason` を通っていない = `projects add` と \
                     直し方の文が食い違う（#2227）"
                        .into(),
                );
            }
        }
    }
    out
}

/// 1. / 5. CLI（`tako master`）
fn scan_cli(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    scan_entry(
        CLI,
        src,
        "fn orchestrator_master(",
        "master_launch::load_profile_in(",
        MASTER_BADS,
        &mut out,
    );
    // 7. `tako solo`（#2233）。修正前は solo の読み込みを自前で握りつぶしていた
    scan_entry(
        CLI,
        src,
        "fn orchestrator_solo(",
        "master_launch::load_solo_in(",
        &["load_solo_profile(", "solo_default_profile()"],
        &mut out,
    );
    // `警告:` の見出しは #2365 から表示言語の `ReplyNote::WarningLabel`
    if let Some((at, window)) = fn_window(src, "fn orchestrator_solo(") {
        if !window_has(&window, "eprintln!(\"{}: {w}\", ReplyNote::WarningLabel") {
            out.push(Offender {
                file: CLI,
                line: at,
                why: "solo の読み込みの warning を `警告:` 行で画面へ出していない（#2233）".into(),
            });
        }
    }
    // 10. `tako orchestrator guide`（#2256）は読めない理由を `警告:` 行で画面へ
    scan_rules(
        CLI,
        src,
        &[Rule {
            fn_needle: "fn orchestrator_guide_local(",
            needs: &[(
                "eprintln!(\"{}: {w}\", ReplyNote::WarningLabel",
                "guide が読めないプロファイルの理由を `警告:` 行で画面へ出していない（#2256）",
            )],
            bans: &[],
        }],
        &mut out,
    );
    if let Some((at, window)) = fn_window(src, "fn orchestrator_master(") {
        if !window_has(&window, "eprintln!(\"{}: {w}\", ReplyNote::WarningLabel") {
            out.push(Offender {
                file: CLI,
                line: at,
                why: "読み込みの warning を `警告:` 行で画面へ出していない（#2227）".into(),
            });
        }
    }
    out
}

/// 6. / 8. dispatch（引き継ぎの後任・self・supervisor status）
fn scan_dispatch(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    scan_entry(
        DISPATCH,
        src,
        "fn dispatch_orchestrator_handoff(",
        "master_launch::load_successor_profile(",
        MASTER_BADS,
        &mut out,
    );
    // (関数の目印, 窓に要るもの, 欠けたときの理由)
    let needs: &[(&str, &str, &str)] = &[
        (
            "fn dispatch_orchestrator_handoff(",
            "warnings.extend(load_warning)",
            "後任を既定で立てた理由を応答の `warnings` へ載せていない = 引き継ぎを呼んだ前任に \
             見えない（#2233）",
        ),
        (
            "fn dispatch_orchestrator_self(",
            "load_for_display(",
            "self が表示用の読み込み（`load_for_display`）を通っていない = 既定値で答えたことを \
             黙る（#2233）",
        ),
        (
            "fn dispatch_orchestrator_self(",
            "warnings.extend(profile_note",
            "self が読めない理由を `warnings` へ出していない（#2233）",
        ),
        (
            "fn dispatch_orchestrator_supervisor(",
            "load_for_display(",
            "supervisor status が表示用の読み込み（`load_for_display`）を通っていない（#2233）",
        ),
        (
            "fn dispatch_orchestrator_supervisor(",
            "v[\"warnings\"]",
            "supervisor status が読めない理由を `warnings` へ出していない（#2233）",
        ),
    ];
    for (fn_needle, need, why) in needs {
        match fn_window(src, fn_needle) {
            None => out.push(Offender {
                file: DISPATCH,
                line: 0,
                why: format!("`{fn_needle}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                if !window_has(&window, need) {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: (*why).to_string(),
                    });
                }
            }
        }
    }
    // 9. / 10. #2256 の経路
    scan_rules(
        DISPATCH,
        src,
        &[
            Rule {
                fn_needle: "fn resolve_caller_profile_with_role(",
                needs: &[(
                    "master_launch::load_caller_profile(",
                    "spawn / 解消エージェントの読み込みが `load_caller_profile` を通っていない（#2256）",
                )],
                bans: &[(
                    "Profile::load(",
                    "呼び出し元のプロファイルを `Profile::load(..)` で直接読んでいる = 読めない default を \
                     黙って組み込み既定へ落とす修正前の形（#2256）",
                )],
            },
            Rule {
                fn_needle: "fn dispatch_orchestrator_spawn(",
                needs: &[
                    (
                        "warning: profile_warning",
                        "spawn が読み込みの warning を受け取っていない（#2256）",
                    ),
                    (
                        "if let Some(w) = profile_warning {",
                        "spawn が読み込みの warning を出していない（#2256）",
                    ),
                    (
                        "\"profile_unreadable\"",
                        "spawn が既定で立てた理由を応答の `warnings` へ載せていない = 呼んだ master に \
                         見えない（#2256）",
                    ),
                    (
                        "persist_log(&format!(\"[spawn]",
                        "spawn が既定で立てた理由を persist.log へ残していない（#2256）",
                    ),
                ],
                bans: &[],
            },
            Rule {
                fn_needle: "fn dispatch_git_resolve_agent(",
                needs: &[
                    (
                        "warning: profile_warning",
                        "解消エージェントが読み込みの warning を受け取っていない（#2256）",
                    ),
                    (
                        "v[\"warnings\"] = json!([w.to_string()]);",
                        "解消エージェントが既定で立てた理由を応答の `warnings` へ載せていない（#2256）",
                    ),
                    (
                        "persist_log(&format!(\"[git-resolve]",
                        "解消エージェントが既定で立てた理由を persist.log へ残していない（#2256）",
                    ),
                ],
                bans: &[],
            },
            Rule {
                fn_needle: "fn dispatch_orchestrator_adopt(",
                needs: &[
                    (
                        "read_for_judgment(",
                        "adopt の起動時プロファイルが判定の読み込み（`read_for_judgment`）を通っていない（#2256）",
                    ),
                    (
                        "warnings.extend(launched_note)",
                        "adopt が組み込み既定と比べて判断したことを `warnings` へ出していない（#2256）",
                    ),
                ],
                bans: &[(
                    "Profile::load(&launched_as",
                    "adopt の起動時プロファイルを `Profile::load(..)` で直接読んでいる = 修正前の形（#2256）",
                )],
            },
            Rule {
                fn_needle: "fn apply_master_pane_profile_defaults(",
                needs: &[
                    (
                        "master_pane_limit_resume_decision(",
                        "自動復帰の既定を理由つきの判定（`master_pane_limit_resume_decision`）で \
                         引いていない（#2256）",
                    ),
                    (
                        "if let Some(note) = unreadable {",
                        "自動復帰の既定を読めないプロファイルの代わりに既定で決めた理由を persist.log へ \
                         出していない（#2256）",
                    ),
                ],
                bans: &[(
                    "master_pane_limit_resume_setting(",
                    "理由を落とす `master_pane_limit_resume_setting` を使っている = 修正前の形（#2256）",
                )],
            },
        ],
        &mut out,
    );
    for fn_needle in [
        "fn dispatch_orchestrator_self(",
        "fn dispatch_orchestrator_supervisor(",
    ] {
        if let Some((at, window)) = fn_window(src, fn_needle) {
            for line in lines_with(at, &window, "Profile::load(") {
                out.push(Offender {
                    file: DISPATCH,
                    line,
                    why: "表示の経路が `Profile::load(..)` を直接使っている = 読めないプロファイルを \
                          黙って既定値で答える修正前の形（#2233）"
                        .into(),
                });
            }
        }
    }
    out
}

/// 8. `tako setup` の表示
fn scan_setup(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn print_next_steps(") {
        None => out.push(Offender {
            file: SETUP,
            line: 0,
            why: "`print_next_steps` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !window_has(&window, "load_for_display(") {
                out.push(Offender {
                    file: SETUP,
                    line: at,
                    why: "setup の表示が `load_for_display` を通っていない（#2233）".into(),
                });
            }
            for line in lines_with(at, &window, "Profile::load(") {
                out.push(Offender {
                    file: SETUP,
                    line,
                    why: "setup の表示が `Profile::load(..)` を直接使っている = 組み込み既定を \
                          「現在の default」として見せる修正前の形（#2233）"
                        .into(),
                });
            }
        }
    }
    match fn_window(src, "fn profile_summary_lines(") {
        None => out.push(Offender {
            file: SETUP,
            line: 0,
            why: "`profile_summary_lines` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !window_has(&window, "警告: {note}") {
                out.push(Offender {
                    file: SETUP,
                    line: at,
                    why: "setup の表示が読めない理由を出していない（#2233）".into(),
                });
            }
        }
    }
    out
}

/// 10. `orchestrator guide`（#2256）
fn scan_guide(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    scan_rules(
        GUIDE,
        src,
        &[Rule {
            fn_needle: "pub fn json(",
            needs: &[
                (
                    "read_for_judgment(",
                    "guide が判定の読み込み（`read_for_judgment`）を通っていない（#2256）",
                ),
                (
                    "local_rules_unreadable(",
                    "読めないプロファイルの `local-rules` が「無い」を返す = master が個人ルールを \
                     存在しないと受け取って動く（#2256）",
                ),
                (
                    "out[\"warnings\"]",
                    "guide が既定で組み立てたことを `warnings` へ出していない（#2256）",
                ),
            ],
            bans: &[
                (
                    "load_profile_of(",
                    "guide がプロファイルを `load_profile_of(..)` で直接読んでいる = 読めないプロファイルを \
                     黙って既定へ落とす修正前の形（#2256）",
                ),
                (
                    "Profile::default()",
                    "guide が読めないプロファイルを `Profile::default()` へ黙って落としている（#2256）",
                ),
            ],
        }],
        &mut out,
    );
    out
}

fn all(s: &Sources) -> Vec<String> {
    scan_launch(&s.launch)
        .iter()
        .chain(scan_orchestrator(&s.orch).iter())
        .chain(scan_cli(&s.cli).iter())
        .chain(scan_dispatch(&s.dispatch).iter())
        .chain(scan_setup(&s.setup).iter())
        .chain(scan_guide(&s.guide).iter())
        .map(Offender::report)
        .collect()
}

#[test]
fn 走査が空振りしていない() {
    let s = sources(&workspace_root());
    for (rel, src, needle) in [
        (LAUNCH, &s.launch, "pub fn plan("),
        (LAUNCH, &s.launch, "pub fn load_profile_in("),
        (LAUNCH, &s.launch, "pub fn load_profile_typed("),
        (LAUNCH, &s.launch, "pub fn load_default_at("),
        (LAUNCH, &s.launch, "pub fn to_json("),
        (ORCH, &s.orch, "pub fn missing_project_profile_reason("),
        (ORCH, &s.orch, "fn missing_project_profile_reason_from("),
        (CLI, &s.cli, "fn orchestrator_master("),
        (LAUNCH, &s.launch, "pub fn load_successor_profile("),
        (LAUNCH, &s.launch, "pub fn load_solo("),
        (LAUNCH, &s.launch, "pub fn load_solo_in("),
        (ORCH, &s.orch, "pub fn load_for_display("),
        (ORCH, &s.orch, "fn load_for_display_at("),
        (CLI, &s.cli, "fn orchestrator_solo("),
        (DISPATCH, &s.dispatch, "fn dispatch_orchestrator_handoff("),
        (DISPATCH, &s.dispatch, "fn dispatch_orchestrator_self("),
        (
            DISPATCH,
            &s.dispatch,
            "fn dispatch_orchestrator_supervisor(",
        ),
        (SETUP, &s.setup, "fn print_next_steps("),
        (SETUP, &s.setup, "fn profile_summary_lines("),
        (LAUNCH, &s.launch, "pub fn load_caller_profile("),
        (ORCH, &s.orch, "pub fn read_for_judgment("),
        (ORCH, &s.orch, "pub fn master_pane_limit_resume_decision("),
        (CLI, &s.cli, "fn orchestrator_guide_local("),
        (
            DISPATCH,
            &s.dispatch,
            "fn resolve_caller_profile_with_role(",
        ),
        (DISPATCH, &s.dispatch, "fn dispatch_orchestrator_spawn("),
        (DISPATCH, &s.dispatch, "fn dispatch_git_resolve_agent("),
        (DISPATCH, &s.dispatch, "fn dispatch_orchestrator_adopt("),
        (
            DISPATCH,
            &s.dispatch,
            "fn apply_master_pane_profile_defaults(",
        ),
        (GUIDE, &s.guide, "pub fn json("),
    ] {
        assert!(
            fn_window(src, needle).is_some(),
            "{rel}: `{needle}` の窓が採れない（本番コードの範囲取りが壊れている）"
        );
    }
}

#[test]
fn 現行実装は違反ゼロ() {
    let found = all(&sources(&workspace_root()));
    assert!(
        found.is_empty(),
        "読めない default.yaml で黙って組み込み既定へ落ちない（#2227 / #2233 / #2256）の不変条件に違反: \
         {found:#?}"
    );
}

/// 関数の窓の中でだけ `from` を `to` へ置き換える（ファイル全体を置換すると
/// 同じ綴りの別の関数まで壊れて「何を測ったか」が濁る）
fn replace_in_fn(src: &str, fn_needle: &str, from: &str, to: &str) -> String {
    let Some((at, window)) = fn_window(src, fn_needle) else {
        panic!("`{fn_needle}` の窓が採れない");
    };
    let lines: Vec<&str> = src.lines().collect();
    let start = at - 1;
    let end = start + window.len();
    let body = window.join("\n");
    let replaced = body.replacen(from, to, 1);
    assert_ne!(
        replaced, body,
        "`{fn_needle}` の中に注入の対象が無い: {from}"
    );
    let mut out: Vec<String> = lines[..start].iter().map(|s| s.to_string()).collect();
    out.push(replaced);
    out.extend(lines[end..].iter().map(|s| s.to_string()));
    out.join("\n")
}

/// 注入の 1 件（どのファイルの・どの関数の中で・何を何へ・名指されるべき行の目印）
struct Injection {
    rel: &'static str,
    fn_needle: &'static str,
    from: &'static str,
    to: &'static str,
    marker: &'static str,
}

/// 注入して、注入した関数の中の `marker` の行が file:line で名指しされることを確かめる
fn assert_named(base: &Sources, inj: &Injection) {
    let mut injected = base.clone();
    let target = match inj.rel {
        LAUNCH => &mut injected.launch,
        ORCH => &mut injected.orch,
        CLI => &mut injected.cli,
        DISPATCH => &mut injected.dispatch,
        SETUP => &mut injected.setup,
        GUIDE => &mut injected.guide,
        other => panic!("未知のファイル: {other}"),
    };
    *target = replace_in_fn(target, inj.fn_needle, inj.from, inj.to);
    let (at, window) = fn_window(target, inj.fn_needle).expect("注入後も窓が採れる");
    let line = window
        .iter()
        .position(|l| l.contains(inj.marker))
        .map(|i| at + i)
        .unwrap_or_else(|| panic!("`{}` が注入後の窓に無い", inj.marker));
    let expected = format!("{}:{line} ", inj.rel);
    let found = all(&injected);
    assert!(
        found.iter().any(|o| o.starts_with(&expected)),
        "注入（{} → {}）を {}:{line} で名指しできていない: {found:#?}",
        inj.from,
        inj.to,
        inj.rel
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let s = sources(&workspace_root());
    for inj in [
        // 注入 1: plan が修正前そのもの（自前で読んで default を組み込み既定へ握りつぶす）
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn plan(",
            from: "let LaunchProfile { profile, warning } = load_profile_in(profile_name, lang)?;",
            to: "let (profile, warning) = (Profile::load(profile_name).unwrap_or_else(|_| Profile::default()), None::<ProfileFallback>);",
            marker: "Profile::load(profile_name)",
        },
        // 注入 2: CLI が修正前そのもの
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_master(",
            from: "} = orchestrator::master_launch::load_profile_in(profile_name, lang)?;",
            to: "} = orchestrator::master_launch::LaunchProfile { profile: orchestrator::Profile::load(profile_name).unwrap_or_else(|_| orchestrator::Profile::default()), warning: None };",
            marker: "orchestrator::Profile::load(profile_name)",
        },
        // 注入 3: CLI が警告を画面へ出さない
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_master(",
            from: "eprintln!(\"{}: {w}\", ReplyNote::WarningLabel.text_in(lang));",
            to: "let _ = w;",
            marker: "fn orchestrator_master(",
        },
        // 注入 4: CLI が master 本人の prompt へ警告を添えない
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_master(",
            from: "orchestrator::master_launch::with_startup_warning(",
            to: "std::convert::identity((",
            marker: "fn orchestrator_master(",
        },
        // 注入 5: default の読み込みが理由を握りつぶす
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_default_at(",
            from: "match orchestrator::profile_base_at(kind, path) {",
            to: "match Ok::<Profile, String>(orchestrator::profile_base_at(kind, path).unwrap_or_default()) {",
            marker: "profile_base_at(kind, path).unwrap_or_default()",
        },
        // 注入 6: `-<key>` の「見つからない」に理由を足さない
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_profile_typed(",
            from: "orchestrator::missing_project_profile_reason(name)",
            to: "None::<String>",
            marker: "pub fn load_profile_typed(",
        },
        // 注入 7: スマホの応答に warnings を載せない
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn to_json(",
            from: "\"warnings\": self.warning.iter().collect::<Vec<_>>(),",
            to: "",
            marker: "pub fn to_json(",
        },
        // 注入 8: 理由の判定が継承元を見ない（`migrate status` / `projects add` と食い違う）
        Injection {
            rel: ORCH,
            fn_needle: "pub fn missing_project_profile_reason(",
            from: "check_project_profile_base(),",
            to: "Ok::<(), String>(()),",
            marker: "pub fn missing_project_profile_reason(",
        },
        // 注入 9: 引き継ぎの後任が修正前そのもの（自前で読んで黙って組み込み既定）
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_handoff(",
            from: "} = orchestrator::master_launch::load_successor_profile(profile_name);",
            to: "} = orchestrator::master_launch::LaunchProfile { profile: orchestrator::Profile::load(profile_name).unwrap_or_default(), warning: None };",
            marker: "orchestrator::Profile::load(profile_name)",
        },
        // 注入 10: 後任の system prompt へ警告を添えない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_handoff(",
            from: "orchestrator::master_launch::with_startup_warning(",
            to: "std::convert::identity((",
            marker: "fn dispatch_orchestrator_handoff(",
        },
        // 注入 11: 前任への応答に警告を載せない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_handoff(",
            from: "warnings.extend(load_warning);",
            to: "let _ = load_warning;",
            marker: "fn dispatch_orchestrator_handoff(",
        },
        // 注入 12: self が修正前そのもの
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_self(",
            from: "orchestrator::load_for_display(orchestrator::ProfileKind::Master, profile_name);",
            to: "(orchestrator::Profile::load(profile_name).unwrap_or_default(), None::<String>);",
            marker: "orchestrator::Profile::load(profile_name)",
        },
        // 注入 13: self が理由を warnings へ出さない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_self(",
            from: "warnings.extend(profile_note.map(|note| note.to_string()));",
            to: "let _ = profile_note;",
            marker: "fn dispatch_orchestrator_self(",
        },
        // 注入 14: supervisor status が修正前そのもの
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_supervisor(",
            from: "let (profile, profile_note) = crate::orchestrator::load_for_display(",
            to: "let (profile, profile_note) = (crate::orchestrator::Profile::load(\"default\").unwrap_or_default(), None::<String>); let _ = (",
            marker: "crate::orchestrator::Profile::load(\"default\")",
        },
        // 注入 15: supervisor status が理由を warnings へ出さない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_supervisor(",
            from: "v[\"warnings\"] = json!([note.to_string()]);",
            to: "let _ = note;",
            marker: "fn dispatch_orchestrator_supervisor(",
        },
        // 注入 16: `tako solo` が修正前そのもの
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_solo(",
            from: "} = orchestrator::master_launch::load_solo_in(profile_name, lang)?;",
            to: "} = orchestrator::master_launch::LaunchProfile { profile: orchestrator::load_solo_profile(profile_name).unwrap_or_else(|_| orchestrator::solo_default_profile()), warning: None };",
            marker: "orchestrator::load_solo_profile(profile_name)",
        },
        // 注入 17: `tako solo` が警告を画面へ出さない
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_solo(",
            from: "eprintln!(\"{}: {w}\", ReplyNote::WarningLabel.text_in(lang));",
            to: "let _ = w;",
            marker: "fn orchestrator_solo(",
        },
        // 注入 18: `tako solo` が solo 本人の prompt へ警告を添えない
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_solo(",
            from: "orchestrator::master_launch::with_startup_warning(",
            to: "std::convert::identity((",
            marker: "fn orchestrator_solo(",
        },
        // 注入 19: solo の default の警告を persist.log へ残さない
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_solo_in(",
            from: "crate::diag::persist_log(&format!(\"[solo] {name}: {}\", w.text_in(Lang::Ja)));",
            to: "let _ = w;",
            marker: "pub fn load_solo_in(",
        },
        // 注入 20: 後任の読み込みが `load_profile_typed` を通らない
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_successor_profile(",
            from: "match load_profile_typed(name) {",
            to: "match Ok::<LaunchProfile, ConfigError>(LaunchProfile { profile: Profile::load(name).unwrap_or_default(), warning: None }) {",
            marker: "pub fn load_successor_profile(",
        },
        // 注入 23: 読めない後任のプロファイルを既定で立てるのに warning を持たない
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_successor_profile(",
            from: "warning: Some(w),",
            to: "warning: None,",
            marker: "pub fn load_successor_profile(",
        },
        // 注入 21: 表示だけの経路が退避つきの読み方を使う（表示のたびに書き込む）
        Injection {
            rel: ORCH,
            fn_needle: "fn load_for_display_at(",
            from: "match read_profile_base_at(kind, path) {",
            to: "match profile_base_at(kind, path) {",
            marker: "match profile_base_at(kind, path) {",
        },
        // 注入 22: setup の表示が修正前そのもの
        Injection {
            rel: SETUP,
            fn_needle: "fn print_next_steps(",
            from: "let (profile, note) = tako_control::orchestrator::load_for_display(",
            to: "let (profile, note) = (Profile::load(\"default\").unwrap_or_default(), None::<String>); let _ = (",
            marker: "Profile::load(\"default\")",
        },
        // ── #2256 ──
        // 注入 24: spawn / 解消エージェントの読み込みが退避つきの読み方を通らない（修正前の黙った既定）
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_caller_profile(",
            from: "match orchestrator::profile_base_at(kind, &path) {",
            to: "match Ok::<Profile, String>(Profile::load(name).unwrap_or_default()) {",
            marker: "pub fn load_caller_profile(",
        },
        // 注入 25: 読めないのに warning を持たない
        Injection {
            rel: LAUNCH,
            fn_needle: "pub fn load_caller_profile(",
            from: "warning: Some(ProfileFallback::CallerLaunch {",
            to: "warning: None, _w: (ProfileFallback::CallerLaunch {",
            marker: "pub fn load_caller_profile(",
        },
        // 注入 26: spawn の呼び出し元の解決が修正前そのもの
        Injection {
            rel: DISPATCH,
            fn_needle: "fn resolve_caller_profile_with_role(",
            from: "crate::orchestrator::master_launch::load_caller_profile(name, launching)",
            to: "crate::orchestrator::master_launch::LaunchProfile { profile: crate::orchestrator::Profile::load(name).unwrap_or_default(), warning: None }",
            marker: "Profile::load(name)",
        },
        // 注入 27: spawn が理由を応答へ載せない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_spawn(",
            from: "\"kind\": \"profile_unreadable\",",
            to: "\"kind\": \"other\",",
            marker: "fn dispatch_orchestrator_spawn(",
        },
        // 注入 28: spawn が理由を persist.log へ残さない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_spawn(",
            from: "crate::diag::persist_log(&format!(\"[spawn] pane={}: {ja}\", new_id.as_u64()));",
            to: "let _ = &w;",
            marker: "fn dispatch_orchestrator_spawn(",
        },
        // 注入 29: 解消エージェントが理由を応答へ載せない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_git_resolve_agent(",
            from: "v[\"warnings\"] = json!([w.to_string()]);",
            to: "let _ = w;",
            marker: "fn dispatch_git_resolve_agent(",
        },
        // 注入 30: 解消エージェントが理由を persist.log へ残さない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_git_resolve_agent(",
            from: "crate::diag::persist_log(&format!(\"[git-resolve] pane={}: {ja}\", new_id.as_u64()));",
            to: "let _ = w;",
            marker: "fn dispatch_git_resolve_agent(",
        },
        // 注入 31: adopt の起動時プロファイルが修正前そのもの
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_adopt(",
            from: "orchestrator::read_for_judgment(orchestrator::ProfileKind::Master, &launched_as);",
            to: "(orchestrator::Profile::load(&launched_as).unwrap_or_default(), None::<String>);",
            marker: "Profile::load(&launched_as)",
        },
        // 注入 32: adopt が既定と比べたことを warnings へ出さない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn dispatch_orchestrator_adopt(",
            from: "warnings.extend(launched_note);",
            to: "let _ = launched_note;",
            marker: "fn dispatch_orchestrator_adopt(",
        },
        // 注入 33: 自動復帰の既定が修正前そのもの
        Injection {
            rel: ORCH,
            fn_needle: "pub fn master_pane_limit_resume_decision(",
            from: "let (profile, why) = read_for_judgment(kind, name);",
            to: "let (profile, why) = (Profile::load(name).unwrap_or_default(), None::<String>);",
            marker: "Profile::load(name)",
        },
        // 注入 34: 自動復帰の既定を理由を落とす旧 API で引く
        Injection {
            rel: DISPATCH,
            fn_needle: "fn apply_master_pane_profile_defaults(",
            from: "crate::orchestrator::master_pane_limit_resume_decision(role);",
            to: "(crate::orchestrator::master_pane_limit_resume_setting(role), None::<String>);",
            marker: "master_pane_limit_resume_setting(role)",
        },
        // 注入 35: 自動復帰の既定の理由を persist.log へ出さない
        Injection {
            rel: DISPATCH,
            fn_needle: "fn apply_master_pane_profile_defaults(",
            from: "if let Some(note) = unreadable {",
            to: "if let Some(note) = None::<String> {",
            marker: "fn apply_master_pane_profile_defaults(",
        },
        // 注入 36: guide が修正前そのもの
        Injection {
            rel: GUIDE,
            fn_needle: "pub fn json(",
            from: "super::read_for_judgment(super::ProfileKind::Master, profile_name);",
            to: "(super::load_profile_of(super::ProfileKind::Master, profile_name).unwrap_or_else(|_| super::Profile::default()), None::<String>);",
            marker: "load_profile_of(",
        },
        // 注入 37: 読めないときの local-rules が「無い」を返す
        Injection {
            rel: GUIDE,
            fn_needle: "pub fn json(",
            from: "Some(w) if g.topic == LOCAL_RULES => local_rules_unreadable(w),",
            to: "Some(_) if false => String::new(),",
            marker: "pub fn json(",
        },
        // 注入 38: guide が warnings を出さない
        Injection {
            rel: GUIDE,
            fn_needle: "pub fn json(",
            from: "out[\"warnings\"] = json!([w]);",
            to: "let _ = w;",
            marker: "pub fn json(",
        },
        // 注入 39: CLI の guide が `警告:` 行を出さない
        Injection {
            rel: CLI,
            fn_needle: "fn orchestrator_guide_local(",
            from: "eprintln!(\"{}: {w}\", ReplyNote::WarningLabel.text_in(cli_lang()));",
            to: "let _ = w;",
            marker: "fn orchestrator_guide_local(",
        },
        // 注入 40: 判定の経路が退避つきの読み方を使う（判定のたびに書き込む）
        Injection {
            rel: ORCH,
            fn_needle: "pub fn read_for_judgment(",
            from: "match read_profile_base_at(kind, &path) {",
            to: "match profile_base_at(kind, &path) {",
            marker: "match profile_base_at(kind, &path) {",
        },
    ] {
        assert_named(&s, &inj);
    }
}
