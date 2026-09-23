//! 起動時ロードの棚卸しと自動修正（Issue #1139）
//!
//! `tako context-budget`（CLI）と MCP `tako_context_budget` の**両方がこの 1 本を通る**。
//! 予算そのものは `tako_core::context_budget` が正本で、ここは
//! 「その cwd で何が強制ロードされるか」を集めて突き合わせるだけ。
//!
//! GUI を必要としないローカル処理（`tako platform` / `tako setup models` と同じ作法）。

use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tako_core::context_budget as budget;
use tako_core::context_budget::ItemKind;

/// 棚卸しの 1 項目
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: ItemKind,
    /// 表示用のパス（ホームは `~` へ畳む）
    pub label: String,
    pub path: Option<PathBuf>,
    pub text: String,
    /// `@import` チェーンで読み込まれるか（= 毎ターン全文が載る）
    pub imported: bool,
    /// どのファイルから `@import` されたか
    pub imported_by: Option<String>,
    /// 内訳（system prompt のときだけ入る。#1154）。
    /// **生成と同じ 1 実装**（`Profile::build_prompt_pieces`）から採るので数え直さない
    pub pieces: Vec<(String, usize)>,
    /// 追記（`prompt_blocks.append`）の**常時部**と、区切りが既に在るか（#1477）。
    /// 超過したときに「区切りを何行ぶん上げれば収まるか」を実測から答えるために持つ
    pub append_always: Option<(String, bool)>,
}

/// ホームを `~` へ畳んだ表示用パス（個人情報を応答へ出さない。#927）。
///
/// **前方一致だけでは足りない**: claude のメモリは cwd を `/` → `-` へ潰した
/// スラグ（`-Users-<名前>-dev-tako`）をパスの**途中**に持つので、畳んだ後も
/// ユーザー名が残る。同じ潰し方をしたホームも併せて置き換える
fn display_path(p: &Path) -> String {
    let Some(home) = tako_core::paths::home_dir() else {
        return p.display().to_string();
    };
    // `~` 短縮の正本は tako_core::paths::shorten_home（#893）
    let shown = tako_core::paths::shorten_home(&p.to_string_lossy());
    let dashed = home.display().to_string().replace(['/', '\\'], "-");
    if dashed.len() > 1 {
        shown.replace(&dashed, "-~")
    } else {
        shown
    }
}

/// 行の中の `@path` を**すべて**取り出す（`@AGENTS.md` / `@.agent/progress.md` / `@~/x.md`）。
///
/// **行頭だけを見てはいけない**: 実際の `AGENTS.md` は
/// `- 現在の作業状況（毎ターン上書き）: @.agent/activeContext.md` のように
/// 箇条書きの途中に書く（そう書いても展開される）。行頭限定にすると
/// **毎ターン全文ロードされている当のファイルを 1 つも数えられない**（実際に踏んだ）。
///
/// バックティックで囲まれた参照は対象外 — それが「毎ターン読まず必要なときだけ Read する」印。
/// 誤検出（メールアドレス・`@mention`）は、`@` の手前が語の途中でないことと、
/// **実在するファイルに解決できること**（`resolve_import`）の 2 段で落とす
fn import_targets(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let b = line.as_bytes();
    let mut i = 0usize;
    let mut in_code = false;
    while i < b.len() {
        if b[i] == b'`' {
            in_code = !in_code;
            i += 1;
            continue;
        }
        if b[i] != b'@' || in_code {
            i += 1;
            continue;
        }
        // 語の途中の `@`（メールアドレス等）は対象外
        let prev_ok = i == 0 || {
            let c = line[..i].chars().next_back().unwrap_or(' ');
            !(c.is_alphanumeric() || c == '_' || c == '.' || c == '-')
        };
        if !prev_ok {
            i += 1;
            continue;
        }
        let rest = &line[i + 1..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '`' || c == '）' || c == ')' || c == '、')
            .unwrap_or(rest.len());
        let token = rest[..end].trim_end_matches([',', '.', '。', ':', '：']);
        if !token.is_empty() {
            out.push(token);
        }
        i += 1 + end;
    }
    out
}

fn resolve_import(base_dir: &Path, target: &str) -> Option<PathBuf> {
    let p = if let Some(rest) = target.strip_prefix("~/") {
        tako_core::paths::home_dir()?.join(rest)
    } else {
        base_dir.join(target)
    };
    p.is_file().then_some(p)
}

