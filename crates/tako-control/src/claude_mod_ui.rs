//! `tako mod ui`（定型の UI 設定 `<data_dir>/claude-mod/ui.json`。FR-2.42.19〜24 / #1960）の tako-control 側
//!
//! 型・語彙・既定・検証・操作の正本は `tako_core::claude_mod_ui`。ここが持つのは
//!
//! - 置き場の排他（`config_io::lock_exclusive`）と読み書き（core の `update_at`）
//! - 壊れた ui.json の persist.log への記録（**壊れた中身ごとに 1 回**。退避は core が #916 の機構で行う）
//! - 応答の JSON（CLI `tako mod ui` と MCP `tako_mod` の `action=ui` が**同じもの**を返す）
//! - setup の段（標準は 1 行の表示・`--answers` の `mod_ui`・`--review` の対話の受け皿）
//! - mod の報告の帯のトグルの取り込み（S3 の `$.store` から正本を移した = [`import_band`]）
//!
//! だけ。CLI（ローカル処理 = GUI が動いていなくても効く）・dispatch（MCP と GUI）・setup が
//! すべて [`run`] / [`apply_answers`] を通る（設計原則 5 の 1:1）。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tako_core::claude_mod::ModBand;
use tako_core::claude_mod_ui::{self as core, LoadState, Loaded, UiAnswers, UiOp, UiRequest};

/// [`run`] が断った理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// 値が語彙・形に合わない（ui.json は書いていない。許される値は文言に載る）
    Invalid(String),
    /// 書けない・新しい形なので書き換えない
    Refused(String),
}

impl RunError {
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(m) | Self::Refused(m) => m,
        }
    }
}

/// 既定の置き場（`<data_dir>/claude-mod/ui.json`）
pub fn default_path() -> Result<PathBuf, String> {
    core::ui_path().ok_or_else(|| "データディレクトリが決まらない（HOME が無い）".into())
}

/// 壊れた中身の申告の重複除け（settings.json と同じ規則 = 壊れた中身ごとに 1 回。#1819）
static UNREADABLE_SEEN: crate::settings::UnreadableSeen = crate::settings::UnreadableSeen::new();

/// 読む。壊れていれば読める部分で動き（core が `.unreadable.bak` へ保全する）、persist.log に 1 行残す
pub fn load(path: &Path) -> Loaded {
    let loaded = core::load_from(path);
    record(path, &loaded.state);
    loaded
}

/// 読んだ様子を persist.log へ（壊れていたときだけ。**利用者が書いた値は載せない**）
fn record(path: &Path, state: &LoadState) {
    match state {
        LoadState::Repaired {
            problems,
            quarantine,
        } => {
            // 記録は**壊れた中身ごとに 1 回**: 退避が今回新しく写した（= この中身を初めて見た）ときだけ。
            // CLI は呼ぶたびに別プロセスなので、プロセス内の重複除けだけでは毎回 1 行増える
            if quarantine.as_ref().is_some_and(|q| !q.copied) {
                return;
            }
            let body = std::fs::read(path).unwrap_or_default();
            let key = format!(
                "{}\n{}",
                problems.join("\n"),
                tako_core::fnv::fnv1a64(&body)
            );
            if !UNREADABLE_SEEN.first_sighting(path, &key) {
                return;
            }
            let where_to = match quarantine {
                Some(q) if q.evicted => format!(
                    "{}・{}",
                    q.path.display(),
                    tako_core::migration::eviction_note()
                ),
                Some(q) => q.path.display().to_string(),
                None => "（退避できず）".into(),
            };
            crate::diag::persist_log(&format!(
                "claude-mod/ui.json の読めない部分を既定で動かす: {}（{}・退避 {where_to}）",
                path.display(),
                problems.join("; ")
            ));
        }
        LoadState::ReadError { reason } => {
            if UNREADABLE_SEEN.first_sighting(path, reason) {
                crate::diag::persist_log(&format!(
                    "claude-mod/ui.json を読めないので既定で動く: {}（{reason}）",
                    path.display()
                ));
            }
        }
        LoadState::Valid | LoadState::Absent => UNREADABLE_SEEN.forget(path),
        LoadState::Future { .. } => {}
    }
}

/// `tako mod ui` / MCP `tako_mod` の `action=ui` の本体
pub fn run(path: &Path, request: &UiRequest) -> Result<Value, RunError> {
    let op = request
        .to_op()
        .map_err(|e| RunError::Invalid(e.message()))?;
    apply_ops(path, &[op])
}

/// setup の `--answers` の `mod_ui`（雛形 → 値 → ボタンの並び。1 つでも断られたら書かない）
pub fn apply_answers(path: &Path, answers: &UiAnswers) -> Result<Value, RunError> {
    let ops = answers
        .to_ops()
        .map_err(|e| RunError::Invalid(e.message()))?;
    apply_ops(path, &ops)
}

