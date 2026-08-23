//! 設定・データファイルの自動マイグレーション機構（Issue #916）
//!
//! ## なぜあるか
//!
//! tako の設定・データファイルのスキーマや置き場を変えたとき、**利用者や master へ
//! 手動の移行作業を要求してはならない**（ユーザー確定方針。#916）。旧形式のファイルは
//! tako 自身が見つけて直す。ここはその「型」で、個々の変換手順（[`Step`]）は
//! ファイルの型を持つ層（tako-control）が登録する。
//!
//! ## 二段構えの発火
//!
//! 1. **`tako setup` 実行時** — 誰の環境でも setup 一発で全ファイルが最新形式になる
//! 2. **実行時の差分検出** — GUI 起動・master 起動・CLI / MCP の経路で旧形式を見たら
//!    その場で移行する（setup を通らない利用者を取り残さない）
//!
//! どちらも同じ [`migrate_text`] を通るので、**発火点が増えても挙動は 1 本**になる。
//!
//! ## 安全要件（全マイグレーション共通・ここで構造的に担保する）
//!
//! - **冪等**: 版数は外部の記録ではなく**ファイルの中身から判定**する（[`SchemaSpec::detect`]）。
//!   #513 の設定共有で別マシンが移行済みのファイルを pull しても、内容が新形式なら
//!   何もしない。「移行した記録」を別ファイルに持つと共有で必ずズレるので持たない
//! - **旧ファイルを消さない**: 書き換える前に [`backup_path`] へ退避する（`.pre-v<N>.bak`）
//! - **解釈できない内容を捨てない**: 変換に失敗したら [`quarantine_path`] へ丸ごと退避し、
//!   何が起きたかを [`FileOutcome::Unreadable`] で申告する（黙って既定値へ落とさない）
//! - **実施の可視化**: 何をどう変えたかは [`MigrationReport`] に残り、CLI / MCP / ログが
//!   同じ 1 本から文言を作る（日英は [`Note`]）
//!
//! ## 版数の付け方
//!
//! 版数は「そのファイル種別のスキーマ世代」で、[`SchemaSpec::target_version`] が現在の正。
//! 1 から始め、スキーマを変えるたびに 1 つ上げて [`Step`] を 1 本足す。
//! **[`SchemaSpec::detect`] は「そのファイルが今どの世代か」を内容から答える**関数で、
//! 版数フィールドを持つ形式ならそれを読み、持たない形式なら構造の特徴で見分ける。

use crate::platform::support::Note;
use std::path::{Path, PathBuf};

/// 永続ファイルの種別 = 版数の番地。
///
/// **新しい設定・データファイルを足したらここへ載せる**（載っていないファイルは
/// `migration_registry_coverage` テストが名指しで落とす）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SchemaId {
    /// `<data_dir>/settings.json`（GUI 設定）
    Settings,
    /// `<data_dir>/layout.json`（タブ・ペイン構成の永続化）
    Layout,
    /// `<data_dir>/sessions.yaml`（セッションカタログ。#112）
    Sessions,
    /// `<data_dir>/workers.yaml`（worker レジストリ。#390）
    Workers,
    /// `<data_dir>/task_checkpoints.yaml`
    TaskCheckpoints,
    /// `<data_dir>/acceptance_gates.yaml`（#244）
    AcceptanceGates,
    /// `<data_dir>/recent.json`（最近使った場所）
    Recent,
    /// `<data_dir>/config-share.json`（設定共有の配線。#513）
    ConfigShare,
    /// `<data_dir>/lid-guard.json`（蓋閉じ継続の残留状態。Windows）
    LidGuard,
    /// `<data_dir>/orchestrator/config.yaml`（setup 状態・オーケストレーター設定）
    OrchestratorConfig,
    /// `<data_dir>/orchestrator/projects.yaml`
    Projects,
    /// `<data_dir>/orchestrator/accounts.yaml`（#504）
    Accounts,
    /// `<data_dir>/orchestrator/profiles/*.yaml`（master プロファイル）
    Profiles,
    /// `<data_dir>/orchestrator/solo-profiles/*.yaml`（solo プロファイル）
    SoloProfiles,
    /// `<data_dir>/orchestrator/ledger.yaml`
    Ledger,
    /// `<data_dir>/orchestrator/handoff/`（引き継ぎ。プロジェクト単位化は #915）
    Handoff,
    /// `<data_dir>/instances/control-*.json`（インスタンス発見。#113）
    DiscoveryInstance,
    /// `<data_dir>/remote/devices.json`（リモートのペアリング。#283）
    RemoteDevices,
}

