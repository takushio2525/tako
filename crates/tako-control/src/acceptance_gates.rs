//! acceptance_gates — AcceptanceGate の Store と YAML 永続化（Issue #244）
//!
//! task_checkpoints.rs / config_io と同パターン: `<data_dir>/acceptance_gates.yaml` に
//! 排他 flock + アトミック書き込み + 世代バックアップで永続化する。
//! データモデルは tako-core::acceptance_gate にある。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tako_core::acceptance_gate::{
    AcceptanceCriterion, AcceptanceGate, CriterionKind, CriterionStatus, GateStatus,
};
use tako_core::task_checkpoint::unix_now;

/// ゲートファイルのパス
/// `TAKO_ACCEPTANCE_GATES_FILE` で上書き可能（隔離検証用）
pub fn store_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TAKO_ACCEPTANCE_GATES_FILE") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    tako_core::paths::data_dir().map(|d| d.join("acceptance_gates.yaml"))
}

/// acceptance_gates.yaml のトップレベルスキーマ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptanceGateStore {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub gates: Vec<AcceptanceGate>,
}

fn default_version() -> u32 {
    1
}

impl Default for AcceptanceGateStore {
    fn default() -> Self {
        Self {
            version: 1,
            gates: Vec::new(),
        }
    }
}

impl AcceptanceGateStore {
    pub fn find(&self, task_id: &str) -> Option<&AcceptanceGate> {
        self.gates.iter().find(|g| g.task_id == task_id)
    }

    pub fn find_mut(&mut self, task_id: &str) -> Option<&mut AcceptanceGate> {
        self.gates.iter_mut().find(|g| g.task_id == task_id)
    }

    pub fn upsert(&mut self, gate: AcceptanceGate) {
        if let Some(existing) = self.find_mut(&gate.task_id) {
            *existing = gate;
        } else {
            self.gates.push(gate);
        }
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("acceptance_gates.yaml の読み取りに失敗: {e}"))?;
        if content.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_yaml::from_str(&content)
            .map_err(|e| format!("acceptance_gates.yaml のパースに失敗: {e}"))
    }

    pub fn load() -> Result<Self, String> {
        let path = store_path().ok_or("データディレクトリを解決できない")?;
        Self::load_from(&path)
    }

    pub fn mutate_at<R>(path: &Path, f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let _lock = crate::config_io::lock_exclusive(path)?;
        let mut store = Self::load_from(path)?;
        let result = f(&mut store);
        let content =
            serde_yaml::to_string(&store).map_err(|e| format!("YAML のシリアライズに失敗: {e}"))?;
        crate::config_io::atomic_write_with_backup(path, &content)?;
        Ok(result)
    }

    pub fn mutate<R>(f: impl FnOnce(&mut Self) -> R) -> Result<R, String> {
        let path = store_path().ok_or("データディレクトリを解決できない")?;
        Self::mutate_at(&path, f)
    }
}

// ────────────────────────────────────────────────────────────────
// gate set
// ────────────────────────────────────────────────────────────────

/// criteria_json から AcceptanceCriterion のベクタをパースする。
/// 形式: [{ "id": "tests_green", "kind": { "type": "command", "cmd": "cargo test" }, ... }, ...]
pub fn parse_criteria(criteria_json: &str) -> Result<Vec<AcceptanceCriterion>, String> {
    let arr: Vec<Value> = serde_json::from_str(criteria_json)
        .map_err(|e| format!("criteria の JSON パースに失敗: {e}"))?;
    let mut criteria = Vec::new();
    for v in &arr {
        let id = v["id"]
            .as_str()
            .ok_or("criterion に id が必要")?
            .to_string();
        let kind: CriterionKind = serde_json::from_value(v["kind"].clone())
            .map_err(|e| format!("criterion kind のパースに失敗 (id={id}): {e}"))?;
        let status = if let Some(s) = v["status"].as_str() {
            match s {
                "pending" => CriterionStatus::Pending,
                "passed" => CriterionStatus::Passed,
                "failed" => CriterionStatus::Failed,
                other => return Err(format!("不明な status: {other}")),
            }
        } else {
            CriterionStatus::Pending
        };
        criteria.push(AcceptanceCriterion {
            id,
            kind,
            status,
            evidence: v["evidence"].as_str().map(String::from),
            checked_at: v["checked_at"].as_i64(),
        });
    }
    Ok(criteria)
}

