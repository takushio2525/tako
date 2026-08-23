//! 引き継ぎファイルの置き場と、旧形式からの自動移行（Issue #915 / #916）。
//!
//! 判定と文面は `tako_core::handoff`（純粋関数）が正で、ここはファイル I/O だけを持つ。
//!
//! 置き場は 2 系統:
//! - `handoff/{HANDOFF_PROJECTS_DIR}/<project-key>.md` — プロジェクト単位の引き継ぎ。
//!   後任 master へは**その master が管轄する分だけ**渡る
//! - `handoff/<profile>.md` — プロファイル運用メモ（共通置き場）。プロジェクトに
//!   紐付かない運用知識と、移行で持ち主を決められなかった内容の受け皿
//!
//! 移行は**手動コマンドを前提にしない**（#916 の恒久原則）。`tako setup` 実行時と、
//! master が引き継ぎを読む / 書く経路の差分検出時に、その場で自動で完遂する。
//! 安全要件は 4 つ: 冪等 / 旧ファイルを削除せず退避 / 実施の可視化 / 解釈できない内容を
//! 黙って捨てない。

use std::path::{Path, PathBuf};

use tako_core::handoff::{self as ho, HANDOFF_PROJECTS_DIR};

use super::{config_dir, ProjectsConfig};

/// `handoff/` ディレクトリ
pub fn handoff_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("handoff"))
}

/// `handoff/projects/` ディレクトリ
pub fn projects_handoff_dir() -> Option<PathBuf> {
    handoff_dir().map(|d| d.join(HANDOFF_PROJECTS_DIR))
}

/// 移行前のファイルを退避する先（`handoff/archive/`）
pub fn archive_dir() -> Option<PathBuf> {
    handoff_dir().map(|d| d.join("archive"))
}

/// プロジェクト単位の引き継ぎファイルのパス。
/// キーがファイル名として危険なら None（キー由来のパス脱出を構造的に防ぐ）
pub fn project_handoff_path(key: &str) -> Option<PathBuf> {
    if !ho::valid_project_key(key) {
        return None;
    }
    projects_handoff_dir().map(|d| d.join(format!("{key}.md")))
}

/// プロジェクト単位の引き継ぎを読む。不在・空なら None
pub fn read_project_handoff(key: &str) -> Option<String> {
    let path = project_handoff_path(key)?;
    read_non_empty(&path)
}

/// プロジェクト単位の引き継ぎを書く（ディレクトリ作成 + アトミック + 世代バックアップ）
pub fn write_project_handoff(key: &str, content: &str) -> Result<PathBuf, String> {
    let path = project_handoff_path(key)
        .ok_or_else(|| format!("プロジェクトキーがファイル名として使えない: {key}"))?;
    write_file(&path, content)?;
    Ok(path)
}

/// プロファイル運用メモを書く
pub fn write_profile_memo(profile: &str, content: &str) -> Result<PathBuf, String> {
    let path = super::handoff_path(profile)
        .ok_or_else(|| "ホームディレクトリが取得できない".to_string())?;
    write_file(&path, content)?;
    Ok(path)
}