impl SchemaId {
    /// 機械可読な識別子（CLI / MCP の応答・ログで使う。`--only` の値でもある）
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Layout => "layout",
            Self::Sessions => "sessions",
            Self::Workers => "workers",
            Self::TaskCheckpoints => "task_checkpoints",
            Self::AcceptanceGates => "acceptance_gates",
            Self::Recent => "recent",
            Self::ConfigShare => "config_share",
            Self::LidGuard => "lid_guard",
            Self::OrchestratorConfig => "orchestrator_config",
            Self::Projects => "projects",
            Self::Accounts => "accounts",
            Self::Profiles => "profiles",
            Self::SoloProfiles => "solo_profiles",
            Self::Ledger => "ledger",
            Self::Handoff => "handoff",
            Self::DiscoveryInstance => "discovery_instance",
            Self::RemoteDevices => "remote_devices",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::all().iter().copied().find(|id| id.as_str() == s)
    }

    /// 全種別（テストと `tako migrate status` の列挙で使う）
    pub fn all() -> &'static [SchemaId] {
        &[
            Self::Settings,
            Self::Layout,
            Self::Sessions,
            Self::Workers,
            Self::TaskCheckpoints,
            Self::AcceptanceGates,
            Self::Recent,
            Self::ConfigShare,
            Self::LidGuard,
            Self::OrchestratorConfig,
            Self::Projects,
            Self::Accounts,
            Self::Profiles,
            Self::SoloProfiles,
            Self::Ledger,
            Self::Handoff,
            Self::DiscoveryInstance,
            Self::RemoteDevices,
        ]
    }
}

/// 1 世代ぶんの変換手順。
///
/// `apply` は**テキスト → テキスト**の純粋関数にする（YAML / JSON いずれも
/// テキストなので、tako-core が serde_yaml へ依存せずに機構だけを持てる）。
/// 未知のキーを落とさないため、実装側は必ず `serde_*::Value` のような
/// 「全体を保持する型」を経由すること（構造体へ通すと未知キーが消える）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// この手順が受け取る版数
    pub from: u32,
    /// この手順が作る版数
    pub to: u32,
    /// 何をするか（日英。CLI / MCP / ログが同じ文言を使う）
    pub describe: Note,
    /// 変換本体。`Ok(None)` = 変えるものが無かった（既に新形式）
    pub apply: fn(&str) -> Result<Option<String>, String>,
}

/// ファイル種別ごとの登録内容
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaSpec {
    pub id: SchemaId,
    /// 現在のスキーマ世代。**スキーマを変えたらここを上げて [`Step`] を足す**
    pub target_version: u32,
    /// 内容から今の世代を答える（冪等性の根拠。外部の記録に頼らない）
    pub detect: fn(&str) -> u32,
    /// `from` の昇順に並べた変換手順
    pub steps: &'static [Step],
}

impl SchemaSpec {
    /// この種別が現状ひとつも移行手順を持たないか（`target_version == 1`）
    pub fn is_pristine(&self) -> bool {
        self.steps.is_empty() && self.target_version == 1
    }
}

/// 版数から目標までの手順を並べる（純粋関数。実ファイルに触らない）。
///
/// 飛び番・逆行・目標超過は**黙って進めずエラー**にする（壊れた登録で
/// 利用者のファイルを触りに行かないため）。
pub fn plan(
    current: u32,
    target: u32,
    steps: &'static [Step],
) -> Result<Vec<&'static Step>, PlanError> {
    if current > target {
        return Err(PlanError::FromFuture { current, target });
    }
    let mut out = Vec::new();
    let mut at = current;
    while at < target {
        let Some(step) = steps.iter().find(|s| s.from == at) else {
            return Err(PlanError::MissingStep { from: at, target });
        };
        if step.to <= at {
            return Err(PlanError::NotAdvancing {
                from: step.from,
                to: step.to,
            });
        }
        at = step.to;
        out.push(step);
        if out.len() > steps.len() {
            return Err(PlanError::Cycle);
        }
    }
    if at != target {
        return Err(PlanError::Overshoot { reached: at, target });
    }
    Ok(out)
}

/// [`plan`] が拒む登録の壊れ方
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// ファイルが目標より新しい（新しい tako が書いたファイルを古い tako が読んだ）
    FromFuture { current: u32, target: u32 },
    /// その版数から進む手順が登録されていない
    MissingStep { from: u32, target: u32 },
    /// 手順が版数を進めない（登録ミス）
    NotAdvancing { from: u32, to: u32 },
    /// 目標をちょうどに踏めなかった（手順の to が飛んでいる）
    Overshoot { reached: u32, target: u32 },
    /// 手順が循環している
    Cycle,
}

