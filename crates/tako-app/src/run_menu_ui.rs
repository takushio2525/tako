//! 実行ボタン横のドロップダウン（FR-3.18.7 / Issue #1733。エピック #1726 の S5。設計
//! `.agent/plans/2026-09-runner-settings.md` §7）: 実行されるコマンド・プロファイル・実行環境・
//! 引数・環境変数・作業ディレクトリ・実行前コマンドを見て、その場で変える。
//!
//! ## 解決と保存はここに無い
//!
//! 見せるものは dispatch `RunResolve`（CLI `tako run <f> --list`・MCP `tako_run_resolve` と同じ応答）を
//! **background で**取って（`dispatch_off_ui` = `prepare_offload` → `run_staged` → `finish_offload`）型へ
//! 写しただけのもの（[`RunView::from_json`]）。描画はその完成品を読むだけで、実行環境の検出
//! （`runtime_env` / `runtime_probe`）を描画から呼ばない（番犬 `issue1733_run_menu_watchdog`）。
//! 一覧のコマンドが `Run` と同じ層（実行環境・保存した引数）を重ねたものになるので、#1730 以前の
//! 「一覧は `python3 a.py`・実際は `.venv` の python」というずれが無くなる。
//!
//! 書き込みは dispatch `RunnerDefaults`（CLI `tako run-config`・MCP `tako_run_defaults`）を直に呼び
//! （#1731 の設計どおり子プロセスを起こさないので UI スレッドで呼べる）、成功したら `RunResolve` を
//! 取り直す。プロファイルの選択も `RunnerDefaults { profile }` で保存する（#1731 以前のメモリ上だけの
//! `preview_run_selected` は廃止。再起動しても・CLI からも同じ選択が見える）。
//!
//! ## 欄
//!
//! 引数・環境変数・作業ディレクトリ・実行前コマンド・実行環境のパスは、行を押すとその場の
//! `TextField`（#1459）で直す。IME の宛先は `AppTextInput::RunMenu`（#561 / #1725 の作法 =
//! 打鍵の振り分けと変換の宛先を同じ判定 [`TakoApp::run_menu_input_active`] で揃える）。
//! 空のまま Enter・`=` の無い環境変数は保存せずに理由を出す。名前の検め（識別子か）・作業ディレクトリが
//! 在るか・実行環境が解けるかは dispatch が決め、その理由をそのまま欄の下に出す（ここで判定を重ねない）

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use gpui::prelude::*;
use gpui::{
    canvas, div, px, svg, ClickEvent, Context, CursorStyle, FontWeight, Keystroke, MouseButton,
    MouseDownEvent, Pixels, Point, SharedString, Size,
};
use serde_json::Value;
use tako_control::protocol::Request;
use tako_core::theme::Theme;
use tako_core::{PaneId, PaneOrigin};

use crate::text_field::TextField;
use crate::{hsla, hsla_alpha, rgba, rgba_alpha, AppTextInput, TakoApp};

/// メニューの幅（px）
const MENU_W: f32 = 380.0;
/// 欄の上限（バイト。貼り付けた巨大な文字列で欄と保存先を膨らませない）
const FIELD_MAX_BYTES: usize = 4096;
/// 畳んだときに出す実行環境の候補の数（conda の env が多い機でも、下の項目の欄が画面の外へ押し出されない）。
/// 残りは「他 N 件を表示」で広げる（入れ子のスクロールにしないのは、この GPUI では内側を回すと外側も一緒に
/// 送られるため）
const RUNTIME_ROWS_FOLDED: usize = 5;

/// A/B（`TAKO_1733_LEGACY=1`）: 一覧を #1733 以前の同期の解決（実行環境・保存した設定を重ねない）で作る。
/// 一覧のコマンドと `Run` が走らせるコマンドのずれ（venv の例）を同じバイナリで見比べるためだけの腕
pub(crate) fn legacy_1733() -> bool {
    std::env::var_os("TAKO_1733_LEGACY").is_some_and(|v| v == "1")
}

// ─── `RunResolve` の応答を写した型 ─────────────────────────────────────

/// 項目の値と出典（`RunResolve` の `config.<項目>` = `{ value, source }`）
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Sourced {
    pub(crate) value: Option<String>,
    /// file / project / declaration / auto / default
    pub(crate) source: String,
}

impl Sourced {
    fn from_json(v: &Value) -> Self {
        Self {
            value: v["value"].as_str().map(str::to_string),
            source: v["source"].as_str().unwrap_or("default").to_string(),
        }
    }
}

/// プロファイル 1 つ（`profiles[]`）
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProfileRow {
    pub(crate) profile: String,
    pub(crate) command: String,
    pub(crate) cwd: String,
}

/// 実行環境の候補 1 つ（`runtimes[]` / `runtime`）
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RuntimeRow {
    /// `<manager>:<key>`（`RunnerDefaults { runtime }` が受ける形）
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) version: Option<String>,
    pub(crate) auto: bool,
    pub(crate) selected: bool,
}

impl RuntimeRow {
    fn from_json(v: &Value) -> Option<Self> {
        Some(Self {
            id: v["id"].as_str()?.to_string(),
            kind: v["kind"].as_str().unwrap_or_default().to_string(),
            label: v["label"].as_str().unwrap_or_default().to_string(),
            version: v["version"].as_str().map(str::to_string),
            auto: v["auto"].as_bool().unwrap_or(false),
            selected: v["selected"].as_bool().unwrap_or(false),
        })
    }

    /// ヘッダに出す短いラベル（`.venv 3.12` / `uv run`）。版は先頭 2 段だけ
    pub(crate) fn short_label(&self) -> String {
        match self.version.as_deref().filter(|v| !v.is_empty()) {
            Some(v) => {
                let short: Vec<&str> = v.split('.').take(2).collect();
                format!("{} {}", self.label, short.join("."))
            }
            None => self.label.clone(),
        }
    }
}

/// 基板の行（`board.candidates[]` / `board.matches[]`。#1952）
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BoardRow {
    pub(crate) fqbn: String,
    pub(crate) name: String,
    /// 繋がっているポート
    pub(crate) port: Option<String>,
    pub(crate) connected: bool,
    /// コアが入っているか（分からなければ `None`）
    pub(crate) installed: Option<bool>,
}

impl BoardRow {
    fn from_json(v: &Value) -> Option<Self> {
        let fqbn = v["fqbn"].as_str()?.to_string();
        Some(Self {
            name: v["name"].as_str().unwrap_or(&fqbn).to_string(),
            port: v["port"].as_str().map(str::to_string),
            connected: v["connected"].as_bool().unwrap_or(false),
            // 一覧（matches）は入っているコアの基板だけ
            installed: v["installed"].as_bool().or(Some(true)),
            fqbn,
        })
    }
}

/// ポートの行（`board.ports[]`）
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PortRow {
    pub(crate) address: String,
    /// `Serial Port (USB)` 等と、そのポートに繋がっている基板の名前
    pub(crate) detail: String,
}

/// `.ino` の基板とポート（`RunResolve` の `board` 欄をそのまま写したもの。正本は sketch.yaml。#1952）
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BoardView {
    pub(crate) fqbn: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) port: Option<String>,
    /// 繋がっている基板 + 今の基板
    pub(crate) candidates: Vec<BoardRow>,
    /// 見つかったポート（`None` = 聞けていない）
    pub(crate) ports: Option<Vec<PortRow>>,
    /// 入っているコアの基板（「基板を探す」が絞る元）
    pub(crate) installed: Vec<BoardRow>,
    /// 注意（default_profile が勝つ・今の基板のコアが無い）と、聞けなかった理由
    pub(crate) notes: Vec<String>,
}

impl BoardView {
    fn from_json(b: &Value) -> Option<Self> {
        if !b.is_object() {
            return None;
        }
        let rows = |key: &str| -> Vec<BoardRow> {
            b[key]
                .as_array()
                .map(|a| a.iter().filter_map(BoardRow::from_json).collect())
                .unwrap_or_default()
        };
        let strs = |key: &str| -> Vec<String> {
            b[key]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut notes = strs("notes");
        notes.extend(strs("errors"));
        Some(Self {
            fqbn: b["fqbn"].as_str().map(str::to_string),
            name: b["name"].as_str().map(str::to_string),
            port: b["port"].as_str().map(str::to_string),
            candidates: rows("candidates"),
            ports: b["ports"].as_array().map(|a| {
                a.iter()
                    .filter_map(|p| {
                        let address = p["address"].as_str()?.to_string();
                        let mut parts: Vec<String> = Vec::new();
                        if let Some(l) = p["protocol_label"].as_str().filter(|l| !l.is_empty()) {
                            parts.push(l.to_string());
                        }
                        parts.extend(
                            p["boards"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|n| n.as_str().map(str::to_string)),
                        );
                        Some(PortRow {
                            address,
                            detail: parts.join(" · "),
                        })
                    })
                    .collect()
            }),
            installed: rows("matches"),
            notes,
        })
    }

    /// 今の基板の表示名（一覧で名前が引けなければ FQBN）
    pub(crate) fn label(&self) -> Option<String> {
        self.name.clone().or_else(|| self.fqbn.clone())
    }
}

/// 「基板を探す」の欄の絞り込み（名前か FQBN の部分一致・大文字小文字を区別しない。空 = 全部）。
/// 一覧の順（arduino-cli の並び）のまま先頭 `limit` 件
pub(crate) fn filter_boards<'a>(
    rows: &'a [BoardRow],
    text: &str,
    limit: usize,
) -> Vec<&'a BoardRow> {
    let q = text.trim().to_lowercase();
    rows.iter()
        .filter(|b| {
            q.is_empty() || b.name.to_lowercase().contains(&q) || b.fqbn.to_lowercase().contains(&q)
        })
        .take(limit)
        .collect()
}

/// 「基板を探す」の欄で Enter: 絞り込みがちょうど 1 件ならその基板、そうでなければ打った文字列を
/// FQBN として渡す（形・コアの有無・実在は dispatch が検めて断る）
pub(crate) fn board_choice(rows: &[BoardRow], text: &str) -> String {
    let typed = text.trim();
    match filter_boards(rows, typed, 2).as_slice() {
        [only] if !rows.iter().any(|b| b.fqbn == typed) => only.fqbn.clone(),
        _ => typed.to_string(),
    }
}

/// 探す欄の下に出す行の上限（数百ある esp32 でもメニューを押し出さない）
const BOARD_ROWS_SHOWN: usize = 8;

/// メニューが見せるもの（`RunResolve` の応答をそのまま型へ写したもの。判定を足さない）
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RunView {
    pub(crate) profiles: Vec<ProfileRow>,
    /// 宣言の既定（`default_profile`）
    pub(crate) default_profile: String,
    /// 保存したプロファイル（`config.profile`）
    pub(crate) profile: Sourced,
    pub(crate) warnings: Vec<String>,
    /// 実行環境の種類（`python` 等。無い拡張子は `None` = 実行環境の節を出さない）
    pub(crate) runtime_kind: Option<String>,
    /// 走らせる候補（`runtime`）
    pub(crate) runtime: Option<RuntimeRow>,
    /// 実行環境の出典（`config.runtime.<kind>.source`）
    pub(crate) runtime_source: String,
    pub(crate) runtimes: Vec<RuntimeRow>,
    /// 保存した実行環境が解けない理由（`runtime_unresolved.reason`）
    pub(crate) runtime_unresolved: Option<String>,
    pub(crate) args: Sourced,
    pub(crate) cwd: Sourced,
    pub(crate) before: Sourced,
    /// 環境変数（名前順 = 応答の object の順）
    pub(crate) env: Vec<(String, Sourced)>,
    /// `.ino` の基板とポート（それ以外のファイルは `None` = 節を出さない。#1952）
    pub(crate) board: Option<BoardView>,
}

