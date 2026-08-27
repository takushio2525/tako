//! 設定・データファイルの移行登録簿（Issue #916）
//!
//! [`tako_core::migration`] が「型」、ここが「中身」。tako が読み書きする永続ファイルを
//! **すべて**版数の番地（[`SchemaId`]）に載せ、
//!
//! - どのファイルが今どの形式か（`detect`）
//! - 形式を変えたときの直し方（`Step`）
//! - 今の形式として読めるか（`validate`）
//!
//! をここ 1 箇所で宣言する。載せ忘れは `migration_registry_coverage` テストが
//! 名指しで落とす（`config_share::catalog` と同じ考え方。#513 / #515）。
//!
//! ## なぜ全部載せるのか（1 件しか移行手順が無いのに）
//!
//! 移行が要るスキーマ変更は「起きてから」対応すると必ず取り残しが出る（#916 の棚卸しで、
//! 版数フィールドを持っているのに誰も読まないファイルが 2 件、破損時に黙って既定値へ
//! 落ちるファイルが 4 件、掃除されない残骸が 1 種見つかった）。番地を先に切っておけば、
//! スキーマを変える PR は**版数を上げて手順を足すだけ**で済み、
//! 忘れたらテストが止める。
//!
//! ## 発火点
//!
//! [`run`] を呼ぶのは 3 か所（`tako setup` / GUI 起動 / master・CLI 経路）。
//! どれも同じ [`run`] を通るので、増えても挙動は 1 本のまま。

use std::path::{Path, PathBuf};

use tako_core::migration::{
    self, FileOutcome, FileReport, MigrationIo, MigrationReport, SchemaId, SchemaSpec, Step,
};
use tako_core::platform::support::Note;

/// 移行を実際に当てるか、見るだけか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 書き換えない（`tako migrate status` / `--dry-run`）
    Check,
    /// 当てる
    Apply,
}

// --- 検証（今の形式として読めるか） -----------------------------------------
//
// 「読めない」を**黙って既定値へ落とさない**ための実装。ここで Err になったものは
// 退避されて `tako migrate status` の警告行になる。