/// 中身から種別を見分ける。
/// **名前ではなく形で見る**（他のリポジトリでも通るように）: `## YYYY-MM-DD` の
/// エントリが 3 件以上並んでいれば作業ログ
fn classify(path: &Path, text: &str) -> ItemKind {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    // `AGENTS.md` / `CLAUDE.md` はどう辿り着いても規約本体。
    // tako の `CLAUDE.md` は `@AGENTS.md` の 1 行リダイレクトなので、
    // 名前で決めないと本体が「ただの取り込み」に落ちて 30 KB の予算が効かない
    if name == "agents.md" || name == "claude.md" {
        return ItemKind::AgentsGuide;
    }
    if budget::parse_log(text).entries.len() >= 3 {
        return ItemKind::ProgressLog;
    }
    if name.contains("activecontext") {
        ItemKind::ActiveContext
    } else {
        ItemKind::Imported
    }
}

/// `@import` チェーンを辿る（循環は visited で止める）
fn walk_imports(start: &Path, out: &mut Vec<Item>, visited: &mut BTreeSet<PathBuf>) {
    let Ok(text) = std::fs::read_to_string(start) else {
        return;
    };
    let base = start.parent().unwrap_or(Path::new(".")).to_path_buf();
    let from = display_path(start);
    for line in text.lines() {
        for target in import_targets(line) {
            let Some(p) = resolve_import(&base, target) else {
                continue;
            };
            let canon = tako_core::platform::path::canonicalize_or_self(&p);
            if !visited.insert(canon.clone()) {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            out.push(Item {
                kind: classify(&p, &body),
                label: display_path(&p),
                path: Some(p.clone()),
                text: body,
                imported: true,
                imported_by: Some(from.clone()),
                pieces: Vec::new(),
                append_always: None,
            });
            walk_imports(&p, out, visited);
        }
    }
}

/// MCP で公開するツールカタログの実体と、ツール 1 本ずつの大きさ（Issue #1539）。
///
/// **スナップショットファイルではなく実行時に組み立てたものを測る**
/// （`testdata/mcp_tools_full_snapshot.json` はカタログが変わっていないことの
/// 突き合わせ用であって、測る対象そのものではない）。形は `tools/list` が返すのと
/// 同じ最小化 JSON = クライアントへ実際に流れるバイト列。
///
/// 内訳は「どのツールを削れば効くか」を `proposals` へ出すために要る。
/// **カタログの組み立ては 1 回だけ**（本文と内訳で 2 回作らない）
pub fn mcp_catalog() -> (String, Vec<(String, usize)>) {
    let tools = crate::mcp::tools();
    let pieces = tools
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap_or("?").to_string(),
                t.to_string().len(),
            )
        })
        .collect();
    (Value::Array(tools).to_string(), pieces)
}

/// claude の永続メモリ（`<config>/projects/<slug>/memory/MEMORY.md`）
fn memory_path(cwd: &Path) -> Option<PathBuf> {
    let config = tako_core::paths::home_dir()?.join(".claude");
    let slug = cwd.display().to_string().replace(['/', '\\'], "-");
    let p = config.join("projects").join(slug).join("memory/MEMORY.md");
    p.is_file().then_some(p)
}