impl RunView {
    /// `RunResolve` の応答から作る
    pub(crate) fn from_json(v: &Value) -> Self {
        let profiles = v["profiles"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| {
                        Some(ProfileRow {
                            profile: p["profile"].as_str()?.to_string(),
                            command: p["command"].as_str().unwrap_or_default().to_string(),
                            cwd: p["cwd"].as_str().unwrap_or_default().to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let runtimes: Vec<RuntimeRow> = v["runtimes"]
            .as_array()
            .map(|a| a.iter().filter_map(RuntimeRow::from_json).collect())
            .unwrap_or_default();
        let runtime = RuntimeRow::from_json(&v["runtime"]);
        let config = &v["config"];
        let config_runtime = config["runtime"].as_object();
        let runtime_kind = runtime
            .as_ref()
            .map(|r| r.kind.clone())
            .or_else(|| runtimes.first().map(|r| r.kind.clone()))
            .or_else(|| config_runtime.and_then(|m| m.keys().next().cloned()))
            .filter(|k| !k.is_empty());
        let runtime_source = runtime_kind
            .as_deref()
            .and_then(|k| config["runtime"][k]["source"].as_str())
            .unwrap_or("auto")
            .to_string();
        let env = config["env"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, s)| (k.clone(), Sourced::from_json(s)))
                    .collect()
            })
            .unwrap_or_default();
        Self {
            profiles,
            default_profile: v["default_profile"]
                .as_str()
                .unwrap_or("default")
                .to_string(),
            profile: Sourced::from_json(&config["profile"]),
            warnings: v["warnings"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|w| w.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            runtime_kind,
            runtime,
            runtime_source,
            runtimes,
            runtime_unresolved: v["runtime_unresolved"]["reason"]
                .as_str()
                .map(str::to_string),
            args: Sourced::from_json(&config["args"]),
            cwd: Sourced::from_json(&config["cwd"]),
            before: Sourced::from_json(&config["before"]),
            env,
            board: BoardView::from_json(&v["board"]),
        }
    }

    /// A/B の腕（`TAKO_1733_LEGACY=1`）: #1733 以前の一覧 = 実行環境・保存した設定を重ねない同期の解決
    fn legacy(path: &Path) -> Result<Self, String> {
        use tako_core::runner;
        let head = runner::read_file_head_for_ui(path)
            .ok_or_else(|| crate::ui_text::preview::run_no_command().to_string())?;
        let settings = tako_control::settings::load();
        let resolution = runner::resolve_file(
            path,
            &head,
            &settings.runner_defaults,
            None,
            None,
            &runner::RuntimeWords::new(),
            &runner::ConfigLayer::default(),
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            profiles: resolution
                .all_profiles
                .iter()
                .map(|p| ProfileRow {
                    profile: p.profile.clone(),
                    command: p.command.clone(),
                    cwd: p.cwd.display().to_string(),
                })
                .collect(),
            default_profile: resolution.plan.profile.clone(),
            ..Self::default()
        })
    }

    /// 走らせるプロファイル（保存した選択 → 宣言の既定 = dispatch `Run` が `profile` 省略時に選ぶもの）
    pub(crate) fn selected_profile(&self) -> &str {
        self.profile
            .value
            .as_deref()
            .unwrap_or(self.default_profile.as_str())
    }

    /// 走らせるプロファイルの行（見つからなければ先頭）
    pub(crate) fn selected_row(&self) -> Option<&ProfileRow> {
        let name = self.selected_profile();
        self.profiles
            .iter()
            .find(|p| p.profile == name)
            .or_else(|| self.profiles.first())
    }

    /// 再生ボタンを押せるか（走らせるコマンドがある）
    pub(crate) fn run_capable(&self) -> bool {
        !self.profiles.is_empty()
    }
}

/// 保存した値（= 出典タグで保存先を移せる・× で消せる）か
fn is_saved_source(source: &str) -> bool {
    matches!(source, "file" | "project")
}

/// 出す実行環境の候補の添字（畳んでいれば先頭 [`RUNTIME_ROWS_FOLDED`] 件 + 選んでいる候補）と、隠した件数
pub(crate) fn visible_runtimes(runtimes: &[RuntimeRow], expanded: bool) -> (Vec<usize>, usize) {
    if expanded || runtimes.len() <= RUNTIME_ROWS_FOLDED {
        return ((0..runtimes.len()).collect(), 0);
    }
    let mut shown: Vec<usize> = (0..RUNTIME_ROWS_FOLDED).collect();
    // 選んでいる候補は畳んでも必ず出す（印の付いた行が見えないと、何で走るかが分からない）
    if let Some(i) = runtimes.iter().position(|r| r.selected) {
        if i >= RUNTIME_ROWS_FOLDED {
            shown.push(i);
        }
    }
    let hidden = runtimes.len() - shown.len();
    (shown, hidden)
}

/// 出典タグのクリックで移す先（保存した値だけ。宣言・自動・既定は移すものが無い）
pub(crate) fn other_scope(source: &str) -> Option<&'static str> {
    match source {
        "file" => Some("project"),
        "project" => Some("file"),
        _ => None,
    }
}

/// 保存先（出典が file / project ならそこへ、それ以外は dispatch の既定 = 省略）
fn scope_of(source: &str) -> Option<&'static str> {
    match source {
        "file" => Some("file"),
        "project" => Some("project"),
        _ => None,
    }
}

// ─── 状態 ──────────────────────────────────────────────────────────────

/// 開いているメニュー（ペインと、画面に収めた左上・高さの上限）
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OpenMenu {
    pub(crate) pane: PaneId,
    left: f32,
    top: f32,
    max_h: f32,
}

/// ペインごとの `RunResolve` の答え
#[derive(Debug, Default)]
pub(crate) struct Resolved {
    pub(crate) path: PathBuf,
    /// 最後に頼んだ問い合わせの番号（古い答えを捨てる）
    seq: u64,
    pub(crate) loading: bool,
    /// 再検出（Tier P の聞き直し）の最中
    pub(crate) refreshing: bool,
    pub(crate) answer: Option<Result<RunView, String>>,
    /// 応答の原文（visual-test が別に取った `RunResolve` と突き合わせる）
    pub(crate) raw: Option<Value>,
}

/// 欄で直す項目
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditTarget {
    Args,
    Cwd,
    Before,
    /// 環境変数（`original` = 直す前の名前。`None` = 追加）
    Env {
        original: Option<String>,
    },
    /// 実行環境の interpreter のパス
    RuntimePath,
    /// `.ino` の基板（打つと入っているコアの基板を絞り込む。#1952）
    Board,
    /// `.ino` の書き込み先のポート（#1952）
    Port,
}

impl EditTarget {
    /// 書くと arduino-cli を起こす（`dispatch_off_ui` で送る）欄か
    fn writes_sketch(&self) -> bool {
        matches!(self, EditTarget::Board | EditTarget::Port)
    }
}

/// 開いている欄
#[derive(Debug)]
pub(crate) struct FieldEdit {
    pub(crate) pane: PaneId,
    pub(crate) target: EditTarget,
    pub(crate) field: TextField,
    /// 保存されなかった理由（欄は開いたまま直して打ち直せる）
    pub(crate) notice: Option<String>,
    /// 保存先（`None` = dispatch の既定）
    scope: Option<&'static str>,
}

/// ドロップダウンの状態（ウィンドウに 1 つ）
#[derive(Debug, Default)]
pub(crate) struct RunMenuUi {
    pub(crate) open: Option<OpenMenu>,
    pub(crate) resolved: HashMap<PaneId, Resolved>,
    /// 取り直しを頼まれたペイン（Context の無い経路 = dispatch の `set_preview`・復元から積む。
    /// `drain_pending_preview_loads` が background へ流す）
    pending: Vec<(PaneId, PathBuf)>,
    pub(crate) edit: Option<FieldEdit>,
    /// メニューの下に出す「保存できなかった」理由
    pub(crate) notice: Option<(PaneId, String)>,
    /// 実行環境の候補を全部出しているか（開き直すと畳む）
    pub(crate) runtimes_expanded: bool,
    /// `arduino-cli board attach` の答えを待っているペイン（#1952）
    pub(crate) attaching: Option<PaneId>,
    seq: u64,
}

impl RunMenuUi {
    /// 描画が読む完成品（答えが無い・エラーなら `None`）
    pub(crate) fn view(&self, pane: PaneId) -> Option<&RunView> {
        self.resolved
            .get(&pane)
            .and_then(|r| r.answer.as_ref())
            .and_then(|a| a.as_ref().ok())
    }

    pub(crate) fn error(&self, pane: PaneId) -> Option<&str> {
        self.resolved
            .get(&pane)
            .and_then(|r| r.answer.as_ref())
            .and_then(|a| a.as_ref().err())
            .map(String::as_str)
    }

    /// 取り直しの最中か（visual-test が答えの届くのを待つための読み口）
    #[cfg(feature = "visual-test")]
    pub(crate) fn loading(&self, pane: PaneId) -> bool {
        self.resolved.get(&pane).is_some_and(|r| r.loading)
    }

    pub(crate) fn is_open(&self, pane: PaneId) -> bool {
        self.open.is_some_and(|m| m.pane == pane)
    }
}

/// `RunnerDefaults { path, … }` の書き込み 1 回ぶん
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Write {
    pub(crate) scope: Option<&'static str>,
    pub(crate) profile: Option<String>,
    pub(crate) runtime: Option<String>,
    pub(crate) reset: Option<Vec<String>>,
    pub(crate) args: Option<String>,
    pub(crate) env: Option<BTreeMap<String, Option<String>>>,
    pub(crate) cwd: Option<String>,
    pub(crate) before: Option<String>,
    /// `.ino` の基板とポート（#1952。arduino-cli を起こすので `dispatch_off_ui` だけで送る）
    pub(crate) board: Option<String>,
    pub(crate) port: Option<String>,
}

impl Write {
    fn into_request(self, path: &Path) -> Request {
        Request::RunnerDefaults {
            ext: None,
            command: None,
            remove: false,
            path: Some(path.display().to_string()),
            pane: None,
            scope: self.scope.map(str::to_string),
            profile: self.profile,
            runtime: self.runtime,
            reset: self.reset,
            args: self.args,
            env: self.env,
            cwd: self.cwd,
            before: self.before,
            board: self.board,
            port: self.port,
        }
    }
}

