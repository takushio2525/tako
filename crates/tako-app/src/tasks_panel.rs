//! 右パネルの `tasks` ビュー —— 人がやることの一覧・詳細・返答（Issue #1450 の分割 B2）
//!
//! ## 正本はここに無い
//!
//! タスクの正本は B1（`tako_core::user_task` + `<data_dir>/orchestrator/user-tasks.yaml`）で、
//! この画面は**読んだ結果のキャッシュ**しか持たない。読み書きはすべて
//! `Request::UserTask`（= CLI `tako todo` / MCP `tako_todo` と同じ 1 経路）を通る。
//! UI から `user_tasks::` / `TaskStore` / YAML を直接触らないことは番犬
//! `issue1450b2_tasks_panel_watchdog` が縛る（設計原則 5）。
//!
//! ## ポーリングは新しいタイマーを作らない
//!
//! 一覧の更新は**既存の 2 秒定期更新ループ**（`main.rs` の periodic tick）に 1 段
//! 足しただけで、条件は `panel_visible`。右パネルを閉じればその tick から撃たなくなる
//! ので「止める処理」を書いていない = **止め忘れが構造的に起きない**
//! （git ビューが `panel_visible && panel_view == Git` で `git_cwd` を決めているのと同じ形）。
//!
//! `list` を定期的に撃つことには**もう 1 つ役目**がある。B1 は
//! `host.prompt_delivery_state`（#1259）を **`UserTask` の dispatch が走るたびに畳み込んで**
//! 配送を `sent` → `delivered` / `failed` へ確定させるので、この画面のポーリングが
//! そのまま「返答が master に届いたか」の確定器になる。だから `list` は
//! background へ逃がさない（host に触れないと永久に `sent` のまま残る）。
//!
//! ## 例外（設計原則 5 の但し書き）
//!
//! クリップボードへの書き込みと URL を既定ブラウザで開く操作だけは dispatch を通らない。
//! 前者は GPUI の `App` が要り（`tako chat copy` / `tako preview-copy-code` も同じ理由で
//! 実書き込みは render 側）、後者は端末リンク・PDF・md プレビューの 3 経路が
//! すべて `os_integration::open_url` 直呼びで統一されている。**読める形**
//! （`copy_texts[].text` / `links[]`）は `tako todo show` が返すので AI からは見える。

use gpui::{
    div, prelude::*, px, ClipboardItem, Context, CursorStyle, FontWeight, MouseButton,
    MouseDownEvent, SharedString,
};
use serde_json::Value;

use crate::sidebar::{NoticeArea, NoticeArm};
use crate::text_field::TextField;
use crate::{hsla, rgba, TakoApp};
use tako_control::protocol::{FileOpKind, Request};
use tako_core::PaneOrigin;

/// 返答コメントの上限（文字数ではなくバイト長。日本語なら 1 文字 3 バイト程度）。
/// **上限に当たったら画面に 1 行出す**ので、黙って切り捨てにはならない
pub(crate) const COMMENT_MAX_BYTES: usize = 4000;

/// #1450 B2 の A/B。`TAKO_1450B2_LEGACY=1` で**同一バイナリのまま**
/// 「開いた瞬間に 1 回読むだけ」へ戻す（= 起票しても一覧に出てこない・
/// 配送が `sent` のまま固まる、の再現）。B1 の `TAKO_1450_LEGACY`（起票通知の抑止）
/// とは**別の軸**なので、片方のアームがもう片方の回帰を隠さない（#1422 の作法）
pub(crate) fn legacy_1450_b2() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var("TAKO_1450B2_LEGACY").map(|v| v == "1") == Ok(true))
}

/// 添付 1 件（パスと**今の**実在）。起票時に実在を問わない設計なので、
/// 「消えた添付」を画面に出せるのは `exists` を毎回読み直しているから
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Attachment {
    pub path: String,
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CopyText {
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResponseRow {
    pub decision: String,
    pub comment: String,
    pub at: i64,
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryRow {
    pub state: String,
    pub pane: Option<u64>,
    pub reason: Option<String>,
}

/// 画面が持つタスク 1 件（`Request::UserTask` の応答 JSON から起こす）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRow {
    pub id: String,
    pub title: String,
    pub body: String,
    pub kind: String,
    pub status: String,
    pub project: Option<String>,
    pub attachments: Vec<Attachment>,
    pub copy_texts: Vec<CopyText>,
    pub links: Vec<String>,
    pub due: Option<String>,
    pub updated_at: i64,
    pub responses: Vec<ResponseRow>,
    pub delivery: Option<DeliveryRow>,
}

/// `list` の応答 1 回ぶん
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TasksSnapshot {
    pub tasks: Vec<TaskRow>,
    /// バッジの正本。**画面で数え直さない**（B1 の `open_count` をそのまま出す）
    pub open_count: usize,
}

/// `list` の応答 JSON を画面の形へ起こす（純関数）。
///
/// 並びは **`updated_at` 降順**（返答が付いた・更新されたものが上に来る）。
/// 同じ時刻なら id の新しい順（`u-12` > `u-2` になるよう数値で比べる）
pub(crate) fn parse_list(value: &Value) -> TasksSnapshot {
    let tasks = value
        .get("tasks")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(parse_task).collect::<Vec<_>>())
        .unwrap_or_default();
    let mut tasks = tasks;
    tasks.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| id_ordinal(&b.id).cmp(&id_ordinal(&a.id)))
    });
    TasksSnapshot {
        tasks,
        open_count: value.get("open_count").and_then(Value::as_u64).unwrap_or(0) as usize,
    }
}

/// `u-12` → 12。綴りが違うものは 0（並びの安定だけに使う）
fn id_ordinal(id: &str) -> u64 {
    id.strip_prefix("u-")
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}

fn parse_task(value: &Value) -> TaskRow {
    let s = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let opt = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    TaskRow {
        id: s("id"),
        title: s("title"),
        body: s("body"),
        kind: s("kind"),
        status: s("status"),
        project: opt("project"),
        attachments: value
            .get("attachments")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|a| Attachment {
                        path: a
                            .get("path")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        exists: a.get("exists").and_then(Value::as_bool).unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        copy_texts: value
            .get("copy_texts")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|c| CopyText {
                        label: c
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        text: c
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        links: value
            .get("links")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        due: opt("due"),
        updated_at: value.get("updated_at").and_then(Value::as_i64).unwrap_or(0),
        responses: value
            .get("responses")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|r| ResponseRow {
                        decision: r
                            .get("decision")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        comment: r
                            .get("comment")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        at: r.get("at").and_then(Value::as_i64).unwrap_or(0),
                        via: r
                            .get("via")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        delivery: value.get("delivery").and_then(|d| {
            let state = d.get("state").and_then(Value::as_str)?.to_string();
            Some(DeliveryRow {
                state,
                pane: d.get("pane").and_then(Value::as_u64),
                reason: d
                    .get("reason")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string),
            })
        }),
    }
}

/// 一覧に出す並び（絞り込みを当てた眺め。純関数）。
///
/// 絞り込みは**取ってきた行の表示側**で当てる（`list` を kind ごとに撃ち直すと、
/// 絞り込みのチップに出す候補が「絞り込んだ結果」からしか作れなくなり、
/// 一度絞ると他の種類へ戻れなくなる）
pub(crate) fn visible_tasks<'a>(
    tasks: &'a [TaskRow],
    kind: Option<&str>,
    project: Option<&str>,
) -> Vec<&'a TaskRow> {
    tasks
        .iter()
        .filter(|t| kind.is_none_or(|k| t.kind == k))
        .filter(|t| project.is_none_or(|p| t.project.as_deref() == Some(p)))
        .collect()
}

