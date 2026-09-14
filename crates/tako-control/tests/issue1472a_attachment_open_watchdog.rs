//! 添付を「既存のプレビューで開く」形を縛る番犬（Issue #1472 の A = PC の右パネル）
//!
//! 相方は `issue1472_attachment_preview_watchdog.rs`（同じ Issue の B = PWA / daemon 側。
//! あちらは「配信経路を増やさない」、こちらは「開く経路とサムネイルの上限」を見る）。
//!
//! # なぜ要るか
//!
//! tako には画像 / 動画 / PDF / Markdown / コードのプレビューが**既に 1 実装**ある
//! （`Request::OpenFile` → `open_plan::preview_route` の対応表）。右パネルの添付から
//! 「その場で見たい」という要求が来たとき、素直に見えて**間違っている**手が 3 つある:
//!
//! - 画面が自前で `set_preview` を呼ぶ / 自前のビューアを描く
//!   → `tako open` / `tako_open_file` と挙動が割れ、設計原則 5 が 1 つ崩れる
//! - 画面が拡張子を自前で見る（`"mp4" => 動画`）
//!   → #1283 で 1 箇所へ寄せた対応表が二重になり、片方だけ直した差が残る
//! - サムネイルを上限なしで展開する
//!   → 300 MB の添付や解凍爆弾で一覧の描画ごと詰まる（#1472 の受け入れ条件）
//!
//! さらに #496 の型（**押した瞬間に一括 dismiss で自分が消えて `on_click` が
//! 発火しない**）は、ハンドラを直呼びするテストでは検出できない。ここでは
//! 「守りの記述が在るか」を静的に縛り、発火そのものは visual-test 項目 151
//! （合成マウス）が押さえる。
//!
//! # 何を縛るか
//!
//! 1. **開く経路は `Request::OpenFile` 1 本** — `user_task_open_preview` が
//!    dispatch を組み、`tasks_panel.rs` は `set_preview` / `PreviewMode` /
//!    `preview::` に触らない（画面がプレビューの中身を持たない）
//! 2. **拡張子の表を画面が持たない** — 振り分けは `open_plan::preview_route` を通る
//! 3. **押下が一括 dismiss に食われない** — 添付行の窓に `on_mouse_down` +
//!    `stop_propagation` が居る（#496 / #503 の作法）
//! 4. **サムネイルに上限がある** — `decode_thumb` がバイト数と画素数の両方を見て、
//!    **画素数は `decode` の前**に見る。デコードの入口は `decode_thumb` だけ
//! 5. **サムネイルが溜まらない** — `ensure_task_thumbs` が欲しい集合以外を落とす
//!
//! # 見逃す側へ倒れないための作り
//!
//! 走査が空振りすれば 1〜5 はすべて無意味に緑になるので、[`走査が空振りしていない`]
//! で窓が採れていることを固定し、[`逆戻りを名指しできる`] で**注入 9 通り**が
//! `file:line` で名指しされることを確かめる。範囲取りは #1420 の 1 実装
//! （`production_range`）を通す

use std::path::{Path, PathBuf};

#[path = "common/production_range.rs"]
mod production_range;

const TASKS_PANEL: &str = "crates/tako-app/src/tasks_panel.rs";

/// 画面がプレビューの**中身**へ手を伸ばしたしるし。開く操作は dispatch 越しなので
/// 画面側にこれらは 1 つも要らない。**本文の markdown 描画**（`preview::markdown_blocks`）は
/// #690 の共有描画なので対象外 — ファイルを読んで表示する側だけを見る
const OWN_VIEWER_MARKS: &[&str] = &[
    "set_preview",
    "PreviewMode",
    "PreviewContent",
    "PreviewState",
    "preview::load",
    "video_player",
];

/// 拡張子の直書き。振り分けの正本は `open_plan::preview_route`（#1283）
const EXTENSION_MARKS: &[&str] = &[
    "\"mp4\"", "\"webm\"", "\"mov\"", "\"png\"", "\"jpg\"", "\"jpeg\"", "\"gif\"", "\"webp\"",
    "\"pdf\"", "\"md\"",
];

/// デコードの入口（`decode_thumb` の外に生えていないこと）
const DECODE_MARKS: &[&str] = &["ImageReader", "image::open", ".decode()"];

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

/// `needle` を含むコード行（1-based の絶対行）をすべて
fn code_lines_with(src: &str, needle: &str) -> Vec<usize> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|(i, _)| i + 1)
        .collect()
}

