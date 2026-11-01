//! **#2646 の番犬**: orchestrator 系の dispatch（Spawn / Ledger / WorkerStatus の準備）のファイルの読み書きと
//! 画面の採取を GUI のメインスレッドで待たない。
//!
//! ## なぜ縛るのか
//!
//! master が常用する 3 つの操作が、そのたびに UI スレッドを止めていた（本番の perf.log 10/28〜11/02:
//! `dispatch:OrchestratorSpawn` 平均 80ms・`dispatch:OrchestratorLedger` 平均 54ms / 最大 661ms・
//! `offload_prepare:OrchestratorWorkerStatus` p50 33ms）。段 1 の隔離 GUI（本番と同じ大きさの合成データ）で
//! spawn は 52ms のうち 50ms が配置の後の記録と I/O（台帳 27ms・レジストリ 10ms・カタログ 5ms・MCP の構成 6ms・
//! 事前信頼 3ms）、台帳は list / stats 14ms・record 27ms が全部 YAML の解釈と書き戻しだった。いまは
//! `prepare_offload` / 着地（`land_offload`）が #1555 / #2630 / #2639 と同じ形で background へ出す。
//!
//! ## 何を確かめるか（逆戻りは file:line を名指して落とす）
//!
//! 1. 門は 1 か所: `fn orchestrator_offload_legacy(` が `ORCHESTRATOR_OFFLOAD_LEGACY_ENV` を読み、
//!    `"TAKO_2646_LEGACY"` の綴りは定数の 1 か所だけ
//! 2. 台帳: `prepare_offload` に門つきの `Request::OrchestratorLedger` の腕があり `OffloadJob::OrchestratorLedger(` を作る。
//!    `run_reply` は同期の腕と同じ `dispatch_orchestrator_ledger(` で答える
//! 3. worker の状態: `prepare_offload` の腕は門の内側で `worker_status_prepare(` を返す。UI スレッドの写し
//!    （`fn worker_status_prepare(` / `fn worker_status_seed(`）はレジストリを読まず（`resolve_worker_query(` /
//!    `WorkerRegistry`）、画面を採らない（`visible_lines(` / `analyze_input(` / `lines_and_input(`）。解決は
//!    `WorkerStatusJob::run` の `resolve_worker_query(`、画面の採取は `fn into_ctx(` の `lines_and_input(`。
//!    worker 指定の写しの着地は `OffloadLanding::Again(`（UI で組まない）
//! 4. spawn: 着地の `fn land_probed(` は門の内側で `spawn_place(` を通し `OffloadLanding::Settled(` で
//!    `OffloadJob::SpawnRecord(` を出す。配置の段 `fn spawn_place(` は記録と I/O を持たず、記録の段 `fn record(self)`
//!    は全部を持ち、起動を積まない（`launch_agent_gated(` は `fn spawn_launch(` だけ）。`run_staged` は記録を済ませてから
//!    `OffloadContinuation::SpawnLaunch(` を返す = **起動を積むのは記録の後**（送達フローが到達を書くレジストリの
//!    エントリが先に在る = #790）。同期の腕 `fn dispatch_orchestrator_spawn(` も配置 → 記録 → 積み込みの順に通す

use std::path::{Path, PathBuf};

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
/// A/B の門の env の綴りを持つ唯一の行
const GATE_CONST: &str = "pub const ORCHESTRATOR_OFFLOAD_LEGACY_ENV: &str = \"TAKO_2646_LEGACY\";";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す）
fn code_of(src: &str, rel: &str) -> String {
    production_range::code_view::without_comments(&production_range::production(src, rel))
}

fn dispatch_code() -> String {
    let src = std::fs::read_to_string(repo_root().join(DISPATCH))
        .unwrap_or_else(|e| panic!("{DISPATCH} を読めない: {e}"));
    code_of(&src, DISPATCH)
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at.min(src.len())].matches('\n').count() + 1
}