impl PlanError {
    pub fn message(&self) -> String {
        match self {
            Self::FromFuture { current, target } => format!(
                "ファイルの形式（v{current}）が この tako が知る形式（v{target}）より新しい。tako を更新してください"
            ),
            Self::MissingStep { from, target } => {
                format!("v{from} から v{target} へ進む移行手順が登録されていない")
            }
            Self::NotAdvancing { from, to } => {
                format!("移行手順の版数が進んでいない（v{from} -> v{to}）")
            }
            Self::Overshoot { reached, target } => {
                format!("移行手順が v{reached} で止まり目標 v{target} に届かない")
            }
            Self::Cycle => "移行手順が循環している".to_string(),
        }
    }
}

/// 1 ファイルの移行結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    /// ファイルが無い（移行するものが無い。エラーではない）
    Absent,
    /// 既に最新形式だった
    UpToDate { version: u32 },
    /// 移行した
    Migrated {
        from: u32,
        to: u32,
        /// 退避先（旧内容はここに残る）
        backup: PathBuf,
        /// 適用した手順の説明（日英）
        applied: Vec<Note>,
    },
    /// 解釈できないので退避だけした（**内容は捨てない**）
    Unreadable { quarantine: PathBuf, reason: String },
    /// 登録の壊れ方が判明したので触らなかった
    Refused { reason: String },
    /// 移行しようとして失敗した（元のファイルは無傷）
    Failed { reason: String },
}

impl FileOutcome {
    /// 実際にファイルを書き換えたか（通知を出すかの判断に使う）
    pub fn changed(&self) -> bool {
        matches!(self, Self::Migrated { .. })
    }

    /// 人が気にするべき状態か（`tako migrate status` の警告行になる）
    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            Self::Unreadable { .. } | Self::Refused { .. } | Self::Failed { .. }
        )
    }

    /// 機械可読な状態名（CLI / MCP の応答で使う）
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::UpToDate { .. } => "up_to_date",
            Self::Migrated { .. } => "migrated",
            Self::Unreadable { .. } => "unreadable",
            Self::Refused { .. } => "refused",
            Self::Failed { .. } => "failed",
        }
    }
}

/// 1 ファイルぶんの記録（種別 + 実パス + 結果）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileReport {
    pub id: SchemaId,
    pub path: PathBuf,
    pub outcome: FileOutcome,
}

/// 一連の移行の記録。CLI / MCP / ログ / 一言通知はすべてここから作る
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub files: Vec<FileReport>,
}

impl MigrationReport {
    pub fn push(&mut self, report: FileReport) {
        self.files.push(report);
    }

    /// 実際に移行したファイル
    pub fn migrated(&self) -> impl Iterator<Item = &FileReport> {
        self.files.iter().filter(|f| f.outcome.changed())
    }

    /// 人へ知らせるべきもの（解釈不能・拒否・失敗）
    pub fn attention(&self) -> impl Iterator<Item = &FileReport> {
        self.files.iter().filter(|f| f.outcome.needs_attention())
    }

    pub fn changed_count(&self) -> usize {
        self.migrated().count()
    }

    /// GUI / CLI の一言通知（何も起きていなければ None）。
    /// **移行したことを黙らない**ための唯一の文言生成口
    pub fn notice(&self) -> Option<String> {
        let migrated = self.changed_count();
        let attention = self.attention().count();
        if migrated == 0 && attention == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if migrated > 0 {
            parts.push(
                NOTICE_MIGRATED
                    .text()
                    .replace("{n}", &migrated.to_string()),
            );
        }
        if attention > 0 {
            parts.push(
                NOTICE_ATTENTION
                    .text()
                    .replace("{n}", &attention.to_string()),
            );
        }
        Some(parts.join(" / "))
    }
}

const NOTICE_MIGRATED: Note = Note::new(
    "設定ファイル {n} 件を新しい形式へ自動移行しました（旧内容は .bak へ退避）",
    "Automatically migrated {n} config file(s) to the new format (old contents kept as .bak)",
);
const NOTICE_ATTENTION: Note = Note::new(
    "設定ファイル {n} 件は読めなかったので退避しました（tako migrate status で確認できます）",
    "{n} config file(s) could not be read and were set aside (see `tako migrate status`)",
);

