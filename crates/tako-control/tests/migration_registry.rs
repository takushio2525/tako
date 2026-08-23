//! 移行登録簿の被覆と、スキーマ変更の検出（Issue #916 要件 1・4）
//!
//! **狙い**: 設定・データファイルのスキーマを変えた人がマイグレーションを
//! 書き忘れたら、レビューの目ではなく**テストが落ちて**気付くこと。
//! `config_share::catalog` の被覆テスト（#513）と `platform::support::MATRIX` の
//! T1（#515）と同じ考え方を「永続ファイルのスキーマ世代」に適用する。
//!
//! ## 2 本立てにしている理由
//!
//! - **被覆**: 共有分類カタログが「設定ファイル」と宣言しているものは、
//!   移行の番地（`SchemaId`）にも載っていなければならない。片方だけに足すと
//!   「共有はされるが移行はされない」ファイルができる
//! - **指紋**: 永続構造体のフィールド集合を固定する。フィールドを増減・改名した
//!   PR はここが落ちるので、`target_version` を上げて `Step` を足したか、
//!   （serde の default / alias で旧ファイルがそのまま読めるので）移行が要らないか、
//!   のどちらかを**明示的に**選ぶことになる

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tako_control::migrations;
use tako_core::migration::SchemaId;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルートを解決できない")
        .to_path_buf()
}

/// 登録簿は全種別を 1 回ずつ持ち、識別子は往復する
#[test]
fn 登録簿は全種別を網羅する() {
    let ids: BTreeSet<&str> = migrations::SPECS.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        migrations::SPECS.len(),
        "識別子が重複している: {ids:?}"
    );
    for id in SchemaId::all() {
        assert!(
            migrations::spec(*id).is_some(),
            "{} が登録簿に無い（tako-control::migrations::SPECS へ 1 行足す）",
            id.as_str()
        );
    }
}

/// 共有分類カタログが「設定ファイル」と宣言しているものは移行の番地にも載っている。
///
/// 対応表をここに置くのは、カタログのパス（`orchestrator/projects.yaml`）と
/// 番地（`SchemaId::Projects`）が 1:1 で対応することを人が読める形で固定するため
#[test]
fn 共有対象の設定ファイルは移行の番地にも載っている() {
    // (カタログ上のパス, 対応する番地)。カタログ側だけに足したらここで落ちる
    const MAPPING: &[(&str, SchemaId)] = &[
        ("settings.json", SchemaId::Settings),
        ("layout.json", SchemaId::Layout),
        ("sessions.yaml", SchemaId::Sessions),
        ("workers.yaml", SchemaId::Workers),
        ("recent.json", SchemaId::Recent),
        ("acceptance_gates.yaml", SchemaId::AcceptanceGates),
        ("task_checkpoints.yaml", SchemaId::TaskCheckpoints),
        ("config-share.json", SchemaId::ConfigShare),
        ("lid-guard.json", SchemaId::LidGuard),
        ("orchestrator/config.yaml", SchemaId::OrchestratorConfig),
        ("orchestrator/projects.yaml", SchemaId::Projects),
        ("orchestrator/accounts.yaml", SchemaId::Accounts),
        ("orchestrator/profiles/", SchemaId::Profiles),
        ("orchestrator/solo-profiles/", SchemaId::SoloProfiles),
        ("orchestrator/ledger.yaml", SchemaId::Ledger),
        ("orchestrator/handoff/", SchemaId::Handoff),
        ("instances/", SchemaId::DiscoveryInstance),
        ("remote/", SchemaId::RemoteDevices),
    ];
    use tako_control::config_share::catalog;
    for (path, id) in MAPPING {
        assert!(
            catalog::CATALOG
                .iter()
                .any(|e| e.root == catalog::Root::TakoData && e.path == *path),
            "共有分類カタログに {path} が無い（対応表の側が古い）"
        );
        assert!(
            migrations::spec(*id).is_some(),
            "{} が移行の番地に無い",
            id.as_str()
        );
    }
    assert_eq!(
        MAPPING.len(),
        SchemaId::all().len(),
        "番地を足したら対応表にも足す（どのファイルの版数かが分からなくなる）"
    );
}

