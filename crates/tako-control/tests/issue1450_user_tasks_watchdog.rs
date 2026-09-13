//! ユーザー向けタスク（#1450）の構造を縛る番犬
//!
//! # なぜ要るか
//!
//! この機能は**入口が 4 つ**（CLI / MCP / PC の画面 / スマホの PWA）ある。
//! 入口ごとに store を直接触る実装が生えると、
//!
//! - 起票の通知が出る経路と出ない経路ができる（= 起票に気づけない画面が生まれる）
//! - 返答の配送が片方の経路にしか無い（= スマホから返した返答だけ master に届かない）
//! - 永続の形が入口ごとにズレる（= 片方が書いた YAML をもう片方が読めない）
//!
//! のどれかが必ず起きる。設計原則 5（UI でできることは AI からもできる）は
//! 「同じ操作が同じ 1 実装を通る」ことでしか守れないので、そこを静的に固定する。
//!
//! # 何を縛るか
//!
//! 1. **操作は tako-core の 1 実装を通る** — CLI が store を直接触らない
//!    （`tako todo` は `Request::UserTask` を組んで dispatch へ渡すだけ）
//! 2. **MCP と CLI が同じ action 語彙を名乗る** — 片方にしか無い操作を作らない
//! 3. **永続の形が 4 か所に揃っている** — `SchemaId::UserTasks` が enum / `SPECS` /
//!    `targets` / 共有分類カタログのすべてに居る（#916。どれか 1 つ欠けると
//!    「移行されないファイル」「読めないまま既定値へ落ちるファイル」ができる）
//! 4. **通知の出し口が増殖しない** — 起票の通知は `notify_ui_info` の 1 実装だけを通り、
//!    `set_remote_notice` の直呼びで別口を作らない（#1399 / #1417 / #1422 と同じ物差し）
//! 5. **配送は既存の経路を呼ぶ** — `Request::Send` と `master_launch::plan` と
//!    `queue_prompt_flow` を通り、独自の tmux 送信・起動コマンド組み立てを持たない
//!    （#1450 の要件追加 ③「既存の spawn / 送達経路を呼ぶ 1 実装。新経路を作らない」）
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜5 はすべて無意味に緑になるので、
//! [`走査が空振りしていない`] で窓が採れていることを固定し、
//! [`逆戻りを名指しできる`] で**注入 7 通り**が `file:line` で名指しされることを
//! 確かめる。範囲取りは #1420 の 1 実装（`production_range`）を通す
//! （`dispatch.rs` は本番コードとテストが交互に並ぶので、雑に切ると
//! 配送の実装が丸ごと視界から消える）。

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CLI: &str = "crates/tako-cli/src/main.rs";
const MCP_REQUEST: &str = "crates/tako-control/src/mcp/request.rs";
const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const MIGRATIONS: &str = "crates/tako-control/src/migrations.rs";
const CATALOG: &str = "crates/tako-control/src/config_share/catalog.rs";
const SCHEMA: &str = "crates/tako-core/src/migration.rs";

/// CLI / MCP / dispatch がすべて名乗る操作の語彙（**ここが正**）
const ACTIONS: &[&str] = &[
    "add", "list", "show", "update", "done", "dismiss", "respond",
];

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

/// 本番コードだけの眺め（#1420 の 1 実装）
fn prod(root: &Path, rel: &'static str) -> String {
    production_range::production(&read(root, rel), rel)
}

/// 1 つの違反（`ファイル:行 — 理由`）
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

/// 関数の窓（宣言行の 1-based 行番号と本文）。
/// 終わりは**宣言行と同じ字下げの `}`**（「宣言行から N 行」で切ると隣の実装を数える）
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

