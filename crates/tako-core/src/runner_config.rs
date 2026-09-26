//! Code Runner の実行設定（Issue #1726 / #1729 S1）
//!
//! 「**このマシンでこのファイル / プロジェクトをどう走らせるか**」の上書きを表す型と、
//! スコープの重ね方（[`merge`]）。コマンドを決める層（宣言 → プロジェクト既定 →
//! 拡張子既定 = [`crate::runner`]）はそのまま残し、その上に 1 層だけ重ねる。
//! 設計: `.agent/plans/2026-09-runner-settings.md` §4
//!
//! ## ここに置かないもの
//!
//! - **読み書き**（`<data_dir>/run-configs.json`）は tako-control の仕事（S3 = #1731）。
//!   ここは I/O を持たない純粋部分だけ
//! - **実行環境の名前**（venv / conda …）。[`RuntimeRef::manager`] は enum ではなく
//!   [`crate::runtime_env`] の表が持つ検出器 ID の文字列にしてある。enum にすると
//!   道具の名前が表の外に並び、「言語を足す = 表に行を足すだけ」が崩れる
//!   （設計書 §5.1。MCP の enum の正本も表になる = #1467）

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 保存する実行環境の参照（**パスを直書きしない**。実行時に
/// [`crate::runtime_env::resolve_ref`] で解く = VS Code の Python 拡張と同じ作法）
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RuntimeRef {
    /// 検出器 ID（`runtime_env` の表の `id`）か [`RuntimeRef::EXPLICIT_PATH`]
    pub manager: String,
    /// 検出器ごとの鍵（環境のディレクトリ・版名・env 名・プログラム名。意味は表の各戦略が決める）
    pub key: String,
}

impl RuntimeRef {
    /// ユーザーが interpreter のパスを直接指定したとき（どの検出器にも属さない）
    pub const EXPLICIT_PATH: &'static str = "path";

    pub fn new(manager: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            manager: manager.into(),
            key: key.into(),
        }
    }
}

/// 実行設定の項目（UI の行・CLI の `--reset <項目>`・MCP の enum の正本）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RunConfigField {
    Profile,
    Runtime,
    Args,
    Env,
    Cwd,
    Before,
}

impl RunConfigField {
    /// 並びは UI の行順（設計書 §7.2）
    pub const ALL: &'static [RunConfigField] = &[
        RunConfigField::Profile,
        RunConfigField::Runtime,
        RunConfigField::Args,
        RunConfigField::Env,
        RunConfigField::Cwd,
        RunConfigField::Before,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Runtime => "runtime",
            Self::Args => "args",
            Self::Env => "env",
            Self::Cwd => "cwd",
            Self::Before => "before",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|f| f.as_str() == s)
    }
}

/// 1 スコープ（ファイル / プロジェクト）ぶんの上書き。`None` / 空 = 上位へ委ねる
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunConfig {
    /// 選択中のプロファイル（宣言名。`None` = `runner.rs` の既定の選び方）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// 実行環境（kind → 選択）。プロジェクトのスコープでは複数言語を持てる
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub runtime: BTreeMap<String, RuntimeRef>,
    /// 引数。**シェルへそのまま渡る 1 本の文字列**（空文字も「引数なし」という明示の値）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// 環境変数（値はリテラル。tako の変数だけ展開する = S4）
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// 作業ディレクトリ（相対はファイルのディレクトリ基準 = `tako:cwd` と同じ意味論）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 実行前コマンド（失敗したら本体を走らせない = S4）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// 最後に書いた時刻（ファイル数の上限で古いものから落とすための LRU の材料。S3）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl RunConfig {
    /// 上書きを 1 つも持たないか（`updated_at` は上書きではないので見ない）
    pub fn is_empty(&self) -> bool {
        RunConfigField::ALL.iter().all(|f| !self.has(*f))
    }

    /// その項目を上書きしているか
    pub fn has(&self, field: RunConfigField) -> bool {
        match field {
            RunConfigField::Profile => self.profile.is_some(),
            RunConfigField::Runtime => !self.runtime.is_empty(),
            RunConfigField::Args => self.args.is_some(),
            RunConfigField::Env => !self.env.is_empty(),
            RunConfigField::Cwd => self.cwd.is_some(),
            RunConfigField::Before => self.before.is_some(),
        }
    }

    /// その項目を消して上位へ委ねる（UI の「既定に戻す」・CLI の `--reset <項目>`）
    pub fn clear(&mut self, field: RunConfigField) {
        match field {
            RunConfigField::Profile => self.profile = None,
            RunConfigField::Runtime => self.runtime.clear(),
            RunConfigField::Args => self.args = None,
            RunConfigField::Env => self.env.clear(),
            RunConfigField::Cwd => self.cwd = None,
            RunConfigField::Before => self.before = None,
        }
    }
}

