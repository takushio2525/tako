//! 右パネル tasks ビュー（#1450 の分割 B2）の構造を縛る番犬
//!
//! # なぜ要るか
//!
//! B1 で「人がやること」の正本と操作は 1 実装（`Request::UserTask`）に揃った。
//! B2 はそこへ **4 つ目の入口（PC の画面）**を足すので、放っておくと B1 で潰した
//! 分岐がそのまま画面側に生える:
//!
//! - 画面が YAML / `TaskStore` を直読みする → 起票の通知も返答の配送も画面経路だけ抜ける
//! - ビュー切替が GUI のタブ専用実装になる → `tako panel --view tasks` で開けない
//!   （設計原則 5 が画面の追加のたびに 1 つずつ崩れる）
//! - ポーリングに「止める処理」が要る形になる → 閉じても撃ち続ける
//! - 返答コメント欄が**手書きのカーソル操作を 3 本目**として増える（#1459）
//!
//! いずれも**動いては見える**（画面は出るし、押せば反応する）ので、目視では気づけない。
//!
//! # 何を縛るか
//!
//! 1. **画面は正本を直読みしない** — `tasks_panel.rs` が `user_tasks::` /
//!    `TaskStore` / `user-tasks.yaml` に触らず、操作は `Request::UserTask` を組む
//! 2. **絵文字を使わない** — 画面に出る文字列（`tasks_panel.rs` / `text_field.rs` /
//!    `ui_text/panel.rs`）に絵文字のコードポイントが 1 つも無い（#217 の全面禁止）
//! 3. **ビュー切替が `tako panel` と同じ語彙を通る** — `PanelViewWire` に `tasks` が
//!    居て `parse` が受け、GUI のタブは同じ `PanelView::Tasks` を代入する
//! 4. **ポーリングが止まる** — 撃つ判断は `tick_user_tasks` の 1 か所にあり、
//!    `panel_visible` と A/B を見る。2 秒ループはそれを呼ぶだけ
//! 5. **手書きのテキスト入力を増やさない** — `tasks_panel.rs` と `right_panel.rs` は
//!    カーソル操作を自前で書かず `TextField` を通す。#1459 で既存 2 本
//!    （git のコミット / ブランチ名）も移し終えたので**猶予は無い**（走査は全面適用）。
//!    丸めの `floor_char_boundary` は `text_field.rs` の非公開関数なので、
//!    他ファイルが同じことをするには自前で書き直すしかない = このマークに掛かる
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜5 はすべて無意味に緑になるので、[`走査が空振りしていない`]
//! で窓が採れていることを固定し、[`逆戻りを名指しできる`] で**注入 10 通り**が
//! `file:line` で名指しされることを確かめる。範囲取りは #1420 の 1 実装
//! （`production_range`）を通す（`main.rs` は本番コードと隔離セルフテストが
//! 交互に並ぶので、雑に切ると走査範囲が黙って縮む）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const TASKS_PANEL: &str = "crates/tako-app/src/tasks_panel.rs";
const TEXT_FIELD: &str = "crates/tako-app/src/text_field.rs";
const UI_TEXT_PANEL: &str = "crates/tako-app/src/ui_text/panel.rs";
const RIGHT_PANEL: &str = "crates/tako-app/src/right_panel.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const PROTOCOL: &str = "crates/tako-control/src/protocol.rs";

/// 画面が正本へ手を伸ばしたしるし（B1 の永続・store の名前）
const DIRECT_STORE_MARKS: &[&str] = &["user_tasks::", "TaskStore", "user-tasks.yaml", "serde_yaml"];

/// 手書きのカーソル操作のしるし。`TextField` を通していれば 1 つも要らない
const HANDROLLED_INPUT_MARKS: &[&str] = &["floor_char_boundary", ".drain(", "char_indices()"];

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

