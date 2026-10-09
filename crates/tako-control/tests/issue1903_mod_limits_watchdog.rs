//! tako mod S2 の続き（#1903。FR-2.42.11 / FR-2.42.18）の番犬: 使用制限の観測時刻・束ね方・
//! ステータスバー・MCP の説明文・検査スクリプトの 1 実装の構造
//!
//! # なぜ要るか
//!
//! ここで縛るものは、どれも**逆戻りしても単体テストも見た目も壊れない**:
//!
//! - ステータスバーが mod を見なくなっても、画面の `5h NN%` がそれらしい値で埋める
//! - mod が heartbeat のたびに `observed_at` を打ち直す形へ戻っても、報告は届き続ける。
//!   壊れるのは「放置したペインの古い % が最新の観測に見える」ことだけ
//! - 束ね方を「`resets_at` が遅い → % が大きい」へ戻しても、% が増えるだけの普段の窓では同じ値になる
//! - 実経路テスト（`scripts/test-claude-mod-1879.sh`）が validate / test を自前で叩く形へ戻ると、
//!   夜間の検査（`scripts/check-claude-mod.sh`）と合否の基準が黙ってずれる
//!
//! # 何を縛るか
//!
//! 1. GUI の `refresh_agent_metrics` が `claude_mod::status_bar_limits` を通して 5h / 7d を上書きする
//! 2. `status_bar_limits` は `account_rate_limits`（1 実装）と `bar_limits`（判断）を通り、A/B を見る
//! 3. 束ね方（`newer_limit`）の既定の順は `observed_at` が先頭（最新の観測）
//! 4. A/B の env（`TAKO_1903_LEGACY`）の名前は tako-core の定数 1 か所だけ
//! 5. mod（`register.ts`）は `stampLimits` を通して `observed_at` を打ち、今の時刻を直に打たない。
//!    窓ごとの時刻は `$.state` に置く（ホットリロードをまたぐ）
//! 6. `check-claude-mod.sh` は文言一致に肯定形の自己検査を持つ・1879 の実経路テストはそれを呼ぶだけ
//! 7. MCP の説明文が S2 の語彙（`ctx_source = mod` / `ctx_mod_reason` / `mod_turn` / `rate_limits` /
//!    `status_source = mod`）の読み方を載せている
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const MAIN: &str = "crates/tako-app/src/main.rs";
const CONTROL_MOD: &str = "crates/tako-control/src/claude_mod.rs";
const CORE_MOD: &str = "crates/tako-core/src/claude_mod.rs";
const REGISTER: &str = "crates/tako-core/claude-mod/hooks/register.ts";
const CHECK: &str = "scripts/check-claude-mod.sh";
const T1879: &str = "scripts/test-claude-mod-1879.sh";
/// MCP カタログは実物（`mcp::tools()`）から読む。名指しはその定義の置き場
const CATALOG: &str = "crates/tako-control/src/mcp/catalog.rs";
const LEGACY_ENV: &str = "\"TAKO_1903_LEGACY\"";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line}: {why}")
}

/// `needle` を含む行から、`close` と一致する最初の行までの窓（1 始まりの行番号つき）
fn window(src: &str, needle: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let end = lines[start..]
        .iter()
        .position(|l| *l == close)
        .map(|i| start + i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// `needle` を含む最初の行の行番号（1 始まり。無ければ 0）
fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .map_or(0, |i| i + 1)
}

/// 行がコメントか（Rust / TypeScript の `//`・シェルの `#`）
fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('#')
}

struct Sources {
    files: Vec<(&'static str, String)>,
    /// MCP のツール名 → 説明文
    catalog: Vec<(String, String)>,
}

impl Sources {
    fn get(&self, file: &str) -> &str {
        self.files
            .iter()
            .find(|(f, _)| *f == file)
            .map(|(_, s)| s.as_str())
            .unwrap_or_else(|| panic!("{file} を読んでいない"))
    }