/// `open` の位置から始まる `{` に対応する `}` の位置（文字列の中の括弧は数えない）
fn block_end(src: &str, open: usize) -> Option<usize> {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let start = code[open..].find('{').map(|i| open + i)?;
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `sig` で始まる関数（か impl）の範囲。見つからなければ `None`
fn span(src: &str, sig: &str) -> Option<(usize, usize)> {
    let at = src.find(sig)?;
    Some((at, block_end(src, at)?))
}

fn all_at(hay: &str, needle: &str) -> Vec<usize> {
    hay.match_indices(needle).map(|(i, _)| i).collect()
}

/// `body` の中の `needle` で始まる腕。腕は次の `stop_at` まで
fn arm_of<'a>(body: &'a str, needle: &str, stop_at: &str) -> Option<(usize, &'a str)> {
    let at = body.find(needle)?;
    let rest = &body[at + 1..];
    let stop = rest.find(stop_at).map_or(rest.len(), |i| i + 1);
    Some((at, &body[at..at + stop]))
}

/// 関数 `sig` の本体を取り、無ければ空振りとして名指す
fn body_of<'a>(src: &'a str, sig: &str, out: &mut Vec<String>) -> Option<(usize, &'a str)> {
    match span(src, sig) {
        Some((start, end)) => Some((start, &src[start..end])),
        None => {
            out.push(format!(
                "{DISPATCH}:1 `{sig}` が無い（走査の空振り。#2646）"
            ));
            None
        }
    }
}