/// gate set のレスポンス JSON を構築する（パス指定版）
pub fn set_gate_payload_at(
    path: &Path,
    task_id: &str,
    criteria_json: &str,
    cwd: Option<&str>,
) -> Result<Value, String> {
    let criteria = parse_criteria(criteria_json)?;
    if criteria.is_empty() {
        return Err("criteria を 1 つ以上指定する".into());
    }
    AcceptanceGateStore::mutate_at(path, |store| {
        let mut gate = AcceptanceGate {
            task_id: task_id.to_string(),
            criteria,
            overall: GateStatus::Pending,
            cwd: cwd.map(String::from),
        };
        gate.recompute_overall();
        store.upsert(gate);
    })?;
    show_gate_payload_at(path, task_id)
}

/// gate set のレスポンス JSON を構築する
pub fn set_gate_payload(
    task_id: &str,
    criteria_json: &str,
    cwd: Option<&str>,
) -> Result<Value, String> {
    let path = store_path().ok_or("データディレクトリを解決できない")?;
    set_gate_payload_at(&path, task_id, criteria_json, cwd)
}

// ────────────────────────────────────────────────────────────────
// gate show
// ────────────────────────────────────────────────────────────────

/// gate show のレスポンス JSON を構築する（パス指定版）
pub fn show_gate_payload_at(path: &Path, task_id: &str) -> Result<Value, String> {
    let store = AcceptanceGateStore::load_from(path)?;
    let gate = store
        .find(task_id)
        .ok_or_else(|| format!("ゲートが見つからない: {task_id}"))?;
    Ok(gate_to_json(gate))
}

/// gate show のレスポンス JSON を構築する
pub fn show_gate_payload(task_id: &str) -> Result<Value, String> {
    let store = AcceptanceGateStore::load()?;
    let gate = store
        .find(task_id)
        .ok_or_else(|| format!("ゲートが見つからない: {task_id}"))?;
    Ok(gate_to_json(gate))
}

fn gate_to_json(gate: &AcceptanceGate) -> Value {
    let criteria: Vec<Value> = gate.criteria.iter().map(criterion_to_json).collect();
    let mut v = json!({
        "task_id": gate.task_id,
        "overall": gate.overall.as_str(),
        "criteria": criteria,
    });
    if let Some(ref cwd) = gate.cwd {
        v.as_object_mut().unwrap().insert("cwd".into(), json!(cwd));
    }
    v
}

fn criterion_to_json(c: &AcceptanceCriterion) -> Value {
    let kind_json = serde_json::to_value(&c.kind).unwrap_or(json!(null));
    let mut v = json!({
        "id": c.id,
        "kind": kind_json,
        "status": c.status.as_str(),
    });
    let obj = v.as_object_mut().unwrap();
    if let Some(ref ev) = c.evidence {
        obj.insert("evidence".into(), json!(ev));
    }
    if let Some(at) = c.checked_at {
        obj.insert("checked_at".into(), json!(at));
    }
    v
}

// ────────────────────────────────────────────────────────────────
// gate record_results（dispatch 経由。check の結果を永続化する）
// ────────────────────────────────────────────────────────────────

/// 個別 criterion の判定結果
pub struct CriterionResult {
    pub id: String,
    pub passed: bool,
    pub evidence: String,
}

