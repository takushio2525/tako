//! Arduino のスケッチの基板とポートの読み書き（Issue #1952 / FR-3.18.8）。
//!
//! 判定・読み方・案内の文は `tako_core::sketch_board`（純粋）にあり、ここは **arduino-cli を起こす**層:
//! `board list`（繋がっている基板とポート）・`board listall`（入っているコアの基板）・`board attach`
//! （`sketch.yaml` へ書く = 書式を tako が真似しない）。起こすのは実行環境の層（#1730 の `${arduino_cli}`）が
//! 見つけた実体で、待ちは Tier P と同じ上限（`probe::runtime_probe_timeout`）。
//!
//! **UI スレッドで子プロセスを待たない**: 呼ぶのは `RunResolve` の background（[`resolve_block`]）と、
//! `.ino` の `RunnerDefaults` の background（`prepare_offload` の `RunConfigSketch`）。offload を使わない
//! host の同期の腕は [`handle_path`] を `probe = false` で呼び、**読むだけなら子プロセスを起こさない**
//! （`sketch.yaml` の 1 枚の読みだけ）。書くときは同期の腕でも起こす（GUI は必ず background を通る =
//! 番犬 `issue1952_sketch_board_watchdog`）。
//!
//! コアの導入（`arduino-cli core install`）は**示すだけで起こさない**（数百 MB。設計 §5.5）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tako_core::platform::support::Platform;
use tako_core::sketch_board as sb;

use crate::dispatch::DispatchError;
use crate::runtime_probe::{self, Prober, Snapshot};

/// 基板・ポートの書き込み（どちらも `None` = 書かない）
#[derive(Debug, Clone, Copy, Default)]
pub struct BoardEdit<'a> {
    pub board: Option<&'a str>,
    pub port: Option<&'a str>,
}

impl BoardEdit<'_> {
    fn given(&self) -> bool {
        self.board.is_some() || self.port.is_some()
    }
}

/// arduino-cli に聞いた答え（聞いていないものは `None`）
#[derive(Debug, Default)]
struct Probed {
    /// `board list` の生の出力（#1950 の案内の `Followup` と 1 回で共有する）
    list_stdout: Option<String>,
    ports: Option<Result<Vec<sb::DetectedPort>, String>>,
    installed: Option<Result<Vec<sb::InstalledBoard>, String>>,
}

impl Probed {
    fn ports(&self) -> Option<&[sb::DetectedPort]> {
        self.ports.as_ref()?.as_ref().ok().map(Vec::as_slice)
    }

    fn installed(&self) -> Option<&[sb::InstalledBoard]> {
        self.installed.as_ref()?.as_ref().ok().map(Vec::as_slice)
    }

    fn errors(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(Err(e)) = &self.ports {
            out.push(e.clone());
        }
        if let Some(Err(e)) = &self.installed {
            out.push(e.clone());
        }
        out
    }
}

/// arduino-cli を 1 回起こして標準出力を返す（失敗・打ち切りは理由の文）
fn run_cli(
    prober: &dyn Prober,
    cli: &Path,
    args: &[&str],
    cwd: &Path,
    budget: Duration,
) -> Result<String, String> {
    let label = tako_core::probe::label(&cli.to_string_lossy(), args);
    match prober.output(&cli.to_string_lossy(), args, Some(cwd), budget) {
        tako_core::probe::Outcome::Done {
            status,
            stdout,
            stderr,
        } => {
            if status.success() {
                Ok(String::from_utf8_lossy(&stdout).into_owned())
            } else {
                let err = String::from_utf8_lossy(&stderr);
                let tail = err.trim().lines().last().unwrap_or_default();
                Err(format!("{label}: {status} {tail}").trim().to_string())
            }
        }
        tako_core::probe::Outcome::TimedOut { waited, .. } => Err(msg_timed_out(&label, waited)),
        tako_core::probe::Outcome::Failed { reason, .. } => Err(format!("{label}: {reason}")),
    }
}

/// `board list` と `board listall` を並べて聞く（どちらも 0.4〜3 秒。実測 1.2.0）
fn probe_all(prober: &dyn Prober, cli: &Path, sketch: &Path, budget: Duration) -> Probed {
    let (list, listall) = std::thread::scope(|s| {
        let list = s.spawn(|| run_cli(prober, cli, sb::LIST_ARGS, sketch, budget));
        let listall = run_cli(prober, cli, sb::LISTALL_ARGS, sketch, budget);
        (
            list.join()
                .unwrap_or_else(|_| Err("board list: panicked".into())),
            listall,
        )
    });
    let mut out = Probed::default();
    fill_list(&mut out, list);
    fill_listall(&mut out, listall);
    out
}