/// 永続構造体のフィールド集合の指紋。
/// ソースを読んで `#[derive(... Deserialize ...)] pub struct X { pub a: T, ... }` を集める
fn fingerprint() -> BTreeMap<String, Vec<String>> {
    // (ファイル, 構造体名) の並び。**永続ファイルへ直に serde される型だけ**を挙げる
    const TARGETS: &[(&str, &[&str])] = &[
        (
            "crates/tako-control/src/settings.rs",
            &["Settings", "ThemePreset"],
        ),
        (
            "crates/tako-control/src/layout.rs",
            &[
                "LayoutFile",
                "WindowLayout",
                "WindowFrame",
                "TabLayout",
                "NodeLayout",
                "PaneLayout",
                "PreviewLayout",
            ],
        ),
        (
            "crates/tako-control/src/sessions.rs",
            &["SessionCatalog", "SessionEntry"],
        ),
        (
            "crates/tako-control/src/orchestrator/registry.rs",
            &["WorkerRegistry", "WorkerEntry"],
        ),
        (
            "crates/tako-control/src/orchestrator/ledger.rs",
            &["Ledger", "LedgerEntry"],
        ),
        (
            "crates/tako-control/src/task_checkpoints.rs",
            &["TaskCheckpointStore"],
        ),
        (
            "crates/tako-core/src/task_checkpoint.rs",
            &["TaskCheckpoint"],
        ),
        (
            "crates/tako-control/src/acceptance_gates.rs",
            &["AcceptanceGateStore"],
        ),
        (
            "crates/tako-core/src/acceptance_gate.rs",
            &["AcceptanceGate", "AcceptanceCriterion"],
        ),
        (
            "crates/tako-control/src/setup.rs",
            &["SetupConfig", "SetupState", "OrchestratorConfig"],
        ),
        (
            "crates/tako-control/src/orchestrator/mod.rs",
            &[
                "ProjectsConfig",
                "ProjectEntry",
                "AccountsConfig",
                "AccountEntry",
                "Profile",
            ],
        ),
        (
            "crates/tako-core/src/recent.rs",
            &["RecentList", "RecentEntry"],
        ),
        ("crates/tako-control/src/discovery.rs", &["ControlInfo"]),
        (
            "crates/tako-control/src/remote_auth.rs",
            &["DevicesFile", "Device"],
        ),
        (
            "crates/tako-control/src/config_share/mod.rs",
            &["ShareState"],
        ),
    ];
    let mut out = BTreeMap::new();
    for (file, names) in TARGETS {
        let text = std::fs::read_to_string(repo_root().join(file))
            .unwrap_or_else(|e| panic!("{file} を読めない: {e}"));
        for name in *names {
            let fields = struct_fields(&text, name)
                .unwrap_or_else(|| panic!("{file} に struct {name} が見つからない"));
            out.insert((*name).to_string(), fields);
        }
    }
    out
}

/// `pub struct <name> { ... }` のフィールド名 / `pub enum <name> { ... }` の
/// バリアント名を宣言順に集める。`#[serde(rename = "x")]` が付いていれば
/// ワイヤ上の名前を採る（実際に永続される名前で固定する）
fn struct_fields(text: &str, name: &str) -> Option<Vec<String>> {
    // 可視性は問わない（`pub(crate) struct DevicesFile` のような非公開型も
    // 永続ファイルの形式を決めているので固定の対象）
    let start = ["struct", "enum"]
        .iter()
        .flat_map(|kind| [format!("{kind} {name} "), format!("{kind} {name}{{")])
        .find_map(|head| text.find(head.as_str()))?;
    let body_start = start + text[start..].find('{')?;
    let mut depth = 0usize;
    let mut end = body_start;
    for (i, c) in text[body_start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &text[body_start..end];
    let mut fields = Vec::new();
    let mut pending_rename: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if let Some(rename) = line
            .strip_prefix("#[serde(rename = \"")
            .and_then(|rest| rest.split('"').next())
        {
            pending_rename = Some(rename.to_string());
            continue;
        }
        // フィールド（`pub a: T` / `pub(crate) a: T` / `a: T`）
        let rest = line
            .strip_prefix("pub(crate) ")
            .or_else(|| line.strip_prefix("pub "))
            .unwrap_or(line);
        // 型が次の行へ折り返している続き行（`std::collections::BTreeMap<...>`）は
        // フィールド宣言ではない。`::` を含む先頭語を弾く
        if rest.contains(':') && !rest.starts_with("//") && !rest.starts_with("std::") {
            if let Some(field) = rest.split(':').next() {
                let field = field.trim();
                if !field.is_empty() && field.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    fields.push(pending_rename.take().unwrap_or_else(|| field.to_string()));
                    continue;
                }
            }
        }
        // enum のバリアント（`Pane(PaneLayout),` / `Split { ... }`）
        if let Some(head) = line.split(['(', '{', ',', ' ']).next() {
            if head.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                && head.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                fields.push(pending_rename.take().unwrap_or_else(|| head.to_string()));
                continue;
            }
        }
        if !line.starts_with("#[") && !line.starts_with("//") && !line.starts_with("///") {
            pending_rename = None;
        }
    }
    Some(fields)
}

/// スキーマ変更の検出。**フィールドを増減・改名したらここが落ちる**
#[test]
fn 永続構造体の指紋がスナップショットと一致する() {
    let path = repo_root().join("crates/tako-control/testdata/persisted_schema_fingerprint.txt");
    let mut rendered = String::new();
    for (name, fields) in fingerprint() {
        rendered.push_str(&format!("{name}: {}\n", fields.join(", ")));
    }
    // 版数も一緒に固定する（スキーマだけ変えて版数を上げ忘れた PR で差分が出るように）
    rendered.push('\n');
    for spec in migrations::SPECS {
        rendered.push_str(&format!(
            "version {}: v{} steps={}\n",
            spec.id.as_str(),
            spec.target_version,
            spec.steps.len()
        ));
    }
    if std::env::var_os("TAKO_UPDATE_SCHEMA_FINGERPRINT").is_some() {
        std::fs::write(&path, &rendered)
            .unwrap_or_else(|e| panic!("指紋を更新できない {}: {e}", path.display()));
        return;
    }
    let snapshot = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("指紋を読めない {}: {e}", path.display()));
    assert_eq!(
        rendered, snapshot,
        "永続ファイルのスキーマが変わった。\n\
         旧いファイルがそのまま読めるか（serde の default / alias で足りるか）を確かめ、\n\
         読めないなら tako-control::migrations で target_version を上げて Step を足すこと。\n\
         そのうえで `TAKO_UPDATE_SCHEMA_FINGERPRINT=1 cargo test -p tako-control \
         --test migration_registry` で指紋を更新する（#916）"
    );
}