fn json_ok<T: serde::de::DeserializeOwned>(text: &str) -> Result<(), String> {
    serde_json::from_str::<T>(text)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn yaml_ok<T: serde::de::DeserializeOwned>(text: &str) -> Result<(), String> {
    serde_yaml::from_str::<T>(text)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn validate_settings(text: &str) -> Result<(), String> {
    json_ok::<crate::settings::Settings>(text)
}

fn validate_layout(text: &str) -> Result<(), String> {
    json_ok::<crate::layout::LayoutFile>(text)
}

fn validate_recent(text: &str) -> Result<(), String> {
    json_ok::<tako_core::recent::RecentList>(text)
}

fn validate_config_share(text: &str) -> Result<(), String> {
    json_ok::<crate::config_share::ShareState>(text)
}

fn validate_remote_devices(text: &str) -> Result<(), String> {
    json_ok::<crate::remote_auth::DevicesFile>(text)
}

fn validate_discovery_instance(text: &str) -> Result<(), String> {
    json_ok::<crate::discovery::ControlInfo>(text)
}

fn validate_sessions(text: &str) -> Result<(), String> {
    yaml_ok::<crate::sessions::SessionCatalog>(text)
}

fn validate_workers(text: &str) -> Result<(), String> {
    yaml_ok::<crate::orchestrator::registry::WorkerRegistry>(text)
}

fn validate_task_checkpoints(text: &str) -> Result<(), String> {
    yaml_ok::<crate::task_checkpoints::TaskCheckpointStore>(text)
}

fn validate_acceptance_gates(text: &str) -> Result<(), String> {
    yaml_ok::<crate::acceptance_gates::AcceptanceGateStore>(text)
}

fn validate_orchestrator_config(text: &str) -> Result<(), String> {
    yaml_ok::<crate::setup::SetupConfig>(text)
}

fn validate_projects(text: &str) -> Result<(), String> {
    yaml_ok::<crate::orchestrator::ProjectsConfig>(text)
}

fn validate_accounts(text: &str) -> Result<(), String> {
    yaml_ok::<crate::orchestrator::AccountsConfig>(text)
}

fn validate_profile(text: &str) -> Result<(), String> {
    yaml_ok::<crate::orchestrator::Profile>(text)
}

fn validate_ledger(text: &str) -> Result<(), String> {
    yaml_ok::<crate::orchestrator::ledger::Ledger>(text)
}

// --- 版数の判定 --------------------------------------------------------------

/// 版数フィールドを持たない形式（構造で世代を分けていない）。常に v1
fn detect_v1(_: &str) -> u32 {
    1
}

/// 最上位の `version:` / `"version":` を読む（持っている形式はこれで判定できる）。
/// 読めなければ v1（版数フィールドが無い世代 = 最初の形式）
fn detect_version_field(text: &str) -> u32 {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(n) = v.get("version").and_then(serde_json::Value::as_u64) {
            return n.min(u32::MAX as u64) as u32;
        }
    }
    if let Ok(v) = serde_yaml::from_str::<serde_yaml::Value>(text) {
        if let Some(n) = v.get("version").and_then(serde_yaml::Value::as_u64) {
            return n.min(u32::MAX as u64) as u32;
        }
    }
    1
}

/// プロファイルの世代。
///
/// - v1: 旧既定値 `claude-opus-4-6[1m]` が残っている（Issue #27）
/// - v2: `bypass_sandbox` を**書いていない** = codex のサンドボックス解除が
///   無条件だった頃に作られたファイル（Issue #981）
/// - v3: 現在
///
/// v2 の判定に「キーの有無」を使えるのは、`Profile::bypass_sandbox` に
/// `skip_serializing_if` を付けていない（= 新しく書いたファイルには必ず載る）から。
/// 外部の記録ではなく**内容だけ**で世代が決まるので冪等性が保てる
fn detect_profile(text: &str) -> u32 {
    let Ok(profile) = serde_yaml::from_str::<crate::orchestrator::Profile>(text) else {
        // 読めないものは validate 側が退避して申告する。版数判定では触らない扱いにする
        return PROFILE_VERSION;
    };
    if profile.model.as_deref() == Some(crate::orchestrator::LEGACY_DEFAULT_MODEL) {
        return 1;
    }
    if !has_top_level_key(text, "bypass_sandbox") {
        return 2;
    }
    PROFILE_VERSION
}

/// YAML の最上位にそのキーがあるか。**値ではなくキーの有無**を見る
/// （`bypass_sandbox: false` と「書いていない」を区別する必要がある）
fn has_top_level_key(text: &str, key: &str) -> bool {
    serde_yaml::from_str::<serde_yaml::Value>(text)
        .ok()
        .and_then(|v| v.get(key).cloned())
        .is_some()
}

// --- 変換手順 ----------------------------------------------------------------

/// プロファイルの現在の世代
const PROFILE_VERSION: u32 = 3;

const PROFILE_V1_TO_V2: Note = Note::new(
    "旧既定モデル claude-opus-4-6[1m] の指定を外す（Pro プランで master が起動できないため。#27）",
    "Drop the legacy default model claude-opus-4-6[1m] (it prevents master from starting on the Pro plan; #27)",
);

/// #27 の移行を機構へ載せたもの。**行単位で `model:` だけを外す**ので、
/// コメントも他の設定もそのまま残る（読めなくなる書式だけ serde 経由で作り直す）
fn strip_legacy_default_model(text: &str) -> Result<Option<String>, String> {
    let profile: crate::orchestrator::Profile =
        serde_yaml::from_str(text).map_err(|e| format!("プロファイルとして読めない: {e}"))?;
    if profile.model.as_deref() != Some(crate::orchestrator::LEGACY_DEFAULT_MODEL) {
        // 既に外れている = 二度目。冪等（何もしない）
        return Ok(None);
    }
    let legacy = crate::orchestrator::LEGACY_DEFAULT_MODEL;
    let is_legacy_model_line = |line: &str| {
        line.strip_prefix("model:").is_some_and(|rest| {
            let value = rest.trim();
            value == legacy || value == format!("'{legacy}'") || value == format!("\"{legacy}\"")
        })
    };
    if text.lines().any(is_legacy_model_line) {
        let kept: Vec<&str> = text.lines().filter(|l| !is_legacy_model_line(l)).collect();
        let mut out = kept.join("\n");
        out.push('\n');
        // 行を抜いた結果が読めるならそれを採る（コメント・書式を最大限保つ）
        if serde_yaml::from_str::<crate::orchestrator::Profile>(&out).is_ok() {
            return Ok(Some(out));
        }
    }
    // 行単位で外せない書式（フロー形式など）は serde で作り直す
    let mut rebuilt = profile;
    rebuilt.model = None;
    serde_yaml::to_string(&rebuilt)
        .map(Some)
        .map_err(|e| format!("YAML の再構成に失敗: {e}"))
}

const PROFILE_V2_TO_V3: Note = Note::new(
    "codex のサンドボックス解除が無条件だった頃の挙動を保つため bypass_sandbox: true を明示する（新規プロファイルは安全側の false。#981）",
    "Pin bypass_sandbox: true so the codex sandbox bypass keeps behaving as it did when it was unconditional (new profiles default to the safe false; #981)",
);

/// #981: 既存プロファイルへ `bypass_sandbox: true` を明示する。
///
/// 既定を安全側（バイパスしない）へ変えると、今まで動いていた codex master /
/// codex worker が承認プロンプトで止まる。**既存ユーザーの体験は変えない**ため、
/// この項目が無いファイル（= 変更前に作られたファイル）には従来と同じ挙動になる値を書く
fn pin_sandbox_bypass(text: &str) -> Result<Option<String>, String> {
    if has_top_level_key(text, "bypass_sandbox") {
        // 既に明示されている = 二度目。冪等（何もしない）
        return Ok(None);
    }
    // 読めることを確かめてから触る（読めないものは validate 側が退避する）
    serde_yaml::from_str::<crate::orchestrator::Profile>(text)
        .map_err(|e| format!("プロファイルとして読めない: {e}"))?;
    // **行の追記で済ませる**（コメントも書式もそのまま残す。serde で作り直すと
    // 利用者が書いたコメントが消える）
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("bypass_sandbox: true\n");
    if serde_yaml::from_str::<crate::orchestrator::Profile>(&out).is_err() {
        // 追記で読めなくなる書式（フロー形式など）は serde で作り直す
        let mut rebuilt: crate::orchestrator::Profile =
            serde_yaml::from_str(text).map_err(|e| format!("プロファイルとして読めない: {e}"))?;
        rebuilt.bypass_sandbox = true;
        return serde_yaml::to_string(&rebuilt)
            .map(Some)
            .map_err(|e| format!("YAML の再構成に失敗: {e}"));
    }
    Ok(Some(out))
}

const PROFILE_STEPS: &[Step] = &[
    Step {
        from: 1,
        to: 2,
        describe: PROFILE_V1_TO_V2,
        // 移行後に利用者が自分で [1m] を選び直したら、それは意図された選択なので触らない（#67）
        once: true,
        apply: strip_legacy_default_model,
    },
    Step {
        from: 2,
        to: 3,
        describe: PROFILE_V2_TO_V3,
        // once にしない。理由は 2 つある:
        // ① 利用者が `--bypass-sandbox false` で戻すと**キーは残る**（v3 のまま）ので
        //    再適用されない = once の目的（利用者の選択を尊重）は detect 側で足りている
        // ② `once_markers`（#27 の `.backup-1m`）は from を問わず「一度当たった」と
        //    答えるので、#27 を通った古い利用者だけこの手順が飛ばされてしまう
        once: false,
        apply: pin_sandbox_bypass,
    },
];

// --- 登録簿 ------------------------------------------------------------------

/// 版数フィールドを持たず移行手順もまだ無い種別の宣言（番地だけ切る）
const fn pristine(id: SchemaId, validate: Option<migration::Validator>) -> SchemaSpec {
    SchemaSpec {
        id,
        target_version: 1,
        detect: detect_v1,
        steps: &[],
        once_markers: &[],
        validate,
        preserve_unreadable: true,
    }
}

/// 読めない内容を**退避しない**種別（秘匿情報つき / tako が作り直せる短命なファイル）
const fn ephemeral(id: SchemaId, validate: Option<migration::Validator>) -> SchemaSpec {
    SchemaSpec {
        id,
        target_version: 1,
        detect: detect_version_field,
        steps: &[],
        once_markers: &[],
        validate,
        preserve_unreadable: false,
    }
}

/// 版数フィールドを持つが移行手順はまだ無い種別（将来の bump をここで受ける）
const fn versioned(id: SchemaId, validate: Option<migration::Validator>) -> SchemaSpec {
    SchemaSpec {
        id,
        target_version: 1,
        detect: detect_version_field,
        steps: &[],
        once_markers: &[],
        validate,
        preserve_unreadable: true,
    }
}

/// プロファイル（master / solo）の登録。同じ `Profile` 型なので世代も手順も共通
const fn profile_spec(id: SchemaId) -> SchemaSpec {
    SchemaSpec {
        id,
        target_version: PROFILE_VERSION,
        detect: detect_profile,
        steps: PROFILE_STEPS,
        // #916 の機構より前に手書きされていた移行（#27）が残した印
        once_markers: &[".backup-1m"],
        validate: Some(validate_profile),
        preserve_unreadable: true,
    }
}

/// 全種別の登録。**新しい永続ファイルを足したらここへ 1 行**
pub const SPECS: &[SchemaSpec] = &[
    pristine(SchemaId::Settings, Some(validate_settings)),
    versioned(SchemaId::Layout, Some(validate_layout)),
    pristine(SchemaId::Sessions, Some(validate_sessions)),
    pristine(SchemaId::Workers, Some(validate_workers)),
    versioned(SchemaId::TaskCheckpoints, Some(validate_task_checkpoints)),
    versioned(SchemaId::AcceptanceGates, Some(validate_acceptance_gates)),
    pristine(SchemaId::Recent, Some(validate_recent)),
    pristine(SchemaId::ConfigShare, Some(validate_config_share)),
    // 蓋閉じの残留状態。中身は Windows 専用だが読める形かの検査は両 OS で同じ
    pristine(SchemaId::LidGuard, None),
    pristine(
        SchemaId::OrchestratorConfig,
        Some(validate_orchestrator_config),
    ),
    pristine(SchemaId::Projects, Some(validate_projects)),
    pristine(SchemaId::Accounts, Some(validate_accounts)),
    profile_spec(SchemaId::Profiles),
    // solo プロファイルも同じ `Profile` 型で、`master_agent: codex` を持てる
    // （`tako solo` は build_master_cmd を通る）。#981 の移行は solo にも要る
    profile_spec(SchemaId::SoloProfiles),
    pristine(SchemaId::Ledger, Some(validate_ledger)),
    // 引き継ぎ（#915）は「プロファイル単位の 1 ファイル」から
    // 「プロジェクト単位の複数ファイル」への**分割**移行なので、テキスト置換の
    // Step では表せない。手順そのものは `orchestrator::handoff_store` が持ち、
    // ここは**発火と可視化だけ**を引き受ける（[`handoff_reports`]）。
    // 「どこで直るか」を 1 本に保つのが番地に載せる目的
    pristine(SchemaId::Handoff, None),
    // 認証トークンを持ち、次回起動で作り直される。退避すると寿命を超えて
    // トークンの写しが残るので**退避しない**（読めない残骸は discovery 側の
    // prune_unparsable_instances が pid 生存で掃除する）
    ephemeral(
        SchemaId::DiscoveryInstance,
        Some(validate_discovery_instance),
    ),
    // 共有分類は Secret（`remote/`）。ペアリング情報の写しを残さない
    ephemeral(SchemaId::RemoteDevices, Some(validate_remote_devices)),
];

/// 種別から登録を引く
pub fn spec(id: SchemaId) -> Option<&'static SchemaSpec> {
    SPECS.iter().find(|s| s.id == id)
}