/// 本番コードだけの眺め（#1420 の 1 実装。**切らずにテスト領域だけを潰す**）
fn prod(root: &Path, rel: &'static str) -> String {
    production_range::production(&read(root, rel), rel)
}

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

/// 関数の窓（1-based の宣言行と本文）。終わりは**宣言行と同じ字下げの `}`**
fn fn_window(src: &str, needle: &str) -> Option<(usize, String)> {
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
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 注釈（`//` `//!` `///`）を落とした眺め。検査は**コードだけ**を見る
fn code_only(window: &str) -> String {
    window
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("//") {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// コードの行だけを走査して `needle` を含む最初の行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

/// 絵文字の範囲（#217 の全面禁止。記号・装飾・旗・補助記号）
fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F300..=0x1FAFF   // 記号・絵・補助記号
        | 0x1F000..=0x1F0FF // 麻雀牌・トランプ
        | 0x2600..=0x27BF   // その他の記号・装飾
        | 0xFE0F            // 異体字セレクタ 16（絵文字表示指定）
        | 0x1F1E6..=0x1F1FF // 地域指示子（旗）
    )
}

// ---------------------------------------------------------------------------
// 検査本体（注入テストから同じ関数を呼べるよう、材料は引数で受ける）
// ---------------------------------------------------------------------------

struct Sources {
    tasks_panel: String,
    text_field: String,
    ui_text_panel: String,
    right_panel: String,
    main: String,
    protocol: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            tasks_panel: prod(root, TASKS_PANEL),
            text_field: prod(root, TEXT_FIELD),
            ui_text_panel: prod(root, UI_TEXT_PANEL),
            right_panel: prod(root, RIGHT_PANEL),
            main: prod(root, MAIN),
            protocol: prod(root, PROTOCOL),
        }
    }
}