fn fill_list(out: &mut Probed, list: Result<String, String>) {
    out.ports = Some(list.as_ref().map_err(Clone::clone).and_then(|stdout| {
        sb::parse_board_list(stdout).ok_or_else(|| msg_unreadable("board list"))
    }));
    out.list_stdout = list.ok();
}

fn fill_listall(out: &mut Probed, listall: Result<String, String>) {
    out.installed = Some(listall.and_then(|stdout| {
        sb::parse_listall(&stdout).ok_or_else(|| msg_unreadable("board listall"))
    }));
}

/// 実行環境の層が見つけた arduino-cli の実体（名前のまま = 見つからなかったなら `None`）
fn cli_of(snap: Option<&Snapshot>) -> Option<PathBuf> {
    runtime_probe::selected_program(snap)
}

// ─── `RunResolve` ──────────────────────────────────────────────────────

/// `RunResolve`（background）の `board` 欄。`board list` / `board listall` を聞いて候補を組み、
/// `board list` の答えは #1950 の案内（繋がった基板が 1 台ならコマンドへ入れる）にも渡す
pub fn resolve_block(
    project: &mut tako_core::runner_project::ProjectMatch,
    sketch: &Path,
    snap: Option<&Snapshot>,
    query: Option<&str>,
) -> Value {
    let cli = cli_of(snap);
    resolve_block_with(
        project,
        sketch,
        cli.as_deref(),
        query,
        runtime_probe::default_prober(),
        tako_core::probe::runtime_probe_timeout(),
        &|project| runtime_probe::apply_project_followup(project, snap),
    )
}

/// `other_followup` = 案内の問い合わせが `board list` でないとき（表の行を変えたとき）の従来の経路
fn resolve_block_with(
    project: &mut tako_core::runner_project::ProjectMatch,
    sketch: &Path,
    cli: Option<&Path>,
    query: Option<&str>,
    prober: &dyn Prober,
    budget: Duration,
    other_followup: &dyn Fn(&mut tako_core::runner_project::ProjectMatch),
) -> Value {
    let probed = match cli {
        Some(cli) => probe_all(prober, cli, sketch, budget),
        None => Probed::default(),
    };
    // 案内の問い合わせが同じ `board list` なら答えを共有する（2 回起こさない）
    match project.followup() {
        Some(f) if f.args == sb::LIST_ARGS => {
            if let Some(stdout) = &probed.list_stdout {
                project.apply_followup(stdout, Platform::current());
            }
        }
        Some(_) => other_followup(project),
        None => {}
    }
    block_json(sketch, &probed, Some(cli), query)
}

// ─── `RunnerDefaults { path }`（`tako run-config` / MCP `tako_run_defaults`） ──

/// 実行設定の読み書き（#1731 / #1732 の `run_configs::handle`）に、`.ino` の基板とポートを重ねる。
///
/// - 書く（`edit` あり）: **先に検める**（スケッチか・arduino-cli があるか・基板のコアが入っているか・
///   その基板があるか・ポートの形）→ 実行設定の他の項目 → `board attach` → 基板を変えたなら
///   そのスケッチの言語サーバを起こし直す。検めで断ったら何も書かない（`board attach` そのものが落ちたときは、
///   同じ呼び出しの実行設定の他の項目は保存済み = エラーの文は attach の失敗だけを言う）
/// - `probe` = 読むときに候補も聞く（background の経路）。`false` なら `sketch.yaml` を読むだけ。
///   書いたときは聞かない（検めの listall だけを使う。候補は書いた後の読み・`RunResolve` が聞く）
pub fn handle_path(
    args: crate::run_configs::PathArgs<'_>,
    edit: BoardEdit<'_>,
    probe: bool,
    lsp: Option<&crate::lsp::LspManager>,
) -> Result<Value, DispatchError> {
    handle_path_with(
        args,
        edit,
        probe,
        lsp,
        runtime_probe::default_prober(),
        tako_core::probe::runtime_probe_timeout(),
        &|file| cli_of(runtime_probe::for_resolve(file, false).as_ref()),
    )
}