// --- 対象ファイルの解決 ------------------------------------------------------

/// その種別が実際に指すファイル（無いものは返さない）。
/// ディレクトリ配下を持つ種別（プロファイル・引き継ぎ・インスタンス）は列挙する
pub fn targets(id: SchemaId) -> Vec<PathBuf> {
    let data = tako_core::paths::data_dir();
    let single = |rel: &str| -> Vec<PathBuf> {
        data.as_ref().map(|d| vec![d.join(rel)]).unwrap_or_default()
    };
    match id {
        SchemaId::Settings => crate::settings::settings_path().into_iter().collect(),
        SchemaId::Layout => crate::layout::layout_path().into_iter().collect(),
        SchemaId::Sessions => single("sessions.yaml"),
        SchemaId::Workers => single("workers.yaml"),
        SchemaId::TaskCheckpoints => single("task_checkpoints.yaml"),
        SchemaId::AcceptanceGates => single("acceptance_gates.yaml"),
        SchemaId::Recent => single("recent.json"),
        SchemaId::ConfigShare => single("config-share.json"),
        SchemaId::LidGuard => single("lid-guard.json"),
        SchemaId::OrchestratorConfig => crate::setup::config_yaml_path().ok().into_iter().collect(),
        SchemaId::Projects => crate::orchestrator::projects_yaml_path()
            .into_iter()
            .collect(),
        SchemaId::Accounts => crate::orchestrator::accounts_yaml_path()
            .into_iter()
            .collect(),
        SchemaId::Profiles => dir_entries(crate::orchestrator::profiles_dir(), ".yaml"),
        SchemaId::SoloProfiles => dir_entries(crate::orchestrator::solo_profiles_dir(), ".yaml"),
        SchemaId::Ledger => crate::orchestrator::ledger::ledger_path()
            .into_iter()
            .collect(),
        // 引き継ぎは**1 ファイル → 複数ファイル**の分割移行（#915）なので、
        // テキスト置換の Step では表せない。専用実装へ委譲する（[`handoff_reports`]）
        SchemaId::Handoff => Vec::new(),
        SchemaId::DiscoveryInstance => {
            dir_entries(data.as_ref().map(|d| d.join("instances")), ".json")
        }
        SchemaId::RemoteDevices => single("remote/devices.json"),
    }
}

/// ディレクトリ配下の対象ファイル。**退避・ロック・作業ファイルは対象にしない**
/// （`.bak` を移行して二重に世代を作る事故を防ぐ）
fn dir_entries(dir: Option<PathBuf>, suffix: &str) -> Vec<PathBuf> {
    let Some(dir) = dir else {
        return Vec::new();
    };
    let Ok(reader) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = reader
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .is_some_and(|name| name.ends_with(suffix) && !is_side_file(&name))
        })
        .collect();
    out.sort();
    out
}

/// tako 自身が作る付随ファイル（移行の対象にしてはいけない）
fn is_side_file(name: &str) -> bool {
    name.ends_with(".lock")
        || name.ends_with(".bak")
        || name.contains(".bak.")
        || name.contains(".bak-")
        || name.contains(".tmp")
        || name.contains(".pre-v")
        || name.contains(".corrupt")
        || name.contains(".backup")
        || name.contains(".wiped")
        || name.contains(".recovery")
}

// --- 実行 --------------------------------------------------------------------

/// アトミック書き込み + 排他ロックで書く [`MigrationIo`]（#169 の config_io を通す）。
/// 移行は read-modify-write なので、GUI と CLI が同時に触っても壊れないことが要る
struct ConfigIo;

impl MigrationIo for ConfigIo {
    fn read(&self, path: &Path) -> std::io::Result<Option<String>> {
        migration::FsIo.read(path)
    }