fn scan(d: &str) -> Vec<String> {
    let mut out = Vec::new();

    // 1. 門は 1 か所
    if let Some((start, body)) = body_of(d, "pub fn orchestrator_offload_legacy(", &mut out) {
        if !body.contains("ORCHESTRATOR_OFFLOAD_LEGACY_ENV") {
            out.push(format!(
                "{DISPATCH}:{} `orchestrator_offload_legacy` が門の env（`ORCHESTRATOR_OFFLOAD_LEGACY_ENV`）を読んでいない（#2646）",
                line_at(d, start)
            ));
        }
    }
    let constant = d.find(GATE_CONST);
    if constant.is_none() {
        out.push(format!(
            "{DISPATCH}:1 `TAKO_2646_LEGACY` の定数（`{GATE_CONST}`）が無い（A/B の門。#2646）"
        ));
    }
    for at in all_at(d, "\"TAKO_2646_LEGACY\"") {
        if constant.is_some_and(|c| (c..c + GATE_CONST.len()).contains(&at)) {
            continue;
        }
        out.push(format!(
            "{DISPATCH}:{} `TAKO_2646_LEGACY` を門の外で綴っている。A/B の門は `orchestrator_offload_legacy()` の 1 か所（#2646）",
            line_at(d, at)
        ));
    }

    // 2. 台帳 / 3. worker の状態（準備の腕）
    if let Some((start, body)) = body_of(d, "pub fn prepare_offload(", &mut out) {
        match arm_of(body, "Request::OrchestratorLedger {", "Request::") {
            None => out.push(format!(
                "{DISPATCH}:{} `prepare_offload` に `Request::OrchestratorLedger` の腕が無い。台帳の YAML の解釈と書き戻しを\
                 UI スレッドで待つ（#2646）。`OffloadJob::OrchestratorLedger(` を作る",
                line_at(d, start)
            )),
            Some((at, arm)) => {
                let line = line_at(d, start + at);
                if !arm.contains("OffloadJob::OrchestratorLedger(") {
                    out.push(format!(
                        "{DISPATCH}:{line} `Request::OrchestratorLedger` の腕が `OffloadJob::OrchestratorLedger(` を作っていない（#2646）"
                    ));
                }
                if !arm.contains("orchestrator_offload_legacy()") {
                    out.push(format!(
                        "{DISPATCH}:{line} `Request::OrchestratorLedger` の腕に A/B の門（`orchestrator_offload_legacy()`）が無い（#2646）"
                    ));
                }
            }
        }
        match arm_of(body, "Request::OrchestratorWorkerStatus {", "Request::") {
            None => out.push(format!(
                "{DISPATCH}:{} `prepare_offload` に `Request::OrchestratorWorkerStatus` の腕が無い（走査の空振り。#2646）",
                line_at(d, start)
            )),
            Some((at, arm)) => {
                let line = line_at(d, start + at);
                let gate = arm.find("orchestrator_offload_legacy()");
                let prep = arm.find("worker_status_prepare(");
                let resolve = arm.find("resolve_worker_query(");
                match (gate, prep) {
                    (Some(g), Some(p)) if g < p && resolve.is_none_or(|r| p < r) => {}
                    _ => out.push(format!(
                        "{DISPATCH}:{line} `Request::OrchestratorWorkerStatus` の腕が門（`orchestrator_offload_legacy()`）の内側で\
                         `worker_status_prepare(` を先に返していない。追跡キーの解決（workers.yaml の読み）と画面の採取を\
                         UI スレッドで待つ（#2646）"
                    )),
                }
            }
        }
    }
    if let Some((start, body)) = body_of(d, "fn run_reply(self)", &mut out) {
        match arm_of(body, "OffloadJob::OrchestratorLedger(", "OffloadJob::") {
            Some((_, arm)) if arm.contains("dispatch_orchestrator_ledger(") => {}
            Some((at, _)) => out.push(format!(
                "{DISPATCH}:{} `OffloadJob::OrchestratorLedger` が同期の腕と同じ本体（`dispatch_orchestrator_ledger(`）で答えていない（#2646）",
                line_at(d, start + at)
            )),
            None => out.push(format!(
                "{DISPATCH}:{} `run_reply` が `OffloadJob::OrchestratorLedger(` を答えていない（background から応答する。#2646）",
                line_at(d, start)
            )),
        }
    }
    // UI スレッドの写しはメモリだけ
    for sig in ["fn worker_status_prepare(", "fn worker_status_seed("] {
        if let Some((start, body)) = body_of(d, sig, &mut out) {
            for needle in [
                "resolve_worker_query(",
                "WorkerRegistry",
                "visible_lines(",
                "analyze_input(",
                "lines_and_input(",
            ] {
                for at in all_at(body, needle) {
                    out.push(format!(
                        "{DISPATCH}:{} `{sig}` が `{needle}` を呼んでいる。UI スレッドの写しはメモリだけを読む\
                         （レジストリの読みと画面の採取は background の `WorkerStatusJob::run` / `into_ctx`。#2646）",
                        line_at(d, start + at)
                    ));
                }
            }
        }
    }
    if let Some((start, body)) = body_of(d, "impl WorkerStatusJob {", &mut out) {
        if !body.contains("resolve_worker_query(") {
            out.push(format!(
                "{DISPATCH}:{} `WorkerStatusJob::run` が追跡キーを `resolve_worker_query(` で解決していない（同期の腕と 1 実装。#2646）",
                line_at(d, start)
            ));
        }
    }
    if let Some((start, body)) = body_of(d, "fn into_ctx(self)", &mut out) {
        if !body.contains("lines_and_input(") {
            out.push(format!(
                "{DISPATCH}:{} `into_ctx` が画面を `Send` な手（`lines_and_input(`）で採っていない（#2646）",
                line_at(d, start)
            ));
        }
    }
    if let Some((start, body)) = body_of(d, "pub fn land_offload(", &mut out) {
        match arm_of(body, "OffloadContinuation::WorkerStatusSeed(", "other =>") {
            Some((_, arm))
                if arm.contains("OffloadLanding::Again(") && !arm.contains("finish_offload(") => {}
            Some((at, _)) => out.push(format!(
                "{DISPATCH}:{} worker 指定の写しの着地が `OffloadLanding::Again(` で background へ戻していない\
                 （UI スレッドで画面を採って状態を組む。#2646）",
                line_at(d, start + at)
            )),
            None => out.push(format!(
                "{DISPATCH}:{} `land_offload` に `OffloadContinuation::WorkerStatusSeed(` の腕が無い（#2646）",
                line_at(d, start)
            )),
        }
    }

    // 4. spawn
    if let Some((start, body)) = body_of(d, "fn land_probed(", &mut out) {
        let line = line_at(d, start);
        let gate = body.find("orchestrator_offload_legacy()");
        let place = body.find("spawn_place(");
        if gate.zip(place).is_none_or(|(g, p)| g >= p) {
            out.push(format!(
                "{DISPATCH}:{line} `land_probed` が門（`orchestrator_offload_legacy()`）の内側で配置の段（`spawn_place(`）を\
                 通していない。spawn の記録と I/O を UI スレッドで待つ（#2646）"
            ));
        }
        if !body.contains("OffloadLanding::Settled(") || !body.contains("OffloadJob::SpawnRecord(")
        {
            out.push(format!(
                "{DISPATCH}:{line} `land_probed` が記録の段を `OffloadLanding::Settled(` + `OffloadJob::SpawnRecord(` で\
                 background へ出していない（#2646）"
            ));
        }
    }
    const RECORDS: [&str; 6] = [
        "worker_mcp::prepare(",
        "ensure_trusted_in(",
        "ensure_bypass_accepted_in(",
        "sessions::record_spawn(",
        "ledger::record_spawn(",
        "registry::record_spawn(",
    ];
    if let Some((start, body)) = body_of(d, "fn spawn_place(", &mut out) {
        for needle in RECORDS.iter().copied().chain(["launch_agent_gated("]) {
            for at in all_at(body, needle) {
                out.push(format!(
                    "{DISPATCH}:{} 配置の段（`spawn_place`）が `{needle}` を呼んでいる。配置の段は UI スレッドに残るので、\
                     記録と I/O は `SpawnPlaced::record`、起動の積み込みは `spawn_launch` へ置く（#2646）",
                    line_at(d, start + at)
                ));
            }
        }
    }
    if let Some((start, body)) = body_of(d, "fn record(self) -> SpawnRecorded", &mut out) {
        for needle in RECORDS {
            if !body.contains(needle) {
                out.push(format!(
                    "{DISPATCH}:{} 記録の段（`SpawnPlaced::record`）に `{needle}` が無い（#2646）",
                    line_at(d, start)
                ));
            }
        }
        for needle in ["launch_agent_gated(", "host."] {
            for at in all_at(body, needle) {
                out.push(format!(
                    "{DISPATCH}:{} 記録の段（`SpawnPlaced::record`）が `{needle}` を使っている。この段は background で走る\
                     （host に触らない）。起動は記録の後に `spawn_launch` が積む（#2646 / #790）",
                    line_at(d, start + at)
                ));
            }
        }
    }
    if let Some((start, body)) = body_of(d, "fn spawn_launch(", &mut out) {
        if !body.contains("launch_agent_gated(") {
            out.push(format!(
                "{DISPATCH}:{} 起動の積み込み（`spawn_launch`）が `launch_agent_gated(` を通していない（#2646）",
                line_at(d, start)
            ));
        }
    }
    if let Some((start, body)) = body_of(d, "pub fn run_staged(self)", &mut out) {
        match arm_of(body, "OffloadJob::SpawnRecord(", "OffloadJob::") {
            Some((_, arm))
                if arm.contains(".record()") && arm.contains("OffloadContinuation::SpawnLaunch(") => {}
            Some((at, _)) => out.push(format!(
                "{DISPATCH}:{} `OffloadJob::SpawnRecord` が記録（`.record()`）を済ませてから `OffloadContinuation::SpawnLaunch(` \
                 を返していない。起動を記録より前に積むと、送達フローが到達を書くレジストリのエントリが無い（#2646 / #790）",
                line_at(d, start + at)
            )),
            None => out.push(format!(
                "{DISPATCH}:{} `run_staged` に `OffloadJob::SpawnRecord(` の腕が無い（#2646）",
                line_at(d, start)
            )),
        }
    }
    if let Some((start, body)) = body_of(d, "fn dispatch_orchestrator_spawn(", &mut out) {
        let place = body.find("spawn_place(");
        let record = body.find(".record()");
        let launch = body.find("spawn_launch(");
        let ordered = match (place, record, launch) {
            (Some(p), Some(r), Some(l)) => p < r && r < l,
            _ => false,
        };
        if !ordered {
            out.push(format!(
                "{DISPATCH}:{} 同期の腕（`dispatch_orchestrator_spawn`）が配置（`spawn_place(`）→ 記録（`.record()`）→ \
                 積み込み（`spawn_launch(`）の順に通していない（段を書き分けると A/B と既定で中身が変わる。#2646）",
                line_at(d, start)
            ));
        }
    }
    out
}