fn apply_ops(path: &Path, ops: &[UiOp]) -> Result<Value, RunError> {
    let mutating = ops.iter().any(|op| !matches!(op, UiOp::Show));
    let loaded = load(path);
    if !mutating {
        return Ok(response(path, &loaded.config, &loaded.state, None));
    }
    // 読み → 当て → 書き を他のプロセス（GUI の取り込み・別の CLI）と直列にする
    let _lock = crate::config_io::lock_exclusive(path).map_err(RunError::Refused)?;
    match core::update_at(path, ops, crate::claude_mod::epoch_ms()) {
        Ok(updated) => Ok(response(
            path,
            &updated.config,
            &updated.before,
            Some((updated.changed, updated.written)),
        )),
        Err(core::UpdateError::Invalid(e)) => Err(RunError::Invalid(e.message())),
        Err(e) => Err(RunError::Refused(e.message())),
    }
}

/// 応答の形（CLI の `--json` と MCP が同じ）。`changed` は変更の操作のときだけ載る
fn response(
    path: &Path,
    config: &core::UiConfig,
    state: &LoadState,
    changed: Option<(bool, bool)>,
) -> Value {
    let mut out = json!({
        "path": path.display().to_string(),
        "state": state.code(),
        "ui": config,
        "values": core::current_values(config),
    });
    if let LoadState::Repaired {
        problems,
        quarantine,
    } = state
    {
        out["problems"] = json!(problems);
        out["quarantine"] = json!(quarantine.as_ref().map(|q| q.path.display().to_string()));
    }
    if let LoadState::Future { version } = state {
        out["note"] = json!(format!(
            "新しい tako が書いた形（schema_version {version}）。読める範囲で使い、書き換えない"
        ));
    }
    match changed {
        Some((changed, written)) => {
            out["changed"] = json!(changed);
            out["written"] = json!(written);
            out["applies_to"] = json!(
                "tako のペインの claude へは次の報告（最大 15 秒）で届く（ui.json は tako の data dir にあり、\
                 Claude Code の設定 dir へは書かない）"
            );
        }
        None => out["choices"] = core::choices(),
    }
    out
}

/// 報告の応答（`tako.view.ui`）に載せる検証済みの値
pub fn view(path: &Path) -> core::UiConfig {
    load(path).config
}

/// mod の報告の帯の状態を取り込む（報告の `toggled_at` が ui.json より新しいときだけ書く）。
/// 書けなくても報告の受け取りは止めない（帯のトグルが mod の `$.store` のままになるだけ）
pub fn import_band(path: &Path, band: &ModBand) {
    let Some(at) = band.toggled_at else {
        return;
    };
    // 安い先読み（ほとんどの報告は取り込むものが無い = ロックを取らない）
    let current = core::load_from(path);
    if current
        .config
        .band
        .toggled_at
        .is_some_and(|known| known >= at)
        || matches!(current.state, LoadState::Future { .. })
    {
        return;
    }
    let Ok(_lock) = crate::config_io::lock_exclusive(path) else {
        return;
    };
    let mut loaded = load(path);
    if matches!(loaded.state, LoadState::Future { .. }) {
        return;
    }
    if core::import_band(&mut loaded.config, band.hidden, at) {
        if let Err(e) = core::save_to(path, &loaded.config) {
            crate::diag::persist_log(&format!(
                "claude-mod/ui.json へ帯のトグルを取り込めない: {}（{e}）",
                path.display()
            ));
        }
    }
}

/// `tako mod`（status）に載せる要約
pub fn status_json(path: &Path, loaded: &Loaded) -> Value {
    json!({
        "path": path.display().to_string(),
        "state": loaded.state.code(),
        "band_hidden": loaded.config.band.hidden,
        "buttons": loaded.config.buttons.iter().map(|b| b.id.clone()).collect::<Vec<_>>(),
    })
}