/// results_json から CriterionResult のベクタをパースする。
/// 形式: [{ "id": "tests_green", "passed": true, "evidence": "exit 0" }, ...]
pub fn parse_results(results_json: &str) -> Result<Vec<CriterionResult>, String> {
    let arr: Vec<Value> = serde_json::from_str(results_json)
        .map_err(|e| format!("results の JSON パースに失敗: {e}"))?;
    let mut results = Vec::new();
    for v in &arr {
        results.push(CriterionResult {
            id: v["id"].as_str().ok_or("result に id が必要")?.to_string(),
            passed: v["passed"].as_bool().unwrap_or(false),
            evidence: v["evidence"].as_str().unwrap_or("").to_string(),
        });
    }
    Ok(results)
}

/// record_results のレスポンス JSON（パス指定版）
pub fn record_results_payload_at(
    path: &Path,
    task_id: &str,
    results_json: &str,
    sync_checkpoint: bool,
) -> Result<Value, String> {
    let results = parse_results(results_json)?;
    let now = unix_now();
    let overall: GateStatus = AcceptanceGateStore::mutate_at(path, |store| {
        let gate = store
            .find_mut(task_id)
            .ok_or_else(|| format!("ゲートが見つからない: {task_id}"))?;
        for r in &results {
            if let Some(c) = gate.criteria.iter_mut().find(|c| c.id == r.id) {
                c.status = if r.passed {
                    CriterionStatus::Passed
                } else {
                    CriterionStatus::Failed
                };
                c.evidence = Some(r.evidence.clone());
                c.checked_at = Some(now);
            }
        }
        gate.recompute_overall();
        Ok::<GateStatus, String>(gate.overall)
    })??;

    if sync_checkpoint && overall == GateStatus::Passed {
        let _ = crate::task_checkpoints::update_phase_payload(task_id, "done", None);
    }

    show_gate_payload_at(path, task_id)
}

/// record_results のレスポンス JSON（+ checkpoint phase 連動）
pub fn record_results_payload(
    task_id: &str,
    results_json: &str,
    sync_checkpoint: bool,
) -> Result<Value, String> {
    let path = store_path().ok_or("データディレクトリを解決できない")?;
    record_results_payload_at(&path, task_id, results_json, sync_checkpoint)
}

// ────────────────────────────────────────────────────────────────
// gate check（CLI / MCP が直接呼ぶ。UI スレッドを塞がない）
// ────────────────────────────────────────────────────────────────

/// Command / PrMerged 述語を実行し、結果を永続化して返す。
/// Custom 述語はスキップする（手動判定のため）。
/// checkpoint 連動: sync_checkpoint=true かつ全 Passed → checkpoint.phase = Done
pub fn execute_gate_check(task_id: &str, sync_checkpoint: bool) -> Result<Value, String> {
    let path = store_path().ok_or("データディレクトリを解決できない")?;
    execute_gate_check_at(&path, task_id, sync_checkpoint)
}

/// パス指定版の execute_gate_check（テスト・隔離用）
pub fn execute_gate_check_at(
    path: &Path,
    task_id: &str,
    sync_checkpoint: bool,
) -> Result<Value, String> {
    let store = AcceptanceGateStore::load_from(path)?;
    let gate = store
        .find(task_id)
        .ok_or_else(|| format!("ゲートが見つからない: {task_id}"))?;

    let cwd = gate.cwd.clone();
    let mut results = Vec::new();

    for criterion in &gate.criteria {
        match &criterion.kind {
            CriterionKind::Command { cmd, expect_exit_0 } => {
                let result = execute_command(cmd, *expect_exit_0, cwd.as_deref());
                results.push(result_to_json(&criterion.id, &result));
            }
            CriterionKind::PrMerged { pr_number, repo } => {
                let result = check_pr_merged(*pr_number, repo.as_deref());
                results.push(result_to_json(&criterion.id, &result));
            }
            CriterionKind::Custom { .. } => {}
        }
    }

    if results.is_empty() {
        return show_gate_payload_at(path, task_id);
    }

    let results_json =
        serde_json::to_string(&results).map_err(|e| format!("JSON シリアライズに失敗: {e}"))?;
    record_results_payload_at(path, task_id, &results_json, sync_checkpoint)
}