#[test]
fn issue2646_orchestrator系はメインスレッドで待たない() {
    let found = scan(&dispatch_code());
    assert!(
        found.is_empty(),
        "#2646 の配線が欠けている（{} 件）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn issue2646_走査が空振りしていない() {
    let d = dispatch_code();
    for sig in [
        "pub fn prepare_offload(",
        "fn run_reply(self)",
        "pub fn run_staged(self)",
        "pub fn land_offload(",
        "fn land_probed(",
        "fn spawn_place(",
        "fn record(self) -> SpawnRecorded",
        "fn spawn_launch(",
        "fn worker_status_prepare(",
        "fn worker_status_seed(",
        "impl WorkerStatusJob {",
        "fn into_ctx(self)",
    ] {
        assert!(span(&d, sig).is_some(), "{DISPATCH} の `{sig}` が見えない");
    }
    // 記録の段は本当に記録を持っている（6 つの呼び出しが本番コードに 1 回ずつ見える）
    for needle in [
        "sessions::record_spawn(",
        "ledger::record_spawn(",
        "registry::record_spawn(",
    ] {
        assert_eq!(
            all_at(&d, needle).len(),
            1,
            "{DISPATCH} の `{needle}` が 1 件でない（走査の前提が変わった）"
        );
    }
}

/// 注入を当てる（`from` が無ければ注入そのものが空振り = 落とす）
fn inject(base: &str, from: &str, to: &str, what: &str) -> String {
    assert!(base.contains(from), "注入「{what}」の当て先が無い: {from}");
    base.replacen(from, to, 1)
}

/// 源の行番号（`needle` の最初の出現）
fn line_of(src: &str, needle: &str) -> usize {
    line_at(src, src.find(needle).expect("注入した箇所がある"))
}

fn assert_named(found: &[String], want: &str, what: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(want)),
        "注入「{what}」を名指しできていない（期待 {want}）: {found:#?}"
    );
}