/// 1: 画面は正本を直読みせず、操作は `Request::UserTask` を組む
fn scan_no_direct_store(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let code = code_only(src);
    for mark in DIRECT_STORE_MARKS {
        if let Some(i) = code_line_with(&code, mark) {
            out.push(Offender {
                file: TASKS_PANEL,
                line: i + 1,
                why: format!(
                    "画面が `{mark}` に触っている（正本を直読みすると、起票の通知も\
                     返答の配送も画面経路だけ抜ける。#1450 B2）"
                ),
            });
        }
    }
    // 操作の 3 本（一覧 / 状態遷移 / 返答）はすべて dispatch を組む
    for needle in [
        "fn refresh_user_tasks(",
        "fn user_task_set_status(",
        "fn user_task_respond(",
    ] {
        match fn_window(src, needle) {
            None => out.push(Offender {
                file: TASKS_PANEL,
                line: 0,
                why: format!("`{needle}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                let body = code_only(&window);
                if !body.contains("Request::UserTask") {
                    out.push(Offender {
                        file: TASKS_PANEL,
                        line: at,
                        why: format!(
                            "`{needle}` が `Request::UserTask` を組んでいない\
                             （dispatch を通らない別経路になっている。#1450 B2）"
                        ),
                    });
                }
                if !body.contains("tako_control::dispatch") {
                    out.push(Offender {
                        file: TASKS_PANEL,
                        line: at,
                        why: format!("`{needle}` が dispatch を呼んでいない（#1450 B2）"),
                    });
                }
            }
        }
    }
    out
}

/// 2: 絵文字を 1 つも置かない（#217 の全面禁止）
fn scan_no_emoji(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src) in [
        (TASKS_PANEL, &sources.tasks_panel),
        (TEXT_FIELD, &sources.text_field),
        (UI_TEXT_PANEL, &sources.ui_text_panel),
    ] {
        for (i, line) in src.lines().enumerate() {
            if let Some(c) = line.chars().find(|c| is_emoji(*c)) {
                out.push(Offender {
                    file,
                    line: i + 1,
                    why: format!(
                        "絵文字 U+{:04X} が居る（UI の絵文字は全面禁止。アイコンは svg の\
                         パスか図形で描く。#217 / #1450 B2）",
                        c as u32
                    ),
                });
            }
        }
    }
    out
}

/// 3: ビュー切替が `tako panel` と同じ語彙・同じ enum を通る
fn scan_view_vocabulary(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    let protocol = code_only(&sources.protocol);
    // CLI / MCP が案内する正式値に載っていないと `--view tasks` が弾かれる。
    // **`VALUES` の行そのもの**を見る（ファイル全体から `"tasks"` を探すと、
    // `parse` の腕が残っているだけで緑になり、注入を見逃す）
    let values_at = code_line_with(&protocol, "pub const VALUES");
    let values_line = values_at.and_then(|i| protocol.lines().nth(i));
    if !values_line.is_some_and(|l| l.contains("\"tasks\"")) {
        out.push(Offender {
            file: PROTOCOL,
            line: values_at.map_or(0, |i| i + 1),
            why: "`PanelViewWire::VALUES` に `tasks` が無い（GUI にタブが在るのに\
                  `tako panel --view tasks` で開けない = 設計原則 5 が崩れる）"
                .into(),
        });
    }
    match fn_window(&sources.protocol, "pub fn parse(s: &str)") {
        None => out.push(Offender {
            file: PROTOCOL,
            line: 0,
            why: "`PanelViewWire::parse` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            if !code_only(&window).contains("PanelViewWire::Tasks") {
                out.push(Offender {
                    file: PROTOCOL,
                    line: at,
                    why: "`PanelViewWire::parse` が `tasks` を受けない（#1450 B2）".into(),
                });
            }
        }
    }
    // GUI のタブは同じ enum を代入する（タブ専用の別状態を作らない）
    let right = code_only(&sources.right_panel);
    if !right.contains("this.panel_view = PanelView::Tasks") {
        out.push(Offender {
            file: RIGHT_PANEL,
            // 行番号の足がかりは**必ず在るもの**にする（タブのラベルは
            // #1479 で `PANEL_TAB_LABELS` の添字引きへ移ったので、綴りの
            // リテラルを足がかりにすると名指しが 0 行へ落ちる）
            line: code_line_with(&right, "PanelView::Tasks")
                .or_else(|| code_line_with(&right, "fn render_panel("))
                .map_or(0, |i| i + 1),
            why: "tasks タブが `PanelView::Tasks` を代入していない（GUI と CLI で\
                  切替の実装が割れている。#1450 B2）"
                .into(),
        });
    }
    if !right.contains("PanelView::Tasks => self.render_tasks_view(cx)") {
        out.push(Offender {
            file: RIGHT_PANEL,
            line: code_line_with(&right, "PanelView::Git => self.render_git_view")
                .map_or(0, |i| i + 1),
            why: "右パネルの枝に tasks ビューが無い（`tako panel --view tasks` で\
                  切り替えても中身が出ない。#1450 B2）"
                .into(),
        });
    }
    out
}

/// 4: ポーリングは `tick_user_tasks` の 1 か所で止まり、2 秒ループはそれを呼ぶだけ
fn scan_polling(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(&sources.tasks_panel, "fn tick_user_tasks(") {
        None => out.push(Offender {
            file: TASKS_PANEL,
            line: 0,
            why: "`tick_user_tasks` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let body = code_only(&window);
            if !body.contains("self.panel_visible") {
                out.push(Offender {
                    file: TASKS_PANEL,
                    line: at,
                    why: "ポーリングが `panel_visible` を見ていない（右パネルを閉じても\
                          2 秒ごとに撃ち続ける = 止める処理が要る形に戻っている。#1450 B2）"
                        .into(),
                });
            }
            if !body.contains("legacy_1450_b2()") {
                out.push(Offender {
                    file: TASKS_PANEL,
                    line: at,
                    why: "ポーリングが A/B（`TAKO_1450B2_LEGACY`）を見ていない\
                          （同一バイナリで「更新されない」を再現できない）"
                        .into(),
                });
            }
        }
    }
    // 2 秒ループは判断を持たない（持つと止め方が 2 か所に割れる）
    let main = code_only(&sources.main);
    if !main.contains("app.tick_user_tasks();") {
        out.push(Offender {
            file: MAIN,
            line: code_line_with(&main, "periodic_prep:user_tasks").map_or(0, |i| i + 1),
            why: "2 秒ループが `tick_user_tasks` を呼んでいない（一覧も配送の確定も\
                  止まる。#1450 B2）"
                .into(),
        });
    }
    if let Some(i) = code_line_with(&main, "app.refresh_user_tasks()") {
        out.push(Offender {
            file: MAIN,
            line: i + 1,
            why: "2 秒ループが `refresh_user_tasks` を直に呼んでいる（撃つ判断が\
                  `tick_user_tasks` の外にも生えると、片方だけ直した「閉じても撃つ」が残る）"
                .into(),
        });
    }
    out
}

/// 右パネルの手書き入力は **#1459 で全部 `TextField` へ移した**ので猶予は無い。
/// 編集操作を委ねているべき打鍵ハンドラ（ファイル / 関数 / 通すべき API）
const INPUT_DELEGATORS: &[(&str, &str, &str)] = &[
    (
        TASKS_PANEL,
        "fn handle_task_comment_key(",
        "handle_edit_key",
    ),
    (RIGHT_PANEL, "fn handle_git_commit_key(", "handle_edit_key"),
    (
        RIGHT_PANEL,
        "fn handle_git_branch_input_key(",
        "handle_edit_key",
    ),
];

/// 5: 手書きのテキスト入力を増やさない（#1459 で既存 2 本も `TextField` へ移し、猶予は空）
fn scan_no_handrolled_input(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    // **猶予表は無い**。入力を持つ 2 ファイルへ同じ走査を掛ける
    for (file, src) in [
        (TASKS_PANEL, &sources.tasks_panel),
        (RIGHT_PANEL, &sources.right_panel),
    ] {
        let code = code_only(src);
        for mark in HANDROLLED_INPUT_MARKS {
            if let Some(i) = code_line_with(&code, mark) {
                out.push(Offender {
                    file,
                    line: i + 1,
                    why: format!(
                        "手書きのカーソル操作（`{mark}`）が居る。編集は `TextField` を通す\
                         （同じ境界処理が 2 本に割れると、片方だけ直したバグが\
                         もう片方に残る。#1459）"
                    ),
                });
            }
        }
    }
    // 打鍵ハンドラが編集操作を `TextField` へ委ねている（走査の空振り / 自前復活の検出）
    for (file, needle, api) in INPUT_DELEGATORS {
        let src = match *file {
            TASKS_PANEL => &sources.tasks_panel,
            _ => &sources.right_panel,
        };
        match fn_window(src, needle) {
            None => out.push(Offender {
                file,
                line: 0,
                why: format!("`{needle}` が見つからない（走査が空振り）"),
            }),
            Some((at, window)) => {
                if !code_only(&window).contains(api) {
                    out.push(Offender {
                        file,
                        line: at,
                        why: format!(
                            "`{needle}` が `TextField::{api}` を通していない\
                             （編集操作が画面ごとに散ると境界バグの直し漏れが増える。#1459）"
                        ),
                    });
                }
            }
        }
    }
    // 丸めの実装は `text_field.rs` の中だけ（`pub` にすると手書きが再び生える）
    let text_field = code_only(&sources.text_field);
    match code_line_with(&text_field, "fn floor_char_boundary(") {
        None => out.push(Offender {
            file: TEXT_FIELD,
            line: 0,
            why: "`floor_char_boundary` が居ない（丸めの 1 実装が消えた = 走査が空振り）".into(),
        }),
        Some(i) => {
            let line = text_field.lines().nth(i).unwrap_or_default();
            if line.contains("pub") {
                out.push(Offender {
                    file: TEXT_FIELD,
                    line: i + 1,
                    why: "`floor_char_boundary` がモジュール外へ公開されている\
                          （呼べる場所が増えると「丸めてから自前で drain する」実装が\
                          再び生える。#1459）"
                        .into(),
                });
            }
        }
    }
    out
}

fn scan_all(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    out.extend(scan_no_direct_store(&sources.tasks_panel));
    out.extend(scan_no_emoji(sources));
    out.extend(scan_view_vocabulary(sources));
    out.extend(scan_polling(sources));
    out.extend(scan_no_handrolled_input(sources));
    out
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[test]
fn tasksビューの構造が保たれている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "#1450 B2 の構造が崩れている:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// 走査が空振りしていれば上のテストは無意味に緑になるので、窓が採れていることを固定する
#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let sources = Sources::load(&root);
    for (label, src, needle) in [
        ("一覧の取得", &sources.tasks_panel, "fn refresh_user_tasks("),
        ("返答", &sources.tasks_panel, "fn user_task_respond("),
        ("ポーリング", &sources.tasks_panel, "fn tick_user_tasks("),
        ("ビューの語彙", &sources.protocol, "pub fn parse(s: &str)"),
        // #1459 で移した右パネルの入力 2 本（窓が採れないと 5 が無意味に緑になる）
        (
            "コミット欄の打鍵",
            &sources.right_panel,
            "fn handle_git_commit_key(",
        ),
        (
            "コミット欄の挿入",
            &sources.right_panel,
            "fn git_commit_insert(",
        ),
        (
            "ブランチ名欄の打鍵",
            &sources.right_panel,
            "fn handle_git_branch_input_key(",
        ),
        (
            "ブランチ名欄の挿入",
            &sources.right_panel,
            "fn git_branch_input_insert(",
        ),
    ] {
        let (at, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{label}（{needle}）の窓が採れない = 走査が壊れている"));
        assert!(at > 0, "{label} の行番号が 0");
        assert!(
            window.lines().count() > 4,
            "{label} の窓が {} 行しかない（走査が壊れている）",
            window.lines().count()
        );
    }
    // 絵文字の走査が本文を見ていること（潰れた眺めを渡していない）
    assert!(
        sources.tasks_panel.lines().count() > 400,
        "tasks_panel.rs の本番コードが {} 行しか残っていない（範囲取りが壊れている）",
        sources.tasks_panel.lines().count()
    );
    // 絵文字判定そのものの検出力
    assert!(is_emoji('\u{1F600}') && is_emoji('\u{2705}'));
    assert!(!is_emoji('あ') && !is_emoji('A') && !is_emoji('・'));
}

/// **手書き入力の猶予は空**（#1459 で既存 2 本を `TextField` へ移し終えた）。
///
/// B2 の時点では git のコミット欄 / ブランチ名欄を名指しで猶予していた。移送が
/// 済んだので、ここでは「猶予表が残っていないか」ではなく**移送の結果**を直接固定する
/// —— 2 本が居て、手書きのマークが 1 つも無く、挿入も `TextField` を通っていること。
#[test]
fn 手書き入力の猶予が空になっている() {
    let root = workspace_root();
    let src = code_only(&prod(&root, RIGHT_PANEL));
    // 猶予していた 2 本は今も在り（消えたら走査が空振りになる）、手書きを持たない
    for (label, needle) in [
        ("git のコミットメッセージ欄", "fn handle_git_commit_key("),
        ("git の新規ブランチ名欄", "fn handle_git_branch_input_key("),
    ] {
        assert!(
            src.contains(needle),
            "{RIGHT_PANEL}: {label}（{needle}）が無い（走査が空振り）"
        );
    }
    for mark in HANDROLLED_INPUT_MARKS {
        assert!(
            !src.contains(mark),
            "{RIGHT_PANEL}: 手書きのカーソル操作（`{mark}`）が残っている。\
             #1459 の移送後は右パネルに 1 つも要らない"
        );
    }
    // 挿入も `TextField` の 1 実装を通る（上限・境界・制御文字の扱いが 2 本に割れない）
    for needle in ["fn git_commit_insert(", "fn git_branch_input_insert("] {
        let (_, window) = fn_window(&src, needle)
            .unwrap_or_else(|| panic!("{RIGHT_PANEL}: `{needle}` の窓が採れない"));
        assert!(
            window.contains(".insert("),
            "{RIGHT_PANEL}: `{needle}` が `TextField::insert` を通していない（#1459）"
        );
    }
    // 3 本目が right_panel.rs に生えていないこと（tasks の入力はここに置かない）
    assert!(
        !src.contains("fn handle_task_comment_key("),
        "{RIGHT_PANEL}: 返答コメント欄の打鍵処理がここに在る。\
         tasks ビューの実装は `tasks_panel.rs` に閉じる（#1450 B2）"
    );
}

/// A/B の逃げ道は **Issue ごとに別の env**（#1422 の規約）。
/// B1（`TAKO_1450_LEGACY` = 起票通知の抑止）と同じ env を読むと、
/// 片方のアームがもう片方の回帰を隠す
#[test]
fn abの逃げ道はb2専用のenvを読む() {
    let root = workspace_root();
    let src = read(&root, TASKS_PANEL);
    assert!(
        src.contains("TAKO_1450B2_LEGACY"),
        "{TASKS_PANEL}: #1450 B2 の A/B（`TAKO_1450B2_LEGACY`）が無い"
    );
    let (_, window) = fn_window(&src, "fn legacy_1450_b2(").expect("`legacy_1450_b2` が無い");
    for other in [
        "TAKO_1450_LEGACY\"",
        "TAKO_1399_LEGACY",
        "TAKO_1417_LEGACY",
        "TAKO_1422_LEGACY",
    ] {
        assert!(
            !window.contains(other),
            "{TASKS_PANEL}: B2 の A/B が `{other}` を読んでいる（軸が混ざると、\
             片方のアームがもう片方の回帰を隠す。#1422）"
        );
    }
}

/// **修正前を再現した注入**が `file:line` で名指しされることを確かめる。
/// 検出力の実証がここ（緑を作るのは簡単だが、落とせることの証明は注入でしかできない）
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) 画面が正本（YAML / store）を直読みする
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "    pub(crate) fn tick_user_tasks(&mut self) {",
        "    pub(crate) fn tick_user_tasks(&mut self) {\n        let _ = tako_control::user_tasks::load();",
    );
    expect_hit(&s, TASKS_PANEL, "`user_tasks::` に触っている");

    // (2) 一覧の取得が dispatch を通らない（画面専用の別経路になる）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.tasks_panel, "fn refresh_user_tasks(").expect("窓");
    s.tasks_panel = s.tasks_panel.replace(
        &window,
        &window.replace("Request::UserTask", "Request::ListPanes"),
    );
    expect_hit(&s, TASKS_PANEL, "`Request::UserTask` を組んでいない");

    // (3) 絵文字が紛れ込む
    let mut s = Sources::load(&root);
    s.tasks_panel = s
        .tasks_panel
        .replace("\"USER TASKS\"", "\"USER TASKS \u{1F4CB}\"");
    expect_hit(&s, TASKS_PANEL, "絵文字 U+1F4CB");

    // (3b) 文言カタログ側に絵文字が紛れ込む
    let mut s = Sources::load(&root);
    s.ui_text_panel = s.ui_text_panel.replace(
        "\"人がやることはありません\"",
        "\"\u{2705} 人がやることはありません\"",
    );
    expect_hit(&s, UI_TEXT_PANEL, "絵文字 U+2705");

    // (4) CLI / MCP の語彙から `tasks` が抜ける（GUI にしか無いビューになる）
    let mut s = Sources::load(&root);
    s.protocol = s.protocol.replace(
        "[\"fleet\", \"orch\", \"git\", \"tasks\", \"diagnostics\"]",
        "[\"fleet\", \"orch\", \"git\", \"diagnostics\"]",
    );
    expect_hit(&s, PROTOCOL, "`PanelViewWire::VALUES` に `tasks` が無い");

    // (5) GUI のタブがビュー切替と別の状態を持つ
    let mut s = Sources::load(&root);
    s.right_panel = s.right_panel.replace(
        "this.panel_view = PanelView::Tasks;",
        "this.tasks_tab_open = true;",
    );
    expect_hit(&s, RIGHT_PANEL, "`PanelView::Tasks` を代入していない");

    // (6) ポーリングが `panel_visible` を見なくなる（閉じても撃ち続ける）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.tasks_panel, "fn tick_user_tasks(").expect("窓");
    s.tasks_panel = s.tasks_panel.replace(
        &window,
        &window.replace("if !self.panel_visible {", "if false {"),
    );
    expect_hit(&s, TASKS_PANEL, "`panel_visible` を見ていない");

    // (7) 2 秒ループが判断を持ち直す（止め方が 2 か所に割れる）
    let mut s = Sources::load(&root);
    s.main = s
        .main
        .replace("app.tick_user_tasks();", "app.refresh_user_tasks();");
    expect_hit(&s, MAIN, "`refresh_user_tasks` を直に呼んでいる");

    // (8) 返答コメント欄が手書きのカーソル操作へ戻る（3 本目の入力が生える）
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "    pub(crate) fn tick_user_tasks(&mut self) {",
        "    pub(crate) fn tick_user_tasks(&mut self) {\n        let mut buf = String::new();\n        buf.drain(0..0);",
    );
    expect_hit(&s, TASKS_PANEL, "手書きのカーソル操作");

    // (9) 右パネルの入力が手書きのカーソル操作へ戻る（#1459 で移した 2 本の逆戻り）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.right_panel, "fn handle_git_branch_input_key(").expect("窓");
    s.right_panel = s.right_panel.replace(
        &window,
        &window.replace(
            "            key => {",
            "            key => {\n                let prev = \"\".char_indices().next_back();",
        ),
    );
    expect_hit(&s, RIGHT_PANEL, "手書きのカーソル操作");

    // (10) 右パネルの打鍵ハンドラが `TextField` を通さなくなる
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.right_panel, "fn handle_git_commit_key(").expect("窓");
    s.right_panel = s.right_panel.replace(
        &window,
        &window.replace("handle_edit_key", "handle_git_commit_edit_key"),
    );
    expect_hit(
        &s,
        RIGHT_PANEL,
        "`TextField::handle_edit_key` を通していない",
    );

    // (11) 丸めがモジュール外へ公開される（呼べる場所が増えると手書きが再び生える）
    let mut s = Sources::load(&root);
    s.text_field = s.text_field.replace(
        "fn floor_char_boundary(",
        "pub(crate) fn floor_char_boundary(",
    );
    expect_hit(&s, TEXT_FIELD, "モジュール外へ公開");
}

/// 注入した材料で走査し、**その file:line と理由**が名指しされることを確かめる
fn expect_hit(sources: &Sources, file: &str, fragment: &str) {
    let offenders = scan_all(sources);
    let report = offenders
        .iter()
        .map(Offender::report)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        offenders
            .iter()
            .any(|o| o.file == file && o.why.contains(fragment)),
        "注入したのに名指しされない（{file} / {fragment:?}）。実際の検出:\n{report}"
    );
    assert!(
        offenders
            .iter()
            .filter(|o| o.file == file && o.why.contains(fragment))
            .all(|o| o.line > 0),
        "行番号が 0 のまま名指ししている（file:line で追えない）:\n{report}"
    );
}