/// 欄の文字列 → 書き込み（保存しないときは理由）。空・`=` の無い環境変数はここで止める。
/// 名前が識別子か・作業ディレクトリが在るか・実行環境が解けるかは dispatch が決める
pub(crate) fn build_write(
    target: &EditTarget,
    text: &str,
    scope: Option<&'static str>,
) -> Result<Write, String> {
    if text.trim().is_empty() {
        return Err(crate::ui_text::preview::run_menu_empty().to_string());
    }
    let mut w = Write {
        scope,
        ..Write::default()
    };
    match target {
        EditTarget::Args => w.args = Some(text.to_string()),
        EditTarget::Cwd => w.cwd = Some(text.to_string()),
        EditTarget::Before => w.before = Some(text.to_string()),
        EditTarget::RuntimePath => w.runtime = Some(text.trim().to_string()),
        EditTarget::Board => w.board = Some(text.trim().to_string()),
        EditTarget::Port => w.port = Some(text.trim().to_string()),
        EditTarget::Env { original } => {
            let Some((name, value)) = text.split_once('=') else {
                return Err(crate::ui_text::preview::run_menu_env_format().to_string());
            };
            let name = name.trim();
            if name.is_empty() {
                return Err(crate::ui_text::preview::run_menu_env_format().to_string());
            }
            let mut env = BTreeMap::new();
            // 名前を変えたら元の名前を消す（同じ呼び出しの中で消してから足す = dispatch の順）
            if let Some(o) = original.as_deref().filter(|o| *o != name) {
                env.insert(o.to_string(), None);
            }
            env.insert(name.to_string(), Some(value.to_string()));
            w.env = Some(env);
        }
    }
    Ok(w)
}

/// 出典タグ・× で扱う項目
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Item {
    Profile,
    Runtime,
    Args,
    Cwd,
    Before,
    Env(String),
}

impl Item {
    /// `RunnerDefaults { reset }` の項目名（`RunConfigField::as_str` と同じ字面）
    fn field(&self) -> &'static str {
        match self {
            Item::Profile => "profile",
            Item::Runtime => "runtime",
            Item::Args => "args",
            Item::Cwd => "cwd",
            Item::Before => "before",
            Item::Env(_) => "env",
        }
    }

    /// 今の値と出典（見ているメニューの完成品から読む）
    fn current(&self, v: &RunView) -> Option<(String, String)> {
        let pick = |s: &Sourced| s.value.clone().map(|val| (val, s.source.clone()));
        match self {
            Item::Profile => pick(&v.profile),
            Item::Runtime => v
                .runtime
                .as_ref()
                .map(|r| (r.id.clone(), v.runtime_source.clone())),
            Item::Args => pick(&v.args),
            Item::Cwd => pick(&v.cwd),
            Item::Before => pick(&v.before),
            Item::Env(name) => v
                .env
                .iter()
                .find(|(k, _)| k == name)
                .and_then(|(_, s)| pick(s)),
        }
    }

    /// その値をそのスコープへ書く
    fn set(&self, w: &mut Write, value: String) {
        match self {
            Item::Profile => w.profile = Some(value),
            Item::Runtime => w.runtime = Some(value),
            Item::Args => w.args = Some(value),
            Item::Cwd => w.cwd = Some(value),
            Item::Before => w.before = Some(value),
            Item::Env(name) => w.env = Some(BTreeMap::from([(name.clone(), Some(value))])),
        }
    }

    /// そのスコープから消す（環境変数はその名前だけ）
    fn unset(&self, w: &mut Write) {
        match self {
            Item::Env(name) => w.env = Some(BTreeMap::from([(name.clone(), None)])),
            other => w.reset = Some(vec![other.field().to_string()]),
        }
    }
}

/// 出典タグのクリック = 保存先を移す書き込み（移す先へ書く → 元から消す）。保存した値でなければ `None`
pub(crate) fn move_writes(item: &Item, v: &RunView) -> Option<[Write; 2]> {
    let (value, from) = item.current(v)?;
    let to = other_scope(&from)?;
    let mut set = Write {
        scope: Some(to),
        ..Write::default()
    };
    item.set(&mut set, value);
    let mut unset = Write {
        scope: scope_of(&from),
        ..Write::default()
    };
    item.unset(&mut unset);
    Some([set, unset])
}

/// × = その項目をそのスコープから消す書き込み。保存した値でなければ `None`
pub(crate) fn reset_write(item: &Item, v: &RunView) -> Option<Write> {
    let (_, from) = item.current(v)?;
    let scope = scope_of(&from)?;
    let mut w = Write {
        scope: Some(scope),
        ..Write::default()
    };
    item.unset(&mut w);
    Some(w)
}

// ─── 操作 ──────────────────────────────────────────────────────────────

impl TakoApp {
    /// 取り直しを積む（Context の無い経路 = プレビューの差し替え・復元・改名の追従から呼ぶ）。
    /// 実際の問い合わせは `drain_pending_preview_loads` が background へ流す
    pub(crate) fn queue_run_resolve(&mut self, pane: PaneId, path: &Path) {
        self.run_menu.pending.retain(|(p, _)| *p != pane);
        self.run_menu.pending.push((pane, path.to_path_buf()));
    }

    /// 積んだ取り直しを流す（`drain_pending_preview_loads` から呼ぶ）
    pub(crate) fn drain_pending_run_resolves(&mut self, cx: &mut Context<Self>) {
        for (pane, path) in std::mem::take(&mut self.run_menu.pending) {
            // 流すまでに閉じた / 別のファイルへ差し替わったペインは頼まない
            if self.previews.get(&pane).is_some_and(|s| s.path == path) {
                self.request_run_resolve(pane, path, false, cx);
            }
        }
    }

    /// `RunResolve` を background で頼む（`refresh` = 実行環境の子プロセスの答えを聞き直す = 再検出）。
    /// 答えが返るまでは前回の答えを出す（別のファイルへ差し替わったなら前の答えは捨てる）
    pub(crate) fn request_run_resolve(
        &mut self,
        pane: PaneId,
        path: PathBuf,
        refresh: bool,
        cx: &mut Context<Self>,
    ) {
        self.run_menu.seq = self.run_menu.seq.wrapping_add(1);
        let seq = self.run_menu.seq;
        let entry = self.run_menu.resolved.entry(pane).or_default();
        if entry.path != path {
            entry.answer = None;
            entry.raw = None;
        }
        entry.path = path.clone();
        entry.seq = seq;
        entry.loading = true;
        entry.refreshing = refresh;
        if legacy_1733() {
            entry.answer = Some(RunView::legacy(&path));
            entry.loading = false;
            entry.refreshing = false;
            cx.notify();
            return;
        }
        // 宛先はそのプレビューのペイン（GUI の中の dispatch には「呼び出し元のペイン」が無いので、
        // 省くと「対象ペインが未指定」で断られる。パスは絶対なので cwd の基準には使われない）
        // #1952: `.ino` なら入っているコアの基板を全部もらう（欄の「基板を探す」がこの一覧を絞る）。
        // それ以外のファイルでは dispatch が無視する
        let request = Request::RunResolve {
            path: path.display().to_string(),
            pane: Some(pane.as_u64()),
            refresh,
            boards: Some(String::new()),
        };
        self.dispatch_off_ui(request, PaneOrigin::User, cx, move |app, result, cx| {
            let Some(entry) = app
                .run_menu
                .resolved
                .get_mut(&pane)
                .filter(|e| e.seq == seq)
            else {
                return;
            };
            entry.loading = false;
            entry.refreshing = false;
            match result {
                Ok(v) => {
                    entry.answer = Some(Ok(RunView::from_json(&v)));
                    entry.raw = Some(v);
                }
                Err(e) => {
                    entry.answer = Some(Err(e.to_string()));
                    entry.raw = None;
                }
            }
            cx.notify();
        });
    }

    /// ペインを閉じた・プレビューでなくなったときの後片付け
    pub(crate) fn forget_run_menu(&mut self, pane: PaneId) {
        self.run_menu.resolved.remove(&pane);
        if self.run_menu.attaching == Some(pane) {
            self.run_menu.attaching = None;
        }
        self.run_menu.pending.retain(|(p, _)| *p != pane);
        if self.run_menu.is_open(pane) {
            self.close_run_menu();
        }
        if self
            .run_menu
            .notice
            .as_ref()
            .is_some_and(|(p, _)| *p == pane)
        {
            self.run_menu.notice = None;
        }
    }

    /// シェブロンの押下: 開く / 閉じる。開くときは `at`（押した位置）の下へ、画面に収めて置き、
    /// 取り直す（CLI / MCP からの保存・宣言の書き換えを拾う。取得中は前回の答えを出す）
    pub(crate) fn toggle_run_menu(
        &mut self,
        pane: PaneId,
        at: Point<Pixels>,
        viewport: Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.run_menu.is_open(pane) {
            self.close_run_menu();
            cx.notify();
            return;
        }
        self.close_run_menu();
        self.run_menu.runtimes_expanded = false;
        let vw = f32::from(viewport.width);
        let vh = f32::from(viewport.height);
        let left = (f32::from(at.x) - MENU_W / 2.0).clamp(4.0, (vw - MENU_W - 4.0).max(4.0));
        let top = f32::from(at.y) + 8.0;
        self.run_menu.open = Some(OpenMenu {
            pane,
            left,
            top,
            max_h: (vh - top - 12.0).max(120.0),
        });
        self.run_menu.notice = None;
        if let Some(path) = self.previews.get(&pane).map(|s| s.path.clone()) {
            self.request_run_resolve(pane, path, false, cx);
        }
        cx.notify();
    }

    /// メニューを閉じる（**閉じる出口はここ 1 本**。欄も閉じる）
    pub(crate) fn close_run_menu(&mut self) {
        self.run_menu.open = None;
        self.close_run_menu_edit();
    }

    /// 書き込みを順に流し、1 つでも断られたらそこで止めて理由を返す。流したら取り直す
    fn run_menu_apply(
        &mut self,
        pane: PaneId,
        writes: Vec<Write>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(path) = self.run_menu.resolved.get(&pane).map(|r| r.path.clone()) else {
            return Ok(());
        };
        let mut outcome = Ok(());
        for w in writes {
            if let Err(e) = tako_control::dispatch(self, w.into_request(&path), PaneOrigin::User) {
                outcome = Err(e.to_string());
                break;
            }
        }
        self.request_run_resolve(pane, path, false, cx);
        outcome
    }

    /// 書き込みの結果をメニューの下に出す（成功なら前の理由を消す）
    fn run_menu_report(&mut self, pane: PaneId, outcome: Result<(), String>) {
        self.run_menu.notice = outcome.err().map(|e| (pane, e));
    }

    /// プロファイルの行: 選んで（保存）走らせる（#453 M4 の「行クリック = 選択 + 実行」を保つ）
    pub(crate) fn run_menu_select_profile(
        &mut self,
        pane: PaneId,
        profile: String,
        cx: &mut Context<Self>,
    ) {
        self.close_run_menu_edit();
        let scope = self
            .run_menu
            .view(pane)
            .and_then(|v| scope_of(&v.profile.source));
        let outcome = self.run_menu_apply(
            pane,
            vec![Write {
                scope,
                profile: Some(profile),
                ..Write::default()
            }],
            cx,
        );
        let ok = outcome.is_ok();
        self.run_menu_report(pane, outcome);
        if ok {
            self.close_run_menu();
            if let Some(path) = self.previews.get(&pane).map(|s| s.path.clone()) {
                self.run_preview_file(pane, &path, cx);
            }
        }
        cx.notify();
    }