/// 移行前の退避先（`<name>.pre-v<from>.bak`）。
/// **世代を名前に持たせる**ので、同じファイルを何度も移行しても前の退避を潰さない
pub fn backup_path(path: &Path, from_version: u32) -> PathBuf {
    sibling(path, &format!(".pre-v{from_version}.bak"))
}

/// 解釈できなかった内容の退避先（`<name>.unreadable.bak`）。
/// 既定値へ落とす前に**必ず**ここへ写す（黙って捨てない）
pub fn quarantine_path(path: &Path) -> PathBuf {
    sibling(path, ".unreadable.bak")
}

/// `<path>` と同じディレクトリに、ファイル名へ接尾辞を足したパスを作る。
/// `with_extension` は既存の拡張子を食う（`settings.json` → `settings.pre-v1`）ので使わない
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    name.push_str(suffix);
    match path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// テキストに対する移行の本体（純粋。ファイル I/O は呼び出し側）。
///
/// 返り値が `Ok(None)` なら書き換え不要（既に最新）。`Ok(Some((text, from, applied)))` なら
/// `text` を書き、`from` の世代として退避を取る。
#[allow(clippy::type_complexity)]
pub fn migrate_text(
    spec: &SchemaSpec,
    text: &str,
) -> Result<Option<(String, u32, Vec<Note>)>, MigrateTextError> {
    let current = (spec.detect)(text);
    let steps = plan(current, spec.target_version, spec.steps).map_err(MigrateTextError::Plan)?;
    if steps.is_empty() {
        return Ok(None);
    }
    let mut body = text.to_string();
    let mut applied = Vec::new();
    for step in steps {
        match (step.apply)(&body) {
            Ok(Some(next)) => {
                body = next;
                applied.push(step.describe);
            }
            // 変換すべき内容が無かった = その世代の差分を既に満たしている。
            // 版数だけ進めて次へ（冪等性の実体。二重適用で壊れない）
            Ok(None) => {}
            Err(reason) => {
                return Err(MigrateTextError::Step {
                    from: step.from,
                    to: step.to,
                    reason,
                })
            }
        }
    }
    if body == text {
        return Ok(None);
    }
    Ok(Some((body, current, applied)))
}

/// [`migrate_text`] の失敗
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrateTextError {
    Plan(PlanError),
    Step { from: u32, to: u32, reason: String },
}

impl MigrateTextError {
    pub fn message(&self) -> String {
        match self {
            Self::Plan(e) => e.message(),
            Self::Step { from, to, reason } => {
                format!("v{from} -> v{to} の移行に失敗: {reason}")
            }
        }
    }