/// `tako mod ui …` の引数（`ui` の後ろの語と `--label` / `--hotkey` / `--id`）を [`UiRequest`] へ。
/// MCP の引数と**同じ形**に落とすので、ここから先は 1 実装（[`run`]）。
///
/// - （なし）/ `show` → 今の値と選べる値
/// - `set <キー> <値…>`（値は空白区切りでも `,` 区切りでもよい）
/// - `button add [<kind>] <中身>` / `button remove <id>` / `button move <id> <先>`
///   （`button` は省いてもよい = `tako mod ui add compact`）
/// - `reset` / `preset <default|recommended|minimal>`
pub fn cli_request(
    words: &[String],
    label: Option<&str>,
    hotkey: Option<&str>,
    id: Option<&str>,
) -> Result<UiRequest, String> {
    use tako_core::claude_mod_ui::{ButtonKind, Preset, SetKey, Vocab};
    let usage = "使い方: tako mod ui [set <キー> <値> | button add [<kind>] <中身> | \
                 button remove <id> | button move <id> <先> | preset <名前> | reset]";
    let mut words: Vec<&str> = words.iter().map(String::as_str).collect();
    if matches!(words.first(), Some(&("button" | "buttons"))) {
        words.remove(0);
        if words.is_empty() {
            return Err(format!(
                "button の後ろに add / remove / move を付ける。{usage}"
            ));
        }
    }
    let is_add = matches!(words.first(), Some(&("add" | "button_add")));
    if !is_add && (label.is_some() || hotkey.is_some() || (id.is_some() && words.is_empty())) {
        return Err("--label / --hotkey / --id は button add のときだけ使う".into());
    }
    let request = |op: &str| UiRequest {
        op: Some(op.into()),
        ..UiRequest::default()
    };
    match words.as_slice() {
        [] | ["show" | "list"] => Ok(UiRequest::default()),
        ["set"] | ["set", _] => Err(format!(
            "set にはキーと値が要る（tako mod ui set <キー> <値>）。キー: {}",
            SetKey::words().join(" / ")
        )),
        ["set", key, value @ ..] => Ok(UiRequest {
            key: Some((*key).into()),
            value: Some(Value::String(value.join(" "))),
            ..request("set")
        }),
        ["add" | "button_add"] => Err(format!(
            "button add には中身が要る（例: tako mod ui button add compact）。kind: {}",
            ButtonKind::words().join(" / ")
        )),
        ["add" | "button_add", value] => Ok(UiRequest {
            value: Some(Value::String((*value).into())),
            label: label.map(Into::into),
            hotkey: hotkey.map(Into::into),
            id: id.map(Into::into),
            ..request("button_add")
        }),
        // 2 語なら前が kind（語彙の外でも kind として渡す = MCP と同じ理由で断られる）
        ["add" | "button_add", kind, value] => Ok(UiRequest {
            kind: Some((*kind).into()),
            value: Some(Value::String((*value).into())),
            label: label.map(Into::into),
            hotkey: hotkey.map(Into::into),
            id: id.map(Into::into),
            ..request("button_add")
        }),
        ["add" | "button_add", ..] => Err(
            "コマンド・文は 1 つの引数で渡す（例: tako mod ui button add shell \"npm test\"）"
                .into(),
        ),
        ["remove" | "rm" | "button_remove", target] => Ok(UiRequest {
            id: Some((*target).into()),
            ..request("button_remove")
        }),
        ["move" | "mv" | "button_move", target, to] => Ok(UiRequest {
            id: Some((*target).into()),
            to: Some(Value::String((*to).into())),
            ..request("button_move")
        }),
        ["remove" | "rm" | "button_remove", ..] => {
            Err("button remove には id を 1 つ付ける（id は tako mod ui の一覧）".into())
        }
        ["move" | "mv" | "button_move", ..] => {
            Err("button move には id と先を付ける（先: 1〜8 / first / last / left / right）".into())
        }
        ["reset"] => Ok(request("reset")),
        ["preset", name] => Ok(UiRequest {
            value: Some(Value::String((*name).into())),
            ..request("preset")
        }),
        ["preset", ..] => Err(format!(
            "preset には名前を 1 つ付ける: {}",
            Preset::words().join(" / ")
        )),
        _ => Err(usage.into()),
    }
}