    /// 実行環境の候補の行: 選ぶ（保存先は今の出典 → 既定 = プロジェクト）
    pub(crate) fn run_menu_select_runtime(
        &mut self,
        pane: PaneId,
        id: String,
        cx: &mut Context<Self>,
    ) {
        self.close_run_menu_edit();
        let scope = self
            .run_menu
            .view(pane)
            .and_then(|v| scope_of(&v.runtime_source));
        let outcome = self.run_menu_apply(
            pane,
            vec![Write {
                scope,
                runtime: Some(id),
                ..Write::default()
            }],
            cx,
        );
        self.run_menu_report(pane, outcome);
        cx.notify();
    }

    /// 出典タグ: 保存先をファイル ↔ プロジェクトで移す
    pub(crate) fn run_menu_move(&mut self, pane: PaneId, item: Item, cx: &mut Context<Self>) {
        self.close_run_menu_edit();
        let Some(writes) = self.run_menu.view(pane).and_then(|v| move_writes(&item, v)) else {
            return;
        };
        let outcome = self.run_menu_apply(pane, writes.to_vec(), cx);
        self.run_menu_report(pane, outcome);
        cx.notify();
    }

    /// ×: その項目をそのスコープから消す（既定 / 自動へ戻す）
    pub(crate) fn run_menu_reset(&mut self, pane: PaneId, item: Item, cx: &mut Context<Self>) {
        self.close_run_menu_edit();
        let Some(write) = self.run_menu.view(pane).and_then(|v| reset_write(&item, v)) else {
            return;
        };
        let outcome = self.run_menu_apply(pane, vec![write], cx);
        self.run_menu_report(pane, outcome);
        cx.notify();
    }

    /// 基板・ポートを選ぶ（#1952。`RunnerDefaults { path, board, port }` = CLI `tako run-config --board/--port`・
    /// MCP `tako_run_defaults` と同じ口）。dispatch が arduino-cli（`board listall` で検めて `board attach`）を
    /// 起こすので **UI スレッドで待たず** `dispatch_off_ui` で送る。断られた理由（コアが入っていない = 導入の
    /// コマンド・基板が無い・形が違う）は欄の下か、欄が無ければメニューの下に出す。答えが来たら取り直す
    pub(crate) fn run_menu_attach(
        &mut self,
        pane: PaneId,
        board: Option<String>,
        port: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.run_menu.resolved.get(&pane).map(|r| r.path.clone()) else {
            return;
        };
        if self.run_menu.attaching == Some(pane) {
            return;
        }
        self.run_menu.attaching = Some(pane);
        self.run_menu.notice = None;
        let request = Write {
            board,
            port,
            ..Write::default()
        }
        .into_request(&path);
        self.dispatch_off_ui(request, PaneOrigin::User, cx, move |app, result, cx| {
            if app.run_menu.attaching == Some(pane) {
                app.run_menu.attaching = None;
            }
            let editing_sketch = app
                .run_menu
                .edit
                .as_ref()
                .is_some_and(|e| e.pane == pane && e.target.writes_sketch());
            match result {
                Ok(v) => {
                    if editing_sketch {
                        app.close_run_menu_edit();
                    }
                    // 取り直し（`board list` で数秒）を待たずに、書いた後の sketch.yaml の値を出す
                    if let Some(view) = app
                        .run_menu
                        .resolved
                        .get_mut(&pane)
                        .and_then(|r| r.answer.as_mut())
                        .and_then(|a| a.as_mut().ok())
                        .and_then(|v| v.board.as_mut())
                    {
                        let b = &v["board"];
                        let fqbn = b["fqbn"].as_str().map(str::to_string);
                        // ポートだけを書いた答えは名前を引いていない（listall を聞かない）= 基板が
                        // 変わらないなら前の名前を保つ
                        let name = b["name"].as_str().map(str::to_string);
                        if fqbn != view.fqbn || name.is_some() {
                            view.name = name;
                        }
                        view.fqbn = fqbn;
                        view.port = b["port"].as_str().map(str::to_string);
                    }
                    app.run_menu.notice = None;
                }
                Err(e) => {
                    let reason = e.to_string();
                    match app.run_menu.edit.as_mut().filter(|_| editing_sketch) {
                        Some(edit) => edit.notice = Some(reason),
                        None => app.run_menu.notice = Some((pane, reason)),
                    }
                }
            }
            app.request_run_resolve(pane, path, false, cx);
            cx.notify();
        });
        cx.notify();
    }

    /// 再検出（`RunResolve { refresh: true }` = CLI `tako run-config <f> --refresh` と同じ口）
    pub(crate) fn run_menu_redetect(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(path) = self.run_menu.resolved.get(&pane).map(|r| r.path.clone()) {
            self.request_run_resolve(pane, path, true, cx);
        }
        cx.notify();
    }

    /// 設定画面の Code Runner タブ（dispatch `Settings` = CLI `tako settings --tab runner`）
    pub(crate) fn run_menu_open_settings(&mut self, cx: &mut Context<Self>) {
        self.close_run_menu();
        if let Err(e) = tako_control::dispatch(
            self,
            Request::Settings {
                action: Some("open".into()),
                tab: Some("runner".into()),
            },
            PaneOrigin::User,
        ) {
            // 弾かれたら黙って捨てない（#1399 の作法。メニューは閉じたので共有の通知欄へ）
            self.notify_ui_dispatch_failed(
                crate::sidebar::NoticeArea::Preview,
                crate::sidebar::NoticeArm::Issue1422,
                crate::ui_text::preview::run_menu_open_settings(),
                None,
                &e,
            );
        }
        cx.notify();
    }

    // ─── 欄 ──────────────────────────────────────────────────────────

    /// 欄を開く（今の値で。保存先は今の出典 → dispatch の既定）
    pub(crate) fn open_run_menu_edit(
        &mut self,
        pane: PaneId,
        target: EditTarget,
        initial: String,
        source: &str,
        cx: &mut Context<Self>,
    ) {
        // 他の入力欄（git の欄・検索欄・名前の変更の欄）は畳む（打鍵と変換の宛先を 1 つに保つ = #503）
        self.clear_text_input_focus();
        let mut field = TextField::default();
        field.set_text(initial);
        self.run_menu.edit = Some(FieldEdit {
            pane,
            target,
            field,
            notice: None,
            scope: scope_of(source),
        });
        cx.notify();
    }

    /// 欄を閉じる（**閉じる出口はここ 1 本**。欄宛ての変換も捨てる = #1750 と同じ理由）
    pub(crate) fn close_run_menu_edit(&mut self) {
        if self.run_menu.edit.take().is_some() {
            self.discard_app_text_ime(AppTextInput::RunMenu);
        }
    }

    /// 欄が**画面に出ていて**打鍵・変換を受けるか（メニューが同じペインで開いていて、そのペインが
    /// 今のタブのプレビュー）。見えない欄に打鍵を吸わせない（#1725 の `inline_edit_visible` と同じ規則）
    pub(crate) fn run_menu_input_active(&self) -> bool {
        self.run_menu.edit.as_ref().is_some_and(|e| {
            self.run_menu.is_open(e.pane)
                && self.workspace.active_tab().tree().contains(e.pane)
                && self.previews.contains_key(&e.pane)
        })
    }

    /// 欄へ文字列を入れる（打鍵・⌘V・IME の確定・unmark の 4 経路の共通。**挿入はここ 1 本**）
    pub(crate) fn run_menu_insert(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some(edit) = self.run_menu.edit.as_mut() {
            let _fits = edit.field.insert(text, FIELD_MAX_BYTES, false);
            edit.notice = None;
        }
        cx.notify();
    }

    /// 欄のキー処理（`handle_key` が欄の出ている間だけ呼ぶ。打鍵はすべてここで消費する）
    pub(crate) fn handle_run_menu_key(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) {
        let m = keystroke.modifiers;
        match keystroke.key.as_str() {
            "enter" => self.submit_run_menu_edit(cx),
            "escape" => {
                self.close_run_menu_edit();
                cx.notify();
            }
            // ⌘V はクリップボード読み出しの共通経路（`paste`）へ任せる（名前の変更の欄と同じ）
            "v" if m.platform => self.paste(cx),
            key => {
                if let Some(edit) = self.run_menu.edit.as_mut() {
                    if edit.field.handle_edit_key(key) {
                        edit.notice = None;
                        cx.notify();
                        return;
                    }
                }
                // 修飾キー付きは打たない（⌘W 等は欄を出している間は効かない = 打鍵を漏らさない）
                if m.platform || m.control {
                    return;
                }
                if let Some(text) = keystroke.key_char.as_ref().filter(|t| !t.is_empty()) {
                    let text = text.clone();
                    self.run_menu_insert(&text, cx);
                }
            }
        }
    }