    /// 登録の壊れ方（= 触らずに拒否すべき）か、変換の失敗か
    pub fn is_plan_error(&self) -> bool {
        matches!(self, Self::Plan(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE_A: Note = Note::new("a", "a");
    const NOTE_B: Note = Note::new("b", "b");

    fn detect_v1(_: &str) -> u32 {
        1
    }

    fn add_a(text: &str) -> Result<Option<String>, String> {
        if text.contains('a') {
            return Ok(None);
        }
        Ok(Some(format!("{text}a")))
    }

    fn add_b(text: &str) -> Result<Option<String>, String> {
        if text.contains('b') {
            return Ok(None);
        }
        Ok(Some(format!("{text}b")))
    }

    fn fail(_: &str) -> Result<Option<String>, String> {
        Err("こわれた".into())
    }

    const STEPS: &[Step] = &[
        Step {
            from: 1,
            to: 2,
            describe: NOTE_A,
            apply: add_a,
        },
        Step {
            from: 2,
            to: 3,
            describe: NOTE_B,
            apply: add_b,
        },
    ];

    #[test]
    fn 識別子は往復する() {
        for id in SchemaId::all() {
            assert_eq!(SchemaId::parse(id.as_str()), Some(*id), "{}", id.as_str());
        }
        assert_eq!(SchemaId::parse("なにか"), None);
    }

    #[test]
    fn 識別子は重複しない() {
        let mut seen = std::collections::BTreeSet::new();
        for id in SchemaId::all() {
            assert!(seen.insert(id.as_str()), "重複: {}", id.as_str());
        }
        assert_eq!(seen.len(), SchemaId::all().len());
    }

    #[test]
    fn planは連続する手順を並べる() {
        let steps = plan(1, 3, STEPS).expect("並べられる");
        assert_eq!(steps.len(), 2);
        assert_eq!((steps[0].from, steps[1].to), (1, 3));
        assert!(plan(3, 3, STEPS).expect("最新なら空").is_empty());
    }

    #[test]
    fn planは壊れた登録を拒む() {
        // 目標より新しいファイル
        assert_eq!(
            plan(4, 3, STEPS),
            Err(PlanError::FromFuture {
                current: 4,
                target: 3
            })
        );
        // 手順が無い
        const GAP: &[Step] = &[Step {
            from: 1,
            to: 2,
            describe: NOTE_A,
            apply: add_a,
        }];
        assert_eq!(
            plan(1, 3, GAP),
            Err(PlanError::MissingStep { from: 2, target: 3 })
        );
        // 進まない手順
        const STUCK: &[Step] = &[Step {
            from: 1,
            to: 1,
            describe: NOTE_A,
            apply: add_a,
        }];
        assert_eq!(
            plan(1, 2, STUCK),
            Err(PlanError::NotAdvancing { from: 1, to: 1 })
        );
        // 飛び越え
        const JUMP: &[Step] = &[Step {
            from: 1,
            to: 3,
            describe: NOTE_A,
            apply: add_a,
        }];
        assert_eq!(
            plan(1, 2, JUMP),
            Err(PlanError::Overshoot {
                reached: 3,
                target: 2
            })
        );
    }

    #[test]
    fn migrate_textは手順を順に当てる() {
        let spec = SchemaSpec {
            id: SchemaId::Settings,
            target_version: 3,
            detect: detect_v1,
            steps: STEPS,
        };
        let (text, from, applied) = migrate_text(&spec, "x").expect("成功").expect("変わる");
        assert_eq!(text, "xab");
        assert_eq!(from, 1);
        assert_eq!(applied.len(), 2);
    }

    /// 冪等性の核: 手順が「もう当たっている」と答えたら書き換えない
    #[test]
    fn migrate_textは二度目に何もしない() {
        let spec = SchemaSpec {
            id: SchemaId::Settings,
            target_version: 3,
            detect: detect_v1,
            steps: STEPS,
        };
        // detect は常に v1 を返すが、内容は既に新形式（a と b がある）
        assert_eq!(migrate_text(&spec, "xab").expect("成功"), None);
    }

    #[test]
    fn migrate_textは失敗を申告する() {
        const BROKEN: &[Step] = &[Step {
            from: 1,
            to: 2,
            describe: NOTE_A,
            apply: fail,
        }];
        let spec = SchemaSpec {
            id: SchemaId::Settings,
            target_version: 2,
            detect: detect_v1,
            steps: BROKEN,
        };
        let err = migrate_text(&spec, "x").expect_err("失敗する");
        assert!(!err.is_plan_error());
        assert!(err.message().contains("こわれた"), "{}", err.message());
    }

    #[test]
    fn 退避先は拡張子を食わない() {
        let p = Path::new("/tmp/dir/settings.json");
        assert_eq!(
            backup_path(p, 1),
            PathBuf::from("/tmp/dir/settings.json.pre-v1.bak")
        );
        assert_eq!(
            backup_path(p, 2),
            PathBuf::from("/tmp/dir/settings.json.pre-v2.bak"),
            "世代が違えば退避先も違う（前の退避を潰さない）"
        );
        assert_eq!(
            quarantine_path(p),
            PathBuf::from("/tmp/dir/settings.json.unreadable.bak")
        );
    }

    #[test]
    fn 通知は起きたことだけを言う() {
        let mut report = MigrationReport::default();
        assert_eq!(report.notice(), None, "何もなければ黙る");
        report.push(FileReport {
            id: SchemaId::Settings,
            path: PathBuf::from("/tmp/settings.json"),
            outcome: FileOutcome::UpToDate { version: 1 },
        });
        assert_eq!(report.notice(), None, "最新なら黙る");
        report.push(FileReport {
            id: SchemaId::Projects,
            path: PathBuf::from("/tmp/projects.yaml"),
            outcome: FileOutcome::Migrated {
                from: 1,
                to: 2,
                backup: PathBuf::from("/tmp/projects.yaml.pre-v1.bak"),
                applied: vec![NOTE_A],
            },
        });
        let notice = report.notice().expect("移行したら言う");
        assert!(notice.contains('1'), "{notice}");
        assert_eq!(report.changed_count(), 1);
        report.push(FileReport {
            id: SchemaId::Recent,
            path: PathBuf::from("/tmp/recent.json"),
            outcome: FileOutcome::Unreadable {
                quarantine: PathBuf::from("/tmp/recent.json.unreadable.bak"),
                reason: "壊れている".into(),
            },
        });
        assert_eq!(report.attention().count(), 1);
        assert!(report.notice().expect("両方言う").contains('/'));
    }
}