    fn get_mut(&mut self, file: &str) -> &mut String {
        &mut self
            .files
            .iter_mut()
            .find(|(f, _)| *f == file)
            .unwrap_or_else(|| panic!("{file} を読んでいない"))
            .1
    }

    fn description(&self, tool: &str) -> &str {
        self.catalog
            .iter()
            .find(|(name, _)| name == tool)
            .map(|(_, d)| d.as_str())
            .unwrap_or("")
    }

    fn description_mut(&mut self, tool: &str) -> &mut String {
        &mut self
            .catalog
            .iter_mut()
            .find(|(name, _)| name == tool)
            .unwrap_or_else(|| panic!("{tool} がカタログに無い"))
            .1
    }
}

fn current() -> Sources {
    let files = [MAIN, CONTROL_MOD, CORE_MOD, REGISTER, CHECK, T1879]
        .into_iter()
        .map(|f| (f, read(f)))
        .collect();
    let catalog = tako_control::mcp::tools()
        .into_iter()
        .filter_map(|t| {
            Some((
                t["name"].as_str()?.to_string(),
                t["description"].as_str()?.to_string(),
            ))
        })
        .collect();
    Sources { files, catalog }
}

/// 1. GUI のステータスバー
fn scan_status_bar(s: &Sources) -> Vec<String> {
    let Some((line, body)) = window(s.get(MAIN), "    fn refresh_agent_metrics(", "    }") else {
        return vec![report(MAIN, 0, "refresh_agent_metrics が見つからない")];
    };
    let mut out = Vec::new();
    if !body.contains("tako_control::claude_mod::status_bar_limits(") {
        out.push(report(
            MAIN,
            line,
            "ステータスバーの 5h / 7d が mod を見ていない（`claude_mod::status_bar_limits(` を通さない = 画面の値だけ）",
        ));
    }
    for assign in [
        "self.agent_metrics.limit_5h = bar.five_hour;",
        "self.agent_metrics.limit_week = bar.seven_day;",
    ] {
        if !body.contains(assign) {
            out.push(report(
                MAIN,
                line,
                &format!("ステータスバーへ mod の値を書いていない（`{assign}` が無い）"),
            ));
        }
    }
    out
}

/// 2. ペインの選び方と判断の 1 実装
fn scan_control(s: &Sources) -> Vec<String> {
    let Some((line, body)) = window(s.get(CONTROL_MOD), "pub fn status_bar_limits(", "}") else {
        return vec![report(CONTROL_MOD, 0, "status_bar_limits が見つからない")];
    };
    let mut out = Vec::new();
    for (needle, why) in [
        (
            "account_rate_limits(host,",
            "束ねた使用制限を `account_rate_limits` の 1 実装から引いていない（self / worker_status / 帯と束ね方がずれる）",
        ),
        (
            "core::bar_limits(",
            "5h / 7d の判断を `tako_core::claude_mod::bar_limits` の 1 本に通していない",
        ),
        (
            "core::limits_legacy()",
            "A/B（TAKO_1903_LEGACY）を見ていない（同一バイナリで #1903 前へ戻せない）",
        ),
    ] {
        if !body.contains(needle) {
            out.push(report(CONTROL_MOD, line, why));
        }
    }
    out
}

/// 3 / 4. 束ね方と A/B の env
fn scan_core(s: &Sources) -> Vec<String> {
    let src = s.get(CORE_MOD);
    let mut out = Vec::new();
    match window(src, "fn newer_limit(", "}") {
        None => out.push(report(CORE_MOD, 0, "newer_limit が見つからない")),
        Some((line, body)) => {
            // 既定のアーム（`} else {` の後ろ）の最初の比較が observed_at
            let default_arm = body.split("} else {").nth(1).unwrap_or("");
            let observed = default_arm.find("a.observed_at");
            let resets = default_arm.find("resets(a)");
            let ok = matches!((observed, resets), (Some(o), Some(r)) if o < r);
            if !ok {
                out.push(report(
                    CORE_MOD,
                    line,
                    "使用制限の束ね方の既定が最新の観測（`observed_at` が先頭）になっていない \
                     = 放置したペインの古い % が勝つ（#1903。設計書 §6）",
                ));
            }
        }
    }
    let hits: Vec<usize> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(LEGACY_ENV))
        .map(|(i, _)| i + 1)
        .collect();
    if hits.len() != 1 {
        out.push(report(
            CORE_MOD,
            hits.first().copied().unwrap_or(0),
            &format!(
                "A/B の env（TAKO_1903_LEGACY）の名前が {} か所に書かれている（定数 1 か所だけにする）",
                hits.len()
            ),
        ));
    }
    for (file, body) in &s.files {
        if *file == CORE_MOD {
            continue;
        }
        for (i, l) in body.lines().enumerate() {
            if !is_comment(l) && l.contains(LEGACY_ENV) {
                out.push(report(
                    file,
                    i + 1,
                    "A/B の env（TAKO_1903_LEGACY）を文字列で読んでいる（`claude_mod::limits_legacy()` を通す）",
                ));
            }
        }
    }
    out
}