/// 絞り込みチップに出す種類（**今ある行に出てくるものだけ**。空の選択肢を出さない）。
/// 並びは `TaskKind::all()` の宣言順に揃える（チップの位置が読み込みごとに動かない）
pub(crate) fn kind_choices(tasks: &[TaskRow]) -> Vec<String> {
    tako_core::user_task::TaskKind::all()
        .iter()
        .map(|k| k.as_str().to_string())
        .filter(|k| tasks.iter().any(|t| &t.kind == k))
        .collect()
}

/// 絞り込みチップに出すプロジェクト（**2 つ以上あるときだけ**画面に出す）
pub(crate) fn project_choices(tasks: &[TaskRow]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tasks {
        if let Some(p) = t.project.as_ref() {
            if !out.iter().any(|q| q == p) {
                out.push(p.clone());
            }
        }
    }
    out.sort();
    out
}

/// 画面が持つ状態（**正本ではない**）
#[derive(Debug, Default)]
pub(crate) struct TasksPanel {
    pub snapshot: TasksSnapshot,
    /// 選択中の id（`None` なら一覧の先頭）
    pub selected: Option<String>,
    pub kind_filter: Option<String>,
    pub project_filter: Option<String>,
    /// 完了・却下も一覧に出す（`list` の `all` に載る = 絞り込みではなく取得条件）
    pub show_done: bool,
    /// 返答フォームで選んだ判断（未選択のあいだは「返す」を押せない）
    pub decision: Option<String>,
    pub comment: TextField,
    pub comment_focused: bool,
    /// 一覧そのものが引けなかった理由（画面を空にして黙らないため）
    pub load_error: Option<String>,
    /// コピーした・上限に当たった等の 1 行（次の操作で消える）
    pub notice: Option<String>,
    /// 一度でも `list` が返ってきたか（読み込み前と 0 件を言い分ける）
    pub loaded: bool,
}

impl TasksPanel {
    /// 選択中のタスク（選択が消えていたら一覧の先頭へ落ちる）
    pub(crate) fn selected_task<'a>(&self, visible: &[&'a TaskRow]) -> Option<&'a TaskRow> {
        self.selected
            .as_ref()
            .and_then(|id| visible.iter().find(|t| &t.id == id).copied())
            .or_else(|| visible.first().copied())
    }

    /// 返答フォームを初期状態へ（タスクを選び直した / 返し終えた）
    pub(crate) fn reset_form(&mut self) {
        self.decision = None;
        self.comment.clear();
        self.comment_focused = false;
    }
}

impl TakoApp {
    /// 定期更新 1 回ぶん（**ポーリングの止め方はここ 1 か所**）。
    ///
    /// 既存の 2 秒ループから毎 tick 呼ばれる。撃つかどうかの判断をループ側に書かず
    /// ここへ置いてあるので、**「止める処理」がどこにも無いのに止まる**
    /// （右パネルを閉じた瞬間から空振りになる）。セルフテスト項目 150 も
    /// この関数を叩くので、検証が通る経路と本番の経路が同じ 1 本になる
    pub(crate) fn tick_user_tasks(&mut self) {
        // 画面が無いときは数えない（起票に気づく道は B1 の通知欄が持っている）
        if !self.panel_visible {
            return;
        }
        // A/B: 立てると「開いた瞬間に 1 回読むだけ」へ戻る
        if legacy_1450_b2() {
            return;
        }
        self.refresh_user_tasks();
    }

    /// 一覧を引き直す（**2 秒ループと、画面を触った直後の両方**から呼ばれる）。
    ///
    /// `Request::UserTask{list}` の 1 経路しか通らないので、CLI / MCP が返すものと
    /// 画面が出すものは同じ。失敗は画面に理由を残す（`load_error`）
    pub(crate) fn refresh_user_tasks(&mut self) {
        let all = self.user_tasks.show_done.then_some(true);
        let request = Request::UserTask {
            action: "list".to_string(),
            id: None,
            title: None,
            body: None,
            kind: None,
            status: None,
            all,
            project: None,
            attachments: None,
            copy_texts: None,
            links: None,
            due: None,
            decision: None,
            comment: None,
            via: None,
            pane: None,
            caller_role: None,
        };
        match tako_control::dispatch(self, request, PaneOrigin::User) {
            Ok(value) => {
                self.user_tasks.snapshot = parse_list(&value);
                self.user_tasks.loaded = true;
                self.user_tasks.load_error = None;
            }
            Err(e) => {
                // 読めなかったときは**前回の眺めを消さない**（一覧が一瞬で空になると
                // 「全部片付いた」に見える）。理由だけを画面へ足す
                self.user_tasks.load_error = Some(e.to_string());
                self.notify_ui_dispatch_failed(
                    NoticeArea::UserTasks,
                    NoticeArm::Issue1450B2,
                    "user_task_list",
                    None,
                    &e,
                );
            }
        }
    }

    /// タスクの状態を変える（完了 / 却下）。成功したらその場で一覧を引き直す
    pub(crate) fn user_task_set_status(&mut self, id: &str, action: &str, cx: &mut Context<Self>) {
        let request = Request::UserTask {
            action: action.to_string(),
            id: Some(id.to_string()),
            title: None,
            body: None,
            kind: None,
            status: None,
            all: None,
            project: None,
            attachments: None,
            copy_texts: None,
            links: None,
            due: None,
            decision: None,
            comment: None,
            via: None,
            pane: None,
            caller_role: None,
        };
        match tako_control::dispatch(self, request, PaneOrigin::User) {
            Ok(_) => {
                self.user_tasks.selected = None;
                self.user_tasks.reset_form();
                self.refresh_user_tasks();
            }
            Err(e) => self.notify_ui_dispatch_failed(
                NoticeArea::UserTasks,
                NoticeArm::Issue1450B2,
                if action == "done" {
                    "user_task_done"
                } else {
                    "user_task_dismiss"
                },
                Some(id),
                &e,
            ),
        }
        cx.notify();
    }