/// その cwd で AI が起動時に強制ロードするものを集める
pub fn inventory(cwd: &Path, profile: Option<&str>) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();

    // 1) グローバル指示ファイル（全プロジェクトの全ターンに載る）
    if let Some(home) = tako_core::paths::home_dir() {
        let g = home.join(".claude/CLAUDE.md");
        if let Ok(text) = std::fs::read_to_string(&g) {
            visited.insert(tako_core::platform::path::canonicalize_or_self(&g));
            items.push(Item {
                kind: ItemKind::GlobalGuide,
                label: display_path(&g),
                path: Some(g.clone()),
                text,
                imported: false,
                imported_by: None,
                pieces: Vec::new(),
                append_always: None,
            });
            walk_imports(&g, &mut items, &mut visited);
        }
    }

    // 2) リポジトリの規約（CLAUDE.md は AGENTS.md への 1 行リダイレクトのことが多い）
    for name in ["CLAUDE.md", "AGENTS.md"] {
        let p = cwd.join(name);
        let canon = tako_core::platform::path::canonicalize_or_self(&p);
        if !p.is_file() || !visited.insert(canon) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        items.push(Item {
            kind: ItemKind::AgentsGuide,
            label: display_path(&p),
            path: Some(p.clone()),
            text,
            imported: false,
            imported_by: None,
            pieces: Vec::new(),
            append_always: None,
        });
        walk_imports(&p, &mut items, &mut visited);
    }

    // 3) claude の永続メモリ
    if let Some(p) = memory_path(cwd) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            items.push(Item {
                kind: ItemKind::Memory,
                label: display_path(&p),
                path: Some(p.clone()),
                text,
                imported: false,
                imported_by: None,
                pieces: Vec::new(),
                append_always: None,
            });
        }
    }

    // 4) master / solo の system prompt（プロファイル別）。
    // #1154: 本文と一緒に**内訳**（どの block / 追記が何バイトか）も採る。
    // 組み立てと同じ 1 実装から採るので、超過したときに何を分ければいいかが即座に分かる
    let profile_name = profile.unwrap_or("default");
    for kind in [
        crate::orchestrator::ProfileKind::Master,
        crate::orchestrator::ProfileKind::Solo,
    ] {
        let Ok(p) = crate::orchestrator::load_profile_of(kind, profile_name) else {
            continue;
        };
        let pieces = match kind {
            crate::orchestrator::ProfileKind::Master => p.system_prompt_pieces(profile_name),
            crate::orchestrator::ProfileKind::Solo => p.solo_system_prompt_pieces(profile_name),
        };
        items.push(Item {
            kind: ItemKind::SystemPrompt,
            label: format!("{} system prompt（{profile_name}）", kind.as_str()),
            path: None,
            text: crate::orchestrator::join_prompt_pieces(&pieces),
            imported: false,
            imported_by: None,
            pieces: pieces
                .iter()
                .map(|piece| (piece.name.clone(), piece.bytes()))
                .collect(),
            // #1477: 区切りより手前（= 実際に prompt へ載っている追記）
            append_always: p
                .prompt_append_text()
                .map(|raw| {
                    let part = tako_core::prompt_append::split(&raw);
                    (part.always.to_string(), part.marked)
                })
                .filter(|(a, _)| !a.trim().is_empty()),
        });
    }

    // 5) MCP で公開するツールカタログ（#1539）。
    // cwd にもプロファイルにも依らない**全エージェント共通の固定費**で、
    // MCP を繋いだ瞬間に `tools/list` の応答として全文が載る。
    // 予算表に項目が無かったあいだ、この量は誰にも測られていなかった
    let (catalog, pieces) = mcp_catalog();
    items.push(Item {
        kind: ItemKind::McpCatalog,
        label: match tako_core::i18n::lang() {
            tako_core::i18n::Lang::Ja => format!("MCP ツールカタログ（{} 本）", pieces.len()),
            tako_core::i18n::Lang::En => format!("MCP tool catalog ({} tools)", pieces.len()),
        },
        path: None,
        text: catalog,
        imported: false,
        imported_by: None,
        pieces,
        append_always: None,
    });

    // 6) 引き継ぎの運用メモ
    if let Some(p) = crate::orchestrator::handoff_path(profile_name) {
        if let Ok(text) = std::fs::read_to_string(&p) {
            items.push(Item {
                kind: ItemKind::HandoffMemo,
                label: display_path(&p),
                path: Some(p.clone()),
                text,
                imported: false,
                imported_by: None,
                pieces: Vec::new(),
                append_always: None,
            });
        }
    }

    items
}

/// 作業ログの復活・重複（#1645）を、置き場のアーカイブと突き合わせて採る。
///
/// アーカイブの置き場は `archive_path_for` = **`fix` が書き込む先と同じ 1 実装**。
/// 作業ログ以外の項目には相手が無いので空を返す
fn progress_revivals(it: &Item) -> (String, Vec<budget::Revival>) {
    if it.kind != ItemKind::ProgressLog {
        return (String::new(), Vec::new());
    }
    let Some(path) = it.path.as_ref() else {
        return (String::new(), Vec::new());
    };
    let archive = archive_path_for(path);
    let label = display_path(&archive);
    let text = std::fs::read_to_string(&archive).unwrap_or_default();
    let hits = budget::revivals(&it.text, &text);
    (label, hits)
}

/// 復活・重複 1 件。**名指しは `Revival::locate` の 1 本**を通す
/// （番犬 `issue1645_progress_revival_watchdog` も同じものを使う）
fn revival_json(it: &Item, r: &budget::Revival, archive_label: &str) -> Value {
    let v = r.violation();
    json!({
        "metric": v.metric.as_str(),
        "kind": r.kind.as_str(),
        "actual": v.actual,
        "limit": v.limit,
        "fixable": v.fixable,
        "line": r.line,
        "heading": r.heading,
        "other": format!("{}:{}", r.other_path(&it.label, archive_label), r.other_line),
        "detail": r.locate(&it.label, archive_label),
        "note": v.note.text(),
        "note_ja": v.note.ja(),
        "note_en": v.note.en(),
    })
}