/// 注釈（`//` で始まる行）を落とした窓。検査は**コードだけ**を見る
fn code_only(window: &str) -> String {
    window
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 窓の中で `needle` を含む最初の**コードの**行（0-based の相対位置）
fn code_line_with(window: &str, needle: &str) -> Option<usize> {
    window
        .lines()
        .position(|l| !l.trim_start().starts_with("//") && l.contains(needle))
}

// ---------------------------------------------------------------------------
// 検査本体（注入テストから同じ関数を呼べるよう、材料は引数で受ける）
// ---------------------------------------------------------------------------

struct Sources {
    dispatch: String,
    cli: String,
    mcp_request: String,
    sidebar: String,
    migrations: String,
    catalog: String,
    schema: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            dispatch: prod(root, DISPATCH),
            cli: prod(root, CLI),
            mcp_request: prod(root, MCP_REQUEST),
            sidebar: prod(root, SIDEBAR),
            migrations: prod(root, MIGRATIONS),
            catalog: read(root, CATALOG),
            schema: read(root, SCHEMA),
        }
    }
}

/// 1: CLI は store を直接触らない（dispatch の 1 実装を通る）
fn scan_cli(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn todo_request(") {
        None => out.push(Offender {
            file: CLI,
            line: 0,
            why: "`todo_request` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("Request::UserTask") {
                out.push(Offender {
                    file: CLI,
                    line: at,
                    why: "`tako todo` が `Request::UserTask` を組んでいない\
                          （dispatch を通らない別経路になっている。#1450）"
                        .into(),
                });
            }
            for direct in ["user_tasks::", "user_task::"] {
                if let Some(i) = code_line_with(&window, direct) {
                    out.push(Offender {
                        file: CLI,
                        line: at + i,
                        why: format!(
                            "CLI が `{direct}` を直に呼んでいる（store をローカルで触ると、\
                             起票の通知も返答の配送も CLI 経路だけ抜ける。#1450）"
                        ),
                    });
                }
            }
            // 全 action が組み立てられているか（片方の入口にしか無い操作を作らない）
            for action in ACTIONS {
                if !code.contains(&format!("base(\"{action}\")")) {
                    out.push(Offender {
                        file: CLI,
                        line: at,
                        why: format!("`tako todo` に action `{action}` が無い（#1450）"),
                    });
                }
            }
        }
    }
    out
}

/// 2: MCP も同じ `Request::UserTask` を組み、同じ action 語彙を公開する
fn scan_mcp(request_src: &str, catalog_actions: &[String]) -> Vec<Offender> {
    let mut out = Vec::new();
    let lines: Vec<&str> = request_src.lines().collect();
    match lines.iter().position(|l| l.contains("\"tako_todo\" =>")) {
        None => out.push(Offender {
            file: MCP_REQUEST,
            line: 0,
            why: "`tako_todo` の腕が見つからない（MCP から操作できない。#1450）".into(),
        }),
        Some(i) => {
            let window: String = lines[i..(i + 30).min(lines.len())].join("\n");
            if !window.contains("Request::UserTask") {
                out.push(Offender {
                    file: MCP_REQUEST,
                    line: i + 1,
                    why: "`tako_todo` が `Request::UserTask` を組んでいない\
                          （CLI と別の経路になっている。#1450）"
                        .into(),
                });
            }
        }
    }
    for action in ACTIONS {
        if !catalog_actions.iter().any(|a| a == action) {
            out.push(Offender {
                file: "crates/tako-control/src/mcp/catalog.rs",
                line: 0,
                why: format!(
                    "MCP の `tako_todo` が action `{action}` を公開していない\
                     （CLI にあって MCP に無い操作 = 設計原則 5 の違反。#1450）"
                ),
            });
        }
    }
    out
}

/// 3: 永続の形が 4 か所に揃っている（#916）
fn scan_persistence(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    for (file, src, needle, why) in [
        (
            SCHEMA,
            &sources.schema,
            "UserTasks",
            "版数の番地（`SchemaId::UserTasks`）が無い",
        ),
        (
            MIGRATIONS,
            &sources.migrations,
            "versioned(SchemaId::UserTasks",
            "`SPECS` に登録されていない（移行の発火から漏れる）",
        ),
        (
            MIGRATIONS,
            &sources.migrations,
            "SchemaId::UserTasks => crate::user_tasks::store_path()",
            "`targets` が対象ファイルを指していない（検査も移行も走らない）",
        ),
        (
            CATALOG,
            &sources.catalog,
            "orchestrator/user-tasks.yaml",
            "共有分類カタログに載っていない（#513 / #916 の対応表が割れる）",
        ),
    ] {
        if !src.contains(needle) {
            out.push(Offender {
                file,
                line: 0,
                why: format!("{why}（#1450 / #916）"),
            });
        }
    }
    // 「読めないものを黙って既定値へ落とさない」（承認待ちが画面から消える）
    if !sources.migrations.contains("fn validate_user_tasks(") {
        out.push(Offender {
            file: MIGRATIONS,
            line: 0,
            why: "`validate_user_tasks` が無い（壊れたファイルが 0 件へ丸められる。#169 / #916）"
                .into(),
        });
    }
    out
}