    /// 返答を送る（`respond` = 積む + 起票 master へ配送。配送は B1 の 1 実装）
    pub(crate) fn user_task_respond(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(decision) = self.user_tasks.decision.clone() else {
            return;
        };
        let comment = self.user_tasks.comment.text().to_string();
        let request = Request::UserTask {
            action: "respond".to_string(),
            id: Some(id.to_string()),
            title: None,
            body: None,
            kind: None,
            status: None,
            all: None,
            project: None,
            attachments: None,
            copy_texts: None,
            links: None,
            due: None,
            decision: Some(decision),
            comment: Some(comment),
            // **どこから返したか**が残る（PWA = B3 は "pwa"）
            via: Some("pc".to_string()),
            pane: None,
            caller_role: None,
        };
        match tako_control::dispatch(self, request, PaneOrigin::User) {
            Ok(_) => {
                self.user_tasks.reset_form();
                // 返した直後の配送状態（多くは `sent`）をすぐ出す。確定はポーリングが行う
                self.refresh_user_tasks();
            }
            Err(e) => self.notify_ui_dispatch_failed(
                NoticeArea::UserTasks,
                NoticeArm::Issue1450B2,
                "user_task_respond",
                Some(id),
                &e,
            ),
        }
        cx.notify();
    }

    /// `copy_texts[index]` をクリップボードへ入れる（**押した 1 件だけ**）。
    ///
    /// dispatch を通らない 2 つの例外のうちの 1 つ（GPUI の `App` が要る。
    /// `tako chat copy` / `tako preview-copy-code` も同じ理由で実書き込みは render 側）。
    /// クリックとセルフテストが同じここを通るので、「押したのに別の本文が入る」を
    /// 機械で押さえられる。**入れられなかったら false**（無言で成功と言わない）
    pub(crate) fn user_task_copy(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let visible = visible_tasks(
            &self.user_tasks.snapshot.tasks,
            self.user_tasks.kind_filter.as_deref(),
            self.user_tasks.project_filter.as_deref(),
        );
        let Some(copy) = self
            .user_tasks
            .selected_task(&visible)
            .and_then(|t| t.copy_texts.get(index))
            .cloned()
        else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(copy.text));
        self.user_tasks.notice = Some(crate::ui_text::panel::tasks_copied(&copy.label));
        cx.notify();
        true
    }

    /// 添付をファイルマネージャで表示する（`tako_file_op` と同じ dispatch）
    fn user_task_reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        let request = Request::FileOp {
            op: FileOpKind::Reveal,
            path: path.to_string(),
            name: None,
            pane: None,
        };
        if let Err(e) = tako_control::dispatch(self, request, PaneOrigin::User) {
            self.notify_ui_dispatch_failed(
                NoticeArea::UserTasks,
                NoticeArm::Issue1450B2,
                "user_task_reveal",
                Some(path),
                &e,
            );
        }
        cx.notify();
    }

    /// 添付をプレビューペインで開く（`tako open <file>` と同じ dispatch）
    fn user_task_open_preview(&mut self, path: &str, cx: &mut Context<Self>) {
        let request = Request::OpenFile {
            pane: None,
            path: path.to_string(),
            mode: None,
            direction: None,
            focus: Some(false),
            new_tab: false,
        };
        match tako_control::dispatch(self, request, PaneOrigin::User) {
            Ok(_) => {
                // UI 経路は PTY 起動待ちを自分で消化する（#1023 の不変条件）。
                // 取り付けに失敗したらプレビューが永久に出ないので、これも黙らせない
                if let Err(reason) = self.attach_pending_sessions(cx) {
                    self.notify_ui_op_failed(
                        NoticeArea::UserTasks,
                        NoticeArm::Issue1450B2,
                        "user_task_open_file",
                        Some(path),
                        &reason,
                    );
                }
            }
            Err(e) => self.notify_ui_dispatch_failed(
                NoticeArea::UserTasks,
                NoticeArm::Issue1450B2,
                "user_task_open_file",
                Some(path),
                &e,
            ),
        }
        cx.notify();
    }

    /// リンクを開く。http / https は既定ブラウザ、それ以外（パス）は tako の中で開く。
    /// **端末リンク・PDF・md プレビューと同じ 1 実装**（`os_integration::open_url`）
    fn user_task_open_link(&mut self, link: &str, cx: &mut Context<Self>) {
        match tako_core::url_guard::check_browser_url(link) {
            Ok(url) => {
                if let Err(e) = tako_control::platform::os_integration::open_url(url) {
                    self.notify_ui_op_failed(
                        NoticeArea::UserTasks,
                        NoticeArm::Issue1450B2,
                        "user_task_open_link",
                        Some(link),
                        &e,
                    );
                }
            }
            Err(_) => {
                // URL でないもの（ローカルパス）は端末リンクと同じ扱いで tako の中へ
                let pane = self.focused_pane();
                self.open_path_in_tako(link, pane, cx);
            }
        }
        cx.notify();
    }

    /// 返答コメント欄へ文字を入れる（打鍵と IME 確定文字列の共通経路）
    pub(crate) fn task_comment_insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if !self
            .user_tasks
            .comment
            .insert(text, COMMENT_MAX_BYTES, true)
        {
            // 上限に当たったことを黙らない（打ったのに増えない、を説明する）
            self.user_tasks.notice = Some(crate::ui_text::panel::tasks_comment_limit(
                COMMENT_MAX_BYTES,
            ));
        }
        cx.notify();
    }

    /// 返答コメント欄のキー処理。扱ったら `true`（呼び出し側が打鍵を消費する）
    pub(crate) fn handle_task_comment_key(
        &mut self,
        keystroke: &gpui::Keystroke,
        cx: &mut Context<Self>,
    ) -> bool {
        match keystroke.key.as_str() {
            // ⌘Enter で送る（git のコミット欄と同じ割り当て）
            "enter" if keystroke.modifiers.platform => {
                if let Some(id) = self.selected_user_task_id() {
                    self.user_task_respond(&id, cx);
                }
                true
            }
            "enter" => {
                self.task_comment_insert("\n", cx);
                true
            }
            "escape" => {
                self.user_tasks.comment_focused = false;
                cx.notify();
                true
            }
            "v" if keystroke.modifiers.platform => {
                // ⌘V はクリップボード読み出しの共通経路（`paste`）へ任せる
                self.paste(cx);
                true
            }
            key => {
                if self.user_tasks.comment.handle_edit_key(key) {
                    cx.notify();
                    return true;
                }
                // 印字文字はここで入れる（修飾キー付きはショートカットなので通す）
                if keystroke.modifiers.platform || keystroke.modifiers.control {
                    return false;
                }
                if let Some(text) = keystroke.key_char.as_ref().filter(|t| !t.is_empty()) {
                    let text = text.clone();
                    self.task_comment_insert(&text, cx);
                    return true;
                }
                // 未知のキーでもコメント欄がフォーカスされている間はターミナルへ漏らさない
                true
            }
        }
    }

    /// いま詳細に出ているタスクの id（返答・完了の宛先）
    pub(crate) fn selected_user_task_id(&self) -> Option<String> {
        let visible = visible_tasks(
            &self.user_tasks.snapshot.tasks,
            self.user_tasks.kind_filter.as_deref(),
            self.user_tasks.project_filter.as_deref(),
        );
        self.user_tasks
            .selected_task(&visible)
            .map(|t| t.id.clone())
    }

    /// tasks ビュー本体
    pub(crate) fn render_tasks_view(
        &mut self,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let tasks = self.user_tasks.snapshot.tasks.clone();
        let kind_filter = self.user_tasks.kind_filter.clone();
        let project_filter = self.user_tasks.project_filter.clone();
        let visible: Vec<TaskRow> =
            visible_tasks(&tasks, kind_filter.as_deref(), project_filter.as_deref())
                .into_iter()
                .cloned()
                .collect();
        let visible_refs: Vec<&TaskRow> = visible.iter().collect();
        let selected = self.user_tasks.selected_task(&visible_refs).cloned();
        let kinds = kind_choices(&tasks);
        let projects = project_choices(&tasks);
        let open_count = self.user_tasks.snapshot.open_count;
        let show_done = self.user_tasks.show_done;
        let load_error = self.user_tasks.load_error.clone();
        let notice = self.user_tasks.notice.clone();
        let loaded = self.user_tasks.loaded;

        let chip = |label: String, id: (&'static str, usize), active: bool| {
            div()
                .id(id)
                .flex_none()
                .px(px(7.0))
                .py(px(2.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .text_size(px(10.5))
                .border_1()
                .border_color(hsla(if active {
                    theme.accent_border_muted
                } else {
                    theme.border_inner
                }))
                .bg(rgba(if active {
                    theme.accent_muted
                } else {
                    theme.chip_surface
                }))
                .text_color(hsla(if active {
                    theme.foreground
                } else {
                    theme.text_muted
                }))
                .child(SharedString::from(label))
        };

        let mut root = div()
            .id("panel-tasks-view")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_color(hsla(theme.foreground));

        // --- ヘッダ（見出し + 未完了件数。件数は B1 の open_count がそのまま） ---
        root = root.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .pt(px(12.0))
                .px(px(14.0))
                .pb(px(6.0))
                .flex_none()
                .child(
                    div()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::BOLD)
                        .text_color(hsla(theme.text_muted))
                        .child("USER TASKS"),
                )
                .child(div().flex_grow(1.0))
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(hsla(theme.text_faint))
                        .child(SharedString::from(crate::ui_text::panel::tasks_open_count(
                            open_count,
                        ))),
                ),
        );

        // --- 絞り込み（種類。候補が 2 つ以上あるときだけ出す） ---
        if kinds.len() > 1 {
            let mut row = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(px(4.0))
                .px(px(10.0))
                .pb(px(4.0))
                .flex_none()
                .child(
                    chip(
                        crate::ui_text::panel::tasks_filter_all().to_string(),
                        ("tasks-kind-chip", usize::MAX),
                        kind_filter.is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.user_tasks.kind_filter = None;
                        cx.notify();
                    })),
                );
            for (i, kind) in kinds.iter().enumerate() {
                let k = kind.clone();
                row = row.child(
                    chip(
                        crate::ui_text::panel::tasks_kind(kind),
                        ("tasks-kind-chip", i),
                        kind_filter.as_deref() == Some(kind.as_str()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.user_tasks.kind_filter = Some(k.clone());
                        cx.notify();
                    })),
                );
            }
            root = root.child(row);
        }

        // --- 絞り込み（プロジェクト。2 つ以上あるときだけ） ---
        if projects.len() > 1 {
            let mut row = div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(px(4.0))
                .px(px(10.0))
                .pb(px(4.0))
                .flex_none()
                .child(
                    chip(
                        crate::ui_text::panel::tasks_filter_all().to_string(),
                        ("tasks-project-chip", usize::MAX),
                        project_filter.is_none(),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.user_tasks.project_filter = None;
                        cx.notify();
                    })),
                );
            for (i, project) in projects.iter().enumerate() {
                let p = project.clone();
                row = row.child(
                    chip(
                        project.clone(),
                        ("tasks-project-chip", i),
                        project_filter.as_deref() == Some(project.as_str()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.user_tasks.project_filter = Some(p.clone());
                        cx.notify();
                    })),
                );
            }
            root = root.child(row);
        }

        // --- 完了も見るトグル（これは絞り込みではなく `list` の取得条件） ---
        root = root.child(
            div()
                .flex()
                .flex_row()
                .px(px(10.0))
                .pb(px(6.0))
                .flex_none()
                .child(
                    chip(
                        crate::ui_text::panel::tasks_show_done().to_string(),
                        ("tasks-show-done", 0),
                        show_done,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.user_tasks.show_done = !this.user_tasks.show_done;
                        this.refresh_user_tasks();
                        cx.notify();
                    })),
                ),
        );

        // --- 通知（コピーした・上限に当たった） ---
        if let Some(text) = notice {
            root = root.child(
                div()
                    .flex_none()
                    .px(px(14.0))
                    .pb(px(4.0))
                    .text_size(px(10.5))
                    .text_color(hsla(theme.text_muted))
                    .child(SharedString::from(text)),
            );
        }
        // --- 一覧が引けなかった理由（画面を空にして黙らない） ---
        if let Some(reason) = load_error {
            root = root.child(
                div()
                    .flex_none()
                    .px(px(14.0))
                    .pb(px(4.0))
                    .text_size(px(10.5))
                    .text_color(hsla(theme.red))
                    .child(SharedString::from(
                        crate::ui_text::panel::tasks_load_failed(&reason),
                    )),
            );
        }

        // --- 一覧 + 詳細（縦 1 列。パネルは 300px 前後なので左右に割らない） ---
        let mut scroll = div()
            .id("tasks-scroll")
            .flex_1()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .px(px(8.0))
            .pb(px(8.0));

        if visible.is_empty() && loaded {
            let text = if tasks.is_empty() {
                crate::ui_text::panel::tasks_empty()
            } else {
                crate::ui_text::panel::tasks_empty_filtered()
            };
            scroll = scroll.child(
                div()
                    .px(px(6.0))
                    .py(px(8.0))
                    .text_size(px(11.5))
                    .text_color(hsla(theme.text_muted))
                    .child(text),
            );
        }

        for (i, task) in visible.iter().enumerate() {
            let is_selected = selected.as_ref().is_some_and(|s| s.id == task.id);
            let id = task.id.clone();
            let done = task.status != "open";
            scroll = scroll.child(
                div()
                    .id(("tasks-row", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .flex_none()
                    .px(px(6.0))
                    .py(px(5.0))
                    .mb(px(2.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .bg(rgba(if is_selected {
                        theme.surface_2
                    } else {
                        theme.surface_0
                    }))
                    .hover(|d| d.bg(rgba(theme.surface_hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.user_tasks.selected.as_deref() != Some(id.as_str()) {
                            this.user_tasks.selected = Some(id.clone());
                            this.user_tasks.reset_form();
                            this.user_tasks.notice = None;
                        }
                        cx.notify();
                    }))
                    // 状態の丸（図形。絵文字は使わない）
                    .child(
                        div()
                            .flex_none()
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgba(kind_color(&theme, &task.kind, done))),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(9.5))
                            .text_color(hsla(theme.text_faint))
                            .child(SharedString::from(task.id.clone())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(px(11.5))
                            .text_color(hsla(if done {
                                theme.text_muted
                            } else {
                                theme.foreground
                            }))
                            .child(SharedString::from(task.title.clone())),
                    )
                    .when(task.delivery.is_some(), |d| {
                        let state = task
                            .delivery
                            .as_ref()
                            .map(|x| x.state.clone())
                            .unwrap_or_default();
                        d.child(
                            div()
                                .flex_none()
                                .w(px(6.0))
                                .h(px(6.0))
                                .rounded_full()
                                .bg(rgba(delivery_color(&theme, &state))),
                        )
                    }),
            );
        }

        if let Some(task) = selected {
            scroll = scroll.child(self.render_task_detail(&task, cx));
        }

        root.child(scroll)
    }
}

/// 種類ごとの色（図形の丸に塗る。文字色は変えない）
fn kind_color(theme: &tako_core::theme::Theme, kind: &str, done: bool) -> tako_core::Rgb {
    if done {
        return theme.text_faint;
    }
    match kind {
        "review" => theme.accent,
        "confirm" => theme.teal,
        "permission" => theme.peach,
        "post" => theme.mauve,
        _ => theme.text_muted,
    }
}

/// 配送の色。**`sent` は「未確定」の色**（緑にしない = 届いたと騙らない）
fn delivery_color(theme: &tako_core::theme::Theme, state: &str) -> tako_core::Rgb {
    match state {
        "delivered" => theme.green,
        "failed" => theme.red,
        _ => theme.yellow,
    }
}

impl TakoApp {
    /// 詳細（本文・添付・コピー・リンク・返答フォーム・やりとり・完了 / 却下）
    fn render_task_detail(
        &mut self,
        task: &TaskRow,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = self.theme.clone();
        let decision = self.user_tasks.decision.clone();
        let comment = self.user_tasks.comment.text().to_string();
        let comment_focused = self.user_tasks.comment_focused;
        let caret = self.user_tasks.comment.cursor();
        let now = tako_core::user_task::unix_now();
        let id = task.id.clone();

        let section = |label: String| {
            div()
                .flex_none()
                .pt(px(8.0))
                .pb(px(3.0))
                .text_size(px(10.0))
                .font_weight(FontWeight::BOLD)
                .text_color(hsla(theme.text_faint))
                .child(SharedString::from(label))
        };

        let mut detail = div()
            .id("tasks-detail")
            .flex_none()
            .flex()
            .flex_col()
            .mt(px(6.0))
            .p(px(8.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(hsla(theme.border_strong))
            .bg(rgba(theme.surface_1));

        // --- 見出し（種類・id・期限） ---
        detail = detail.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.0))
                .flex_none()
                .child(
                    div()
                        .flex_none()
                        .px(px(5.0))
                        .rounded(px(4.0))
                        .bg(rgba(theme.chip_surface))
                        .text_size(px(9.5))
                        .text_color(hsla(theme.text_muted))
                        .child(SharedString::from(crate::ui_text::panel::tasks_kind(
                            &task.kind,
                        ))),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(9.5))
                        .text_color(hsla(theme.text_faint))
                        .child(SharedString::from(task.id.clone())),
                )
                .when_some(task.due.clone(), |d, due| {
                    d.child(
                        div()
                            .flex_none()
                            .text_size(px(9.5))
                            .text_color(hsla(theme.peach))
                            .child(SharedString::from(crate::ui_text::panel::tasks_due(&due))),
                    )
                }),
        );
        detail = detail.child(
            div()
                .flex_none()
                .pt(px(3.0))
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .child(SharedString::from(task.title.clone())),
        );

        // --- 本文（markdown。プレビュー / アップデート画面と同じ 1 実装で描く） ---
        if task.body.trim().is_empty() {
            detail = detail.child(
                div()
                    .flex_none()
                    .pt(px(4.0))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.text_muted))
                    .child(crate::ui_text::panel::tasks_body_empty()),
            );
        } else {
            let blocks = crate::preview::markdown_blocks(&task.body);
            let (elements, _layouts) = crate::md_view::render_document(&theme, &blocks, None);
            detail = detail.child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .pt(px(4.0))
                    .text_size(px(11.0))
                    .children(elements),
            );
        }

        // --- 添付（消えたものは「見つかりません」を出す） ---
        if !task.attachments.is_empty() {
            detail = detail.child(section(
                crate::ui_text::panel::tasks_attachments().to_string(),
            ));
            for (i, att) in task.attachments.iter().enumerate() {
                let path_reveal = att.path.clone();
                let path_open = att.path.clone();
                let shown = short_path(&att.path);
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(5.0))
                    .flex_none()
                    .py(px(2.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            // 狭いパネルで長いパスが折り返して行が膨らむのを止める
                            // （末尾 2 要素にしても /var/folders/… の一時パスは長い）
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(10.5))
                            .text_color(hsla(if att.exists {
                                theme.text_secondary
                            } else {
                                theme.text_faint
                            }))
                            .child(SharedString::from(shown)),
                    );
                if att.exists {
                    row = row
                        .child(
                            small_button(
                                &theme,
                                ("tasks-att-reveal", i),
                                crate::ui_text::pane_menu::reveal(
                                    tako_control::platform::os_integration::file_manager(),
                                )
                                .to_string(),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.user_task_reveal(&path_reveal, cx);
                                },
                            )),
                        )
                        .child(
                            small_button(
                                &theme,
                                ("tasks-att-open", i),
                                crate::ui_text::panel::tasks_open_preview().to_string(),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.user_task_open_preview(&path_open, cx);
                                },
                            )),
                        );
                } else {
                    row = row.child(
                        div()
                            .flex_none()
                            .text_size(px(9.5))
                            .text_color(hsla(theme.red))
                            .child(crate::ui_text::panel::tasks_attachment_missing()),
                    );
                }
                detail = detail.child(row);
            }
        }

        // --- コピー用テキスト（貼り先ごとに 1 つずつ入れる） ---
        if !task.copy_texts.is_empty() {
            detail = detail.child(section(
                crate::ui_text::panel::tasks_copy_texts().to_string(),
            ));
            let mut row = div().flex().flex_row().flex_wrap().gap(px(4.0)).flex_none();
            for (i, copy) in task.copy_texts.iter().enumerate() {
                let shown = if copy.label.is_empty() {
                    crate::ui_text::panel::tasks_copy_button().to_string()
                } else {
                    copy.label.clone()
                };
                row = row.child(small_button(&theme, ("tasks-copy", i), shown).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.user_task_copy(i, cx);
                    }),
                ));
            }
            detail = detail.child(row);
        }

        // --- リンク ---
        if !task.links.is_empty() {
            detail = detail.child(section(crate::ui_text::panel::tasks_links().to_string()));
            for (i, link) in task.links.iter().enumerate() {
                let target = link.clone();
                detail = detail.child(
                    div()
                        .id(("tasks-link", i))
                        .flex_none()
                        .py(px(1.0))
                        .cursor_pointer()
                        .text_size(px(10.5))
                        .text_color(hsla(theme.accent))
                        .child(SharedString::from(link.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.user_task_open_link(&target, cx);
                        })),
                );
            }
        }

        // --- 返答フォーム（判断チップ + コメント + 返す） ---
        detail = detail.child(section(
            crate::ui_text::panel::tasks_respond_heading().to_string(),
        ));
        let mut chips = div().flex().flex_row().flex_wrap().gap(px(4.0)).flex_none();
        for (i, d) in tako_core::user_task::Decision::all().iter().enumerate() {
            let key = d.as_str().to_string();
            let active = decision.as_deref() == Some(d.as_str());
            let pick = key.clone();
            chips = chips.child(
                div()
                    .id(("tasks-decision", i))
                    .flex_none()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .text_size(px(10.5))
                    .border_1()
                    .border_color(hsla(if active {
                        theme.accent_border_muted
                    } else {
                        theme.border_inner
                    }))
                    .bg(rgba(if active {
                        theme.accent_muted
                    } else {
                        theme.chip_surface
                    }))
                    .text_color(hsla(if active {
                        theme.foreground
                    } else {
                        theme.text_muted
                    }))
                    .child(SharedString::from(crate::ui_text::panel::tasks_decision(
                        &key,
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.user_tasks.decision = Some(pick.clone());
                        this.user_tasks.notice = None;
                        cx.notify();
                    })),
            );
        }
        detail = detail.child(chips);

        // コメント欄（クリックでフォーカス。打鍵は handle_task_comment_key が受ける）。
        // キャレットは**行ごとに**描く（git のコミット欄は 1 行なので前後を split して
        // 済むが、ここは改行を受けるので「キャレットが載っている行」だけを split する）
        let mut box_ = div()
            .id("tasks-comment")
            .flex_none()
            .flex()
            .flex_col()
            .mt(px(4.0))
            .px(px(6.0))
            .py(px(4.0))
            .min_h(px(40.0))
            .rounded(px(6.0))
            .cursor(CursorStyle::IBeam)
            .border_1()
            .border_color(hsla(if comment_focused {
                theme.accent_border_muted
            } else {
                theme.border_inner
            }))
            .bg(rgba(theme.surface_0))
            .text_size(px(11.0))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    // #496: 押した瞬間にルートの dismiss で自分が消えないよう伝播を止める
                    this.clear_text_input_focus();
                    this.user_tasks.comment_focused = true;
                    this.user_tasks.comment.move_end();
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
        if comment.is_empty() && !comment_focused {
            box_ = box_.child(
                div()
                    .text_color(hsla(theme.text_faint))
                    .child(crate::ui_text::panel::tasks_comment_placeholder()),
            );
        } else {
            for (i, (line, split)) in caret_lines(&comment, caret).into_iter().enumerate() {
                let mut row = div().flex().flex_row().items_center().flex_none();
                match split.filter(|_| comment_focused) {
                    Some(at) => {
                        let (before, after) = line.split_at(at);
                        row = row
                            .child(
                                div()
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(before.to_string())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .w(px(1.5))
                                    .h(px(13.0))
                                    .bg(rgba(theme.accent)),
                            )
                            .child(
                                div()
                                    .text_color(hsla(theme.foreground))
                                    .child(SharedString::from(after.to_string())),
                            );
                    }
                    None => {
                        row = row.child(
                            div()
                                .text_color(hsla(theme.foreground))
                                .child(SharedString::from(line.to_string())),
                        );
                    }
                }
                box_ = box_.child(row.id(("tasks-comment-line", i)));
            }
        }
        detail = detail.child(box_);

        // 送信ボタン（判断が未選択なら押せない = 誤爆防止。理由も出す）
        let can_send = decision.is_some();
        let send_id = id.clone();
        detail = detail.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .flex_none()
                .mt(px(4.0))
                .child(
                    div()
                        .id("tasks-send")
                        .flex_none()
                        .px(px(9.0))
                        .py(px(3.0))
                        .rounded(px(6.0))
                        .text_size(px(11.0))
                        .when(can_send, |d| {
                            d.cursor_pointer()
                                .bg(rgba(theme.accent_muted))
                                .border_color(hsla(theme.accent_border_muted))
                                .text_color(hsla(theme.foreground))
                        })
                        .when(!can_send, |d| {
                            d.bg(rgba(theme.chip_surface))
                                .border_color(hsla(theme.border_inner))
                                .text_color(hsla(theme.text_faint))
                        })
                        .border_1()
                        .child(crate::ui_text::panel::tasks_send())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.user_tasks.decision.is_some() {
                                this.user_task_respond(&send_id, cx);
                            }
                        })),
                )
                .when(!can_send, |d| {
                    d.child(
                        div()
                            .flex_none()
                            .text_size(px(9.5))
                            .text_color(hsla(theme.text_faint))
                            .child(crate::ui_text::panel::tasks_pick_decision()),
                    )
                }),
        );

        // --- 配送状態（sent を「届いた」と書かない。失敗は理由つき） ---
        if let Some(delivery) = task.delivery.as_ref() {
            let mut line = format!(
                "{}: {}",
                crate::ui_text::panel::tasks_delivery_label(),
                crate::ui_text::panel::tasks_delivery(&delivery.state)
            );
            if let Some(pane) = delivery.pane {
                line.push_str(&format!(" (pane {pane})"));
            }
            if let Some(reason) = delivery.reason.as_ref() {
                line.push_str(&format!(" - {reason}"));
            }
            detail = detail.child(
                div()
                    .flex_none()
                    .mt(px(4.0))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(5.0))
                    .child(
                        div()
                            .flex_none()
                            .mt(px(4.0))
                            .w(px(6.0))
                            .h(px(6.0))
                            .rounded_full()
                            .bg(rgba(delivery_color(&theme, &delivery.state))),
                    )
                    .child(
                        // 理由は長い（`プロファイル '…' が見つからない: <パス>`）ので、
                        // 縮められる箱にして**折り返させる**（min_w を 0 にしないと
                        // flex が縮めてくれず、パネルの外へ溢れて読めなくなる）
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_muted))
                            .child(SharedString::from(line)),
                    ),
            );
        }

        // --- やりとり（スレッド。新しい順） ---
        if !task.responses.is_empty() {
            detail = detail.child(section(crate::ui_text::panel::tasks_thread().to_string()));
            for (i, r) in task.responses.iter().enumerate().rev() {
                detail = detail.child(
                    div()
                        .id(("tasks-response", i))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .py(px(2.0))
                        .child(
                            div()
                                .text_size(px(9.5))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(format!(
                                    "{} / {} / {}",
                                    crate::ui_text::panel::tasks_decision(&r.decision),
                                    r.via,
                                    crate::ui_text::panel::tasks_ago(now - r.at)
                                ))),
                        )
                        .when(!r.comment.is_empty(), |d| {
                            d.child(
                                div()
                                    .text_size(px(10.5))
                                    .text_color(hsla(theme.text_secondary))
                                    .child(SharedString::from(r.comment.clone())),
                            )
                        }),
                );
            }
        }

        // --- 完了 / 却下 ---
        let done_id = id.clone();
        let dismiss_id = id;
        detail.child(
            div()
                .flex()
                .flex_row()
                .gap(px(5.0))
                .flex_none()
                .pt(px(8.0))
                .child(
                    small_button(
                        &theme,
                        ("tasks-done", 0),
                        crate::ui_text::panel::tasks_done().to_string(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.user_task_set_status(&done_id, "done", cx);
                    })),
                )
                .child(
                    small_button(
                        &theme,
                        ("tasks-dismiss", 0),
                        crate::ui_text::panel::tasks_dismiss().to_string(),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.user_task_set_status(&dismiss_id, "dismiss", cx);
                    })),
                ),
        )
    }
}