struct CheckResult {
    passed: bool,
    evidence: String,
}

fn result_to_json(id: &str, result: &CheckResult) -> Value {
    json!({
        "id": id,
        "passed": result.passed,
        "evidence": result.evidence,
    })
}

/// Command 述語の実行。
///
/// **述語はシェル構文を含む「1 本の文字列」**（`cargo test --workspace && git diff --quiet`
/// のような形）なので、どのシェルに渡すかは抽象境界 B1
/// （[`tako_core::platform::shell::output_command`]）が決める。ここで `sh -c` を
/// 直書きしていたため Windows では `CreateProcess` が失敗し、コマンド型ゲートが
/// **一切判定できなかった**（#935）。述語を書く側から見た方言（POSIX / PowerShell）は
/// `tako_core::platform::shell::script_dialect()` で分かる
fn execute_command(cmd: &str, expect_exit_0: bool, cwd: Option<&str>) -> CheckResult {
    eprintln!("[gate check] command: {cmd}");
    if let Some(d) = cwd {
        eprintln!("[gate check] cwd: {d}");
    }

    // コンソールウィンドウの抑止（#586）は境界の内側で掛かる
    let mut command = tako_core::platform::shell::output_command(cmd);
    if let Some(d) = cwd {
        command.current_dir(d);
    }
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());

    match command.output() {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let passed = if expect_exit_0 {
                exit_code == 0
            } else {
                exit_code != 0
            };
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let evidence = format_evidence(exit_code, &stdout, &stderr);
            CheckResult { passed, evidence }
        }
        Err(e) => CheckResult {
            passed: false,
            evidence: format!("コマンド実行に失敗: {e}"),
        },
    }
}

/// PrMerged 述語の判定
fn check_pr_merged(pr_number: u32, repo: Option<&str>) -> CheckResult {
    let mut args = vec![
        "pr".to_string(),
        "view".to_string(),
        pr_number.to_string(),
        "--json".to_string(),
        "state,mergedAt".to_string(),
    ];
    if let Some(r) = repo {
        args.push("--repo".to_string());
        args.push(r.to_string());
    }
    eprintln!("[gate check] gh {}", args.join(" "));

    // #586: GUI プロセスから到達するのでコンソールウィンドウを出させない
    match tako_core::platform::process::no_console_window(
        std::process::Command::new("gh").args(&args),
    )
    .output()
    {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return CheckResult {
                    passed: false,
                    evidence: format!(
                        "gh pr view failed (exit {}): {}",
                        output.status,
                        stderr.trim()
                    ),
                };
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            match serde_json::from_str::<Value>(&stdout) {
                Ok(v) => {
                    let state = v["state"].as_str().unwrap_or("UNKNOWN");
                    let merged = state == "MERGED";
                    let evidence = if merged {
                        let merged_at = v["mergedAt"].as_str().unwrap_or("?");
                        format!("PR #{pr_number} MERGED at {merged_at}")
                    } else {
                        format!("PR #{pr_number} state={state} (not merged)")
                    };
                    CheckResult {
                        passed: merged,
                        evidence,
                    }
                }
                Err(e) => CheckResult {
                    passed: false,
                    evidence: format!("gh output のパースに失敗: {e}"),
                },
            }
        }
        Err(e) => CheckResult {
            passed: false,
            evidence: format!("gh の実行に失敗: {e}"),
        },
    }
}