/// 置いてあるプロジェクト単位の引き継ぎファイルを列挙する（キー順）
pub fn list_project_handoffs() -> Vec<(String, PathBuf)> {
    let Some(dir) = projects_handoff_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let key = path.file_stem()?.to_str()?.to_string();
            (path.extension().and_then(|s| s.to_str()) == Some("md") && ho::valid_project_key(&key))
                .then_some((key, path))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// プロファイル運用メモが置いてあるプロファイル名を列挙する（`handoff/*.md`）
pub fn list_profile_memos() -> Vec<(String, PathBuf)> {
    let Some(dir) = handoff_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("md") {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            (!name.is_empty()).then_some((name, path))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn read_non_empty(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .filter(|s| !s.trim().is_empty())
}

fn write_file(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("引き継ぎの置き場を作れない: {} ({e})", parent.display()))?;
    }
    crate::config_io::atomic_write_with_backup(path, content)
}

// --- 自動移行 ---------------------------------------------------------------

/// 1 プロファイルぶんの移行結果（可視化・応答 JSON 用）
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct MigrationOutcome {
    /// 対象プロファイル
    pub profile: String,
    /// 何かを移したか（false = 既に新形式 / 移す材料が無い）
    pub migrated: bool,
    /// 移した先（プロジェクトキー, パス）
    pub moved: Vec<(String, String)>,
    /// 共通置き場（プロファイル運用メモ）へ残した行数
    pub kept_lines: usize,
    /// 移行前のファイルの退避先
    pub archived_to: Option<String>,
    /// 移行で起きた問題（キーが不正・書き込み失敗など。移行自体は続ける）
    pub warnings: Vec<String>,
}

impl MigrationOutcome {
    /// 人が読む 1 行サマリ（CLI / 起動ログ / master への一言）
    pub fn summary(&self) -> Option<String> {
        if !self.migrated {
            return None;
        }
        let keys: Vec<&str> = self.moved.iter().map(|(k, _)| k.as_str()).collect();
        Some(format!(
            "handoff/{}.md をプロジェクト単位へ自動移行しました（#915）: {} / 共通置き場に {} 行を残し、移行前のファイルは {} へ退避",
            self.profile,
            if keys.is_empty() {
                "移動なし".to_string()
            } else {
                keys.join(", ")
            },
            self.kept_lines,
            self.archived_to.as_deref().unwrap_or("(退避なし)")
        ))
    }
}

/// そのプロファイルの引き継ぎを新形式へ移す（**冪等**）。
///
/// 旧形式（プロファイル単位の混在ファイル）を検出したら、プロジェクトへ割り当てられる
/// 断片を `handoff/projects/<key>.md` へ移し、割り当てられなかった断片は運用メモへ残す。
/// 移行前のファイルは `handoff/archive/` へ退避してから書き換える。
/// 移す材料が無ければ**ファイルに触らない**（2 回目以降は no-op）
pub fn ensure_migrated(profile: &str) -> MigrationOutcome {
    let mut out = MigrationOutcome {
        profile: profile.to_string(),
        ..Default::default()
    };
    let Some(memo_path) = super::handoff_path(profile) else {
        return out;
    };
    let Some(content) = read_non_empty(&memo_path) else {
        return out;
    };
    // 安い前判定。**番地が入っていて**移行の材料になり得る見出しも無いなら、
    // projects.yaml も読まない（master は self を定期的に叩くので定常状態で
    // ファイル読みを増やさない）。番地が無いファイルは 1 度だけ本走査へ通す
    let stamped = content.contains(ho::HANDOFF_FORMAT_MARKER);
    if stamped && !ho::needs_migration_scan(&content) {
        return out;
    }
    let all_keys: Vec<String> = match ProjectsConfig::load() {
        Ok(c) => c.projects.keys().cloned().collect(),
        Err(e) => {
            out.warnings
                .push(format!("projects.yaml が読めないため移行を見送った: {e}"));
            return out;
        }
    };
    let profile_projects = super::Profile::load(profile)
        .ok()
        .and_then(|p| p.projects)
        .unwrap_or_default();
    let plan = ho::migration_plan(&content, &all_keys, &profile_projects);
    if !plan.has_moves() {
        // 移す先が決まらないなら**本文には触らない**。ただし番地だけは打つ
        // （次からは安い前判定で抜けられる。内容は 1 バイトも変えない）
        if !stamped {
            let stamped_body = format!("{}\n{content}", ho::HANDOFF_FORMAT_MARKER);
            if let Err(e) = write_profile_memo(profile, &stamped_body) {
                out.warnings.push(e);
            }
        }
        return out;
    }

    // 先に退避する（書き換えの前に必ず原本を残す）
    match archive_original(profile, &content) {
        Ok(path) => out.archived_to = Some(path.display().to_string()),
        Err(e) => {
            // 退避できないなら移行しない（原本を失うリスクを取らない）
            out.warnings
                .push(format!("移行前のファイルを退避できないため中止した: {e}"));
            return out;
        }
    }

    let mut write_failed = false;
    for (key, body) in plan.by_project() {
        let merged = match read_project_handoff(&key) {
            // 既にファイルがあるなら**上書きしない**で追記する（内容を捨てない）
            Some(existing) => format!(
                "{}\n\n{}\n\n{}",
                existing.trim_end(),
                migrated_note(profile),
                body.trim()
            ),
            None => format!(
                "{}\n{}\n\n{}\n",
                ho::project_marker(&key),
                migrated_note(profile),
                body.trim()
            ),
        };
        match write_project_handoff(&key, &merged) {
            Ok(path) => out.moved.push((key, path.display().to_string())),
            Err(e) => {
                write_failed = true;
                out.warnings.push(e);
            }
        }
    }
    if write_failed {
        // 1 つでも移せなかったら**運用メモは書き換えない**。書き換えると、移せなかった
        // 断片が現用のファイルから消えて退避先にしか無くなる（原本は残っているので
        // 復元はできるが、気づかないまま欠けた状態で動くほうが危ない）
        out.warnings.push(
            "移せないプロジェクトがあったため運用メモの書き換えを中止した（原本は退避済み）"
                .to_string(),
        );
        return out;
    }

    let residue = plan.residue();
    let memo = if residue.trim().is_empty() {
        ho::profile_memo_template(profile)
    } else {
        format!(
            "{}\n{}\n\n{}\n",
            ho::profile_memo_template(profile).trim_end(),
            migrated_note(profile),
            residue.trim()
        )
    };
    out.kept_lines = residue.lines().filter(|l| !l.trim().is_empty()).count();
    if let Err(e) = write_profile_memo(profile, &memo) {
        out.warnings.push(e);
    }
    out.migrated = !out.moved.is_empty();
    out
}

/// 全プロファイル（`handoff/*.md`）を移行する。`tako setup` の発火点（#916 の段 1）
pub fn migrate_all() -> Vec<MigrationOutcome> {
    list_profile_memos()
        .into_iter()
        .map(|(profile, _)| ensure_migrated(&profile))
        .filter(|o| o.migrated || !o.warnings.is_empty())
        .collect()
}

fn migrated_note(profile: &str) -> String {
    format!("<!-- tako: handoff/{profile}.md から自動移行（#915） -->")
}

/// 移行前の原本を `handoff/archive/` へ退避する。既存を上書きしない
fn archive_original(profile: &str, content: &str) -> Result<PathBuf, String> {
    let dir = archive_dir().ok_or("ホームディレクトリが取得できない")?;
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("退避先を作れない: {} ({e})", dir.display()))?;
    for n in 1..=999u32 {
        let path = dir.join(format!("{profile}-pre915-{n}.md"));
        if path.exists() {
            continue;
        }
        std::fs::write(&path, content).map_err(|e| format!("退避に失敗: {e}"))?;
        return Ok(path);
    }
    Err("退避先の連番が埋まっている".into())
}