/// 小さなボタン（右パネルのチップと同じ見た目。アイコンは使わず字だけ）
fn small_button(
    theme: &tako_core::theme::Theme,
    id: (&'static str, usize),
    label: String,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(6.0))
        .py(px(2.0))
        .rounded(px(5.0))
        .cursor_pointer()
        .text_size(px(10.0))
        .border_1()
        .border_color(hsla(theme.border_inner))
        .bg(rgba(theme.chip_surface))
        .text_color(hsla(theme.text_secondary))
        .child(SharedString::from(label))
}

/// 本文を行へ割り、**キャレットが載っている行**にだけ行内オフセットを付けて返す。
///
/// キャレットのバイトオフセットは本文全体のものなので、行に割った後は
/// 「どの行の、その行の何バイト目か」へ写し替える必要がある。改行そのものの上に
/// キャレットが在るとき（行末）は**その行の末尾**に付ける（次の行の先頭にしない）
fn caret_lines(text: &str, caret: usize) -> Vec<(&str, Option<usize>)> {
    let mut out: Vec<(&str, Option<usize>)> = Vec::new();
    let mut offset = 0usize;
    let mut placed = false;
    for line in text.split('\n') {
        let end = offset + line.len();
        let split = if !placed && caret <= end {
            placed = true;
            Some(caret.saturating_sub(offset).min(line.len()))
        } else {
            None
        };
        out.push((line, split));
        offset = end + 1; // 改行 1 バイトぶん
    }
    // 本文より後ろを指していたら（丸め前の値が来たとき）最終行の末尾へ落とす。
    // **キャレットが 1 つも描かれない**のが一番まずい（打っている場所が消える）
    if !placed {
        if let Some(last) = out.last_mut() {
            last.1 = Some(last.0.len());
        }
    }
    out
}