/// 5. mod の観測時刻
fn scan_register(s: &Sources) -> Vec<String> {
    let src = s.get(REGISTER);
    let mut out = Vec::new();
    match window(src, "async function buildReport(", "}") {
        None => out.push(report(REGISTER, 0, "buildReport が見つからない")),
        Some((line, body)) => {
            if !body.contains("rate_limits: await stampLimits(") {
                out.push(report(
                    REGISTER,
                    line,
                    "報告の rate_limits が stampLimits を通っていない（observed_at を値の変化時だけ打つ 1 実装）",
                ));
            }
        }
    }
    match window(src, "async function stampLimits(", "}") {
        None => out.push(report(REGISTER, 0, "stampLimits が見つからない")),
        Some((line, body)) => {
            if !body.contains("$.state.get(LIMITS_SEEN)")
                || !body.contains("$.state.set(LIMITS_SEEN")
            {
                out.push(report(
                    REGISTER,
                    line,
                    "窓ごとの観測時刻を $.state に置いていない（モジュール変数だとホットリロードのたびに全窓が「今」の観測になる）",
                ));
            }
            if !body.contains("same ? prev.observed_at : at") {
                out.push(report(
                    REGISTER,
                    line,
                    "値が同じ窓に前の観測時刻を使っていない（heartbeat で observed_at を打ち直す = #1903 前）",
                ));
            }
        }
    }
    for (i, l) in src.lines().enumerate() {
        if !is_comment(l) && l.contains("observed_at: at") {
            out.push(report(
                REGISTER,
                i + 1,
                "observed_at に今の時刻を直に打っている（値の変化時だけ打つのは stampLimits の 1 本）",
            ));
        }
    }
    out
}

/// 6. 検査スクリプトの 1 実装と自己検査
fn scan_scripts(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    let check = s.get(CHECK);
    for (needle, why) in [
        (
            "grep -q 'gating hook with .catch: '",
            "validate の注記の肯定形（gating hook with .catch:）を要求していない = 文言が変わると .catch 抜けの検出が黙って外れる",
        ),
        (
            "grep -Eo '^Ran [0-9]+ test'",
            "test の要約行（Ran N tests）を読んでいない = 文言が変わると 0 本の検出が黙って外れる",
        ),
    ] {
        if !check.contains(needle) {
            out.push(report(
                CHECK,
                line_of(check, "validate --strict"),
                why,
            ));
        }
    }
    let t = s.get(T1879);
    match window(t, "phase_static() {", "}") {
        None => out.push(report(T1879, 0, "phase_static が見つからない")),
        Some((line, body)) => {
            if !body.contains("\"$REPO_ROOT/scripts/check-claude-mod.sh\"") {
                out.push(report(
                    T1879,
                    line,
                    "段 0 が scripts/check-claude-mod.sh を呼んでいない（夜間の検査と合否の基準がずれる）",
                ));
            }
        }
    }
    for (i, l) in t.lines().enumerate() {
        if !is_comment(l) && (l.contains("plugin validate") || l.contains("plugin test")) {
            out.push(report(
                T1879,
                i + 1,
                "validate / test を自前で叩いている（検査の本体は scripts/check-claude-mod.sh の 1 実装）",
            ));
        }
    }
    out
}