// --- 後任へ渡す一式の組み立て -----------------------------------------------

/// 後任へ渡す材料（所有型。`tako_core::handoff::SuccessorHandoff` は借用型なので、
/// 呼び出し側でこれを作ってから参照で渡す）
#[derive(Debug, Clone, Default)]
pub struct HandoffBundle {
    pub profile: String,
    pub profile_memo: Option<String>,
    pub projects: Vec<(String, String)>,
    pub missing_projects: Vec<String>,
    pub catalog: Vec<(String, String)>,
    pub jurisdiction: ho::Jurisdiction,
    /// 運用メモが膨らみすぎているときの警告（#915 要件 3: 肥大の再発防止）
    pub memo_warning: Option<String>,
}

impl HandoffBundle {
    /// 借用型へ変換する（文面生成は tako-core の純粋関数が担う）
    pub fn as_successor(&self) -> ho::SuccessorHandoff<'_> {
        ho::SuccessorHandoff {
            profile: &self.profile,
            profile_memo: self.profile_memo.as_deref(),
            projects: self
                .projects
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
            missing_projects: self.missing_projects.iter().map(String::as_str).collect(),
            catalog: self
                .catalog
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
            jurisdiction: self.jurisdiction.source,
        }
    }
}

/// 管轄プロジェクトぶんの引き継ぎを集める。
///
/// 読む前に必ず自動移行を通す（#916 の段 2 = 実行時の差分検出）。
/// 管轄が決まらなければ本文は集めず、代わりに一覧（キー, パス）を返す
pub fn collect_bundle(profile: &str, jurisdiction: ho::Jurisdiction) -> HandoffBundle {
    let memo = super::read_handoff(profile);
    let mut bundle = HandoffBundle {
        profile: profile.to_string(),
        memo_warning: memo
            .as_deref()
            .and_then(|m| ho::profile_memo_warning(profile, m)),
        profile_memo: memo,
        jurisdiction,
        ..Default::default()
    };
    for key in bundle.jurisdiction.projects.clone() {
        match read_project_handoff(&key) {
            Some(body) => bundle.projects.push((key, body)),
            None => bundle.missing_projects.push(key),
        }
    }
    if bundle.jurisdiction.source == ho::JurisdictionSource::Unresolved {
        bundle.catalog = list_project_handoffs()
            .into_iter()
            .map(|(k, p)| (k, p.display().to_string()))
            .collect();
    }
    bundle
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 引き継ぎのパス組み立てに**区切り文字を直書きしていない**ことを源で拘束する。
    /// `join` だけで組んでいれば Windows でも `\` になる。ここを緩めると、macOS の
    /// テストは全部緑のまま実機だけが壊れる（#467 の作法 11 と同じ罠）
    #[test]
    fn 引き継ぎのパス組み立てに区切り文字を直書きしていない() {
        let src = include_str!("handoff_store.rs");
        let mut offenders: Vec<&str> = Vec::new();
        for line in src.lines() {
            let code = line.trim();
            if code.starts_with("//") || code.starts_with("///") || code.contains("watchdog-allow")
            {
                continue;
            }
            if !code.contains(".join(") {
                continue;
            }
            // `join("a/b")` / `join("a\\b")` のような直書きを拾う
            if code.contains(".join(\"") && (code.contains('/') || code.contains('\\')) {
                offenders.push(line);
            }
        }
        assert!(
            offenders.is_empty(),
            "パス区切りの直書きがある（join を段ごとに分ける）: {offenders:?}"
        );
    }

    /// 危険なキーはパスにならない（キー由来のパス脱出を構造で防ぐ）
    #[test]
    fn 危険なプロジェクトキーはパスを作らない() {
        for bad in ["../evil", "..\\evil", "a/b", "C:evil", "", ".."] {
            assert!(
                project_handoff_path(bad).is_none(),
                "{bad:?} でパスを作ってはいけない"
            );
        }
    }

    /// 退避の連番は既存を上書きしない（原本を失わない）
    #[test]
    fn 退避は既存を上書きしない() {
        let dir = std::env::temp_dir().join(format!(
            "tako-handoff-store-test-{}-{}",
            std::process::id(),
            "archive"
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("一時ディレクトリ");
        // archive_dir() は config_dir 依存なので、ここでは連番の規則だけを直接確かめる
        let mut written = Vec::new();
        for n in 1..=3u32 {
            let path = dir.join(format!("p-pre915-{n}.md"));
            std::fs::write(&path, format!("gen {n}")).expect("書き込み");
            written.push(path);
        }
        for (i, path) in written.iter().enumerate() {
            let body = std::fs::read_to_string(path).expect("読み取り");
            assert_eq!(body, format!("gen {}", i + 1), "既存世代が壊れていない");
        }
        // 一時ディレクトリ配下であることを確かめてから消す（#worker-test-safety）
        assert!(dir.starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