fn item_json(it: &Item, revived: &[budget::Revival], archive_label: &str) -> Value {
    let m = budget::measure(it.kind, &it.text);
    let vs = budget::violations(it.kind, &m);
    let mut o = json!({
        "kind": it.kind.as_str(),
        "kind_label": it.kind.label().text(),
        "path": it.label,
        "imported": it.imported,
        "bytes": m.bytes,
        "lines": m.lines,
        "est_tokens": m.est_tokens,
        "auto_fixable": it.kind.auto_fixable(),
    });
    if let Some(by) = &it.imported_by {
        o["imported_by"] = json!(by);
    }
    if !it.pieces.is_empty() {
        o["pieces"] = json!(pieces_json(&it.pieces));
    }
    if let Some(e) = m.entries {
        o["entries"] = json!(e);
        o["work_days"] = json!(m.work_days.unwrap_or(0));
        o["longest_entry_lines"] = json!(m.longest_entry_lines.unwrap_or(0));
        o["over_long_entries"] = json!(m.over_long_entries.unwrap_or(0));
    }
    let mut all: Vec<Value> = vs.iter().map(|v| violation_json(it, v)).collect();
    all.extend(revived.iter().map(|r| revival_json(it, r, archive_label)));
    if !all.is_empty() {
        o["violations"] = json!(all);
    }
    o
}

/// 内訳（大きい順。**何を分ければ効くか**が先頭に来る）。
/// 個人のホームパスは名前へ入れない（`piece_name` がファイル名だけを添える。#927）
fn pieces_json(pieces: &[(String, usize)]) -> Vec<Value> {
    let mut sorted: Vec<&(String, usize)> = pieces.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    sorted
        .iter()
        .map(|(name, bytes)| json!({ "name": name, "bytes": bytes }))
        .collect()
}

fn violation_json(it: &Item, v: &budget::Violation) -> Value {
    let mut o = json!({
        "metric": v.metric.as_str(),
        "actual": v.actual,
        "limit": v.limit,
        "fixable": v.fixable,
        // 理由文は表示言語に追従する。docs / 生成物が実行環境の言語で変わらないよう
        // 日英も併せて返す（#591 と同じ作法）
        "note": v.note.text(),
        "note_ja": v.note.ja(),
        "note_en": v.note.en(),
    });
    if let Some(a) = split_advice_for(it, v) {
        // #1477: 「大きい」だけでなく**何行動かせば収まるか**を実測から返す
        o["advice"] = json!(if tako_core::i18n::lang() == tako_core::i18n::Lang::Ja {
            &a.ja
        } else {
            &a.en
        });
        o["advice_ja"] = json!(a.ja);
        o["advice_en"] = json!(a.en);
        o["move_lines"] = json!(a.move_lines);
        o["marker"] = json!(tako_core::prompt_append::MARKER);
    }
    o
}

/// 分割しても常時部が予算を超えるときの具体値（#1477）。
///
/// 追記を持たない（= 超過が tako 自身の生成物だけで起きている）プロファイルでは
/// `None`。そちらは利用者が直せるものではないので、既定の理由文だけを返す
fn split_advice_for(
    it: &Item,
    v: &budget::Violation,
) -> Option<tako_core::prompt_append::SplitAdvice> {
    if it.kind != ItemKind::SystemPrompt || v.metric != budget::Metric::Bytes {
        return None;
    }
    let (always, marked) = it.append_always.as_ref()?;
    Some(tako_core::prompt_append::split_advice(
        v.actual.saturating_sub(v.limit),
        always,
        *marked,
    ))
}