/// 7. MCP の説明文
fn scan_catalog(s: &Sources) -> Vec<String> {
    let src = read(CATALOG);
    let mut out = Vec::new();
    for (tool, words) in [
        (
            "tako_orchestrator_self",
            &[
                "ctx_source = mod",
                "ctx_mod_reason",
                "mod_turn",
                "rate_limits",
            ][..],
        ),
        (
            "tako_orchestrator_worker_status",
            &[
                "status_source = mod",
                "ctx_mod_reason",
                "mod_turn",
                "rate_limits",
            ][..],
        ),
    ] {
        let desc = s.description(tool);
        let missing: Vec<&str> = words
            .iter()
            .copied()
            .filter(|w| !desc.contains(w))
            .collect();
        if !missing.is_empty() {
            out.push(report(
                CATALOG,
                line_of(&src, &format!("\"name\": \"{tool}\"")),
                &format!(
                    "{tool} の説明文に mod の語彙の読み方が無い（{}）",
                    missing.join(" / ")
                ),
            ));
        }
    }
    out
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_status_bar(s);
    out.extend(scan_control(s));
    out.extend(scan_core(s));
    out.extend(scan_register(s));
    out.extend(scan_scripts(s));
    out.extend(scan_catalog(s));
    out
}

#[test]
fn 使用制限の観測時刻とステータスバーの構造が契約どおり() {
    let found = all(&current());
    assert!(found.is_empty(), "\n{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    for (file, open, close) in [
        (MAIN, "    fn refresh_agent_metrics(", "    }"),
        (CONTROL_MOD, "pub fn status_bar_limits(", "}"),
        (CORE_MOD, "fn newer_limit(", "}"),
        (REGISTER, "async function buildReport(", "}"),
        (REGISTER, "async function stampLimits(", "}"),
        (T1879, "phase_static() {", "}"),
    ] {
        let (_, body) = window(s.get(file), open, close)
            .unwrap_or_else(|| panic!("{file} の窓が採れない（{open}）"));
        assert!(body.lines().count() >= 5, "{file} の窓が短すぎる（{open}）");
    }
    assert!(s.catalog.len() > 100, "MCP カタログが読めていない");
}