/// 狭いパネルに収まる形へ縮めたパス（末尾 2 要素だけ出す）。
/// **ホームパスを隠す意図ではない**（画面はユーザー自身のもの）が、
/// 300px に長い絶対パスを流し込むと行が潰れるので末尾を優先する
fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.rsplit(std::path::MAIN_SEPARATOR).collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].to_string(),
        _ => format!("{}{}{}", parts[1], std::path::MAIN_SEPARATOR, parts[0]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task_json(id: &str, kind: &str, updated_at: i64) -> Value {
        json!({
            "id": id,
            "title": format!("title {id}"),
            "body": "",
            "kind": kind,
            "status": "open",
            "attachments": [],
            "copy_texts": [],
            "links": [],
            "created_at": 1,
            "updated_at": updated_at,
            "responses": [],
        })
    }

    #[test]
    fn 一覧はupdated_at降順で並ぶ() {
        let value = json!({
            "tasks": [task_json("u-1", "review", 10), task_json("u-2", "post", 30), task_json("u-3", "confirm", 20)],
            "open_count": 3,
        });
        let snap = parse_list(&value);
        let ids: Vec<&str> = snap.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["u-2", "u-3", "u-1"]);
        assert_eq!(snap.open_count, 3);
    }

    /// 同じ時刻なら id の**新しい順**（一覧の並びが読み込みごとに揺れない）
    #[test]
    fn 同時刻はidの新しい順で安定する() {
        let value = json!({
            "tasks": [task_json("u-2", "review", 5), task_json("u-12", "review", 5), task_json("u-7", "review", 5)],
            "open_count": 3,
        });
        let snap = parse_list(&value);
        let ids: Vec<&str> = snap.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["u-12", "u-7", "u-2"]);
    }

    /// バッジの正本は `open_count`。**画面で数え直さない**ので、
    /// `--all` で完了も混ざった一覧でも未完了の件数が出る
    #[test]
    fn バッジは行数ではなくopen_countを出す() {
        let mut done = task_json("u-9", "post", 40);
        done["status"] = json!("done");
        let value = json!({ "tasks": [task_json("u-1", "review", 10), done], "open_count": 1 });
        let snap = parse_list(&value);
        assert_eq!(snap.tasks.len(), 2);
        assert_eq!(snap.open_count, 1);
    }

    #[test]
    fn 添付の実在とコピー用テキストと配送を読み取る() {
        let value = json!({
            "tasks": [{
                "id": "u-4",
                "title": "投稿",
                "body": "# 見出し",
                "kind": "post",
                "status": "open",
                "project": "tako",
                "attachments": [
                    { "path": "/tmp/a.mp4", "exists": true },
                    { "path": "/tmp/gone.png", "exists": false },
                ],
                "copy_texts": [{ "label": "本文", "text": "hello" }],
                "links": ["https://example.com"],
                "due": "2026-09-20",
                "created_at": 1,
                "updated_at": 2,
                "responses": [
                    { "decision": "needs_change", "comment": "直して", "at": 3, "via": "pwa" },
                ],
                "delivery": { "state": "failed", "reason": "profile 不明", "pane": null },
            }],
            "open_count": 1,
        });
        let snap = parse_list(&value);
        let t = &snap.tasks[0];
        assert_eq!(t.attachments.len(), 2);
        assert!(t.attachments[0].exists);
        assert!(!t.attachments[1].exists);
        assert_eq!(t.copy_texts[0].label, "本文");
        assert_eq!(t.copy_texts[0].text, "hello");
        assert_eq!(t.links, ["https://example.com"]);
        assert_eq!(t.due.as_deref(), Some("2026-09-20"));
        assert_eq!(t.project.as_deref(), Some("tako"));
        assert_eq!(t.responses[0].decision, "needs_change");
        assert_eq!(t.responses[0].via, "pwa");
        let d = t.delivery.as_ref().expect("配送が読める");
        assert_eq!(d.state, "failed");
        assert_eq!(d.reason.as_deref(), Some("profile 不明"));
        assert_eq!(d.pane, None);
    }

    /// 応答が壊れていても画面は落ちない（0 件として出す）
    #[test]
    fn 応答が想定外でも落ちない() {
        assert_eq!(parse_list(&json!({})).tasks.len(), 0);
        assert_eq!(parse_list(&json!({ "tasks": "x" })).tasks.len(), 0);
        let partial = parse_list(&json!({ "tasks": [{ "id": "u-1" }] }));
        assert_eq!(partial.tasks[0].title, "");
        assert!(partial.tasks[0].delivery.is_none());
    }

    #[test]
    fn 絞り込みは種類とプロジェクトの両方に効く() {
        let mut a = parse_list(&json!({ "tasks": [
            task_json("u-1", "review", 3),
            task_json("u-2", "post", 2),
            task_json("u-3", "review", 1),
        ]}))
        .tasks;
        a[0].project = Some("tako".into());
        a[2].project = Some("other".into());
        assert_eq!(visible_tasks(&a, None, None).len(), 3);
        assert_eq!(visible_tasks(&a, Some("review"), None).len(), 2);
        assert_eq!(visible_tasks(&a, Some("review"), Some("tako")).len(), 1);
        assert_eq!(visible_tasks(&a, Some("post"), Some("tako")).len(), 0);
    }

    /// チップは**今ある行に出てくる種類だけ**、並びは宣言順（位置が動かない）
    #[test]
    fn 種類チップは存在するものだけを宣言順で出す() {
        let tasks = parse_list(&json!({ "tasks": [
            task_json("u-1", "post", 3),
            task_json("u-2", "review", 2),
            task_json("u-3", "post", 1),
        ]}))
        .tasks;
        assert_eq!(kind_choices(&tasks), ["review", "post"]);
        assert!(kind_choices(&[]).is_empty());
    }

    #[test]
    fn プロジェクトチップは重複を畳んで並べる() {
        let mut tasks = parse_list(&json!({ "tasks": [
            task_json("u-1", "post", 3),
            task_json("u-2", "review", 2),
            task_json("u-3", "post", 1),
        ]}))
        .tasks;
        tasks[0].project = Some("zeta".into());
        tasks[1].project = Some("alpha".into());
        tasks[2].project = Some("zeta".into());
        assert_eq!(project_choices(&tasks), ["alpha", "zeta"]);
    }

    /// 選択が消えた（完了して一覧から落ちた）ら先頭へ落ちる
    #[test]
    fn 選択が消えたら先頭へ落ちる() {
        let tasks = parse_list(&json!({ "tasks": [
            task_json("u-1", "review", 3),
            task_json("u-2", "review", 2),
        ]}))
        .tasks;
        let visible: Vec<&TaskRow> = tasks.iter().collect();
        let mut panel = TasksPanel::default();
        assert_eq!(
            panel.selected_task(&visible).map(|t| t.id.as_str()),
            Some("u-1")
        );
        panel.selected = Some("u-2".into());
        assert_eq!(
            panel.selected_task(&visible).map(|t| t.id.as_str()),
            Some("u-2")
        );
        panel.selected = Some("u-99".into());
        assert_eq!(
            panel.selected_task(&visible).map(|t| t.id.as_str()),
            Some("u-1")
        );
        assert_eq!(panel.selected_task(&[]).map(|t| t.id.as_str()), None);
    }

    /// 配送の色は **`sent` を緑にしない**（「届いた」と騙らない物差しを色でも守る）
    #[test]
    fn 配送の色はsentを届いた扱いにしない() {
        let theme = tako_core::theme::Theme::default();
        assert_eq!(delivery_color(&theme, "delivered"), theme.green);
        assert_eq!(delivery_color(&theme, "failed"), theme.red);
        assert_eq!(delivery_color(&theme, "sent"), theme.yellow);
        assert_eq!(delivery_color(&theme, "launched"), theme.yellow);
    }

    /// キャレットは**載っている行にだけ** 1 つ付く（行末の改行では次行へ送らない）
    #[test]
    fn キャレットは載っている行にだけ付く() {
        let lines = caret_lines("ab\ncd", 1);
        assert_eq!(lines, [("ab", Some(1)), ("cd", None)]);
        // 行末（改行の直前）
        let lines = caret_lines("ab\ncd", 2);
        assert_eq!(lines, [("ab", Some(2)), ("cd", None)]);
        // 2 行目の途中
        let lines = caret_lines("ab\ncd", 4);
        assert_eq!(lines, [("ab", None), ("cd", Some(1))]);
        // 末尾
        let lines = caret_lines("ab\ncd", 5);
        assert_eq!(lines, [("ab", None), ("cd", Some(2))]);
        // 空文字
        assert_eq!(caret_lines("", 0), [("", Some(0))]);
        // 範囲外でも 1 つだけ付く（行の末尾へ丸める）
        let lines = caret_lines("ab", 99);
        assert_eq!(lines, [("ab", Some(2))]);
    }

    #[test]
    fn 長いパスは末尾2要素へ縮む() {
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            short_path(&format!("{sep}a{sep}b{sep}c.mp4")),
            format!("b{sep}c.mp4")
        );
        assert_eq!(short_path("c.mp4"), "c.mp4");
        assert_eq!(short_path(""), "");
    }
}