/// 状態の棚卸し（何も書き換えない）
pub fn report(cwd: &Path, profile: Option<&str>) -> Result<Value, String> {
    let items = inventory(cwd, profile);
    let mut import_bytes = 0usize;
    let mut total_tokens = 0usize;
    let mut violations = 0usize;
    let mut fixable = 0usize;
    let mut proposals: Vec<Value> = Vec::new();

    let items_json: Vec<Value> = items
        .iter()
        .map(|it| {
            let m = budget::measure(it.kind, &it.text);
            total_tokens += m.est_tokens;
            // `@import` の合計。規約本体（AGENTS.md / CLAUDE.md）は
            // それ自身の予算で見るのでここには足さない
            if it.imported && it.kind != ItemKind::AgentsGuide {
                import_bytes += m.bytes;
            }
            for v in budget::violations(it.kind, &m) {
                violations += 1;
                if v.fixable {
                    fixable += 1;
                } else {
                    let mut prop = json!({
                        "path": it.label,
                        "metric": v.metric.as_str(),
                        "actual": v.actual,
                        "limit": v.limit,
                        "next_step": v.note.text(),
                        "next_step_ja": v.note.ja(),
                        "next_step_en": v.note.en(),
                    });
                    // #1154: system prompt はどの block が何バイトかまで出す
                    // （「大きい」だけ言われても何を分ければいいか分からない）
                    if !it.pieces.is_empty() {
                        prop["pieces"] = json!(pieces_json(&it.pieces));
                    }
                    // #1477: 追記があるなら「区切りを何行上げれば収まるか」まで出す
                    if let Some(a) = split_advice_for(it, &v) {
                        prop["advice"] = json!(a.ja);
                        prop["advice_ja"] = json!(a.ja);
                        prop["advice_en"] = json!(a.en);
                        prop["move_lines"] = json!(a.move_lines);
                    }
                    proposals.push(prop);
                }
            }
            // #1645: アーカイブへ移したはずのエントリが戻っていないか。
            // 予算を超えなくても違反（重複したまま main に載る）
            let (archive_label, revived) = progress_revivals(it);
            for r in &revived {
                violations += 1;
                let v = r.violation();
                proposals.push(json!({
                    "path": format!("{}:{}", it.label, r.line),
                    "metric": v.metric.as_str(),
                    "actual": v.actual,
                    "limit": v.limit,
                    "kind": r.kind.as_str(),
                    "detail": r.locate(&it.label, &archive_label),
                    "next_step": v.note.text(),
                    "next_step_ja": v.note.ja(),
                    "next_step_en": v.note.en(),
                }));
            }
            item_json(it, &revived, &archive_label)
        })
        .collect();

    if let Some(v) = budget::import_total_violation(import_bytes) {
        violations += 1;
        proposals.push(json!({
            "path": "@import",
            "metric": v.metric.as_str(),
            "actual": v.actual,
            "limit": v.limit,
            "next_step": v.note.text(),
            "next_step_ja": v.note.ja(),
            "next_step_en": v.note.en(),
        }));
    }

    Ok(json!({
        "cwd": display_path(cwd),
        "profile": profile.unwrap_or("default"),
        "items": items_json,
        "totals": {
            "est_tokens": total_tokens,
            "import_bytes": import_bytes,
            "import_bytes_limit": budget::IMPORT_TOTAL_MAX_BYTES,
        },
        "violations": violations,
        "fixable": fixable,
        "proposals": proposals,
        "budget": budget_json(),
        "fix_command": "tako context-budget fix",
        "ok": violations == 0,
    }))
}

/// 予算表そのもの（規約文との一致をテストで固定するため応答にも載せる）
pub fn budget_json() -> Value {
    json!({
        "progress_log": {
            "max_work_days": budget::PROGRESS_MAX_WORK_DAYS,
            "max_entries": budget::PROGRESS_MAX_ENTRIES,
            "max_entry_lines": budget::PROGRESS_MAX_ENTRY_LINES,
            "max_bytes": budget::PROGRESS_MAX_BYTES,
            "archive_retain_days": budget::ARCHIVE_RETAIN_DAYS,
        },
        "agents_guide": { "max_bytes": budget::AGENTS_GUIDE_MAX_BYTES },
        "import_total": { "max_bytes": budget::IMPORT_TOTAL_MAX_BYTES },
        "active_context": { "max_lines": budget::ACTIVE_CONTEXT_MAX_LINES },
        "handoff_memo": { "max_lines": budget::HANDOFF_MEMO_MAX_LINES },
        "global_guide": { "max_bytes": budget::GLOBAL_GUIDE_MAX_BYTES },
        // #1539: MCP を繋いだエージェント全員が起動時に受け取る最大の固定費
        "mcp_catalog": { "max_bytes": budget::MCP_CATALOG_MAX_BYTES },
        "system_prompt": {
            "max_bytes": budget::SYSTEM_PROMPT_MAX_BYTES,
            // #1477: tako 自身の取り分と、利用者の追記へ明け渡した残り
            "base_max_bytes": budget::SYSTEM_PROMPT_BASE_MAX_BYTES,
            "append_max_bytes": budget::PROMPT_APPEND_MAX_BYTES,
            "append_marker": tako_core::prompt_append::MARKER,
        },
    })
}

/// アーカイブの置き場（`progress.md` → `progress-archive.md`）
pub fn archive_path_for(log: &Path) -> PathBuf {
    let stem = log
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("progress");
    log.with_file_name(format!("{stem}-archive.md"))
}

/// 「今日」の 1970-01-01 からの日数
fn today_days() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 / 86_400)
        .unwrap_or(0)
}