fn assert_named(found: &[String], file: &str, why: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(file) && f.contains(why)),
        "注入を名指しできない（{file} / {why}）:\n{}",
        found.join("\n")
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let base = current();
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    let replace = |s: &mut Sources, file: &str, from: &str, to: &str| {
        let body = s.get_mut(file);
        assert!(body.contains(from), "{file} に注入の元（{from}）が無い");
        *body = body.replacen(from, to, 1);
    };
    // 1. ステータスバーを画面だけへ戻す
    let found = mutate(&|s| {
        replace(
            s,
            MAIN,
            "tako_control::claude_mod::status_bar_limits(",
            "screen_only_limits(",
        )
    });
    assert_named(&found, MAIN, "ステータスバーの 5h / 7d が mod を見ていない");
    // 2. 束ねた値を経由せず hub を直に引く・A/B を見ない
    let found = mutate(&|s| {
        replace(
            s,
            CONTROL_MOD,
            ".map(|pane| account_rate_limits(host, *pane, now))",
            ".map(|pane| own_limits(*pane))",
        )
    });
    assert_named(&found, CONTROL_MOD, "account_rate_limits");
    let found = mutate(&|s| replace(s, CONTROL_MOD, "if core::limits_legacy() {", "if false {"));
    assert_named(&found, CONTROL_MOD, "TAKO_1903_LEGACY");
    // 3. 束ね方を S2 の順（resets_at が先）へ戻す
    let found = mutate(&|s| {
        replace(
            s,
            CORE_MOD,
            "        a.observed_at\n            .cmp(&b.observed_at)\n            .then(resets(a).cmp(&resets(b)))",
            "        resets(a)\n            .cmp(&resets(b))\n            .then(a.observed_at.cmp(&b.observed_at))",
        )
    });
    assert_named(&found, CORE_MOD, "最新の観測");
    // 4. A/B の env を別の場所で読む
    let found = mutate(&|s| {
        s.get_mut(MAIN).push_str(
            "\nfn legacy() -> bool { std::env::var_os(\"TAKO_1903_LEGACY\").is_some() }\n",
        )
    });
    assert_named(&found, MAIN, "TAKO_1903_LEGACY");
    // 5. mod が heartbeat のたびに今の時刻を打つ形へ戻す（#1903 前の rateLimit(limit, at)）
    let found = mutate(&|s| {
        replace(
            s,
            REGISTER,
            "rate_limits: await stampLimits($, usage.rateLimits, at),",
            "rate_limits: usage.rateLimits.map(limit => ({ kind: limit.kind, percent_used: limit.percentUsed, resets_at: limit.resetsAt, observed_at: at })),",
        )
    });
    assert_named(&found, REGISTER, "stampLimits を通っていない");
    assert_named(&found, REGISTER, "今の時刻を直に打っている");
    let found = mutate(&|s| {
        replace(
            s,
            REGISTER,
            "const observed = same ? prev.observed_at : at",
            "const observed = at",
        )
    });
    assert_named(&found, REGISTER, "前の観測時刻を使っていない");
    let found = mutate(&|s| {
        replace(
            s,
            REGISTER,
            "seen = (await $.state.get(LIMITS_SEEN)).value ?? {}",
            "seen = seenLimits",
        )
    });
    assert_named(&found, REGISTER, "$.state に置いていない");
    // 6. 自己検査を外す・1879 が validate を自前で叩く
    let found = mutate(&|s| {
        replace(
            s,
            CHECK,
            "elif ! grep -q 'gating hook with .catch: '",
            "elif false",
        )
    });
    assert_named(&found, CHECK, "gating hook with .catch:");
    let found = mutate(&|s| {
        replace(
            s,
            T1879,
            "  out=\"$(/bin/bash \"$REPO_ROOT/scripts/check-claude-mod.sh\" 2>&1)\" || rc=$?",
            "  out=\"$(CLAUDE_CONFIG_DIR=\"$cfg\" claude plugin validate \"$REPO_ROOT/crates/tako-core/claude-mod\" 2>&1)\" || rc=$?",
        )
    });
    assert_named(&found, T1879, "自前で叩いている");
    assert_named(&found, T1879, "scripts/check-claude-mod.sh を呼んでいない");
    // 7. MCP の説明文から語彙を落とす
    let found = mutate(&|s| {
        let d = s.description_mut("tako_orchestrator_self");
        *d = d.replace("ctx_mod_reason", "理由");
    });
    assert_named(&found, CATALOG, "tako_orchestrator_self の説明文");
    let found = mutate(&|s| {
        let d = s.description_mut("tako_orchestrator_worker_status");
        *d = d.replace(
            "status_source = mod（tako mod の報告）/ ",
            "status_source = ",
        );
    });
    assert_named(&found, CATALOG, "tako_orchestrator_worker_status の説明文");
    // 注入の前の状態は綺麗（上の名指しは注入が原因）
    assert!(all(&base).is_empty(), "\n{}", all(&base).join("\n"));
}