/// コマンド出力を短縮して証拠にする
fn format_evidence(exit_code: i32, stdout: &str, stderr: &str) -> String {
    let stdout_trimmed = stdout.trim();
    let stderr_trimmed = stderr.trim();
    let mut parts = vec![format!("exit {exit_code}")];
    if !stdout_trimmed.is_empty() {
        let lines: Vec<&str> = stdout_trimmed.lines().collect();
        if lines.len() <= 5 {
            parts.push(format!("stdout: {stdout_trimmed}"));
        } else {
            let last5: Vec<&str> = lines[lines.len() - 5..].to_vec();
            parts.push(format!(
                "stdout (last 5/{} lines): {}",
                lines.len(),
                last5.join("\n")
            ));
        }
    }
    if !stderr_trimmed.is_empty() {
        let lines: Vec<&str> = stderr_trimmed.lines().collect();
        if lines.len() <= 5 {
            parts.push(format!("stderr: {stderr_trimmed}"));
        } else {
            let last5: Vec<&str> = lines[lines.len() - 5..].to_vec();
            parts.push(format!(
                "stderr (last 5/{} lines): {}",
                lines.len(),
                last5.join("\n")
            ));
        }
    }
    let joined = parts.join("; ");
    if joined.len() > EVIDENCE_MAX_BYTES {
        format!(
            "{}...(truncated)",
            &joined[..floor_char_boundary(&joined, EVIDENCE_MAX_BYTES)]
        )
    } else {
        joined
    }
}

/// evidence の上限（バイト）
const EVIDENCE_MAX_BYTES: usize = 2000;