/// 自動で直せるものだけ直す（= 作業ログの移送）。
///
/// **総エントリ数が保たれないときは書き込まない**（移送で失われるものが無いことを
/// 実装ではなくデータで確かめてから書く）
pub fn fix(cwd: &Path, profile: Option<&str>, dry_run: bool) -> Result<Value, String> {
    let items = inventory(cwd, profile);
    let today = today_days();
    let mut changes: Vec<Value> = Vec::new();
    let mut changed = 0usize;

    for it in items.iter().filter(|i| i.kind == ItemKind::ProgressLog) {
        let Some(path) = it.path.clone() else {
            continue;
        };
        let archive = archive_path_for(&path);
        let archive_text = std::fs::read_to_string(&archive).unwrap_or_default();
        let plan = budget::plan_prune(&it.text, &archive_text, today);

        if !plan.entries_preserved() {
            return Err(format!(
                "{}: 移送でエントリ数が合わない（前 {} / 残 {} + 移送 {}）ので書き込まない",
                it.label,
                plan.entries_before,
                plan.kept.len(),
                plan.archived.len()
            ));
        }
        if plan.is_noop() {
            changes.push(json!({
                "path": it.label,
                "changed": false,
                "entries_before": plan.entries_before,
                "entries_kept": plan.kept.len(),
            }));
            continue;
        }

        changed += 1;
        let mut c = json!({
            "path": it.label,
            "archive_path": display_path(&archive),
            "changed": true,
            "entries_before": plan.entries_before,
            "entries_kept": plan.kept.len(),
            "entries_archived": plan.archived.len(),
            "entries_preserved": plan.entries_before == plan.kept.len() + plan.archived.len(),
            "archive_lines_removed_by_age": plan.aged_out.len(),
            "bytes_before": it.text.len(),
            "bytes_after": plan.progress_text.len(),
            "est_tokens_before": budget::estimate_tokens(&it.text),
            "est_tokens_after": budget::estimate_tokens(&plan.progress_text),
        });
        if dry_run {
            // 何を移すのかを 1 行ずつ見せる（適用前に読める形）
            c["archived_preview"] = json!(plan
                .archived
                .iter()
                .map(|e| e.archive_line())
                .collect::<Vec<_>>());
            if !plan.aged_out.is_empty() {
                c["aged_out_preview"] = json!(plan.aged_out);
            }
        } else {
            std::fs::write(&archive, &plan.archive_text)
                .map_err(|e| format!("{}: {e}", display_path(&archive)))?;
            std::fs::write(&path, &plan.progress_text).map_err(|e| format!("{}: {e}", it.label))?;
        }
        changes.push(c);
    }

    // 直せなかったぶんは提案として返す（何をどこへ分けるかは人間の判断）
    let after = report(cwd, profile)?;
    Ok(json!({
        "dry_run": dry_run,
        "changed": changed,
        "changes": changes,
        "proposals": after["proposals"],
        "violations_after": after["violations"],
        "ok": after["violations"].as_u64().unwrap_or(0) == 0,
    }))
}

/// `tako setup` / `tako master` が出す 1 行（#322 の最簡形）。
/// 超過が無ければ `None`（黙って素通りする）。
///
/// #1154: **自動で直せるものが 1 件も無いときに `fix` を案内しない**
/// （`fix` は作業ログの移送しかできないので、system prompt が超えているときに
/// 「いま直す: fix（自動で直せる 0 件）」を出すのは嘘の案内になる）。
/// その場合は代わりに**何が超えているか**を 1 行で名指しする
pub fn startup_line(cwd: &Path, profile: Option<&str>) -> Option<String> {
    let r = report(cwd, profile).ok()?;
    let violations = r["violations"].as_u64().unwrap_or(0);
    if violations == 0 {
        return None;
    }
    let tokens = r["totals"]["est_tokens"].as_u64().unwrap_or(0);
    let fixable = r["fixable"].as_u64().unwrap_or(0);
    if fixable > 0 {
        return Some(match tako_core::i18n::lang() {
            tako_core::i18n::Lang::Ja => format!(
                "起動時ロードが予算超過: {violations} 件（概算 {tokens} トークン）。\
                 いま直す: tako context-budget fix（自動で直せる {fixable} 件）",
            ),
            tako_core::i18n::Lang::En => format!(
                "Startup load is over budget: {violations} item(s) (~{tokens} tokens). \
                 Fix now: tako context-budget fix ({fixable} auto-fixable)",
            ),
        });
    }
    let worst = worst_proposal(&r);
    Some(match tako_core::i18n::lang() {
        tako_core::i18n::Lang::Ja => format!(
            "起動時ロードが予算超過: {violations} 件（概算 {tokens} トークン）。{worst}\
             内訳: tako context-budget",
        ),
        tako_core::i18n::Lang::En => format!(
            "Startup load is over budget: {violations} item(s) (~{tokens} tokens). {worst}\
             Breakdown: tako context-budget",
        ),
    })
}