/// 1 行の要約（setup の表示）
pub fn summary(config: &core::UiConfig) -> String {
    use tako_core::claude_mod_ui::Vocab;
    let buttons = if config.buttons.is_empty() {
        "なし".to_string()
    } else {
        config
            .buttons
            .iter()
            .map(|b| b.label.as_str())
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let bar = if config.usage_bar.place == core::UsageBarPlace::Off {
        "off".to_string()
    } else {
        format!(
            "{}（{}）",
            config.usage_bar.place.as_str(),
            config
                .usage_bar
                .items
                .iter()
                .map(|i| i.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let band = if config.band.hidden {
        "帯は隠す・".to_string()
    } else {
        String::new()
    };
    format!("{band}ボタン {buttons}・バー {bar}")
}

/// `tako setup` の段。`answers` があれば当て、無ければ今の値を 1 行で見せる（質問は増やさない = #262）。
/// **setup を止めない**: 書けなかったら `[warn]` の行にする（ui.json が無くても既定で動く）
pub fn run_setup_stage(answers: Option<&UiAnswers>) -> Vec<String> {
    let path = match default_path() {
        Ok(p) => p,
        Err(e) => {
            return vec![format!(
                "  [warn] Claude Code の画面の UI 設定を扱えない: {e}"
            )]
        }
    };
    if let Some(answers) = answers.filter(|a| !a.is_empty()) {
        return match apply_answers(&path, answers) {
            Ok(out) => {
                let config: core::UiConfig =
                    serde_json::from_value(out["ui"].clone()).unwrap_or_default();
                vec![format!(
                    "  [input] Claude Code の画面の UI（ui.json）: {}",
                    summary(&config)
                )]
            }
            Err(e) => vec![format!(
                "  [warn] Claude Code の画面の UI（ui.json）を変えられない: {}",
                e.message()
            )],
        };
    }
    let loaded = load(&path);
    let mut lines = vec![format!(
        "  [ok] Claude Code の画面の UI（tako mod）: {}（変える: tako mod ui）",
        summary(&loaded.config)
    )];
    match &loaded.state {
        LoadState::Repaired { quarantine, .. } => lines.push(format!(
            "  [warn] ui.json の読めない部分は既定で動かす（元は {} に残した。直す: tako mod ui reset）",
            quarantine
                .as_ref()
                .map(|q| tako_core::paths::shorten_home(&q.path.display().to_string()))
                .unwrap_or_else(|| "退避できず".into())
        )),
        LoadState::ReadError { reason } => lines.push(format!(
            "  [warn] ui.json を読めないので既定で動かす（{reason}）"
        )),
        _ => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        // テストの一時の置き場（#944 の隔離先 = プロセスの終わりに消える）。製品が data dir に
        // 置くファイルではないので、式を分けて共有分類カタログの被覆の走査に載せない
        let scratch = tako_core::paths::data_dir().expect("テストの data dir");
        let dir = scratch.join(format!(
            "mod-ui-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(core::FILENAME)
    }

    fn req(v: Value) -> UiRequest {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn 見るだけは選べる値を返し書かない() {
        let path = tmp("show");
        let out = run(&path, &UiRequest::default()).unwrap();
        assert_eq!(out["state"], "absent");
        assert_eq!(out["ui"]["buttons"][0]["id"], "compact");
        assert!(out["choices"]["set"]["usage_bar.place"]["values"].is_array());
        assert!(out.get("changed").is_none());
        assert!(!path.exists());
    }

    #[test]
    fn 変更は書いて断った値は書かない() {
        let path = tmp("set");
        let out = run(
            &path,
            &req(json!({"op": "button_add", "value": "split-right"})),
        )
        .unwrap();
        assert_eq!(out["changed"], true);
        assert_eq!(out["ui"]["buttons"][1]["hotkey"], "s");
        assert!(out.get("choices").is_none());
        let before = std::fs::read_to_string(&path).unwrap();
        let err = run(
            &path,
            &req(json!({"op": "set", "key": "colors.dim", "value": "#123456"})),
        )
        .unwrap_err();
        assert!(matches!(err, RunError::Invalid(_)));
        assert!(err.message().contains("suggestion"), "{}", err.message());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn 帯のトグルは報告の新しい方だけ取り込む() {
        let path = tmp("band");
        let band = |hidden: bool, at: Option<u64>| ModBand {
            hidden,
            shown: false,
            columns: None,
            segments: Vec::new(),
            toggled_at: at,
        };
        import_band(&path, &band(true, None));
        assert!(
            !path.exists(),
            "切り替えたことの無い mod の報告では書かない"
        );
        import_band(&path, &band(true, Some(50)));
        assert!(view(&path).band.hidden);
        import_band(&path, &band(false, Some(40)));
        assert!(view(&path).band.hidden, "古い報告は捨てる");
        run(
            &path,
            &req(json!({"op": "set", "key": "band.hidden", "value": false})),
        )
        .unwrap();
        let after_cli = view(&path);
        assert!(!after_cli.band.hidden);
        assert!(after_cli.band.toggled_at.unwrap() > 50);
        import_band(&path, &band(true, Some(50)));
        assert!(!view(&path).band.hidden, "CLI の切り替えの方が新しい");
    }

    #[test]
    fn 壊れたuijsonは既定で動き退避が残る() {
        let path = tmp("broken");
        std::fs::write(&path, "{\"schema_version\": 1, \"buttons\": 7").unwrap();
        let out = run(&path, &UiRequest::default()).unwrap();
        assert_eq!(out["state"], "repaired");
        assert_eq!(out["ui"], json!(core::UiConfig::default()));
        let bak = out["quarantine"].as_str().unwrap();
        assert!(bak.ends_with("ui.json.unreadable.bak"), "{bak}");
        assert!(Path::new(bak).exists());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "{\"schema_version\": 1, \"buttons\": 7",
            "見るだけでは元を書き換えない"
        );
    }
}