#[test]
fn issue2646_注入_台帳の準備の門を外すと名指す() {
    let d = inject(
        &dispatch_code(),
        "        } if !orchestrator_offload_legacy() => {\n            Some(Ok(OffloadJob::OrchestratorLedger(",
        "        } => {\n            Some(Ok(OffloadJob::OrchestratorLedger(",
        "台帳の準備の門を外す",
    );
    let (start, _) = span(&d, "pub fn prepare_offload(").unwrap();
    let line = line_at(
        &d,
        start + d[start..].find("Request::OrchestratorLedger {").unwrap(),
    );
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{line}"),
        "台帳の準備の門を外す",
    );
}

#[test]
fn issue2646_注入_台帳の準備の腕を外すと名指す() {
    let base = dispatch_code();
    let (start, _) = span(&base, "pub fn prepare_offload(").unwrap();
    let d = inject(
        &base,
        "        Request::OrchestratorLedger {\n            action,",
        "        Request::RemoteAgents if false => None,\n        Request::OrchestratorLedgerX {\n            action,",
        "台帳の準備の腕を外す",
    );
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{}", line_at(&base, start)),
        "台帳の準備の腕を外す",
    );
}

#[test]
fn issue2646_注入_状態の写しでレジストリを読むと名指す() {
    let d = inject(
        &dispatch_code(),
        "    let seed = match (&worker, pane_id) {",
        "    let _ = resolve_worker_query(pane_id, worker.as_deref(), None, None);\n    let seed = match (&worker, pane_id) {",
        "写しでレジストリを読む",
    );
    let line = line_of(
        &d,
        "let _ = resolve_worker_query(pane_id, worker.as_deref()",
    );
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{line}"),
        "写しでレジストリを読む",
    );
}