    /// Enter: 欄の文字列を保存する（保存されなければ欄は開いたまま理由を出す）
    pub(crate) fn submit_run_menu_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.run_menu.edit.as_ref() else {
            return;
        };
        let pane = edit.pane;
        // #1952: 基板・ポートは arduino-cli を起こすので background へ（欄は答えが来るまで開いたまま）
        if edit.target.writes_sketch() {
            let text = edit.field.text().to_string();
            if text.trim().is_empty() {
                if let Some(edit) = self.run_menu.edit.as_mut() {
                    edit.notice = Some(crate::ui_text::preview::run_menu_empty().to_string());
                }
                cx.notify();
                return;
            }
            let (board, port) = match edit.target {
                EditTarget::Board => {
                    let rows = self
                        .run_menu
                        .view(pane)
                        .and_then(|v| v.board.as_ref())
                        .map(|b| b.installed.as_slice())
                        .unwrap_or_default();
                    (Some(board_choice(rows, &text)), None)
                }
                _ => (None, Some(text.trim().to_string())),
            };
            self.run_menu_attach(pane, board, port, cx);
            return;
        }
        let write = build_write(&edit.target, edit.field.text(), edit.scope);
        let outcome = match write {
            Ok(w) => self.run_menu_apply(pane, vec![w], cx),
            Err(reason) => Err(reason),
        };
        match outcome {
            Ok(()) => {
                self.close_run_menu_edit();
                self.run_menu.notice = None;
            }
            Err(reason) => {
                if let Some(edit) = self.run_menu.edit.as_mut() {
                    edit.notice = Some(reason);
                }
            }
        }
        cx.notify();
    }

    // ─── 描画 ────────────────────────────────────────────────────────

    /// ドロップダウンの描画（ルートの子。外側の押下で閉じる）。完成品（[`RunView`]）を読むだけ
    pub(crate) fn render_run_menu_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let open = self.run_menu.open?;
        let pane = open.pane;
        // 別のタブへ移った・プレビューでなくなったペインのメニューは描かない（戻れば同じ位置に出る）
        if !self.workspace.active_tab().tree().contains(pane) || !self.previews.contains_key(&pane)
        {
            return None;
        }
        let theme = self.theme.clone();
        let probe = self.panel_click_probe_bounds.clone();
        let mut sections: Vec<gpui::AnyElement> = Vec::new();

        match (self.run_menu.view(pane), self.run_menu.error(pane)) {
            (Some(view), _) => {
                sections.push(self.render_run_menu_command(view, &theme));
                if view.profiles.len() >= 2 {
                    sections.push(self.render_run_menu_profiles(pane, view, &theme, cx));
                }
                if view.runtime_kind.is_some() {
                    sections.push(self.render_run_menu_runtime(pane, view, &theme, cx));
                }
                // #1952: `.ino` の基板とポート（正本は sketch.yaml）
                if let Some(board) = view.board.as_ref() {
                    sections.push(self.render_run_menu_board(pane, board, &theme, cx));
                    sections.push(self.render_run_menu_port(pane, board, &theme, cx));
                }
                sections.push(self.render_run_menu_fields(pane, view, &theme, cx));
            }
            (None, Some(err)) => sections.push(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.yellow))
                    .child(SharedString::from(err.to_string()))
                    .into_any_element(),
            ),
            (None, None) => sections.push(
                div()
                    .px(px(10.0))
                    .py(px(8.0))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.text_faint))
                    .child(crate::ui_text::preview::run_menu_loading())
                    .into_any_element(),
            ),
        }

        // 設定画面の Code Runner タブ
        let settings_probe = probe.clone();
        sections.push(
            div()
                .id("run-menu-settings")
                .relative()
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(4.0))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(hsla(theme.accent))
                .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.run_menu_open_settings(cx);
                }))
                .child(crate::ui_text::preview::run_menu_open_settings())
                .child(probe_canvas(settings_probe, "run-menu-settings".into()))
                .into_any_element(),
        );
        if let Some((_, notice)) = self.run_menu.notice.as_ref().filter(|(p, _)| *p == pane) {
            sections.push(
                div()
                    .px(px(10.0))
                    .pb(px(6.0))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.yellow))
                    .child(SharedString::from(notice.clone()))
                    .into_any_element(),
            );
        }

        let mut body = div()
            .id("run-menu-body")
            .flex()
            .flex_col()
            .max_h(px(open.max_h))
            .overflow_y_scroll();
        for (i, section) in sections.into_iter().enumerate() {
            if i > 0 {
                body = body.child(
                    div()
                        .my(px(4.0))
                        .h(px(1.0))
                        .w_full()
                        .bg(hsla(theme.border_subtle)),
                );
            }
            body = body.child(section);
        }

        Some(
            div()
                .id("run-menu-dismiss")
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .size_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.close_run_menu();
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("run-menu")
                        .absolute()
                        .left(px(open.left))
                        .top(px(open.top))
                        .w(px(MENU_W))
                        .bg(rgba(theme.surface_1))
                        .border_1()
                        .border_color(hsla(theme.border_default))
                        .rounded(px(8.0))
                        .shadow_lg()
                        .p(px(6.0))
                        // メニューの中の押下で外側（閉じる）へ届かせない
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
                        )
                        .child(body)
                        .child(probe_canvas(probe, "run-menu".into())),
                )
                .into_any_element(),
        )
    }

    /// 実行されるコマンド（全文を折り返す）+ 作業ディレクトリ + 注意
    fn render_run_menu_command(&self, view: &RunView, theme: &Theme) -> gpui::AnyElement {
        let row = view.selected_row();
        div()
            .flex()
            .flex_col()
            .gap(px(3.0))
            .px(px(10.0))
            .py(px(6.0))
            .child(section_title(
                crate::ui_text::preview::run_menu_command_title(),
                theme,
            ))
            .child(
                div()
                    .font_family(SharedString::from(theme.font_family.clone()))
                    .text_size(px(11.5))
                    .text_color(hsla(theme.foreground))
                    .child(SharedString::from(
                        row.map(|r| r.command.clone()).unwrap_or_default(),
                    )),
            )
            .children(row.filter(|r| !r.cwd.is_empty()).map(|r| {
                div()
                    .text_size(px(10.5))
                    .text_color(hsla(theme.text_faint))
                    .child(SharedString::from(
                        crate::ui_text::preview::run_menu_in_dir(&tako_core::paths::shorten_home(
                            &r.cwd,
                        )),
                    ))
            }))
            .children(view.warnings.iter().map(|w| {
                div()
                    .text_size(px(10.5))
                    .text_color(hsla(theme.yellow))
                    .child(SharedString::from(w.clone()))
            }))
            .into_any_element()
    }

    /// プロファイル（2 つ以上のとき）。行 = 選んで走らせる
    fn render_run_menu_profiles(
        &self,
        pane: PaneId,
        view: &RunView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let selected = view.selected_profile().to_string();
        let mut col = div().flex().flex_col().px(px(4.0)).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px(px(6.0))
                .py(px(4.0))
                .child(section_title(
                    crate::ui_text::preview::run_menu_profiles(),
                    theme,
                ))
                .child(self.source_controls(pane, Item::Profile, &view.profile.source, theme, cx)),
        );
        for (i, p) in view.profiles.iter().enumerate() {
            let name = p.profile.clone();
            let is_selected = name == selected;
            let display = if name == "default" {
                crate::ui_text::preview::run_profile_default().to_string()
            } else {
                name.clone()
            };
            col = col.child(
                div()
                    .id(("run-menu-profile", i))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(6.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.run_menu_select_profile(pane, name.clone(), cx);
                    }))
                    .child(check_slot(is_selected, theme))
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(11.5))
                            .text_color(hsla(if is_selected {
                                theme.foreground
                            } else {
                                theme.text_secondary
                            }))
                            .child(SharedString::from(display)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_size(px(10.0))
                            .text_color(hsla(theme.text_faint))
                            // 文字数で切る（#1728。バイト位置で切ると文字の途中に当たって描画中に panic する）
                            .child(SharedString::from(tako_core::text::truncate_chars(
                                &p.command, 40,
                            ))),
                    ),
            );
        }
        col.into_any_element()
    }

    /// 実行環境（候補・自動の印・再検出・パスを指定）
    fn render_run_menu_runtime(
        &self,
        pane: PaneId,
        view: &RunView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let kind = view.runtime_kind.clone().unwrap_or_default();
        let refreshing = self
            .run_menu
            .resolved
            .get(&pane)
            .is_some_and(|r| r.refreshing);
        let probe = self.panel_click_probe_bounds.clone();
        let mut col = div().flex().flex_col().px(px(4.0)).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .px(px(6.0))
                .py(px(4.0))
                .child(section_title(
                    &crate::ui_text::preview::run_menu_runtime(&kind),
                    theme,
                ))
                .child(self.source_controls(pane, Item::Runtime, &view.runtime_source, theme, cx)),
        );
        if view.runtimes.is_empty() {
            col = col.child(
                div()
                    .px(px(6.0))
                    .py(px(4.0))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.text_faint))
                    .child(crate::ui_text::preview::run_menu_no_candidates()),
            );
        }
        let (shown, hidden) = visible_runtimes(&view.runtimes, self.run_menu.runtimes_expanded);
        for i in shown {
            let r = &view.runtimes[i];
            let id = r.id.clone();
            let key = format!("run-menu-runtime-{i}");
            col = col.child(
                div()
                    .id(SharedString::from(key.clone()))
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(6.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.run_menu_select_runtime(pane, id.clone(), cx);
                    }))
                    .child(check_slot(r.selected, theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .text_size(px(11.5))
                            .text_color(hsla(if r.selected {
                                theme.foreground
                            } else {
                                theme.text_secondary
                            }))
                            .child(SharedString::from(r.label.clone())),
                    )
                    .when(r.auto, |d| {
                        d.child(pill(crate::ui_text::preview::run_menu_auto_mark(), theme))
                    })
                    .children(r.version.clone().map(|v| {
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(hsla(theme.text_faint))
                            .child(SharedString::from(v))
                    }))
                    .child(probe_canvas(probe.clone(), key)),
            );
        }
        if hidden > 0 {
            col = col.child(
                action_button(
                    "run-menu-runtimes-more",
                    crate::file_icons::ui_icon::CHEVRON_DOWN,
                    &crate::ui_text::preview::run_menu_more(hidden),
                    theme,
                )
                .mx(px(2.0))
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.run_menu.runtimes_expanded = true;
                    cx.notify();
                }))
                .child(probe_canvas(probe.clone(), "run-menu-runtimes-more".into())),
            );
        }
        // 再検出・パスを指定
        let path_source = view.runtime_source.clone();
        col = col.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(4.0))
                .px(px(2.0))
                .py(px(2.0))
                .child(
                    action_button(
                        "run-menu-redetect",
                        crate::file_icons::ui_icon::REFRESH,
                        if refreshing {
                            crate::ui_text::preview::run_menu_redetecting()
                        } else {
                            crate::ui_text::preview::run_menu_redetect()
                        },
                        theme,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.run_menu_redetect(pane, cx);
                    }))
                    .child(probe_canvas(probe.clone(), "run-menu-redetect".into())),
                )
                .child(
                    action_button(
                        "run-menu-pick-path",
                        crate::file_icons::ui_icon::FOLDER,
                        crate::ui_text::preview::run_menu_pick_path(),
                        theme,
                    )
                    .on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_run_menu_edit(
                                pane,
                                EditTarget::RuntimePath,
                                String::new(),
                                &path_source,
                                cx,
                            );
                        },
                    )),
                ),
        );
        if let Some(reason) = &view.runtime_unresolved {
            col = col.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .text_size(px(10.5))
                    .text_color(hsla(theme.yellow))
                    .child(SharedString::from(reason.clone())),
            );
        }
        col =
            col.children(self.render_run_menu_field_editor(pane, &EditTarget::RuntimePath, theme));
        col.into_any_element()
    }

    /// 基板の節（#1952）: 今の基板・注意・繋がっている基板・「基板を探す」（打つと入っているコアの基板を
    /// 絞り込む欄）。行を押すと `run_menu_attach`（background の `RunnerDefaults { board }`）
    fn render_run_menu_board(
        &self,
        pane: PaneId,
        board: &BoardView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let probe = self.panel_click_probe_bounds.clone();
        let mut col = div()
            .flex()
            .flex_col()
            .px(px(4.0))
            .child(sketch_section_header(
                crate::ui_text::preview::run_menu_board(),
                theme,
            ));
        // 今の基板（未選択は黄色）
        col = col.child(
            div()
                .id("run-menu-board-current")
                .relative()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .px(px(6.0))
                .py(px(4.0))
                .child(check_slot(board.fqbn.is_some(), theme))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .text_size(px(11.5))
                        .text_color(hsla(if board.fqbn.is_some() {
                            theme.foreground
                        } else {
                            theme.yellow
                        }))
                        .child(SharedString::from(board.label().unwrap_or_else(|| {
                            crate::ui_text::preview::run_menu_board_unset().to_string()
                        }))),
                )
                .children(
                    board
                        .fqbn
                        .clone()
                        .filter(|f| board.name.as_deref() != Some(f.as_str()))
                        .map(|f| {
                            div()
                                .flex_none()
                                .font_family(SharedString::from(theme.font_family.clone()))
                                .text_size(px(10.0))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(f))
                        }),
                )
                .child(probe_canvas(probe.clone(), "run-menu-board-current".into())),
        );
        for note in &board.notes {
            col = col.child(
                div()
                    .px(px(6.0))
                    .py(px(2.0))
                    .text_size(px(10.5))
                    .text_color(hsla(theme.yellow))
                    .child(SharedString::from(note.clone())),
            );
        }
        // 繋がっている基板（押すと基板とそのポートを選ぶ）
        let current = (board.fqbn.as_deref(), board.port.as_deref());
        for (i, c) in board.candidates.iter().filter(|c| c.connected).enumerate() {
            let key = format!("run-menu-board-connected-{i}");
            let (fqbn, port) = (c.fqbn.clone(), c.port.clone());
            col = col.child(
                board_row(
                    &key,
                    c,
                    current == (Some(c.fqbn.as_str()), c.port.as_deref()),
                    theme,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.run_menu_attach(pane, Some(fqbn.clone()), port.clone(), cx);
                }))
                .child(probe_canvas(probe.clone(), key)),
            );
        }
        // 基板を探す（欄が開いていれば欄と絞り込んだ行）
        let editing = self
            .run_menu
            .edit
            .as_ref()
            .filter(|e| e.pane == pane && e.target == EditTarget::Board);
        match editing {
            Some(edit) => {
                col = col.children(self.render_run_menu_field_editor(
                    pane,
                    &EditTarget::Board,
                    theme,
                ));
                col = col.child(
                    div()
                        .px(px(6.0))
                        .text_size(px(10.0))
                        .text_color(hsla(theme.text_faint))
                        .child(SharedString::from(
                            crate::ui_text::preview::run_menu_board_installed(
                                board.installed.len(),
                            ),
                        )),
                );
                let rows = filter_boards(&board.installed, edit.field.text(), BOARD_ROWS_SHOWN);
                if rows.is_empty() {
                    col = col.child(
                        div()
                            .px(px(6.0))
                            .py(px(3.0))
                            .text_size(px(10.5))
                            .text_color(hsla(theme.text_faint))
                            .child(crate::ui_text::preview::run_menu_board_no_match()),
                    );
                }
                for (i, b) in rows.into_iter().enumerate() {
                    let key = format!("run-menu-board-match-{i}");
                    let fqbn = b.fqbn.clone();
                    col = col.child(
                        board_row(
                            &key,
                            b,
                            board.fqbn.as_deref() == Some(b.fqbn.as_str()),
                            theme,
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.run_menu_attach(pane, Some(fqbn.clone()), None, cx);
                        }))
                        .child(probe_canvas(probe.clone(), key)),
                    );
                }
            }
            None => {
                col = col.child(
                    action_button(
                        "run-menu-board-search",
                        crate::file_icons::ui_icon::SEARCH,
                        crate::ui_text::preview::run_menu_board_search(),
                        theme,
                    )
                    .mx(px(2.0))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_run_menu_edit(
                            pane,
                            EditTarget::Board,
                            String::new(),
                            "default",
                            cx,
                        );
                    }))
                    .child(probe_canvas(probe.clone(), "run-menu-board-search".into())),
                );
            }
        }
        if self.run_menu.attaching == Some(pane) {
            col = col.child(
                div()
                    .px(px(6.0))
                    .py(px(3.0))
                    .text_size(px(10.5))
                    .text_color(hsla(theme.text_faint))
                    .child(crate::ui_text::preview::run_menu_board_attaching()),
            );
        }
        col.into_any_element()
    }

    /// ポートの節（#1952）: 見つかったポート（押すと選ぶ）・選んであるが今は無いポート・「ポートを指定」
    fn render_run_menu_port(
        &self,
        pane: PaneId,
        board: &BoardView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let probe = self.panel_click_probe_bounds.clone();
        let mut col = div()
            .flex()
            .flex_col()
            .px(px(4.0))
            .child(sketch_section_header(
                crate::ui_text::preview::run_menu_port(),
                theme,
            ));
        let ports = board.ports.clone().unwrap_or_default();
        let mut rows: Vec<(String, String, bool)> = ports
            .iter()
            .map(|p| {
                (
                    p.address.clone(),
                    p.detail.clone(),
                    board.port.as_deref() == Some(p.address.as_str()),
                )
            })
            .collect();
        // 選んであるポートが今は見つからない（繋いでいない）なら印つきで先頭へ
        if let Some(cur) = board.port.as_deref() {
            if !ports.iter().any(|p| p.address == cur) {
                rows.insert(
                    0,
                    (
                        cur.to_string(),
                        crate::ui_text::preview::run_menu_port_absent().to_string(),
                        true,
                    ),
                );
            }
        }
        if rows.is_empty() && board.ports.is_some() {
            col = col.child(
                div()
                    .px(px(6.0))
                    .py(px(4.0))
                    .text_size(px(11.0))
                    .text_color(hsla(theme.text_faint))
                    .child(crate::ui_text::preview::run_menu_port_none()),
            );
        }
        for (i, (address, detail, selected)) in rows.into_iter().enumerate() {
            let key = format!("run-menu-port-{i}");
            let choose = address.clone();
            col = col.child(
                div()
                    .id(SharedString::from(key.clone()))
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(6.0))
                    .py(px(5.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.run_menu_attach(pane, None, Some(choose.clone()), cx);
                    }))
                    .child(check_slot(selected, theme))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .font_family(SharedString::from(theme.font_family.clone()))
                            .text_size(px(11.0))
                            .text_color(hsla(if selected {
                                theme.foreground
                            } else {
                                theme.text_secondary
                            }))
                            .child(SharedString::from(address)),
                    )
                    .when(!detail.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_size(px(10.0))
                                .text_color(hsla(theme.text_faint))
                                .child(SharedString::from(tako_core::text::truncate_chars(
                                    &detail, 32,
                                ))),
                        )
                    })
                    .child(probe_canvas(probe.clone(), key)),
            );
        }
        let editing = self
            .run_menu
            .edit
            .as_ref()
            .is_some_and(|e| e.pane == pane && e.target == EditTarget::Port);
        if editing {
            col = col.children(self.render_run_menu_field_editor(pane, &EditTarget::Port, theme));
        } else {
            let initial = board.port.clone().unwrap_or_default();
            col = col.child(
                action_button(
                    "run-menu-port-pick",
                    crate::file_icons::ui_icon::PENCIL,
                    crate::ui_text::preview::run_menu_port_pick(),
                    theme,
                )
                .mx(px(2.0))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_run_menu_edit(pane, EditTarget::Port, initial.clone(), "default", cx);
                }))
                .child(probe_canvas(probe, "run-menu-port-pick".into())),
            );
        }
        col.into_any_element()
    }

    /// 引数・環境変数・作業ディレクトリ・実行前コマンド（行を押すとその場で直す）
    fn render_run_menu_fields(
        &self,
        pane: PaneId,
        view: &RunView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut col = div().flex().flex_col().px(px(4.0));
        for (target, item, label, value) in [
            (
                EditTarget::Args,
                Item::Args,
                crate::ui_text::preview::run_menu_args(),
                &view.args,
            ),
            (
                EditTarget::Cwd,
                Item::Cwd,
                crate::ui_text::preview::run_menu_cwd(),
                &view.cwd,
            ),
            (
                EditTarget::Before,
                Item::Before,
                crate::ui_text::preview::run_menu_before(),
                &view.before,
            ),
        ] {
            col = col.child(self.render_run_menu_field_row(
                pane,
                FieldRow {
                    key: item.field().to_string(),
                    label: label.to_string(),
                    value: value.value.clone(),
                    source: value.source.clone(),
                    target: target.clone(),
                    item,
                },
                theme,
                cx,
            ));
            col = col.children(self.render_run_menu_field_editor(pane, &target, theme));
            if target == EditTarget::Args {
                // 環境変数は引数の下（1 変数 1 行 + 追加の行）
                col = col.child(div().px(px(6.0)).pt(px(4.0)).child(section_title(
                    crate::ui_text::preview::run_menu_env(),
                    theme,
                )));
                for (name, s) in &view.env {
                    let target = EditTarget::Env {
                        original: Some(name.clone()),
                    };
                    col = col.child(self.render_run_menu_field_row(
                        pane,
                        FieldRow {
                            key: format!("env-{name}"),
                            label: String::new(),
                            value: Some(format!("{name}={}", s.value.clone().unwrap_or_default())),
                            source: s.source.clone(),
                            target: target.clone(),
                            item: Item::Env(name.clone()),
                        },
                        theme,
                        cx,
                    ));
                    col = col.children(self.render_run_menu_field_editor(pane, &target, theme));
                }
                let add = EditTarget::Env { original: None };
                let probe = self.panel_click_probe_bounds.clone();
                col = col.child(
                    action_button(
                        "run-menu-env-add",
                        crate::file_icons::ui_icon::PLUS,
                        crate::ui_text::preview::run_menu_env_add(),
                        theme,
                    )
                    .mx(px(2.0))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_run_menu_edit(
                            pane,
                            EditTarget::Env { original: None },
                            String::new(),
                            "default",
                            cx,
                        );
                    }))
                    .child(probe_canvas(probe, "run-menu-env-add".into())),
                );
                col = col.children(self.render_run_menu_field_editor(pane, &add, theme));
            }
        }
        col.into_any_element()
    }

    /// 項目の 1 行（名前 / 値（押すと欄）/ 出典タグ / ×）
    fn render_run_menu_field_row(
        &self,
        pane: PaneId,
        row: FieldRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let probe = self.panel_click_probe_bounds.clone();
        let FieldRow {
            key,
            label,
            value,
            source,
            target,
            item,
        } = row;
        let editing = self
            .run_menu
            .edit
            .as_ref()
            .is_some_and(|e| e.pane == pane && e.target == target);
        // 欄の初期値は今の値（環境変数は `NAME=VALUE` の形のまま）
        let initial = value.clone().unwrap_or_default();
        let value_key = format!("run-menu-{key}-value");
        let open_source = source.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .px(px(6.0))
            .py(px(3.0))
            .when(!label.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .w(px(104.0))
                        .text_size(px(11.0))
                        .text_color(hsla(theme.text_secondary))
                        .child(SharedString::from(label)),
                )
            })
            .child(
                div()
                    .id(SharedString::from(value_key.clone()))
                    .relative()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .px(px(4.0))
                    .py(px(2.0))
                    .rounded(px(3.0))
                    .cursor(CursorStyle::IBeam)
                    .when(editing, |d| d.bg(rgba_alpha(theme.accent, 0.10)))
                    .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                    .font_family(SharedString::from(theme.font_family.clone()))
                    .text_size(px(11.0))
                    .text_color(hsla(if value.is_some() {
                        theme.foreground
                    } else {
                        theme.text_faint
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_run_menu_edit(
                            pane,
                            target.clone(),
                            initial.clone(),
                            &open_source,
                            cx,
                        );
                    }))
                    .child(SharedString::from(value.clone().unwrap_or_else(|| {
                        crate::ui_text::preview::run_menu_unset().to_string()
                    })))
                    .child(probe_canvas(probe, value_key)),
            )
            .child(self.source_controls_keyed(pane, item, &source, &key, theme, cx))
            .into_any_element()
    }

    /// 開いている欄（その項目の欄が開いていなければ `None`）
    fn render_run_menu_field_editor(
        &self,
        pane: PaneId,
        target: &EditTarget,
        theme: &Theme,
    ) -> Option<gpui::AnyElement> {
        let edit = self
            .run_menu
            .edit
            .as_ref()
            .filter(|e| e.pane == pane && &e.target == target)?;
        let (before, after) = edit.field.split_at_caret();
        let probe = self.panel_click_probe_bounds.clone();
        let field = div()
            .id("run-menu-input")
            .relative()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .overflow_hidden()
            .px(px(6.0))
            .py(px(3.0))
            .rounded(px(4.0))
            .cursor(CursorStyle::IBeam)
            .border_1()
            .border_color(hsla(theme.accent_border_muted))
            .bg(rgba(theme.surface_0))
            .font_family(SharedString::from(theme.font_family.clone()))
            .text_size(px(12.0))
            .text_color(hsla(theme.foreground))
            .child(
                div()
                    .flex_none()
                    .child(SharedString::from(before.to_string())),
            )
            // #561: 未確定文字列は欄の中へ出す（変換の宛先 = AppTextInput::RunMenu）
            .children(self.text_input_marked_at(AppTextInput::RunMenu, theme, 12.0))
            .child(self.text_input_caret(AppTextInput::RunMenu, theme))
            .child(
                div()
                    .flex_none()
                    .child(SharedString::from(after.to_string())),
            )
            .child(probe_canvas(probe.clone(), "run-menu-input".into()));
        // 保存されなかった理由は操作の案内と差し替えて欄の真下に出す（欄がメニューの下端に寄っていても、
        // 欄と一緒に見える位置に置く）
        let below = match edit.notice.clone() {
            Some(reason) => div()
                .relative()
                .text_size(px(10.5))
                .text_color(hsla(theme.yellow))
                .child(SharedString::from(reason))
                .child(probe_canvas(probe, "run-menu-input-notice".into())),
            None => div()
                .text_size(px(10.5))
                .text_color(hsla_alpha(theme.foreground, 0.6))
                .child(if edit.target == EditTarget::Board {
                    crate::ui_text::preview::run_menu_board_hint()
                } else {
                    crate::ui_text::preview::run_menu_edit_hint()
                }),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .px(px(6.0))
                .py(px(4.0))
                .child(field)
                .child(below)
                .into_any_element(),
        )
    }

    /// 出典タグ + ×（見出しの横。プローブのキーは項目名）
    fn source_controls(
        &self,
        pane: PaneId,
        item: Item,
        source: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let key = item.field().to_string();
        self.source_controls_keyed(pane, item, source, &key, theme, cx)
    }

    /// 出典タグ（保存した値ならクリックで保存先を移す）+ ×（保存した値だけ = 既定に戻す）
    fn source_controls_keyed(
        &self,
        pane: PaneId,
        item: Item,
        source: &str,
        key: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let saved = is_saved_source(source);
        let probe = self.panel_click_probe_bounds.clone();
        let tag_key = format!("run-menu-{key}-tag");
        let reset_key = format!("run-menu-{key}-reset");
        let tip = crate::ui_text::preview::run_menu_move_to(source);
        let tip_theme = theme.clone();
        let reset_theme = theme.clone();
        let move_item = item.clone();
        let mut tag = div()
            .id(SharedString::from(tag_key.clone()))
            .relative()
            .flex_none()
            .px(px(6.0))
            .py(px(1.0))
            .rounded(px(8.0))
            .border_1()
            .text_size(px(10.0))
            .border_color(hsla(if saved {
                theme.accent_border_muted
            } else {
                theme.border_subtle
            }))
            .text_color(hsla(if saved {
                theme.accent
            } else {
                theme.text_faint
            }))
            .child(crate::ui_text::preview::run_menu_source(source))
            .child(probe_canvas(probe.clone(), tag_key));
        if saved {
            tag = tag
                .cursor_pointer()
                .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
                .tooltip(move |_, cx| {
                    cx.new(|_| crate::tab_bar::HintTooltip::new(tip.to_string(), tip_theme.clone()))
                        .into()
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.run_menu_move(pane, move_item.clone(), cx);
                }));
        }
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .child(tag)
            .when(saved, |d| {
                d.child(
                    div()
                        .id(SharedString::from(reset_key.clone()))
                        .relative()
                        .flex_none()
                        .p(px(2.0))
                        .rounded(px(3.0))
                        .cursor_pointer()
                        .hover(|d| d.bg(rgba(reset_theme.surface_hover_strong)))
                        .tooltip(move |_, cx| {
                            cx.new(|_| {
                                crate::tab_bar::HintTooltip::new(
                                    crate::ui_text::preview::run_menu_reset().to_string(),
                                    reset_theme.clone(),
                                )
                            })
                            .into()
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.run_menu_reset(pane, item.clone(), cx);
                        }))
                        .child(
                            svg()
                                .path(crate::file_icons::ui_icon::CLOSE)
                                .w(px(9.0))
                                .h(px(9.0))
                                .text_color(hsla(theme.text_tertiary)),
                        )
                        .child(probe_canvas(probe, reset_key)),
                )
            })
            .into_any_element()
    }
}