/// 値の出どころ（UI の出典タグ・応答の `source`）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldSource {
    /// ファイルのスコープに保存した値
    File,
    /// プロジェクトのスコープに保存した値
    Project,
    /// ファイル内の宣言（`tako:cwd` 等）
    Declaration,
    /// 自動検出（実行環境だけ）
    Auto,
    /// どこにも無い = 既定の動き
    Default,
}

impl FieldSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Project => "project",
            Self::Declaration => "declaration",
            Self::Auto => "auto",
            Self::Default => "default",
        }
    }
}

/// 出どころ付きの値
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub source: FieldSource,
}

impl<T> Sourced<T> {
    fn new(value: T, source: FieldSource) -> Self {
        Self { value, source }
    }
}

/// 宣言（ファイル内の `tako:` 行）が実行設定の項目へ渡すもの。
/// 宣言が明示したときだけ `Some`（`RunPlan::cwd` は既定のディレクトリでも埋まるので使わない）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Declared {
    pub cwd: Option<String>,
}

/// スコープを重ねた結果。`None` / 空の項目は既定（[`FieldSource::Default`]）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub profile: Option<Sourced<String>>,
    pub runtime: BTreeMap<String, Sourced<RuntimeRef>>,
    pub args: Option<Sourced<String>>,
    pub env: BTreeMap<String, Sourced<String>>,
    pub cwd: Option<Sourced<String>>,
    pub before: Option<Sourced<String>>,
}

impl EffectiveConfig {
    /// 項目の出どころ（`env` / `runtime` はキーごとに違いうるので、いちばん強い出どころを返す）
    pub fn source_of(&self, field: RunConfigField) -> FieldSource {
        fn strongest<'a>(it: impl Iterator<Item = &'a FieldSource>) -> FieldSource {
            it.copied().min().unwrap_or(FieldSource::Default)
        }
        match field {
            RunConfigField::Profile => self
                .profile
                .as_ref()
                .map_or(FieldSource::Default, |s| s.source),
            RunConfigField::Runtime => strongest(self.runtime.values().map(|s| &s.source)),
            RunConfigField::Args => self
                .args
                .as_ref()
                .map_or(FieldSource::Default, |s| s.source),
            RunConfigField::Env => strongest(self.env.values().map(|s| &s.source)),
            RunConfigField::Cwd => self.cwd.as_ref().map_or(FieldSource::Default, |s| s.source),
            RunConfigField::Before => self
                .before
                .as_ref()
                .map_or(FieldSource::Default, |s| s.source),
        }
    }
}