/// `max` バイト以下で**文字境界**へ丸めた位置。
///
/// `&s[..max]` の素の切り出しは**マルチバイト文字の途中で panic する**。
/// 述語の出力は任意なので日本語が来うるし、#935 で Windows の出力を UTF-8 へ
/// 揃えたことで**正しい多バイト列が届くようになった**（それまでは CP932 が
/// `from_utf8_lossy` で置換文字へ潰れていて偶然 3 バイト境界に乗っていた）。
/// `str::floor_char_boundary` は unstable なので自前で持つ
fn floor_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut i = max;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tako-gate-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 述語を走らせるシェルの方言（#935）。
    ///
    /// **`true` / `false` / `pwd` をリテラルで書かない**: PowerShell に `true` /
    /// `false` は無く（実機実測: `The term 'true' is not recognized` で exit 1）、
    /// `pwd` は表として整形されてパスが切られる。境界が実際に起こすシェルへ
    /// 合わせた形を `ShellDialect` から作れば、判定の意図（0 で終わる / 非 0 で
    /// 終わる / cwd を出す）だけをテストに残せる
    fn dialect() -> tako_core::platform::shell_dialect::ShellDialect {
        tako_core::platform::shell::script_dialect()
    }

    /// 述語 1 本ぶんの JSON（コマンド文字列は引用符を含むので必ず serde で符号化する）
    fn command_criterion(id: &str, cmd: &str) -> serde_json::Value {
        json!({ "id": id, "kind": { "type": "command", "cmd": cmd } })
    }

    #[test]
    fn set_and_show_roundtrip() {
        let dir = temp_dir("set-show");
        let path = dir.join("acceptance_gates.yaml");

        let criteria = r#"[
            {"id": "tests", "kind": {"type": "command", "cmd": "cargo test"}},
            {"id": "pr", "kind": {"type": "pr_merged", "pr_number": 100}}
        ]"#;
        let result = set_gate_payload_at(&path, "task-1", criteria, Some("/tmp")).unwrap();
        assert_eq!(result["task_id"], "task-1");
        assert_eq!(result["overall"], "pending");
        assert_eq!(result["criteria"].as_array().unwrap().len(), 2);
        assert_eq!(result["cwd"], "/tmp");

        let shown = show_gate_payload_at(&path, "task-1").unwrap();
        assert_eq!(shown["overall"], "pending");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_results_updates_criteria() {
        let dir = temp_dir("record");
        let path = dir.join("acceptance_gates.yaml");

        let criteria = r#"[
            {"id": "a", "kind": {"type": "command", "cmd": "true"}},
            {"id": "b", "kind": {"type": "custom", "description": "manual"}}
        ]"#;
        set_gate_payload_at(&path, "task-1", criteria, None).unwrap();

        let results = r#"[{"id": "a", "passed": true, "evidence": "exit 0"}]"#;
        let updated = record_results_payload_at(&path, "task-1", results, false).unwrap();
        assert_eq!(updated["criteria"][0]["status"], "passed");
        assert_eq!(updated["criteria"][1]["status"], "pending");
        assert_eq!(updated["overall"], "pending");

        let results2 = r#"[{"id": "b", "passed": true, "evidence": "confirmed"}]"#;
        let updated2 = record_results_payload_at(&path, "task-1", results2, false).unwrap();
        assert_eq!(updated2["overall"], "passed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_command_true_false() {
        let d = dialect();
        let r = execute_command(&d.exit_status(0), true, None);
        assert!(r.passed, "evidence: {}", r.evidence);
        assert!(r.evidence.contains("exit 0"), "evidence: {}", r.evidence);

        let r = execute_command(&d.exit_status(1), true, None);
        assert!(!r.passed, "evidence: {}", r.evidence);
        assert!(r.evidence.contains("exit 1"), "evidence: {}", r.evidence);
    }

    /// 非 0 の終了コードが**そのまま**取れる（1 へ丸められない）。
    ///
    /// Windows は `-EncodedCommand` の終了コードが `$LASTEXITCODE` を素通ししない
    /// （実機実測: `cmd /c exit 7` が親から見て 1）ので、明示 `exit` を外すと
    /// 「どの値で落ちたか」が evidence から消える（#935）
    #[test]
    fn execute_command_は実際の終了コードをevidenceへ残す() {
        let r = execute_command(&dialect().exit_status(7), true, None);
        assert!(!r.passed, "evidence: {}", r.evidence);
        assert!(r.evidence.contains("exit 7"), "evidence: {}", r.evidence);
    }

    #[test]
    fn execute_command_with_output() {
        let r = execute_command(&dialect().echo("hello"), true, None);
        assert!(r.passed, "evidence: {}", r.evidence);
        assert!(r.evidence.contains("hello"), "evidence: {}", r.evidence);
    }

    #[test]
    fn execute_command_with_cwd() {
        // 実在する一時ディレクトリで測る（`/tmp` は Windows に無い）。突き合わせは
        // **末尾の要素**で行う: macOS の `/var/folders/…` は `/private` 付きへ
        // 解決されうるので、フルパスの一致は経路によって崩れる
        let dir = temp_dir("cwd");
        let leaf = dir
            .file_name()
            .expect("一時ディレクトリ名")
            .to_string_lossy()
            .to_string();

        let r = execute_command(&dialect().print_cwd(), true, dir.to_str());
        assert!(r.passed, "evidence: {}", r.evidence);
        assert!(
            r.evidence.contains(&leaf),
            "cwd が反映されていない（leaf={leaf}）: {}",
            r.evidence
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 非 ASCII の出力が evidence で読める形で返る（#935）。
    ///
    /// Windows の PowerShell 5.1 はパイプ相手へ既定のコードページ（日本語環境では
    /// CP932）で書くため、UTF-8 として不正なバイト列になり `from_utf8_lossy` が
    /// 置換文字へ潰す。**失敗したゲートの理由を読むのが evidence の目的**なので、
    /// ここが化けると機能の半分が死ぬ
    #[test]
    fn execute_command_は非asciiの出力を化けさせない() {
        let r = execute_command(&dialect().echo("検証テスト"), true, None);
        assert!(r.passed, "evidence: {}", r.evidence);
        assert!(
            r.evidence.contains("検証テスト"),
            "evidence: {}",
            r.evidence
        );
        assert!(
            !r.evidence.contains('\u{fffd}'),
            "置換文字が混ざった（符号化が合っていない）: {}",
            r.evidence
        );
    }

    #[test]
    fn gate_check_with_command() {
        let dir = temp_dir("check-cmd");
        let path = dir.join("acceptance_gates.yaml");

        let d = dialect();
        let criteria = json!([
            command_criterion("pass", &d.echo("ok")),
            command_criterion("fail", &d.exit_status(1)),
        ])
        .to_string();
        set_gate_payload_at(&path, "task-1", &criteria, None).unwrap();

        let result = execute_gate_check_at(&path, "task-1", false).unwrap();
        assert_eq!(result["criteria"][0]["status"], "passed", "{result:#}");
        assert_eq!(result["criteria"][1]["status"], "failed", "{result:#}");
        assert_eq!(result["overall"], "failed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gate_check_skips_custom() {
        let dir = temp_dir("check-custom");
        let path = dir.join("acceptance_gates.yaml");

        let criteria = json!([
            command_criterion("auto", &dialect().exit_status(0)),
            {"id": "manual", "kind": {"type": "custom", "description": "check manually"}},
        ])
        .to_string();
        set_gate_payload_at(&path, "task-1", &criteria, None).unwrap();

        let result = execute_gate_check_at(&path, "task-1", false).unwrap();
        assert_eq!(result["criteria"][0]["status"], "passed", "{result:#}");
        assert_eq!(result["criteria"][1]["status"], "pending", "{result:#}");
        assert_eq!(result["overall"], "pending");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nonexistent_task_errors() {
        let dir = temp_dir("nonexistent");
        let path = dir.join("acceptance_gates.yaml");

        let result = show_gate_payload_at(&path, "no-such-task");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("見つからない"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_criteria_rejected() {
        let dir = temp_dir("empty-crit");
        let path = dir.join("acceptance_gates.yaml");

        let result = set_gate_payload_at(&path, "task-1", "[]", None);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("1 つ以上"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_evidence_truncates_long_output() {
        let long_line = "x".repeat(300);
        let stdout = (0..100)
            .map(|i| format!("line {i}: {long_line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let evidence = format_evidence(0, &stdout, "");
        assert!(evidence.len() <= 2100);
        assert!(evidence.contains("truncated") || evidence.contains("last 5/"));
    }

    /// 上限での切り詰めが**マルチバイト文字の途中で panic しない**（#935）。
    ///
    /// 素の `&joined[..2000]` は境界を外れると panic する。述語の出力は任意なので
    /// 日本語が来うるし、Windows の出力を UTF-8 へ揃えた（#935）ことで
    /// **正しい多バイト列が届くようになった**ぶん現実的な経路になった
    #[test]
    fn format_evidence_は多バイト文字の途中で切らない() {
        // 上限をまたぐ位置に 3 バイト文字が並ぶよう、境界を 1 バイトずつ動かして総当たり
        for pad in 0..8 {
            let stdout = format!("{}{}", "x".repeat(pad), "あ".repeat(1200));
            let evidence = format_evidence(0, &stdout, "");
            assert!(
                evidence.ends_with("...(truncated)"),
                "pad={pad} で切り詰めが起きていない"
            );
            // 切り詰めた本文が壊れていない（置換文字が混ざらない）
            assert!(
                !evidence.contains('\u{fffd}'),
                "pad={pad} で文字が壊れた: {}",
                &evidence[..40.min(evidence.len())]
            );
            assert!(evidence.len() <= EVIDENCE_MAX_BYTES + 32, "pad={pad}");
        }
    }

    #[test]
    fn missing_file_returns_empty_store() {
        let dir = temp_dir("missing");
        let path = dir.join("nonexistent.yaml");
        let store = AcceptanceGateStore::load_from(&path).unwrap();
        assert!(store.gates.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_yaml_returns_error() {
        let dir = temp_dir("corrupt");
        let path = dir.join("acceptance_gates.yaml");
        std::fs::write(&path, "{{invalid yaml").unwrap();
        let result = AcceptanceGateStore::load_from(&path);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