/// 添付 1 件を組み立てている塊（`for (i, att) in …` から行を積むまで）。
/// **行の中のボタンまで**を窓に入れる: 行が押せること・その押下が守られていること・
/// ボタンが行のクリックへ落ちないことは 1 つの塊として見ないと判らない
fn attachment_row_window(src: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains("for (i, att) in task.attachments.iter().enumerate()"))?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| l.trim() == "detail = detail.child(row.child(label));")
        .map(|(i, _)| i)
        .unwrap_or(lines.len() - 1);
    Some((start + 1, lines[start..=end].join("\n")))
}

// ---------------------------------------------------------------------------
// 検査本体（注入テストから同じ関数を呼べるよう、材料は引数で受ける）
// ---------------------------------------------------------------------------

struct Sources {
    tasks_panel: String,
}

impl Sources {
    fn load(root: &Path) -> Self {
        Self {
            tasks_panel: prod(root, TASKS_PANEL),
        }
    }
}

/// 1: 開く経路は `Request::OpenFile` 1 本で、画面はプレビューの中身を持たない
fn scan_single_open_route(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn user_task_open_preview(") {
        None => out.push(Offender {
            file: TASKS_PANEL,
            line: 0,
            why: "`user_task_open_preview` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let body = code_only(&window);
            for needle in ["Request::OpenFile", "tako_control::dispatch"] {
                if !body.contains(needle) {
                    out.push(Offender {
                        file: TASKS_PANEL,
                        line: at,
                        why: format!(
                            "添付を開く経路が `{needle}` を通らない（`tako open` / \
                             `tako_open_file` と挙動が割れる。設計原則 5。#1472）"
                        ),
                    });
                }
            }
        }
    }
    let code = code_only(src);
    for mark in OWN_VIEWER_MARKS {
        for line in code_lines_with(&code, mark) {
            out.push(Offender {
                file: TASKS_PANEL,
                line,
                why: format!(
                    "画面が `{mark}` に触っている（プレビューの中身は dispatch の向こう側の\
                     1 実装が持つ。画面に写すと `tako open` と割れる。#1472）"
                ),
            });
        }
    }
    out
}

/// 2: 拡張子の表を画面が持たない（振り分けは `open_plan` の 1 実装）
fn scan_no_extension_table(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let code = code_only(src);
    for mark in EXTENSION_MARKS {
        for line in code_lines_with(&code, mark) {
            out.push(Offender {
                file: TASKS_PANEL,
                line,
                why: format!(
                    "画面が拡張子 `{mark}` を直に見ている（対応表は \
                     `open_plan::preview_route` の 1 実装。#1283 / #1472）"
                ),
            });
        }
    }
    if !code.contains("open_plan::preview_route") {
        out.push(Offender {
            file: TASKS_PANEL,
            // 振り分けを使う場所（添付の塊）を指す。0 行だと直し先が分からない
            line: attachment_row_window(src).map(|(at, _)| at).unwrap_or(1),
            why: "`open_plan::preview_route` を通っていない（振り分けを自前で持っている。\
                  #1472）"
                .into(),
        });
    }
    out
}

/// 3: 行の押下が一括 dismiss に食われない（#496 / #503 の作法）
fn scan_click_survives_dismiss(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    let Some((at, window)) = attachment_row_window(src) else {
        out.push(Offender {
            file: TASKS_PANEL,
            line: 0,
            why: "添付行の窓が見つからない（走査が空振り）".into(),
        });
        return out;
    };
    let body = code_only(&window);
    // 行そのものが押せる
    if !body.contains(".on_click(") {
        out.push(Offender {
            file: TASKS_PANEL,
            line: at,
            why: "添付の行が押せない（小さなボタンだけを狙わせると、#1472 の\
                  「押しても見られない」へ戻る）"
                .into(),
        });
    }
    // 押下が一括 dismiss より先に確定する
    if !(body.contains(".on_mouse_down(") && body.contains("stop_propagation")) {
        out.push(Offender {
            file: TASKS_PANEL,
            line: at,
            why: "添付行の押下が `on_mouse_down` + `stop_propagation` で守られていない\
                  （ルート div の一括 dismiss で押下の瞬間に消え、`on_click` が一度も\
                  発火しない = #496 の型。ハンドラ直呼びのテストでは気づけない）"
                .into(),
        });
    }
    // 行の中のボタンも同じ守りを持つ（持たないと「Finder で表示」がプレビューも開く）
    if body.matches("stop_propagation").count() < 3 {
        out.push(Offender {
            file: TASKS_PANEL,
            line: at,
            why: "行の中のボタンが伝播を止めていない（行のクリックへも落ちて、\
                  1 回の押下で 2 つの操作が走る。#1472）"
                .into(),
        });
    }
    // 実マウスで押すための矩形（visual-test 項目 151 がここを読む）
    if !body.contains("panel_click_probe_bounds") {
        out.push(Offender {
            file: TASKS_PANEL,
            line: at,
            why: "添付行の実描画矩形を記録していない（合成マウスで押せなくなり、\
                  #496 型の回帰を機械で見られなくなる。#1472）"
                .into(),
        });
    }
    out
}

/// 4: サムネイルに上限がある（**画素数は decode の前に見る**）
fn scan_thumb_limits(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn decode_thumb(") {
        None => out.push(Offender {
            file: TASKS_PANEL,
            line: 0,
            why: "`decode_thumb` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let body = code_only(&window);
            for mark in ["THUMB_MAX_FILE_BYTES", "THUMB_MAX_PIXELS"] {
                if !body.contains(mark) {
                    out.push(Offender {
                        file: TASKS_PANEL,
                        line: at,
                        why: format!(
                            "サムネイルが `{mark}` を見ていない（大きい添付や解凍爆弾で\
                             一覧の描画ごと詰まる。#1472）"
                        ),
                    });
                }
            }
            // 画素の検査が展開より**前**に居る（後ろだと先に展開してしまう）
            let guard = body.find("THUMB_MAX_PIXELS");
            let decode = body.find(".decode()");
            if let (Some(g), Some(d)) = (guard, decode) {
                if g > d {
                    out.push(Offender {
                        file: TASKS_PANEL,
                        line: at,
                        why: "画素数の検査が展開より後ろに居る（**展開してから**捨てるので\
                              上限の意味が無い。ヘッダだけで判るので前で落とす。#1472）"
                            .into(),
                    });
                }
            }
        }
    }
    // デコードの入口は `decode_thumb` だけ（他所に生えると上限が効かない）
    let Some((_, decode_window)) = fn_window(src, "fn decode_thumb(") else {
        return out;
    };
    let outside = code_only(&src.replace(&decode_window, ""));
    for mark in DECODE_MARKS {
        for line in code_lines_with(&outside, mark) {
            out.push(Offender {
                file: TASKS_PANEL,
                line,
                why: format!(
                    "`decode_thumb` の外に画像展開の入口 `{mark}` が居る\
                     （上限を通らない経路が生える。#1472）"
                ),
            });
        }
    }
    out
}

/// 5: サムネイルが溜まらない（**いま開いている 1 件のぶんだけ**持つ）
fn scan_thumb_eviction(src: &str) -> Vec<Offender> {
    let mut out = Vec::new();
    match fn_window(src, "fn ensure_task_thumbs(") {
        None => out.push(Offender {
            file: TASKS_PANEL,
            line: 0,
            why: "`ensure_task_thumbs` が見つからない（走査が空振り）".into(),
        }),
        Some((at, window)) => {
            let body = code_only(&window);
            if !body.contains("thumbs.retain(") {
                out.push(Offender {
                    file: TASKS_PANEL,
                    line: at,
                    why: "欲しい集合の外を落としていない（タスクを見て回るほど画像が\
                          溜まり続ける。捨てる処理を別に書くのではなく、毎回揃える形で\
                          溜まらないようにする。#1472）"
                        .into(),
                });
            }
            if !body.contains("thumbable(") {
                out.push(Offender {
                    file: TASKS_PANEL,
                    line: at,
                    why: "サムネイルの対象を `thumbable` で絞っていない（動画や消えた添付まで\
                          読みに行く。#1472）"
                        .into(),
                });
            }
        }
    }
    out
}

fn scan_all(sources: &Sources) -> Vec<Offender> {
    let mut out = Vec::new();
    out.extend(scan_single_open_route(&sources.tasks_panel));
    out.extend(scan_no_extension_table(&sources.tasks_panel));
    out.extend(scan_click_survives_dismiss(&sources.tasks_panel));
    out.extend(scan_thumb_limits(&sources.tasks_panel));
    out.extend(scan_thumb_eviction(&sources.tasks_panel));
    out
}

// ---------------------------------------------------------------------------
// テスト
// ---------------------------------------------------------------------------

#[test]
fn 走査が空振りしていない() {
    let root = workspace_root();
    let src = prod(&root, TASKS_PANEL);
    assert!(
        src.lines().count() > 200,
        "本番コードの眺めが短すぎる（範囲取りが壊れている）: {} 行",
        src.lines().count()
    );
    for needle in [
        "fn user_task_open_preview(",
        "fn decode_thumb(",
        "fn ensure_task_thumbs(",
    ] {
        assert!(
            fn_window(&src, needle).is_some(),
            "`{needle}` の窓が採れない（走査が空振りする）"
        );
    }
    let (_, row) = attachment_row_window(&src).expect("添付行の窓が採れる");
    assert!(
        row.lines().count() > 20 && row.contains("\"tasks-att-row\""),
        "添付行の窓が短すぎる / 行の組み立てを含んでいない: {} 行",
        row.lines().count()
    );
}

#[test]
fn 添付は既存のプレビューで開く形になっている() {
    let root = workspace_root();
    let offenders = scan_all(&Sources::load(&root));
    assert!(
        offenders.is_empty(),
        "添付プレビューの形が崩れている（#1472）:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn expect_hit(sources: &Sources, file: &str, needle: &str) {
    let offenders = scan_all(sources);
    assert!(
        offenders
            .iter()
            .any(|o| o.file == file && o.why.contains(needle) && o.line > 0),
        "注入を名指しできていない（file={file} needle={needle}）。採れた指摘:\n{}",
        offenders
            .iter()
            .map(Offender::report)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let root = workspace_root();
    assert!(
        scan_all(&Sources::load(&root)).is_empty(),
        "注入前が既に汚れている"
    );

    // (1) 開く経路が dispatch を離れる（画面が自前で開く）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.tasks_panel, "fn user_task_open_preview(").expect("窓");
    s.tasks_panel = s.tasks_panel.replace(
        &window,
        &window.replace("Request::OpenFile", "Request::ListPanes"),
    );
    expect_hit(&s, TASKS_PANEL, "`Request::OpenFile` を通らない");

    // (2) 画面が自前のビューアを持ち始める
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "    /// 添付をファイルマネージャで表示する",
        "    fn open_myself(&mut self) { let _ = self.set_preview(); }\n\
         \n    /// 添付をファイルマネージャで表示する",
    );
    expect_hit(&s, TASKS_PANEL, "`set_preview` に触っている");

    // (3) 画面が拡張子を直に見る（対応表が二重になる）
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "pub(crate) fn thumbable(att: &Attachment) -> bool {",
        "pub(crate) fn thumbable(att: &Attachment) -> bool {\n\
         \x20   if att.path.ends_with(\"mp4\") { return false; }",
    );
    expect_hit(&s, TASKS_PANEL, "拡張子 `\"mp4\"` を直に見ている");

    // (4) 振り分けが `open_plan` を離れる
    let mut s = Sources::load(&root);
    s.tasks_panel = s
        .tasks_panel
        .replace("open_plan::preview_route", "my_own_route");
    expect_hit(&s, TASKS_PANEL, "`open_plan::preview_route` を通っていない");

    // (5) 行の押下が一括 dismiss から守られなくなる（#496 の型）
    let mut s = Sources::load(&root);
    let (_, row) = attachment_row_window(&s.tasks_panel).expect("窓");
    s.tasks_panel = s.tasks_panel.replace(
        &row,
        &row.replace(
            "cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),",
            "cx.listener(|_, _: &MouseDownEvent, _, _| ()),",
        ),
    );
    expect_hit(&s, TASKS_PANEL, "伝播を止めていない");

    // (6) 行そのものが押せなくなる（小さなボタンだけへ戻る）
    let mut s = Sources::load(&root);
    let (_, row) = attachment_row_window(&s.tasks_panel).expect("窓");
    s.tasks_panel = s
        .tasks_panel
        .replace(&row, &row.replace(".on_click(", ".on_nothing("));
    expect_hit(&s, TASKS_PANEL, "添付の行が押せない");

    // (7) サムネイルの上限が外れる
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.tasks_panel, "fn decode_thumb(").expect("窓");
    s.tasks_panel = s.tasks_panel.replace(
        &window,
        &window.replace("THUMB_MAX_PIXELS", "u64::from(u32::MAX) * 4"),
    );
    expect_hit(&s, TASKS_PANEL, "`THUMB_MAX_PIXELS` を見ていない");

    // (8) 上限を通らない展開の入口が別に生える
    let mut s = Sources::load(&root);
    s.tasks_panel = s.tasks_panel.replace(
        "pub(crate) fn thumbable(att: &Attachment) -> bool {",
        "pub(crate) fn thumbable(att: &Attachment) -> bool {\n\
         \x20   let _ = image::open(&att.path).map(|i| i.decode());",
    );
    expect_hit(&s, TASKS_PANEL, "画像展開の入口 `image::open` が居る");

    // (9) サムネイルが溜まり続ける（欲しい集合の外を落とさない）
    let mut s = Sources::load(&root);
    let (_, window) = fn_window(&s.tasks_panel, "fn ensure_task_thumbs(").expect("窓");
    s.tasks_panel = s.tasks_panel.replace(
        &window,
        &window.replace("self.user_tasks.thumbs.retain(", "let _ = ("),
    );
    expect_hit(&s, TASKS_PANEL, "欲しい集合の外を落としていない");
}