    fn write(&self, path: &Path, text: &str) -> std::io::Result<()> {
        crate::config_io::atomic_write(path, text).map_err(std::io::Error::other)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// 見るだけの [`MigrationIo`]。`write` を呼ばれたら**エラーにする**ので、
/// Check モードで誤って書く実装が入ったらテストで落ちる
struct ReadOnlyIo;

impl MigrationIo for ReadOnlyIo {
    fn read(&self, path: &Path) -> std::io::Result<Option<String>> {
        migration::FsIo.read(path)
    }

    fn write(&self, path: &Path, _text: &str) -> std::io::Result<()> {
        Err(std::io::Error::other(format!(
            "確認モードでは書き込まない: {}",
            path.display()
        )))
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

/// 登録簿の全ファイルを最新形式へ揃える（`Mode::Check` なら見るだけ）。
///
/// **発火点はここを呼ぶ**（setup / GUI 起動 / master・CLI 経路）。
/// `only` を渡すとその種別だけ扱う
pub fn run(mode: Mode, only: Option<SchemaId>) -> MigrationReport {
    let mut report = MigrationReport::default();
    // **ユニットテストは本番のデータディレクトリを触らない**。#916 の棚卸しで、
    // 隔離せずに本番の profiles/ を書くテストが実際に残骸を作っていたのが見つかった
    // （`_tako_822_set_.yaml`）。同じ穴を機構自身が開けないよう、テストビルドでは
    // `TAKO_DATA_DIR` で明示的に隔離されているときだけ動かす
    if cfg!(test) && std::env::var_os("TAKO_DATA_DIR").is_none() {
        return report;
    }
    for spec in SPECS {
        if only.is_some_and(|id| id != spec.id) {
            continue;
        }
        if spec.id == SchemaId::Handoff {
            for file in handoff_reports(mode) {
                report.push(file);
            }
            continue;
        }
        for path in targets(spec.id) {
            report.push(migrate_one(spec, &path, mode));
        }
    }
    report
}

/// 引き継ぎの移行（#915）を共通の記録へ載せる。
///
/// 手順は `handoff_store` が持つ（1 ファイル → 複数ファイルの分割なので Step で
/// 表せない）。ここがやるのは**同じ発火点から呼ぶこと**と、結果を
/// [`MigrationReport`] の語彙へ翻訳することだけ。これで
/// `tako migrate` / GUI 起動 / `tako setup` のどれからでも引き継ぎまで面倒を見る
fn handoff_reports(mode: Mode) -> Vec<FileReport> {
    use crate::orchestrator::handoff_store;
    let dir = handoff_store::handoff_dir();
    let path_of = |profile: &str| {
        dir.as_ref()
            .map(|d| d.join(format!("{profile}.md")))
            .unwrap_or_else(|| PathBuf::from(format!("{profile}.md")))
    };
    if mode == Mode::Check {
        // 書かずに「旧形式が残っているか」だけを見る。判定は handoff_store の
        // 前判定と同じ材料（形式マーカーの有無 + 分割の材料になる見出し）を使う
        return handoff_store::list_profile_memos()
            .into_iter()
            .filter(|(_, path)| {
                let Ok(text) = std::fs::read_to_string(path) else {
                    return false;
                };
                let stamped = text.contains(tako_core::handoff::HANDOFF_FORMAT_MARKER);
                !stamped || tako_core::handoff::needs_migration_scan(&text)
            })
            .map(|(profile, _)| FileReport {
                id: SchemaId::Handoff,
                path: path_of(&profile),
                outcome: FileOutcome::Migrated {
                    from: 1,
                    to: 2,
                    backup: handoff_store::archive_dir()
                        .unwrap_or_else(|| PathBuf::from("archive")),
                    applied: vec![HANDOFF_V1_TO_V2],
                },
            })
            .collect();
    }
    // 実際に何が変わったかは**中身を前後で比べて**決める。
    // `MigrationOutcome::migrated` は「プロジェクトへ移した行があるか」なので、
    // 形式マーカーの付与だけで済んだファイルが false になり、
    // status の予告（2 件）と run の報告（1 件）が食い違っていた
    let before: Vec<(String, PathBuf, Option<String>)> = handoff_store::list_profile_memos()
        .into_iter()
        .map(|(profile, path)| {
            let text = std::fs::read_to_string(&path).ok();
            (profile, path, text)
        })
        .collect();
    let outcomes = handoff_store::migrate_all();
    let mut reports = Vec::new();
    for (profile, path, old) in before {
        let now = std::fs::read_to_string(&path).ok();
        let outcome = outcomes.iter().find(|o| o.profile == profile);
        let warnings = outcome.map(|o| o.warnings.clone()).unwrap_or_default();
        if !warnings.is_empty() {
            reports.push(FileReport {
                id: SchemaId::Handoff,
                path,
                outcome: FileOutcome::Failed {
                    reason: warnings.join(" / "),
                },
            });
            continue;
        }
        if now == old {
            continue;
        }
        let backup = outcome
            .and_then(|o| o.archived_to.as_deref().map(PathBuf::from))
            .unwrap_or_else(|| sibling_bak(&path));
        reports.push(FileReport {
            id: SchemaId::Handoff,
            path,
            outcome: FileOutcome::Migrated {
                from: 1,
                to: 2,
                backup,
                applied: vec![HANDOFF_V1_TO_V2],
            },
        });
    }
    reports
}

/// `handoff_store` の書き込みが作る世代バックアップ（`<name>.bak.1`）。
/// 分割が起きず形式マーカーの付与だけだった場合の退避先はこちら
fn sibling_bak(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    name.push_str(".bak.1");
    match path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

const HANDOFF_V1_TO_V2: Note = Note::new(
    "プロファイル単位の引き継ぎをプロジェクト単位へ分割する（#915）",
    "Split the per-profile handoff into per-project files (#915)",
);

fn migrate_one(spec: &SchemaSpec, path: &Path, mode: Mode) -> FileReport {
    // 無いファイルは触らない
    if !path.exists() {
        return FileReport {
            id: spec.id,
            path: path.to_path_buf(),
            outcome: FileOutcome::Absent,
        };
    }
    // **まず読み取りだけで判定する**。`config_io` のロックファイルは
    // 「消すと排他が破れる」ので削除できない設計（#169）。だから
    // **書く必要があると分かってから**しか取ってはいけない。
    // 無条件に取ると、全ファイルが最新のときでも起動のたびに `.lock` が増える
    // （実測: 本番のデータディレクトリに 167 個作ってしまった。うち 160 個は
    // `instances/control-*.json` の分）
    let dry = dry_run(spec, path);
    if mode == Mode::Check {
        return dry;
    }
    match &dry.outcome {
        // 書き換えるのはここだけ。ロックを取り、その下でもう一度読み直す
        // （待っている間に別プロセスが移行を済ませていれば、内容ベースの
        // 冪等性で `migrate_file` が「もう当たっている」と判断して何もしない）
        FileOutcome::Migrated { .. } => {
            let _lock = crate::config_io::lock_exclusive(path).ok();
            migration::migrate_file(spec, path, &ConfigIo)
        }
        // 退避だけが要る。書き先は `<name>.unreadable.bak` で**元のファイルは触らない**
        // のでロックは要らない（同着しても内容は同じで、先に在れば上書きしない）。
        // ここを `dry` のまま返すと「退避した」と申告しながら 1 バイトも書いていない
        // 嘘の応答になる（実際に一度そうしてしまった）
        FileOutcome::Unreadable { .. } => migration::migrate_file(spec, path, &ConfigIo),
        // 何もすることが無い（最新 / 不在 / 未来の形式で拒否 / 読めない）
        _ => dry,
    }
}

/// 書かずに「何が起きるか」を出す。`ReadOnlyIo` は `write` を必ず失敗させるので、
/// 退避しようとした時点で `Failed` になる。それは**移行が必要**という意味なので
/// 「これから移行する」へ言い換える
fn dry_run(spec: &SchemaSpec, path: &Path) -> FileReport {
    let mut report = migration::migrate_file(spec, path, &ReadOnlyIo);
    if let FileOutcome::Failed { .. } = report.outcome {
        if let Ok(Some(text)) = migration::FsIo.read(path) {
            let from = (spec.detect)(&text);
            report.outcome = FileOutcome::Migrated {
                from,
                to: spec.target_version,
                backup: migration::backup_path(path, from),
                applied: pending_notes(spec, from),
            };
        }
    }
    report
}

/// `from` から目標までに当たる手順の説明（Check モードの表示用）
fn pending_notes(spec: &SchemaSpec, from: u32) -> Vec<Note> {
    migration::plan(from, spec.target_version, spec.steps)
        .map(|steps| steps.iter().map(|s| s.describe).collect())
        .unwrap_or_default()
}

/// この プロセスで実行時発火を済ませたか。移行は冪等なので**1 プロセス 1 回**でよく、
/// dispatch のような頻繁な経路から呼ばれても全設定ファイルを読み直さない
static RUNTIME_FIRED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// **実行時の差分検出からの発火**（二段構えの 2 段目。#916）。
///
/// GUI 起動・master 起動・CLI / MCP の経路の入口で呼ぶ。旧形式を見つけたらその場で
/// 直し、何をしたかを persist.log へ残して一言（呼び出し側が出す文言）を返す。
/// 何も起きていなければ `None`（黙る）。
///
/// 1 プロセスで 1 回だけ実際に走る。明示的にやり直したいときは [`run`] を使う
pub fn ensure_migrated() -> Option<String> {
    if !take_runtime_slot() {
        return None;
    }
    let report = run(Mode::Apply, None);
    record(&report, "runtime");
    report.notice()
}

/// `tako setup` からの発火（二段構えの 1 段目）。
/// setup は「誰の環境でも一発で最新形式にする」担保なので、
/// 1 プロセス 1 回の制限にかからず必ず走る
pub fn run_for_setup() -> MigrationReport {
    RUNTIME_FIRED.store(true, std::sync::atomic::Ordering::SeqCst);
    let report = run(Mode::Apply, None);
    record(&report, "setup");
    report
}

/// `tako setup` が画面へ出す行（何もなければ空）。
/// **移行は黙って済ませない**（設定ファイルを書き換えた事実は必ず見せる）
pub fn setup_lines() -> Vec<String> {
    let report = run_for_setup();
    let mut lines = Vec::new();
    for file in &report.files {
        match &file.outcome {
            FileOutcome::Migrated {
                backup, applied, ..
            } => {
                for note in applied {
                    lines.push(format!("{}: {}", file.path.display(), note.text()));
                }
                lines.push(format!("旧内容の退避先: {}", backup.display()));
            }
            FileOutcome::Unreadable { quarantine, reason } => lines.push(match quarantine {
                Some(dest) => format!(
                    "{} は読めなかったので {} へ退避しました（{reason}）",
                    file.path.display(),
                    dest.display()
                ),
                None => format!(
                    "{} は読めないので既定値で動きます（{reason}）",
                    file.path.display()
                ),
            }),
            FileOutcome::Refused { reason } | FileOutcome::Failed { reason } => {
                lines.push(format!("{}: {reason}", file.path.display()))
            }
            FileOutcome::Absent | FileOutcome::UpToDate { .. } => {}
        }
    }
    lines
}

/// 移行の結果を JSON へ（**CLI と MCP はここ 1 本を通る** = 1:1 が構造的に保たれる）。
///
/// `action` = "status"（既定。見るだけ）/ "run"（当てる）。`only` でファイル種別を絞る
pub fn report_json(action: &str, only: Option<&str>) -> Result<serde_json::Value, String> {
    let mode = match action {
        "status" | "check" => Mode::Check,
        "run" | "apply" => Mode::Apply,
        other => return Err(format!("不明な action: {other}（status | run）")),
    };
    let only = match only {
        None => None,
        Some(name) => Some(SchemaId::parse(name).ok_or_else(|| {
            let names: Vec<&str> = SchemaId::all().iter().map(|i| i.as_str()).collect();
            format!("不明なファイル種別: {name}（{}）", names.join(" | "))
        })?),
    };
    let report = if mode == Mode::Apply {
        let report = run(Mode::Apply, only);
        record(&report, "cli");
        report
    } else {
        run(Mode::Check, only)
    };
    Ok(serde_json::json!({
        "action": action,
        "applied": mode == Mode::Apply,
        "migrated": report.changed_count(),
        "needs_attention": report.attention().count(),
        "notice": report.notice_for(mode == Mode::Apply),
        "files": report
            .files
            .iter()
            .map(|f| file_json(f, mode == Mode::Apply))
            .collect::<Vec<_>>(),
    }))
}

/// 1 ファイルの結果を JSON へ。`applied = false`（見るだけ）のときは
/// 「これからこうなる」であることが読み手に分かるキー名にする
/// （退避していないのに `backup` / `quarantine` と書くと、AI が
/// 「退避済み」と誤って報告してしまう）
fn file_json(file: &FileReport, did_apply: bool) -> serde_json::Value {
    let mut obj = serde_json::json!({
        "schema": file.id.as_str(),
        "path": file.path.display().to_string(),
        "state": file.outcome.kind(),
    });
    let map = obj.as_object_mut().expect("object");
    match &file.outcome {
        FileOutcome::UpToDate { version } => {
            map.insert("version".into(), (*version).into());
        }
        FileOutcome::Migrated {
            from,
            to,
            backup,
            applied,
        } => {
            map.insert("from".into(), (*from).into());
            map.insert("to".into(), (*to).into());
            map.insert(
                if did_apply {
                    "backup"
                } else {
                    "backup_planned"
                }
                .into(),
                backup.display().to_string().into(),
            );
            // 上位の `applied`（真偽値 = 当てたか）と紛れないよう `steps` と呼ぶ
            map.insert(
                "steps".into(),
                applied
                    .iter()
                    .map(|n| serde_json::Value::from(n.text()))
                    .collect::<Vec<_>>()
                    .into(),
            );
        }
        FileOutcome::Unreadable { quarantine, reason } => {
            // 退避しない種別（秘匿情報つき / 短命）はキー自体を出さない
            if let Some(dest) = quarantine {
                map.insert(
                    if did_apply {
                        "quarantine"
                    } else {
                        "quarantine_planned"
                    }
                    .into(),
                    dest.display().to_string().into(),
                );
            }
            map.insert("reason".into(), reason.clone().into());
        }
        FileOutcome::Refused { reason } | FileOutcome::Failed { reason } => {
            map.insert("reason".into(), reason.clone().into());
        }
        FileOutcome::Absent => {}
    }
    obj
}

/// 実施の可視化。**何をどう変えたかは必ず監査ログへ残す**（黙って直さない）
fn record(report: &MigrationReport, origin: &str) {
    for file in &report.files {
        match &file.outcome {
            FileOutcome::Migrated {
                from, to, backup, ..
            } => crate::diag::persist_log(&format!(
                "移行: {} v{from} -> v{to}: {}（退避 {}・発生源 {origin}）",
                file.id.as_str(),
                file.path.display(),
                backup.display()
            )),
            FileOutcome::Unreadable { quarantine, reason } => {
                let where_to = match quarantine {
                    Some(dest) => format!("退避 {}", dest.display()),
                    None => "退避しない種別".to_string(),
                };
                crate::diag::persist_log(&format!(
                    "移行できず: {} {}（{reason}・{where_to}・発生源 {origin}）",
                    file.id.as_str(),
                    file.path.display()
                ))
            }
            FileOutcome::Refused { reason } | FileOutcome::Failed { reason } => {
                crate::diag::persist_log(&format!(
                    "移行を中止: {} {}（{reason}・発生源 {origin}）",
                    file.id.as_str(),
                    file.path.display()
                ))
            }
            FileOutcome::Absent | FileOutcome::UpToDate { .. } => {}
        }
    }
}

/// 実行時発火の枠を取る。**最初の 1 回だけ true**。
/// 判定をここへ切り出してあるのは、テストが本番のデータディレクトリへ触らずに
/// 「一度だけ」を確かめられるようにするため（ユニットテストが本番設定を
/// 書き換える型の事故は #916 の棚卸しで実際に見つかっている）
fn take_runtime_slot() -> bool {
    take_slot(&RUNTIME_FIRED)
}

/// [`take_runtime_slot`] の純粋部分（テストが自分の旗で確かめられるように分けてある）
fn take_slot(flag: &std::sync::atomic::AtomicBool) -> bool {
    !flag.swap(true, std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 登録簿は全種別を 1 回ずつ載せる（載せ忘れ・二重登録をここで止める）
    #[test]
    fn 登録簿は全種別を一度ずつ載せる() {
        for id in SchemaId::all() {
            let found = SPECS.iter().filter(|s| s.id == *id).count();
            assert_eq!(found, 1, "{} の登録が {found} 件", id.as_str());
        }
        assert_eq!(SPECS.len(), SchemaId::all().len());
    }

    /// 登録の整合性: 目標版数まで手順が繋がっている（飛び番・逆行をここで止める）
    #[test]
    fn 登録簿の手順は目標まで繋がる() {
        for spec in SPECS {
            migration::plan(1, spec.target_version, spec.steps).unwrap_or_else(|e| {
                panic!("{} の手順が繋がらない: {}", spec.id.as_str(), e.message())
            });
        }
    }

    #[test]
    fn 付随ファイルは移行対象にしない() {
        for name in [
            "default.yaml.lock",
            "default.yaml.bak.1",
            "default.yaml.bak-before-kaitai",
            "default.yaml.backup-1m",
            "settings.json.tmp",
            "settings.json.pre-v1.bak",
            "layout.json.corrupt",
            "projects.yaml.wiped-141102.bak",
            "layout.json.recovery-1657.bak",
        ] {
            assert!(is_side_file(name), "{name} は対象外のはず");
        }
        for name in ["default.yaml", "settings.json", "control-1264.json"] {
            assert!(!is_side_file(name), "{name} は対象のはず");
        }
    }

    #[test]
    fn 版数フィールドを読む() {
        assert_eq!(detect_version_field(r#"{"version":3}"#), 3);
        assert_eq!(detect_version_field("version: 2\ngates: {}\n"), 2);
        assert_eq!(detect_version_field("gates: {}\n"), 1, "無ければ v1");
        assert_eq!(detect_version_field("こわれた {"), 1);
    }

    #[test]
    fn プロファイルの世代は内容だけで決まる() {
        // v1: 旧既定モデルが残っている（#27）
        let legacy = format!("model: {}\n", crate::orchestrator::LEGACY_DEFAULT_MODEL);
        assert_eq!(detect_profile(&legacy), 1);
        // v2: bypass_sandbox を書いていない = #981 より前に作られたファイル
        assert_eq!(detect_profile("model: claude-sonnet-5\n"), 2);
        assert_eq!(detect_profile("effort: high\n"), 2);
        // v3: 明示されている（false でも「書いてある」= 現行世代）
        assert_eq!(
            detect_profile("effort: high\nbypass_sandbox: false\n"),
            PROFILE_VERSION
        );
        assert_eq!(
            detect_profile("effort: high\nbypass_sandbox: true\n"),
            PROFILE_VERSION
        );
    }

    /// 手書きのテンプレートも現行世代でなければならない。
    /// **実測で踏んだ**: `ensure_defaults` の default.yaml は serde ではなく
    /// テンプレート文字列なので、`bypass_sandbox` を書き忘れると新規インストールが
    /// v2 と判定され、移行が `true`（= 危険な旧既定）を書き込んでしまう（#981）
    #[test]
    fn 手書きのプロファイルテンプレートは現行世代である() {
        for (label, text) in [
            ("master", crate::orchestrator::DEFAULT_PROFILE_YAML),
            ("solo", crate::orchestrator::SOLO_DEFAULT_PROFILE_YAML),
        ] {
            assert_eq!(
                detect_profile(text),
                PROFILE_VERSION,
                "{label} のテンプレートが移行対象になっている（新規インストールへ移行が当たる）"
            );
            assert_eq!(
                migration::migrate_text(profiles_spec(), text).expect("読める"),
                None,
                "{label}: 生成直後のファイルを書き換えてはいけない"
            );
        }
    }

    /// #981 の核心: **新しく作ったプロファイルへ true を書き込まない**
    /// （`Profile::default()` を保存したファイルは既に v3 なので移行対象外）
    #[test]
    fn 新規プロファイルは移行対象にならない() {
        let yaml = serde_yaml::to_string(&crate::orchestrator::Profile::default()).expect("書ける");
        assert!(
            yaml.contains("bypass_sandbox: false"),
            "新規プロファイルは安全側の値を明示する: {yaml}"
        );
        assert_eq!(detect_profile(&yaml), PROFILE_VERSION);
        assert_eq!(pin_sandbox_bypass(&yaml).expect("成功"), None, "触らない");
    }

    /// #981 の移行: 既存ファイル（キーが無い）へ true を書き、コメントと書式を残す
    #[test]
    fn 既存プロファイルにはbypass_sandboxのtrueが明示される() {
        let text = "# ユーザーのコメント\nmaster_agent: codex\neffort: high\n";
        let out = pin_sandbox_bypass(text).expect("成功").expect("変わる");
        assert!(out.contains("# ユーザーのコメント"), "{out}");
        assert!(out.contains("master_agent: codex"), "{out}");
        assert!(out.contains("bypass_sandbox: true"), "{out}");
        let p: crate::orchestrator::Profile = serde_yaml::from_str(&out).expect("読める");
        assert!(p.bypass_sandbox, "移行後は従来と同じ挙動になる");
        // 冪等
        assert_eq!(pin_sandbox_bypass(&out).expect("成功"), None);
        // 利用者が false へ戻したら尊重する（キーは残るので再適用されない）
        let disabled = out.replace("bypass_sandbox: true", "bypass_sandbox: false");
        assert_eq!(detect_profile(&disabled), PROFILE_VERSION);
        assert_eq!(pin_sandbox_bypass(&disabled).expect("成功"), None);
    }

    #[test]
    fn bypass_sandboxの明示はフロー形式でも通る() {
        let out = pin_sandbox_bypass("{effort: high, master_agent: codex}\n")
            .expect("成功")
            .expect("変わる");
        let p: crate::orchestrator::Profile = serde_yaml::from_str(&out).expect("読める");
        assert!(p.bypass_sandbox);
        assert_eq!(p.effort, "high");
    }

    /// solo プロファイルも同じ型・同じ世代なので同じ手順が当たる（#981）
    #[test]
    fn soloプロファイルも同じ移行を通る() {
        let master = spec(SchemaId::Profiles).expect("登録されている");
        let solo = spec(SchemaId::SoloProfiles).expect("登録されている");
        assert_eq!(solo.target_version, master.target_version);
        assert_eq!(solo.steps.len(), master.steps.len());
        assert!(
            !solo.is_pristine(),
            "solo にも手順が要る（master_agent: codex）"
        );
    }

    #[test]
    fn 旧既定モデル除去は他の行とコメントを残す() {
        let text = format!(
            "# ユーザーのコメント\nmodel: {}\neffort: high\n",
            crate::orchestrator::LEGACY_DEFAULT_MODEL
        );
        let out = strip_legacy_default_model(&text)
            .expect("成功")
            .expect("変わる");
        assert!(!out.contains("model:"), "{out}");
        assert!(out.contains("# ユーザーのコメント"), "{out}");
        assert!(out.contains("effort: high"), "{out}");
        // 冪等
        assert_eq!(strip_legacy_default_model(&out).expect("成功"), None);
    }

    #[test]
    fn 旧既定モデル除去はフロー形式でも通る() {
        let text = format!(
            "{{model: '{}', effort: high}}\n",
            crate::orchestrator::LEGACY_DEFAULT_MODEL
        );
        let out = strip_legacy_default_model(&text)
            .expect("成功")
            .expect("変わる");
        let p: crate::orchestrator::Profile = serde_yaml::from_str(&out).expect("読める");
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "high");
    }

    #[test]
    fn 明示された別モデルには触らない() {
        assert_eq!(
            strip_legacy_default_model("model: claude-opus-5\n").expect("成功"),
            None
        );
    }
    // --- #27 の移行を機構の上で通す（旧 orchestrator::migrate_legacy_model_file の
    // 振る舞いをそのまま引き継ぐ。ここが production の Profiles spec を実際に通る） ---

    fn profiles_spec() -> &'static SchemaSpec {
        spec(SchemaId::Profiles).expect("Profiles は登録されている")
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tako-migrations-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
        dir
    }

    #[test]
    fn 旧既定モデルはコメントと他の設定を残して除去される() {
        let dir = temp_dir("legacy");
        let path = dir.join("default.yaml");
        std::fs::write(
            &path,
            "# user comment\nmodel: claude-opus-4-6[1m]\neffort: high\nworker_model_policy: inherit\n",
        )
        .expect("書ける");
        let report = migration::migrate_file(profiles_spec(), &path, &migration::FsIo);
        assert!(report.outcome.changed(), "{report:?}");
        let migrated = std::fs::read_to_string(&path).expect("読める");
        assert!(!migrated.contains("model:"), "{migrated}");
        assert!(migrated.contains("# user comment"), "{migrated}");
        assert!(migrated.contains("effort: high"), "{migrated}");
        let p: crate::orchestrator::Profile = serde_yaml::from_str(&migrated).expect("読める");
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "high");
        assert!(
            migration::backup_path(&path, 1).is_file(),
            "旧内容が退避される"
        );
        // 2 回目は何もしない
        let again = migration::migrate_file(profiles_spec(), &path, &migration::FsIo);
        assert!(!again.outcome.changed(), "{again:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 利用者が明示したモデルには触らない() {
        let dir = temp_dir("keep");
        let path = dir.join("default.yaml");
        // 旧既定値と異なる明示指定（[1m] を含んでいても）は opt-in として尊重。
        // #981 の関心事を混ぜないため bypass_sandbox は明示しておく（= v3 の形）
        std::fs::write(
            &path,
            "model: claude-fable-5[1m]\neffort: max\nbypass_sandbox: false\n",
        )
        .expect("書ける");
        let report = migration::migrate_file(profiles_spec(), &path, &migration::FsIo);
        assert!(!report.outcome.changed(), "{report:?}");
        assert!(std::fs::read_to_string(&path)
            .expect("読める")
            .contains("claude-fable-5[1m]"));
        // model 無しのファイルも触らない
        std::fs::write(&path, "effort: max\nbypass_sandbox: false\n").expect("書ける");
        assert!(
            !migration::migrate_file(profiles_spec(), &path, &migration::FsIo)
                .outcome
                .changed()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Issue #67: 移行後に利用者が profiles set で [1m] を再設定したら保持する
    #[test]
    fn 再設定された旧モデルは二度目に消さない() {
        let dir = temp_dir("issue67");
        let path = dir.join("default.yaml");
        std::fs::write(&path, "model: claude-opus-4-6[1m]\neffort: high\n").expect("書ける");
        assert!(
            migration::migrate_file(profiles_spec(), &path, &migration::FsIo)
                .outcome
                .changed(),
            "初回は移行する"
        );
        // 利用者が意図して再設定（#981 の移行は初回で済んでいるので値は残す）
        std::fs::write(
            &path,
            "model: claude-opus-4-6[1m]\neffort: high\nbypass_sandbox: true\n",
        )
        .expect("書ける");
        let report = migration::migrate_file(profiles_spec(), &path, &migration::FsIo);
        assert!(!report.outcome.changed(), "{report:?}");
        assert!(std::fs::read_to_string(&path)
            .expect("読める")
            .contains("model: claude-opus-4-6[1m]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 旧機構（#27）の印しか無い利用者にも二度当てない
    #[test]
    fn 旧機構の印だけがある場合も再設定を尊重する() {
        let dir = temp_dir("legacy-marker");
        let path = dir.join("default.yaml");
        std::fs::write(
            &path,
            "model: claude-opus-4-6[1m]\neffort: high\nbypass_sandbox: true\n",
        )
        .expect("書ける");
        std::fs::write(dir.join("default.yaml.backup-1m"), "旧い退避").expect("書ける");
        let report = migration::migrate_file(profiles_spec(), &path, &migration::FsIo);
        assert!(!report.outcome.changed(), "{report:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn modelだけのファイルも移行できる() {
        let dir = temp_dir("only");
        let path = dir.join("default.yaml");
        std::fs::write(&path, "model: claude-opus-4-6[1m]\n").expect("書ける");
        assert!(
            migration::migrate_file(profiles_spec(), &path, &migration::FsIo)
                .outcome
                .changed()
        );
        let migrated = std::fs::read_to_string(&path).expect("読める");
        let p: crate::orchestrator::Profile = serde_yaml::from_str(&migrated).expect("読める");
        assert_eq!(p.model, None);
        assert_eq!(p.effort, "max", "serde default で補われる");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 読めないプロファイルは移行せず退避する() {
        let dir = temp_dir("broken");
        let path = dir.join("default.yaml");
        std::fs::write(&path, "model: [こわれた\n").expect("書ける");
        let report = migration::migrate_file(profiles_spec(), &path, &migration::FsIo);
        match &report.outcome {
            FileOutcome::Unreadable { quarantine, .. } => {
                let dest = quarantine.as_ref().expect("プロファイルは退避する種別");
                assert_eq!(
                    std::fs::read_to_string(dest).expect("読める"),
                    "model: [こわれた\n"
                );
            }
            other => panic!("退避されるはず: {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(&path).expect("読める"),
            "model: [こわれた\n",
            "元は触らない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 引き継ぎ（#915）は分割移行なので Step では表せないが、**番地には載っている**。
    /// 走査の対象は持たず（専用実装へ委譲する）、報告は共通の語彙へ翻訳される
    #[test]
    fn 引き継ぎは番地に載るが走査対象を持たない() {
        let spec = spec(SchemaId::Handoff).expect("登録されている");
        assert!(spec.is_pristine(), "テキスト置換の手順は持たない");
        assert!(
            targets(SchemaId::Handoff).is_empty(),
            "ファイル走査ではなく handoff_store へ委譲する"
        );
    }

    /// トークン・秘匿情報を持つ種別は読めなくても**写しを残さない**。
    /// 退避は「利用者が手で書いた情報を守る」ためのものなので、寿命の短い
    /// トークンつきファイルには当てはまらない（写しが寿命を超えて残る）
    #[test]
    fn 秘匿情報つきの種別は退避しない() {
        for id in [SchemaId::DiscoveryInstance, SchemaId::RemoteDevices] {
            let spec = spec(id).expect("登録されている");
            assert!(
                !spec.preserve_unreadable,
                "{} は退避してはいけない（トークン / ペアリング情報の写しが残る）",
                id.as_str()
            );
            assert!(
                spec.validate.is_some(),
                "{} は退避しないが「読めない」ことは申告する",
                id.as_str()
            );
        }
        // それ以外は利用者の手書き情報を守るので退避する
        for id in [SchemaId::Settings, SchemaId::Profiles, SchemaId::Projects] {
            assert!(
                spec(id).expect("登録されている").preserve_unreadable,
                "{} は退避する",
                id.as_str()
            );
        }
    }

    #[test]
    fn 退避しない種別は退避先を返さない() {
        let dir = temp_dir("no-quarantine");
        let path = dir.join("control-999999.json");
        std::fs::write(&path, "{ broken").expect("書ける");
        let spec = spec(SchemaId::DiscoveryInstance).expect("登録されている");
        let report = migration::migrate_file(spec, &path, &migration::FsIo);
        match &report.outcome {
            FileOutcome::Unreadable { quarantine, .. } => assert!(
                quarantine.is_none(),
                "退避先を返してはいけない: {quarantine:?}"
            ),
            other => panic!("読めない扱いになるはず: {other:?}"),
        }
        assert!(
            !migration::quarantine_path(&path).exists(),
            "写しを作ってはいけない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **最新のファイルには `.lock` を作らない**（#916 の自省）。
    /// `config_io` のロックファイルは「消すと排他が破れる」ので削除できない設計。
    /// だから書く必要があると分かるまで取ってはいけない。無条件に取っていた結果、
    /// 本番のデータディレクトリへ 167 個の空ロックを作ってしまった
    #[test]
    fn 最新のファイルにはロックを作らない() {
        let dir = temp_dir("no-lock");
        // 既に新形式のプロファイル（移行の必要なし）
        let up_to_date = dir.join("default.yaml");
        std::fs::write(&up_to_date, "effort: high\nbypass_sandbox: false\n").expect("書ける");
        let spec = profiles_spec();
        let report = migrate_one(spec, &up_to_date, Mode::Apply);
        assert!(
            matches!(report.outcome, FileOutcome::UpToDate { .. }),
            "{report:?}"
        );
        assert!(
            !dir.join("default.yaml.lock").exists(),
            "最新のファイルにロックを作ってはいけない"
        );

        // 読めないファイルは**退避される**が、元を書き換えないのでロックは作らない。
        // 「退避した」と申告しながら 1 バイトも書かない嘘を防ぐ（実際に一度やった）
        let broken = dir.join("broken.yaml");
        std::fs::write(&broken, "model: [x\n").expect("書ける");
        let report = migrate_one(spec, &broken, Mode::Apply);
        match &report.outcome {
            FileOutcome::Unreadable { quarantine, .. } => {
                let dest = quarantine.as_ref().expect("退避する種別");
                assert_eq!(
                    std::fs::read_to_string(dest).expect("申告した退避先が実在する"),
                    "model: [x\n"
                );
            }
            other => panic!("退避されるはず: {other:?}"),
        }
        assert!(
            !dir.join("broken.yaml.lock").exists(),
            "読めないファイルにロックを作ってはいけない"
        );

        // 無いファイルもロックを作らない
        let absent = dir.join("absent.yaml");
        assert_eq!(
            migrate_one(spec, &absent, Mode::Apply).outcome,
            FileOutcome::Absent
        );
        assert!(!dir.join("absent.yaml.lock").exists());

        // 確認モードは旧形式でもロックを作らない
        let legacy = dir.join("legacy.yaml");
        std::fs::write(
            &legacy,
            format!("model: {}\n", crate::orchestrator::LEGACY_DEFAULT_MODEL),
        )
        .expect("書ける");
        let checked = migrate_one(spec, &legacy, Mode::Check);
        assert!(checked.outcome.changed(), "移行が要ると分かる: {checked:?}");
        assert!(
            !dir.join("legacy.yaml.lock").exists(),
            "確認モードでロックを作ってはいけない"
        );

        // 実際に移行するときだけロックを取る（そのときは残っていてよい = config_io の規約）
        assert!(migrate_one(spec, &legacy, Mode::Apply).outcome.changed());
        assert!(
            dir.join("legacy.yaml.lock").exists(),
            "書くときは排他を取る"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 実行時発火は 1 プロセス 1 回（頻繁な経路から呼んでも重くならない）。
    /// **本番のデータディレクトリへは触らずに**枠取りだけを確かめる
    #[test]
    fn 実行時発火の枠は一度だけ取れる() {
        let flag = std::sync::atomic::AtomicBool::new(false);
        assert!(take_slot(&flag), "最初の 1 回だけ走る");
        assert!(!take_slot(&flag), "2 回目は走らない");
    }

    /// 機構自身が本番のデータディレクトリを触る穴を開けていないこと
    #[test]
    fn テストビルドでは隔離されていない限り走らない() {
        assert!(
            std::env::var_os("TAKO_DATA_DIR").is_some() || run(Mode::Apply, None).files.is_empty(),
            "隔離されていないテストで本番を触ってはいけない"
        );
    }
}