/// スコープを重ねる（設計書 §4.3）。
///
/// ```text
/// file 設定 > project 設定 > 宣言（cwd）> 自動（実行環境）> 既定
/// ```
///
/// - **保存した設定は宣言より強い**: 宣言はファイルの作者の既定で、保存値はこのマシンの
///   利用者が後から明示した上書き。どちらが効いているかは出どころで常に見える
/// - `env` は**キー単位**で重ねる（file の `A` が project の `A` を上書きし、他のキーは残る）
/// - `runtime` は **kind 単位**で重ねる。`auto` は検出が自動で選んだもの（kind → 参照）
/// - CLI / MCP の一回限りの指定はここを通さない（呼び出し側で最後に上書きする）
pub fn merge(
    file: Option<&RunConfig>,
    project: Option<&RunConfig>,
    declared: &Declared,
    auto: &BTreeMap<String, RuntimeRef>,
) -> EffectiveConfig {
    let layers = [(file, FieldSource::File), (project, FieldSource::Project)];

    let scalar = |pick: fn(&RunConfig) -> Option<&String>| -> Option<Sourced<String>> {
        layers
            .iter()
            .find_map(|(cfg, src)| cfg.and_then(pick).map(|v| Sourced::new(v.clone(), *src)))
    };

    let mut env = BTreeMap::new();
    let mut runtime = BTreeMap::new();
    for (kind, r) in auto {
        runtime.insert(kind.clone(), Sourced::new(r.clone(), FieldSource::Auto));
    }
    // 弱い順に上書きしていく（project → file）
    for (cfg, src) in layers.iter().rev() {
        let Some(cfg) = cfg else { continue };
        for (k, v) in &cfg.env {
            env.insert(k.clone(), Sourced::new(v.clone(), *src));
        }
        for (kind, r) in &cfg.runtime {
            runtime.insert(kind.clone(), Sourced::new(r.clone(), *src));
        }
    }

    EffectiveConfig {
        profile: scalar(|c| c.profile.as_ref()),
        runtime,
        args: scalar(|c| c.args.as_ref()),
        env,
        cwd: scalar(|c| c.cwd.as_ref()).or_else(|| {
            declared
                .cwd
                .as_ref()
                .map(|v| Sourced::new(v.clone(), FieldSource::Declaration))
        }),
        before: scalar(|c| c.before.as_ref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RunConfig {
        RunConfig::default()
    }

    fn auto_of(kind: &str, manager: &str, key: &str) -> BTreeMap<String, RuntimeRef> {
        BTreeMap::from([(kind.to_string(), RuntimeRef::new(manager, key))])
    }

    #[test]
    fn 何も無ければ全項目が既定() {
        let eff = merge(None, None, &Declared::default(), &BTreeMap::new());
        assert_eq!(eff, EffectiveConfig::default());
        for f in RunConfigField::ALL {
            assert_eq!(eff.source_of(*f), FieldSource::Default, "{f:?}");
        }
    }

    #[test]
    fn ファイルがプロジェクトより強い() {
        let mut file = cfg();
        file.args = Some("--file".into());
        let mut project = cfg();
        project.args = Some("--project".into());
        project.before = Some("make".into());
        project.profile = Some("test".into());

        let eff = merge(
            Some(&file),
            Some(&project),
            &Declared::default(),
            &BTreeMap::new(),
        );
        assert_eq!(
            eff.args,
            Some(Sourced::new("--file".into(), FieldSource::File))
        );
        // ファイルが持たない項目はプロジェクトから来る
        assert_eq!(
            eff.before,
            Some(Sourced::new("make".into(), FieldSource::Project))
        );
        assert_eq!(
            eff.profile,
            Some(Sourced::new("test".into(), FieldSource::Project))
        );
    }

    #[test]
    fn 空文字の引数は上位を塞ぐ明示の値() {
        let mut file = cfg();
        file.args = Some(String::new());
        let mut project = cfg();
        project.args = Some("--project".into());
        let eff = merge(
            Some(&file),
            Some(&project),
            &Declared::default(),
            &BTreeMap::new(),
        );
        assert_eq!(
            eff.args,
            Some(Sourced::new(String::new(), FieldSource::File))
        );
    }

    #[test]
    fn 保存した作業ディレクトリは宣言より強い() {
        let declared = Declared {
            cwd: Some("..".into()),
        };
        let eff = merge(None, None, &declared, &BTreeMap::new());
        assert_eq!(
            eff.cwd,
            Some(Sourced::new("..".into(), FieldSource::Declaration))
        );

        let mut project = cfg();
        project.cwd = Some("build".into());
        let eff = merge(None, Some(&project), &declared, &BTreeMap::new());
        assert_eq!(
            eff.cwd,
            Some(Sourced::new("build".into(), FieldSource::Project))
        );
    }

    #[test]
    fn 環境変数はキー単位で重なる() {
        let mut project = cfg();
        project.env.insert("A".into(), "project".into());
        project.env.insert("B".into(), "project".into());
        let mut file = cfg();
        file.env.insert("A".into(), "file".into());
        file.env.insert("C".into(), "file".into());

        let eff = merge(
            Some(&file),
            Some(&project),
            &Declared::default(),
            &BTreeMap::new(),
        );
        assert_eq!(eff.env["A"], Sourced::new("file".into(), FieldSource::File));
        assert_eq!(
            eff.env["B"],
            Sourced::new("project".into(), FieldSource::Project)
        );
        assert_eq!(eff.env["C"], Sourced::new("file".into(), FieldSource::File));
        assert_eq!(eff.env.len(), 3);
        assert_eq!(eff.source_of(RunConfigField::Env), FieldSource::File);
    }

    #[test]
    fn 実行環境はkind単位で重なり保存値が自動より強い() {
        let mut project = cfg();
        project
            .runtime
            .insert("lang-a".into(), RuntimeRef::new("m1", "p"));
        project
            .runtime
            .insert("lang-b".into(), RuntimeRef::new("m2", "p"));
        let mut file = cfg();
        file.runtime
            .insert("lang-a".into(), RuntimeRef::new("m3", "f"));
        let auto = BTreeMap::from([
            ("lang-a".to_string(), RuntimeRef::new("auto", "a")),
            ("lang-c".to_string(), RuntimeRef::new("auto", "c")),
        ]);

        let eff = merge(Some(&file), Some(&project), &Declared::default(), &auto);
        assert_eq!(
            eff.runtime["lang-a"],
            Sourced::new(RuntimeRef::new("m3", "f"), FieldSource::File)
        );
        assert_eq!(
            eff.runtime["lang-b"],
            Sourced::new(RuntimeRef::new("m2", "p"), FieldSource::Project)
        );
        assert_eq!(
            eff.runtime["lang-c"],
            Sourced::new(RuntimeRef::new("auto", "c"), FieldSource::Auto)
        );
    }

    #[test]
    fn 自動だけなら出どころは自動() {
        let eff = merge(None, None, &Declared::default(), &auto_of("k", "m", "x"));
        assert_eq!(eff.source_of(RunConfigField::Runtime), FieldSource::Auto);
    }

    #[test]
    fn clearは項目だけを消す() {
        let mut c = cfg();
        c.args = Some("x".into());
        c.env.insert("A".into(), "1".into());
        c.runtime.insert("k".into(), RuntimeRef::new("m", "x"));
        c.clear(RunConfigField::Env);
        assert!(c.env.is_empty());
        assert!(c.has(RunConfigField::Args));
        assert!(c.has(RunConfigField::Runtime));
        for f in RunConfigField::ALL {
            c.clear(*f);
        }
        assert!(c.is_empty());
    }

    #[test]
    fn updated_atは上書きに数えない() {
        let c = RunConfig {
            updated_at: Some("2026-09-26T00:00:00Z".into()),
            ..cfg()
        };
        assert!(c.is_empty());
    }

    #[test]
    fn 項目名は往復できて重複しない() {
        let mut seen = std::collections::BTreeSet::new();
        for f in RunConfigField::ALL {
            assert_eq!(RunConfigField::parse(f.as_str()), Some(*f));
            assert!(seen.insert(f.as_str()), "重複: {}", f.as_str());
        }
        assert_eq!(RunConfigField::parse("target"), None, "target は S6 で足す");
    }

    #[test]
    fn 空の項目は保存形に出ない() {
        let json = serde_json::to_string(&cfg()).unwrap();
        assert_eq!(json, "{}");

        let mut c = cfg();
        c.args = Some("--verbose".into());
        c.runtime.insert("k".into(), RuntimeRef::new("m", ".venv"));
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(
            json,
            r#"{"runtime":{"k":{"manager":"m","key":".venv"}},"args":"--verbose"}"#
        );
        let back: RunConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn 知らない項目があっても読める() {
        // S6 の target のように後から足す項目を、古い版の tako が読んでも壊れない
        let back: RunConfig = serde_json::from_str(r#"{"args":"x","target":"new"}"#).unwrap();
        assert_eq!(back.args.as_deref(), Some("x"));
    }
}