#[test]
fn issue2646_注入_状態の写しで画面を採ると名指す() {
    let d = inject(
        &dispatch_code(),
        "        screen: host.session(target).map(|session| session.access()),",
        "        screen: host.session(target).map(|session| { let _ = session.visible_lines(); session.access() }),",
        "写しで画面を採る",
    );
    let line = line_of(&d, "let _ = session.visible_lines();");
    assert_named(&scan(&d), &format!("{DISPATCH}:{line}"), "写しで画面を採る");
}

#[test]
fn issue2646_注入_状態の準備を同期の解決へ戻すと名指す() {
    let d = inject(
        &dispatch_code(),
        "            if !orchestrator_offload_legacy() && !crate::diag::issue1968_legacy() {\n                return Some(worker_status_prepare(",
        "            if false {\n                return Some(worker_status_prepare(",
        "状態の準備を同期の解決へ戻す",
    );
    let (start, _) = span(&d, "pub fn prepare_offload(").unwrap();
    let line = line_at(
        &d,
        start
            + d[start..]
                .find("Request::OrchestratorWorkerStatus {")
                .unwrap(),
    );
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{line}"),
        "状態の準備を同期の解決へ戻す",
    );
}

#[test]
fn issue2646_注入_配置の段で台帳へ書くと名指す() {
    let d = inject(
        &dispatch_code(),
        "    Ok(SpawnPlaced {\n",
        "    let _ = crate::orchestrator::ledger::record_spawn(project, label, None, None, \"x\", None, None);\n    Ok(SpawnPlaced {\n",
        "配置の段で台帳へ書く",
    );
    let line = line_of(
        &d,
        "let _ = crate::orchestrator::ledger::record_spawn(project",
    );
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{line}"),
        "配置の段で台帳へ書く",
    );
}

#[test]
fn issue2646_注入_記録の段で起動を積むと名指す() {
    let d = inject(
        &dispatch_code(),
        "        crate::request_claude_scan();",
        "        let _ = launch_agent_gated(todo!(), PaneOrigin::Mcp, todo!());\n        crate::request_claude_scan();",
        "記録の段で起動を積む",
    );
    let line = line_of(&d, "let _ = launch_agent_gated(todo!()");
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{line}"),
        "記録の段で起動を積む",
    );
}

#[test]
fn issue2646_注入_起動を記録より前に積むと名指す() {
    let d = inject(
        &dispatch_code(),
        "            OffloadJob::SpawnRecord(placed) => {\n                OffloadOutcome::OnUi(OffloadContinuation::SpawnLaunch(Box::new(placed.record())))",
        "            OffloadJob::SpawnRecord(placed) => {\n                OffloadOutcome::Reply(Ok(serde_json::json!({ \"pane\": placed.new_id.as_u64() })))",
        "起動を記録より前に積む",
    );
    let line = line_of(&d, "OffloadJob::SpawnRecord(placed) =>");
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{line}"),
        "起動を記録より前に積む",
    );
}

#[test]
fn issue2646_注入_spawnの着地を同期のdispatchへ戻すと名指す() {
    let d = inject(
        &dispatch_code(),
        "    if !orchestrator_offload_legacy() {\n        if let Some(params) = spawn_params_of(&probed.request) {",
        "    if false {\n        if let Some(params) = spawn_params_of(&probed.request) {",
        "spawn の着地を同期へ戻す",
    );
    let line = line_of(&d, "fn land_probed(");
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{line}"),
        "spawn の着地を同期へ戻す",
    );
}

#[test]
fn issue2646_注入_門の外でab_envを綴ると名指す() {
    let d = inject(
        &dispatch_code(),
        "fn land_probed(\n",
        "fn injected_gate() -> bool { std::env::var(\"TAKO_2646_LEGACY\").is_ok() }\n\nfn land_probed(\n",
        "門の外で env を綴る",
    );
    let line = line_of(&d, "fn injected_gate()");
    assert_named(
        &scan(&d),
        &format!("{DISPATCH}:{line}"),
        "門の外で env を綴る",
    );
}