fn handle_path_with(
    args: crate::run_configs::PathArgs<'_>,
    edit: BoardEdit<'_>,
    probe: bool,
    lsp: Option<&crate::lsp::LspManager>,
    prober: &dyn Prober,
    budget: Duration,
    find_cli: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Result<Value, DispatchError> {
    let file = args.file;
    // 前後の空白は落とす（CLI はそのまま渡す。空になれば形の検めで断る）
    let edit = BoardEdit {
        board: edit.board.map(str::trim),
        port: edit.port.map(str::trim),
    };
    if edit.given() && sb::legacy() {
        return Err(DispatchError::InvalidParams(
            "board / port はこの版では使えない（TAKO_1952_LEGACY=1）".into(),
        ));
    }
    let sketch = sb::sketch_dir_of(file);
    if edit.given() && sketch.is_none() {
        return Err(DispatchError::InvalidParams(msg_not_sketch(file)));
    }

    // 書く前に検める（断ったら実行設定の他の項目も書かない）
    let mut probed = Probed::default();
    let cli = match &sketch {
        Some(_) if edit.given() || probe => find_cli(file),
        _ => None,
    };
    if let (Some(sketch), true) = (&sketch, edit.given()) {
        let cli = cli
            .as_deref()
            .ok_or_else(|| DispatchError::Operation(msg_cli_missing()))?;
        let installed: Vec<sb::InstalledBoard> = if edit.board.is_some() {
            let listall = run_cli(prober, cli, sb::LISTALL_ARGS, sketch, budget);
            fill_listall(&mut probed, listall);
            match &probed.installed {
                Some(Ok(list)) => list.clone(),
                Some(Err(e)) => {
                    return Err(DispatchError::Operation(msg_listall_failed(e)));
                }
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        sb::check(edit.board, edit.port, &installed).map_err(|r| {
            DispatchError::InvalidParams(sb::rejection_message(
                &r,
                &cli.to_string_lossy(),
                Platform::current(),
            ))
        })?;
    }

    let mut value = crate::run_configs::handle(args)?;
    let Some(sketch) = sketch else {
        return Ok(value);
    };

    let mut attached = Value::Null;
    let mut lsp_restart = Value::Null;
    if let (Some(cli), true) = (cli.as_deref(), edit.given()) {
        let argv = sb::attach_args(edit.board, edit.port, &sketch);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        run_cli(prober, cli, &argv, &sketch, budget)
            .map_err(|e| DispatchError::Operation(msg_attach_failed(&e)))?;
        crate::diag::persist_log(&format!(
            "sketch.yaml を arduino-cli board attach で書いた（#1952）: board={} port={}",
            edit.board.is_some(),
            edit.port.is_some()
        ));
        attached = json!({ "board": edit.board, "port": edit.port });
        // 言語サーバは基板で組み直す（ポートは見ない）。前提（基板）つきの器だけ・このスケッチだけ
        if edit.board.is_some() {
            if let Some(lsp) = lsp {
                lsp_restart = lsp.restart_requiring(&sketch);
            }
        }
    }
    // 候補を聞くのは読むときだけ。書いた後は聞き直さない（`board list` は 2〜3 秒かかり、ドロップダウンは
    // 書いた後に `RunResolve` を取り直す = 同じ問い合わせを 2 回待たせない。検めで聞いた listall は使う）
    if let (Some(cli), true) = (cli.as_deref(), probe && !edit.given()) {
        probed = probe_all(prober, cli, &sketch, budget);
    }
    if let Value::Object(map) = &mut value {
        map.insert(
            "board".into(),
            block_json(
                &sketch,
                &probed,
                (edit.given() || probe).then_some(cli.as_deref()),
                None,
            ),
        );
        if edit.given() {
            map.insert("attached".into(), attached);
            map.insert("lsp".into(), lsp_restart);
        }
    }
    Ok(value)
}

// ─── 応答 ──────────────────────────────────────────────────────────────

/// `board` 欄（`RunResolve` と `RunnerDefaults` で同じ形）。`ports` / `installed_boards` / `matches` は
/// 聞いていなければ `null`（= 分からない。空の配列 = 聞いて 0 件）。`looked` = arduino-cli を探したか
/// （`Some(None)` = 探して見つからない。`None` = 探していない = 読むだけの同期の腕）
fn block_json(
    sketch: &Path,
    probed: &Probed,
    looked: Option<Option<&Path>>,
    query: Option<&str>,
) -> Value {
    let cli = looked.flatten();
    let d = sb::sketch_defaults(sketch);
    let installed = probed.installed();
    let ports = probed.ports();
    let fqbn = d.fqbn.as_deref();
    let core_installed = fqbn.and_then(|f| {
        let list = installed?;
        let core = sb::core_of(f)?;
        Some(list.iter().any(|b| b.core == core))
    });
    let mut notes: Vec<String> = Vec::new();
    if let Some(p) = &d.profile {
        notes.push(sb::profile_overrides_note(p));
    }
    if let (Some(false), Some(f)) = (core_installed, fqbn) {
        let cli = cli.map_or_else(|| "arduino-cli".to_string(), |c| c.display().to_string());
        notes.push(sb::rejection_message(
            &sb::Rejection::CoreMissing {
                fqbn: f.to_string(),
                core: sb::core_of(f).unwrap_or_default().to_string(),
            },
            &cli,
            Platform::current(),
        ));
    }
    let mut errors = probed.errors();
    if looked == Some(None) {
        errors.push(msg_cli_missing());
    }
    json!({
        "sketch": sketch.display().to_string(),
        "fqbn": d.fqbn,
        "name": fqbn.and_then(|f| sb::name_of(installed?, f)),
        "port": d.port,
        "profile": d.profile,
        "core_installed": core_installed,
        "candidates": sb::candidates(ports, installed, fqbn)
            .into_iter()
            .map(|c| json!({
                "fqbn": c.fqbn,
                "name": c.name,
                "port": c.port,
                "connected": c.connected,
                "installed": c.installed,
            }))
            .collect::<Vec<_>>(),
        "ports": ports.map(|ps| ps.iter().filter(|p| !p.address.is_empty()).map(|p| json!({
            "address": p.address,
            "label": p.label,
            "protocol": p.protocol,
            "protocol_label": p.protocol_label,
            "boards": p.boards.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>()),
        "installed_boards": installed.map(<[_]>::len),
        "matches": query.and_then(|q| installed.map(|list| sb::matches(list, q)
            .into_iter()
            .map(|b| json!({ "fqbn": b.fqbn, "name": b.name }))
            .collect::<Vec<_>>())),
        "notes": notes,
        "errors": errors,
    })
}

// ─── 文（日英） ─────────────────────────────────────────────────────────

fn tr(ja: String, en: String) -> String {
    match tako_core::i18n::lang() {
        tako_core::i18n::Lang::Ja => ja,
        tako_core::i18n::Lang::En => en,
    }
}

fn msg_not_sketch(file: &Path) -> String {
    let f = file.display();
    tr(
        format!("基板とポートを選べるのは Arduino のスケッチ（.ino）だけ（指定: {f}）"),
        format!("A board and port can only be selected for an Arduino sketch (.ino) (got {f})"),
    )
}

fn msg_cli_missing() -> String {
    tr(
        "arduino-cli が見つからないので基板の一覧を聞けない（Arduino IDE か arduino-cli を入れると見つかる。\
         macOS: brew install --cask arduino-ide / Windows: winget install --id ArduinoSA.IDE.stable）"
            .to_string(),
        "arduino-cli was not found, so the boards cannot be listed (install Arduino IDE or arduino-cli. \
         macOS: brew install --cask arduino-ide / Windows: winget install --id ArduinoSA.IDE.stable)"
            .to_string(),
    )
}

fn msg_listall_failed(e: &str) -> String {
    tr(
        format!("入っているコアの基板の一覧を取れないので、確かめずには書かない（sketch.yaml は変えていない）: {e}"),
        format!("Could not list the boards of the installed cores, so nothing was written without checking (sketch.yaml is unchanged): {e}"),
    )
}

fn msg_attach_failed(e: &str) -> String {
    tr(
        format!("arduino-cli board attach が失敗した: {e}"),
        format!("arduino-cli board attach failed: {e}"),
    )
}

fn msg_unreadable(what: &str) -> String {
    tr(
        format!("{what} の答えを読めない（arduino-cli の版が想定と違う）"),
        format!("Could not read the output of {what} (unexpected arduino-cli version)"),
    )
}

fn msg_timed_out(label: &str, waited: Duration) -> String {
    let secs = waited.as_secs_f32();
    tr(
        format!("{label}: {secs:.1} 秒で打ち切った"),
        format!("{label}: timed out after {secs:.1}s"),
    )
}

#[cfg(test)]
mod tests;