/// 4: 起票の通知は 1 実装だけを通る
fn scan_notice(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn notify_user_task_added(") {
        None => out.push(Offender {
            file: SIDEBAR,
            line: 0,
            why: "`notify_user_task_added` が見つからない（起票しても画面が無言。#1450）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            if !code.contains("self.notify_ui_info(") {
                out.push(Offender {
                    file: SIDEBAR,
                    line: at,
                    why: "起票の通知が `notify_ui_info` を通っていない\
                          （出し口が 2 本になると片方だけ無言になる。#1417 と同じ物差し）"
                        .into(),
                });
            }
            if let Some(i) = code_line_with(&window, "set_remote_notice(") {
                out.push(Offender {
                    file: SIDEBAR,
                    line: at + i,
                    why: "起票の通知が通知欄を直に触っている（A/B の逃げ道と画面の別\
                          （`NoticeArea`）を素通りする。#1450）"
                        .into(),
                });
            }
        }
    }
    // 成功系の出し口そのものが A/B の逃げ道を持っているか
    match fn_window(src, "fn notify_ui_info(") {
        None => out.push(Offender {
            file: SIDEBAR,
            line: 0,
            why: "`notify_ui_info`（成功系の出し口）が無い（#1450）".into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            for mark in ["arm.suppressed()", "self.set_remote_notice("] {
                if !code.contains(mark) {
                    out.push(Offender {
                        file: SIDEBAR,
                        line: at,
                        why: format!(
                            "`notify_ui_info` が {mark} を通っていない\
                             （通知欄と A/B の逃げ道は 1 実装が担う。#1450）"
                        ),
                    });
                }
            }
        }
    }
    out
}

/// 5: 配送は既存の経路を呼ぶ（新しい送達経路を作らない）
fn scan_delivery(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    // (a) 生きている master へは `Request::Send`（= tako_send_input と同じ腕）
    match fn_window(src, "fn deliver_user_task_response(") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`deliver_user_task_response` が見つからない（返答が master へ届かない。#1450）"
                .into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            for (mark, why) in [
                (
                    "Request::Send {",
                    "返答の送達が `Request::Send` を通っていない（#1450 の要件追加 ③ は\
                     「既存の送達経路を呼ぶ」。独自経路は #1259 の顛末追跡から外れる）",
                ),
                (
                    "choose_target(",
                    "配送先の判断が `user_tasks::choose_target` の 1 実装を通っていない",
                ),
            ] {
                if !code.contains(mark) {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: why.into(),
                    });
                }
            }
            for (bad, why) in [
                (
                    "deliver_via_tmux",
                    "配送が tmux 経路を直に叩いている（ペインが解決できている送達で\
                     使う経路ではない。#1450）",
                ),
                (
                    "queue_write(",
                    "配送が PTY へ直接書いている（送達確認が付かない = #640 の事故）",
                ),
            ] {
                if let Some(i) = code_line_with(&window, bad) {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at + i,
                        why: why.into(),
                    });
                }
            }
        }
    }
    // (b) master が居ないときは `master_launch::plan`（= tako master / #1078 と同じ組み立て）
    match fn_window(src, "fn launch_master_for_response(") {
        None => out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`launch_master_for_response` が見つからない（master 不在で無言になる。#1450）"
                .into(),
        }),
        Some((at, window)) => {
            let code = code_only(&window);
            for (mark, why) in [
                (
                    "master_launch::plan(",
                    "master の起動が `master_launch::plan` を通っていない（CLI / PWA と\
                     組み立てが割れると「この経路の master だけ prompt が付かない」= #761）",
                ),
                (
                    "Request::TabNew {",
                    "master を新しいタブへ起こしていない（#1450 の要件追加 ③）",
                ),
                (
                    "queue_command_flow(",
                    "起動コマンドが #640 の送達確認つきフローを通っていない（作ったばかりの\
                     ペインはセッションの取り付けが次の tick なので、その場の書き込みは落ちる）",
                ),
                (
                    "queue_prompt_flow(",
                    "初回メッセージが送達確認つきの経路を通っていない（起動直後の素の\
                     シェルへ流れ込む = #694 / #1006 の事故）",
                ),
                (
                    "DeliveryState::Failed",
                    "起動に失敗したときの記録が無い（配送失敗が無言になる。#1450）",
                ),
            ] {
                if !code.contains(mark) {
                    out.push(Offender {
                        file: DISPATCH,
                        line: at,
                        why: why.into(),
                    });
                }
            }
        }
    }
    // (c) 決着していない配送を畳み込む口がある（`sent` のまま固まらない）
    if !src.contains("fn reconcile_user_task_deliveries(") {
        out.push(Offender {
            file: DISPATCH,
            line: 0,
            why: "`reconcile_user_task_deliveries` が無い（配送が `sent` のまま固まり、\
                  `delivered` を誰も観測できない。#1450 / #1259）"
                .into(),
        });
    }
    out
}