/// 項目の行の材料
struct FieldRow {
    /// プローブ・要素 ID の鍵（`args` / `env-NAME` …）
    key: String,
    label: String,
    value: Option<String>,
    source: String,
    target: EditTarget,
    item: Item,
}

/// 節の見出し
fn section_title(text: &str, theme: &Theme) -> gpui::Div {
    div()
        .text_size(px(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hsla(theme.text_tertiary))
        .child(SharedString::from(text.to_string()))
}

/// 基板・ポートの節の見出し（出典は常に sketch.yaml = 押しても移さない・消さない。ツールチップで書き先を示す）
fn sketch_section_header(title: &str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    let tip_theme = theme.clone();
    div()
        .id(SharedString::from(format!(
            "run-menu-sketch-header-{title}"
        )))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px(px(6.0))
        .py(px(4.0))
        .child(section_title(title, theme))
        .child(
            div()
                .id(SharedString::from(format!("run-menu-sketch-tag-{title}")))
                .flex_none()
                .px(px(6.0))
                .py(px(1.0))
                .rounded(px(8.0))
                .border_1()
                .border_color(hsla(theme.border_subtle))
                .text_size(px(10.0))
                .text_color(hsla(theme.text_faint))
                .tooltip(move |_, cx| {
                    cx.new(|_| {
                        crate::tab_bar::HintTooltip::new(
                            crate::ui_text::preview::run_menu_board_source_tip().to_string(),
                            tip_theme.clone(),
                        )
                    })
                    .into()
                })
                .child(crate::ui_text::preview::run_menu_board_source()),
        )
}

/// 基板の 1 行（印・名前・接続中 / コア未導入の印・ポートか FQBN）
fn board_row(key: &str, b: &BoardRow, selected: bool, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(key.to_string()))
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .py(px(5.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
        .child(check_slot(selected, theme))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_size(px(11.5))
                .text_color(hsla(if selected {
                    theme.foreground
                } else {
                    theme.text_secondary
                }))
                .child(SharedString::from(b.name.clone())),
        )
        .when(b.connected, |d| {
            d.child(pill(
                crate::ui_text::preview::run_menu_board_connected(),
                theme,
            ))
        })
        .when(b.installed == Some(false), |d| {
            d.child(warn_pill(
                crate::ui_text::preview::run_menu_board_core_missing(),
                theme,
            ))
        })
        .child(
            div()
                .flex_none()
                .font_family(SharedString::from(theme.font_family.clone()))
                .text_size(px(10.0))
                .text_color(hsla(theme.text_faint))
                .child(SharedString::from(tako_core::text::truncate_chars(
                    b.port.as_deref().unwrap_or(&b.fqbn),
                    28,
                ))),
        )
}

/// 注意の印（コアが入っていない基板）
fn warn_pill(text: &str, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .px(px(5.0))
        .rounded(px(6.0))
        .bg(rgba_alpha(theme.yellow, 0.15))
        .text_size(px(9.5))
        .text_color(hsla(theme.yellow))
        .child(SharedString::from(text.to_string()))
}

/// 選択の印の欄（選ばれていなければ同じ幅の空き = 行の頭を揃える）
fn check_slot(selected: bool, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .w(px(12.0))
        .h(px(12.0))
        .when(selected, |d| {
            d.child(
                svg()
                    .path(crate::file_icons::ui_icon::CHECK)
                    .w(px(12.0))
                    .h(px(12.0))
                    .text_color(hsla(theme.green)),
            )
        })
}

/// 小さな印（自動選択の候補）
fn pill(text: &str, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .px(px(5.0))
        .rounded(px(6.0))
        .bg(rgba_alpha(theme.green, 0.15))
        .text_size(px(9.5))
        .text_color(hsla(theme.green))
        .child(SharedString::from(text.to_string()))
}

/// アイコン + 文言のボタン（再検出・パスを指定・追加）
fn action_button(
    id: &'static str,
    icon: &'static str,
    label: &str,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.0))
        .px(px(6.0))
        .py(px(4.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(|d| d.bg(rgba(theme.surface_hover_strong)))
        .text_size(px(11.0))
        .text_color(hsla(theme.text_secondary))
        .child(
            svg()
                .path(icon)
                .w(px(11.0))
                .h(px(11.0))
                .text_color(hsla(theme.text_tertiary)),
        )
        .child(SharedString::from(label.to_string()))
}

/// 実矩形を記録する（visual-test が実マウスで押す・写すため。描画の正には使わない）
fn probe_canvas(
    probe: std::rc::Rc<std::cell::RefCell<HashMap<String, gpui::Bounds<Pixels>>>>,
    key: String,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, _, _| {
            probe.borrow_mut().insert(key.clone(), bounds);
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `RunResolve` の応答の形（#1730 / #1731 / #1732 の dispatch が返すもの）
    fn sample() -> Value {
        json!({
            "path": "/p/a.py",
            "profiles": [
                { "profile": "default", "command": "/p/.venv/bin/python a.py --v", "cwd": "/p", "source": "declaration" },
                { "profile": "test", "command": "/p/.venv/bin/python -m pytest a.py", "cwd": "/p", "source": "declaration" },
            ],
            "warnings": ["注意"],
            "default_profile": "default",
            "config": {
                "profile": { "value": "test", "source": "file" },
                "runtime": { "python": { "value": { "manager": "venv", "key": "/p/.venv" }, "source": "project" } },
                "args": { "value": "--v", "source": "file" },
                "env": { "APP_ENV": { "value": "dev", "source": "project" } },
                "cwd": { "value": null, "source": "default" },
                "before": { "value": null, "source": "default" },
            },
            "runtime": { "id": "venv:/p/.venv", "kind": "python", "label": ".venv", "version": "3.12.4", "auto": true, "selected": true, "source": "project" },
            "runtime_unresolved": null,
            "runtimes": [
                { "id": "venv:/p/.venv", "kind": "python", "label": ".venv", "version": "3.12.4", "auto": true, "selected": true },
                { "id": "system:python3", "kind": "python", "label": "python3", "version": null, "auto": false, "selected": false },
            ],
        })
    }

    #[test]
    fn 応答をそのまま写す() {
        let v = RunView::from_json(&sample());
        assert_eq!(v.profiles.len(), 2);
        assert_eq!(
            v.selected_profile(),
            "test",
            "保存したプロファイルが既定より勝つ（Run と同じ）"
        );
        assert_eq!(
            v.selected_row().map(|r| r.command.as_str()),
            Some("/p/.venv/bin/python -m pytest a.py")
        );
        assert_eq!(v.runtime_kind.as_deref(), Some("python"));
        assert_eq!(v.runtime_source, "project");
        assert_eq!(v.runtimes.len(), 2);
        assert!(v.runtimes[0].auto && v.runtimes[0].selected);
        assert_eq!(v.args.value.as_deref(), Some("--v"));
        assert_eq!(v.args.source, "file");
        assert_eq!(
            v.cwd,
            Sourced {
                value: None,
                source: "default".into()
            }
        );
        assert_eq!(v.env.len(), 1);
        assert_eq!(v.warnings, vec!["注意".to_string()]);
        assert_eq!(
            v.runtime.as_ref().map(RuntimeRow::short_label).as_deref(),
            Some(".venv 3.12")
        );
    }

    #[test]
    fn 実行環境の無い拡張子は節を出さない() {
        let v = RunView::from_json(&json!({
            "profiles": [{ "profile": "default", "command": "bash a.sh", "cwd": "/p" }],
            "default_profile": "default",
            "config": { "runtime": {} },
            "runtime": null,
            "runtimes": [],
        }));
        assert!(v.runtime_kind.is_none());
        assert!(v.run_capable());
        assert_eq!(v.selected_profile(), "default");
    }

    #[test]
    fn 空と等号の無い環境変数は保存しない() {
        for target in [
            EditTarget::Args,
            EditTarget::Cwd,
            EditTarget::Before,
            EditTarget::RuntimePath,
            EditTarget::Env { original: None },
        ] {
            assert!(build_write(&target, "", None).is_err(), "{target:?}: 空");
            assert!(
                build_write(&target, "   ", None).is_err(),
                "{target:?}: 空白だけ"
            );
        }
        let env = EditTarget::Env { original: None };
        assert!(build_write(&env, "NOEQUAL", None).is_err());
        assert!(build_write(&env, "=v", None).is_err());
        // 値は空でもよい（空の環境変数は正当）・名前の検めは dispatch に任せる（ここでは通す）
        assert!(build_write(&env, "A=", None).is_ok());
        assert!(build_write(&env, "1BAD=x", None).is_ok());
    }

    #[test]
    fn 環境変数の改名は元の名前を消して足す() {
        let w = build_write(
            &EditTarget::Env {
                original: Some("OLD".into()),
            },
            "NEW=1=2",
            Some("project"),
        )
        .unwrap();
        assert_eq!(w.scope, Some("project"));
        assert_eq!(
            w.env,
            Some(BTreeMap::from([
                ("NEW".to_string(), Some("1=2".to_string())),
                ("OLD".to_string(), None),
            ]))
        );
        // 同じ名前なら消さない
        let w = build_write(
            &EditTarget::Env {
                original: Some("A".into()),
            },
            "A=x",
            None,
        )
        .unwrap();
        assert_eq!(
            w.env,
            Some(BTreeMap::from([("A".to_string(), Some("x".to_string()))]))
        );
    }

    #[test]
    fn 出典タグで保存先を移す書き込み() {
        let v = RunView::from_json(&sample());
        let [set, unset] = move_writes(&Item::Args, &v).unwrap();
        assert_eq!(
            (set.scope, set.args.as_deref()),
            (Some("project"), Some("--v"))
        );
        assert_eq!(
            (unset.scope, unset.reset),
            (Some("file"), Some(vec!["args".to_string()]))
        );

        let [set, unset] = move_writes(&Item::Runtime, &v).unwrap();
        assert_eq!(
            (set.scope, set.runtime.as_deref()),
            (Some("file"), Some("venv:/p/.venv"))
        );
        assert_eq!(unset.reset, Some(vec!["runtime".to_string()]));

        let [set, unset] = move_writes(&Item::Env("APP_ENV".into()), &v).unwrap();
        assert_eq!(set.scope, Some("file"));
        assert_eq!(
            set.env,
            Some(BTreeMap::from([(
                "APP_ENV".to_string(),
                Some("dev".to_string())
            )]))
        );
        assert_eq!(
            unset.env,
            Some(BTreeMap::from([("APP_ENV".to_string(), None)]))
        );

        // 保存していない（既定・自動・宣言）ものは移さない
        assert!(move_writes(&Item::Cwd, &v).is_none());
        assert!(reset_write(&Item::Before, &v).is_none());
        let reset = reset_write(&Item::Profile, &v).unwrap();
        assert_eq!(
            (reset.scope, reset.reset),
            (Some("file"), Some(vec!["profile".to_string()]))
        );
    }

    #[test]
    fn 候補が多ければ畳み選んでいる候補は残す() {
        let rows = |n: usize, selected: usize| -> Vec<RuntimeRow> {
            (0..n)
                .map(|i| RuntimeRow {
                    id: format!("conda:e{i}"),
                    kind: "python".into(),
                    label: format!("e{i}"),
                    version: None,
                    auto: i == 0,
                    selected: i == selected,
                })
                .collect()
        };
        // 少なければ全部
        assert_eq!(visible_runtimes(&rows(3, 0), false), (vec![0, 1, 2], 0));
        assert_eq!(
            visible_runtimes(&rows(5, 4), false),
            (vec![0, 1, 2, 3, 4], 0)
        );
        // 多ければ先頭 5 件 + 隠した件数
        assert_eq!(
            visible_runtimes(&rows(9, 1), false),
            (vec![0, 1, 2, 3, 4], 4)
        );
        // 選んでいる候補が畳んだ側にあっても出す
        assert_eq!(
            visible_runtimes(&rows(9, 7), false),
            (vec![0, 1, 2, 3, 4, 7], 3)
        );
        // 広げれば全部
        assert_eq!(visible_runtimes(&rows(9, 7), true).0.len(), 9);
        // 候補 0
        assert_eq!(visible_runtimes(&[], false), (vec![], 0));
    }

    /// `RunResolve` の `board` 欄の形（#1952 の dispatch が返すもの。Windows の COM ポートを含む）
    fn sketch_sample() -> Value {
        json!({
            "profiles": [{ "profile": "default", "command": "arduino-cli compile", "cwd": "/s/Blink" }],
            "default_profile": "default",
            "config": { "runtime": {} },
            "board": {
                "sketch": "/s/Blink",
                "fqbn": "arduino:avr:uno",
                "name": "Arduino UNO",
                "port": "COM9",
                "profile": null,
                "core_installed": true,
                "candidates": [
                    { "fqbn": "arduino:avr:uno", "name": "Arduino Uno", "port": "COM3", "connected": true, "installed": true },
                    { "fqbn": "arduino:renesas_uno:unor4wifi", "name": "Arduino UNO R4 WiFi", "port": "COM7", "connected": true, "installed": false },
                ],
                "ports": [
                    { "address": "COM3", "label": "COM3", "protocol": "serial", "protocol_label": "Serial Port (USB)", "boards": ["Arduino Uno"] },
                    { "address": "COM1", "label": "COM1", "protocol": "serial", "protocol_label": "Serial Port", "boards": [] },
                ],
                "installed_boards": 3,
                "matches": [
                    { "fqbn": "arduino:avr:uno", "name": "Arduino UNO" },
                    { "fqbn": "arduino:avr:nano", "name": "Arduino Nano" },
                    { "fqbn": "arduino:avr:mega", "name": "Arduino Mega or Mega 2560" },
                ],
                "notes": ["注意"],
                "errors": ["board list: 打ち切った"],
            },
        })
    }

    #[test]
    fn 基板の欄を写す() {
        let v = RunView::from_json(&sketch_sample());
        let b = v.board.expect("board 欄");
        assert_eq!(b.fqbn.as_deref(), Some("arduino:avr:uno"));
        assert_eq!(b.label().as_deref(), Some("Arduino UNO"));
        assert_eq!(b.port.as_deref(), Some("COM9"));
        assert_eq!(b.candidates.len(), 2);
        assert_eq!(b.candidates[1].installed, Some(false));
        let ports = b.ports.unwrap();
        assert_eq!(
            ports.iter().map(|p| p.address.as_str()).collect::<Vec<_>>(),
            vec!["COM3", "COM1"]
        );
        assert_eq!(ports[0].detail, "Serial Port (USB) · Arduino Uno");
        assert_eq!(b.installed.len(), 3);
        assert!(b.installed.iter().all(|r| r.installed == Some(true)));
        // 注意と聞けなかった理由はどちらも節の中に出す
        assert_eq!(
            b.notes,
            vec!["注意".to_string(), "board list: 打ち切った".to_string()]
        );

        // スケッチでないファイル（board = null）は節を出さない・ports が null = 聞けていない
        assert!(RunView::from_json(&sample()).board.is_none());
        let mut no_ports = sketch_sample();
        no_ports["board"]["ports"] = Value::Null;
        assert!(RunView::from_json(&no_ports).board.unwrap().ports.is_none());
    }

    #[test]
    fn 基板を探す欄の絞り込みとenter() {
        let b = RunView::from_json(&sketch_sample()).board.unwrap();
        let rows = &b.installed;
        assert_eq!(filter_boards(rows, "", 8).len(), 3);
        assert_eq!(filter_boards(rows, "", 2).len(), 2, "上限で打ち切る");
        assert_eq!(
            filter_boards(rows, "NANO", 8)
                .iter()
                .map(|r| r.fqbn.as_str())
                .collect::<Vec<_>>(),
            vec!["arduino:avr:nano"]
        );
        assert!(filter_boards(rows, "esp32", 8).is_empty());
        // 1 件に絞れたらその基板・FQBN そのものならそのまま・当たらなければ打った文字列（dispatch が検める）
        assert_eq!(board_choice(rows, " nano "), "arduino:avr:nano");
        assert_eq!(board_choice(rows, "arduino:avr:uno"), "arduino:avr:uno");
        assert_eq!(board_choice(rows, "arduino"), "arduino");
        assert_eq!(
            board_choice(rows, "arduino:renesas_uno:unor4wifi"),
            "arduino:renesas_uno:unor4wifi"
        );
    }

    #[test]
    fn 基板とポートの欄の書き込み() {
        let w = build_write(&EditTarget::Board, " arduino:avr:uno ", None).unwrap();
        assert_eq!(w.board.as_deref(), Some("arduino:avr:uno"));
        let w = build_write(&EditTarget::Port, "COM3", None).unwrap();
        assert_eq!(w.port.as_deref(), Some("COM3"));
        for t in [EditTarget::Board, EditTarget::Port] {
            assert!(build_write(&t, "  ", None).is_err());
            assert!(t.writes_sketch());
        }
        assert!(!EditTarget::Args.writes_sketch());
        match (Write {
            board: Some("arduino:avr:uno".into()),
            port: Some("COM3".into()),
            ..Write::default()
        })
        .into_request(Path::new("/s/Blink/Blink.ino"))
        {
            Request::RunnerDefaults {
                path,
                board,
                port,
                scope,
                ..
            } => {
                assert_eq!(path.as_deref(), Some("/s/Blink/Blink.ino"));
                assert_eq!(board.as_deref(), Some("arduino:avr:uno"));
                assert_eq!(port.as_deref(), Some("COM3"));
                assert!(scope.is_none(), "基板・ポートは tako の保存先に持たない");
            }
            other => panic!("RunnerDefaults ではない: {other:?}"),
        }
    }

    #[test]
    fn 書き込みは実行設定の口へ渡る() {
        let req = Write {
            scope: Some("file"),
            args: Some("--x".into()),
            ..Write::default()
        }
        .into_request(Path::new("/p/a.py"));
        match req {
            Request::RunnerDefaults {
                ext,
                path,
                scope,
                args,
                ..
            } => {
                assert!(ext.is_none(), "拡張子既定の側へは行かない");
                assert_eq!(path.as_deref(), Some("/p/a.py"));
                assert_eq!(scope.as_deref(), Some("file"));
                assert_eq!(args.as_deref(), Some("--x"));
            }
            other => panic!("RunnerDefaults ではない: {other:?}"),
        }
    }
}