/// 1 行で名指しする対象。**master 自身の system prompt を優先する**
/// （`tako master` はこれからその prompt を渡すところで、master 自身は
/// 自分の prompt の大きさを他の手段では知れない）。無ければ超過分が最大のもの
fn worst_proposal(report: &Value) -> String {
    let empty = vec![];
    let proposals = report["proposals"].as_array().unwrap_or(&empty);
    let over = |p: &Value| {
        p["actual"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(p["limit"].as_u64().unwrap_or(0))
    };
    let prompt_paths: Vec<&str> = report["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|it| it["kind"] == ItemKind::SystemPrompt.as_str())
        .filter_map(|it| it["path"].as_str())
        .collect();
    let pick = proposals
        .iter()
        .filter(|p| prompt_paths.contains(&p["path"].as_str().unwrap_or_default()))
        .max_by_key(|p| over(p))
        .or_else(|| proposals.iter().max_by_key(|p| over(p)));
    match pick {
        Some(p) => format!(
            "{}: {} {} > {}。",
            p["path"].as_str().unwrap_or("?"),
            p["metric"].as_str().unwrap_or("?"),
            p["actual"].as_u64().unwrap_or(0),
            p["limit"].as_u64().unwrap_or(0),
        ),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 箇条書きの途中のimportも取り出す() {
        assert_eq!(import_targets("@AGENTS.md"), vec!["AGENTS.md"]);
        // 実際の AGENTS.md はこの形で書く。行頭限定だと 1 つも数えられない
        assert_eq!(
            import_targets("- 現在の作業状況（毎ターン上書き）: @.agent/activeContext.md"),
            vec![".agent/activeContext.md"]
        );
        assert_eq!(import_targets("@a.md と @b.md"), vec!["a.md", "b.md"]);
    }

    #[test]
    fn バックティック参照とメールアドレスは取り込みでない() {
        // バックティックで囲むのが「毎ターン読まない」印
        assert!(import_targets("詳細は `@.agent/x.md` にある").is_empty());
        assert!(import_targets("`x` @`.agent/x.md`").is_empty());
        assert!(import_targets("user@example.com").is_empty());
        assert!(import_targets("@").is_empty());
    }

    #[test]
    fn 句読点や閉じ括弧は取り込み先に含めない() {
        assert_eq!(import_targets("（@.agent/x.md）"), vec![".agent/x.md"]);
        assert_eq!(import_targets("@x.md、それと"), vec!["x.md"]);
        assert_eq!(import_targets("@x.md。"), vec!["x.md"]);
    }

    #[test]
    fn 規約本体は辿り着き方によらず規約として数える() {
        // tako の CLAUDE.md は `@AGENTS.md` の 1 行リダイレクト。
        // 取り込み経由で見つけても 30 KB の予算が効かないといけない
        assert_eq!(
            classify(Path::new("/r/AGENTS.md"), "# x\n"),
            ItemKind::AgentsGuide
        );
        assert_eq!(
            classify(Path::new("/r/CLAUDE.md"), "@AGENTS.md\n"),
            ItemKind::AgentsGuide
        );
    }

    #[test]
    fn 種別は名前ではなく形で見分ける() {
        let log = "## 2026-09-01（a）\n- x\n\n## 2026-09-02（b）\n- y\n\n## 2026-09-03（c）\n- z\n";
        assert_eq!(
            classify(Path::new("/tmp/なんでもよい.md"), log),
            ItemKind::ProgressLog
        );
        assert_eq!(
            classify(Path::new("/tmp/activeContext.md"), "# 現在状態\n"),
            ItemKind::ActiveContext
        );
        assert_eq!(
            classify(Path::new("/tmp/other.md"), "# x\n"),
            ItemKind::Imported
        );
    }

    #[test]
    fn 表示用パスはスラグの中のホームも畳む() {
        // claude のメモリは cwd を `/` → `-` へ潰したスラグをパスの途中に持つので、
        // 前方一致で `~` にするだけではユーザー名が残る（実測で踏んだ）
        let Some(home) = tako_core::paths::home_dir() else {
            return;
        };
        let dashed = home.display().to_string().replace(['/', '\\'], "-");
        let p = home.join(format!(
            ".claude/projects/{dashed}-dev-tako/memory/MEMORY.md"
        ));
        let shown = display_path(&p);
        let user = home
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        assert!(!user.is_empty());
        assert!(
            !shown.contains(&user),
            "表示用パスにユーザー名が残っている: {shown}"
        );
        // 区切りは**元の文字をそのまま残す**のが `shorten_home` の契約なので、
        // Windows では `~\.claude/projects/…` になる（#1278）。
        // 見たいのは「`~` 起点でスラグの位置まで畳めている」ことなので区切りを正規化する
        let normalized = shown.replace('\\', "/");
        assert!(
            normalized.starts_with("~/.claude/projects/"),
            "`~` 起点に畳めていない: {shown}"
        );
    }

    /// #1154: `fix` で直せないものだけが超えているときに `fix` を案内しない
    /// （`tako master` の起動前 1 行が「自動で直せる 0 件」を勧める嘘になっていた）
    #[test]
    fn 自動で直せないときは何が超えているかを名指しする() {
        let report = json!({
            "items": [
                { "kind": "system_prompt", "path": "master system prompt（p）" },
                { "kind": "global_guide", "path": "~/.claude/CLAUDE.md" },
            ],
            "proposals": [
                // 超過分はこちらの方が大きいが、master 自身の prompt を優先する
                { "path": "~/.claude/CLAUDE.md", "metric": "bytes", "actual": 90000, "limit": 24576 },
                { "path": "master system prompt（p）", "metric": "bytes", "actual": 30000, "limit": 24576 },
            ],
        });
        let line = worst_proposal(&report);
        assert!(
            line.starts_with("master system prompt（p）: bytes 30000 > 24576"),
            "master 自身の prompt を優先して名指しする: {line}"
        );

        // system prompt が超えていなければ超過分が最大のものを名指しする
        let report = json!({
            "items": [{ "kind": "global_guide", "path": "~/.claude/CLAUDE.md" }],
            "proposals": [
                { "path": "a.md", "metric": "lines", "actual": 100, "limit": 80 },
                { "path": "~/.claude/CLAUDE.md", "metric": "bytes", "actual": 90000, "limit": 24576 },
            ],
        });
        assert!(worst_proposal(&report).starts_with("~/.claude/CLAUDE.md: bytes 90000 > 24576"));

        // 提案が無ければ黙る（1 行に余計な句読点を出さない）
        assert_eq!(worst_proposal(&json!({})), "");
    }

    /// #1539: MCP カタログは cwd にもプロファイルにも依らず必ず 1 件載る。
    /// **CLI（`tako context-budget`）と MCP（`tako_context_budget`）はどちらも
    /// `report` → `inventory` を通る 1 実装**なので、ここが両方の申告を決める
    #[test]
    fn mcpカタログはcwdに依らず棚卸しへ必ず載る() {
        // 何も置いていない一時ディレクトリ（規約も作業ログも無い場所）
        let cwd = std::env::temp_dir();
        let items = inventory(&cwd, None);
        let found: Vec<&Item> = items
            .iter()
            .filter(|i| i.kind == ItemKind::McpCatalog)
            .collect();
        assert_eq!(found.len(), 1, "MCP カタログは 1 件だけ載る");
        let it = found[0];
        assert!(it.path.is_none(), "ファイルではないのでパスは持たない");
        assert!(!it.imported, "`@import` チェーンではない");
        assert!(
            it.text.starts_with('[') && it.text.len() > 100_000,
            "実行時に組み立てたカタログ本体が入る（{} bytes）",
            it.text.len()
        );
        assert_eq!(
            it.pieces.len(),
            crate::mcp::tools().len(),
            "内訳はツール本数ぶん"
        );
        // JSON にも同じ種別で出る（表示と `--json` は同じ 1 件を見る）
        let o = item_json(it, &[], "");
        assert_eq!(o["kind"], "mcp_catalog");
        assert_eq!(o["bytes"].as_u64().unwrap() as usize, it.text.len());
        assert_eq!(o["lines"], 0, "1 本の JSON なので行数は測らない");
        assert_eq!(o["auto_fixable"], false);
    }

    #[test]
    fn 予算表jsonにmcpカタログの上限が載る() {
        assert_eq!(
            budget_json()["mcp_catalog"]["max_bytes"].as_u64().unwrap() as usize,
            budget::MCP_CATALOG_MAX_BYTES
        );
    }

    #[test]
    fn カタログの実体は最小化jsonで内訳の合計とほぼ一致する() {
        let (text, pieces) = mcp_catalog();
        assert!(
            !text.contains('\n'),
            "`tools/list` と同じ最小化 JSON（改行なし）"
        );
        // `[` `]` と区切りの `,` のぶんだけ本文が大きい
        let sum: usize = pieces.iter().map(|(_, b)| b).sum();
        assert_eq!(text.len(), sum + 2 + pieces.len().saturating_sub(1));
        assert!(pieces.iter().all(|(n, b)| n.starts_with("tako_") && *b > 0));
    }

    #[test]
    fn アーカイブの置き場は隣に作る() {
        assert_eq!(
            archive_path_for(Path::new("/a/b/progress.md")),
            PathBuf::from("/a/b/progress-archive.md")
        );
    }
}