/// MCP カタログが公開している `tako_todo` の action 語彙
fn catalog_actions() -> Vec<String> {
    let tool = tako_control::mcp::tools()
        .into_iter()
        .find(|t| t["name"] == "tako_todo")
        .expect("`tako_todo` が MCP カタログに無い（#1450）");
    tool["inputSchema"]["properties"]["action"]["enum"]
        .as_array()
        .expect("action の enum")
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

fn scan_all(sources: &Sources, actions: &[String]) -> Vec<Offender> {
    let mut out = scan_cli(&sources.cli);
    out.extend(scan_mcp(&sources.mcp_request, actions));
    out.extend(scan_persistence(sources));
    out.extend(scan_notice(&sources.sidebar));
    out.extend(scan_delivery(&sources.dispatch));
    out
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[test]
fn 現行コードは構造を守っている() {
    let root = workspace_root();
    let sources = Sources::load(&root);
    let offenders = scan_all(&sources, &catalog_actions());
    assert!(
        offenders.is_empty(),
        "#1450 の構造が崩れている:\n{}",
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
        ("CLI の組み立て", &sources.cli, "fn todo_request("),
        ("配送", &sources.dispatch, "fn deliver_user_task_response("),
        (
            "master 起動",
            &sources.dispatch,
            "fn launch_master_for_response(",
        ),
        ("通知", &sources.sidebar, "fn notify_user_task_added("),
    ] {
        let (at, window) = fn_window(src, needle)
            .unwrap_or_else(|| panic!("{label}（{needle}）の窓が採れない = 走査が壊れている"));
        assert!(at > 0, "{label} の行番号が 0");
        assert!(
            window.lines().count() > 5,
            "{label} の窓が {} 行しかない（走査が壊れている）",
            window.lines().count()
        );
    }
    let actions = catalog_actions();
    assert_eq!(
        actions.len(),
        ACTIONS.len(),
        "MCP が公開する action の数が正本と違う: {actions:?}"
    );
}

/// **修正前を再現した注入**が `file:line` で名指しされることを確かめる。
/// 検出力の実証がここ（緑を作るのは簡単だが、落とせることの証明は注入でしかできない）
#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    let base = Sources::load(&root);
    let actions = catalog_actions();
    assert!(
        scan_all(&base, &actions).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) CLI が store を直接触る（ローカル処理へ戻す）
    let mut s = Sources::load(&root);
    s.cli = s.cli.replace(
        "    let caller_role = std::env::var(\"TAKO_ORCHESTRATOR_ROLE\").ok();",
        "    let _ = crate::user_tasks::load();\n    let caller_role = std::env::var(\"TAKO_ORCHESTRATOR_ROLE\").ok();",
    );
    expect_hit(&s, &actions, CLI, "CLI が `user_tasks::` を直に呼んでいる");

    // (2) CLI から action が 1 つ消える（MCP にだけ在る操作になる）
    let mut s = Sources::load(&root);
    s.cli = s.cli.replace("base(\"respond\")", "base(\"reply\")");
    expect_hit(&s, &actions, CLI, "action `respond` が無い");

    // (3) MCP が別の Request を組む（CLI と経路が割れる）
    let mut s = Sources::load(&root);
    s.mcp_request = s.mcp_request.replace(
        "\"tako_todo\" => Request::UserTask {",
        "\"tako_todo\" => Request::List {",
    );
    expect_hit(
        &s,
        &actions,
        MCP_REQUEST,
        "`Request::UserTask` を組んでいない",
    );

    // (4) 永続の番地が `SPECS` から抜ける（移行の発火から漏れる）
    let mut s = Sources::load(&root);
    s.migrations = s.migrations.replace(
        "versioned(SchemaId::UserTasks",
        "versioned(SchemaId::Ledger2",
    );
    expect_hit(&s, &actions, MIGRATIONS, "`SPECS` に登録されていない");

    // (5) 起票の通知が通知欄を直に触る（出し口が 2 本になる）
    let mut s = Sources::load(&root);
    s.sidebar = s.sidebar.replace(
        "        self.notify_ui_info(\n            NoticeArea::UserTasks,",
        "        self.set_remote_notice(String::new(), false);\n        self.notify_ui_info(\n            NoticeArea::UserTasks,",
    );
    expect_hit(&s, &actions, SIDEBAR, "通知欄を直に触っている");

    // (6) 配送が `Request::Send` を通らない（独自経路になる）
    let mut s = Sources::load(&root);
    let (at, window) = fn_window(&s.dispatch, "fn deliver_user_task_response(").expect("窓");
    let _ = at;
    s.dispatch = s.dispatch.replace(
        &window,
        &window.replace("Request::Send {", "Request::Read {"),
    );
    expect_hit(&s, &actions, DISPATCH, "`Request::Send` を通っていない");

    // (7) master の起動が独自組み立てになる（#761 の取り違えが戻る）
    let mut s = Sources::load(&root);
    s.dispatch = s
        .dispatch
        .replace("master_launch::plan(profile)", "some_other_plan(profile)");
    expect_hit(
        &s,
        &actions,
        DISPATCH,
        "`master_launch::plan` を通っていない",
    );
}

/// 注入した材料で走査し、**その file:line と理由**が名指しされることを確かめる
fn expect_hit(sources: &Sources, actions: &[String], file: &str, fragment: &str) {
    let offenders = scan_all(sources, actions);
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
            .all(|o| o.line > 0
                || o.why.contains("登録")
                || o.why.contains("公開")
                || o.why.contains("カタログ")),
        "行番号が 0 のまま名指ししている（file:line で追えない）:\n{report}"
    );
}

/// A/B の逃げ道は **Issue ごとに別の env**（#1422 の規約）。
/// 同じ env を読むと、片方のアームがもう片方の回帰を隠す
#[test]
fn abの逃げ道は1450専用のenvを読む() {
    let root = workspace_root();
    let src = read(&root, SIDEBAR);
    assert!(
        src.contains("TAKO_1450_LEGACY"),
        "{SIDEBAR}: #1450 の A/B（`TAKO_1450_LEGACY`）が無い。\
         同一バイナリで「起票しても無言」を再現できないと、通知の検出力を実証できない"
    );
    let (at, window) = fn_window(&src, "fn legacy_1450(").expect("`legacy_1450` が無い");
    for other in ["TAKO_1399_LEGACY", "TAKO_1417_LEGACY", "TAKO_1422_LEGACY"] {
        assert!(
            !window.contains(other),
            "{SIDEBAR}:{at} #1450 のアームが {other} を読んでいる（軸が混ざる。#1422）"
        );
    }
}
