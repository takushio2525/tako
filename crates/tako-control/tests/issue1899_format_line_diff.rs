//! #1899 の e2e: 整形の答えを**全文を 1 つの書き換えで返す**言語サーバ（偽サーバの規則
//! `whole: true`）を実プロセスで起こし、`LspManager::format` の答えを GUI / dispatch と同じ
//! 1 実装（`TextBuffer::apply_changes`）で当てて、undo の履歴・undo 1 回の戻り・当てた本文を測る。
//!
//! ## 何が起きていたか
//!
//! #1869 で整形は「離れた箇所ごとの差分を 1 件に並べる」形へ寄せたが、全文を 1 つの
//! `TextEdit` で返すサーバの答えは、最小化（同じ頭と尻を削る）しても**最初に変わった所から
//! 最後に変わった所までの 1 か所**が残る。10 万行の行末の空白を消す整形なら本文 2 本ぶん
//! （約 17 MB）を undo へ積み、予算 8 MB を超えてその前の打鍵の履歴を捨てていた。
//!
//! `apply_changes` が改行をまたぐ書き換えを行ごとの置き換えへ割る（`text_edit::line_split`。
//! 突き合わせは `text_diff::changed_regions`）。A/B の旧腕は `TAKO_1899_LEGACY=1`
//! （このファイルを旧腕で走らせると 10 万行の検査が名指しで落ちる）。
//!
//! 実の rust-analyzer は `--ignored` の 1 本（`TAKO_1899_REAL_RA=<実行ファイルの実体>`）が測る。
//! その 1 本の旧腕は `TAKO_1899_LEGACY=1` を組んだ子で当てる（プロセスの env は書き換えない =
//! `--include-ignored` で並列に回しても 10 万行の検査へ漏れない。#2400）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{FormatAnswer, FormatRequest, Launch, LspConfig, LspManager};
use tako_core::lsp::format::FormatOptions;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::text_edit::{AppliedChanges, IndentUnit, TextBuffer};

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// undo の履歴の予算（`text_edit::UNDO_BYTE_LIMIT` と同じ 8 MiB。非公開なので値で持つ）
const UNDO_BYTE_LIMIT: usize = 8 * 1024 * 1024;

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-1899-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        )
        .unwrap();
        Self(dir)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.0.join(rel);
        std::fs::write(&path, body).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 偽サーバの設定。規則のファイルは manager ごとに分ける（サーバは最初の問い合わせで起きるので、
/// 同じ置き場の 2 つ目の規則が 1 つ目を上書きすると両方が 2 つ目で答える）
fn config(scratch: &Scratch, name: &str, rules: &Value) -> LspConfig {
    // 初回 exec の検査を時計の外で済ませる（#2228）
    tako_core::fake_exec::warm_binary(FAKE);
    let rules_path = scratch.0.join(format!("format-{name}.json"));
    std::fs::write(&rules_path, rules.to_string()).unwrap();
    let args = vec![
        "--scenario".to_string(),
        "normal".to_string(),
        "--format".to_string(),
        rules_path.display().to_string(),
    ];
    launcher_config(FAKE.to_string(), args)
}

fn launcher_config(program: String, args: Vec<String>) -> LspConfig {
    LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: program.clone(),
                args: args.clone(),
            },
            program_path: program.clone(),
        }),
        request_timeout: Duration::from_secs(120),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    }
}

fn request(path: &Path, text: &str) -> FormatRequest {
    FormatRequest {
        path: path.to_path_buf(),
        text: text.to_string(),
        range: None,
        options: FormatOptions::from_indent(IndentUnit::Spaces(4)),
        timeout: Duration::from_secs(120),
    }
}

/// 同じ本文を 2 つのサーバ（行ごとの答え / 全文 1 つの答え）で整形した答え
fn both_answers(label: &str, text: &str) -> (FormatAnswer, FormatAnswer) {
    let scratch = Scratch::new(label);
    let path = scratch.write("src/main.rs", text);
    let per_line = LspManager::new(config(&scratch, "per-line", &json!([])));
    let whole = LspManager::new(config(&scratch, "whole", &json!([{ "whole": true }])));
    let a = per_line
        .format(&request(&path, text))
        .expect("行ごとの答え");
    let b = whole
        .format(&request(&path, text))
        .expect("全文 1 つの答え");
    per_line.shutdown_all(Duration::from_secs(2));
    whole.shutdown_all(Duration::from_secs(2));
    (a, b)
}

/// 答えを編集バッファへ当てる（GUI / dispatch と同じ 1 実装 = `apply_changes`）
fn apply(buffer: &mut TextBuffer, answer: &FormatAnswer) -> AppliedChanges {
    buffer
        .apply_changes(answer.changes.clone(), Some(buffer.version()))
        .unwrap()
}

/// 10 万行（#1660 の上限 10 MB / 10 万行の付近）の本文。行末に空白 2 つ（整形が消す）
fn hundred_thousand_lines() -> String {
    (0..100_000)
        .map(|i| {
            format!(
                "    let value_{i:06} = compute(alpha, beta, gamma, delta, epsilon, zeta, eta, theta);  \n"
            )
        })
        .collect()
}

/// 受け入れ条件 1〜3: 全文 1 つの答えでも、undo の履歴は行ごとの差（2 バイト × 10 万 + 箇所の表）
/// 程度に収まり、その前の打鍵の履歴も残り、undo 1 回で整形の前とバイト一致へ戻る。当てた本文は
/// 行ごとの答えを当てたときとバイト一致
#[test]
fn 十万行の全文置き換えの整形も行ごとの差だけを積み_undo_1_回で戻る() {
    let original = hundred_thousand_lines();
    // 整形の前の打鍵 1 塊（これが整形の後も戻れること = 履歴が捨てられていない）
    let typed = format!("//{original}");
    let (per_line, whole) = both_answers("big", &typed);
    assert_eq!(per_line.changes.len(), 100_000, "行ごとの答えは 10 万件");
    assert_eq!(
        whole.changes.len(),
        1,
        "全文 1 つの答えは最小化しても 1 か所（先頭の行末〜末尾の行末）"
    );

    let mut expected_buffer = TextBuffer::from_text(PathBuf::from("expected.rs"), typed.clone());
    apply(&mut expected_buffer, &per_line);
    let expected = expected_buffer.text().to_string();

    let mut buffer = TextBuffer::from_text(PathBuf::from("big.rs"), original.clone());
    buffer.set_cursor(0, false);
    buffer.insert("//");
    assert_eq!(buffer.text(), typed);
    let applied = apply(&mut buffer, &whole);
    let bytes = buffer.undo_history_bytes();
    let depth = buffer.undo_depth();
    eprintln!(
        "#1899 10 万行の全文 1 つの答え: 当てた箇所 {} / undo の履歴 {bytes} B / 深さ {depth}（legacy={}）",
        applied.changes,
        std::env::var("TAKO_1899_LEGACY").unwrap_or_default()
    );
    assert_eq!(
        buffer.text().as_bytes(),
        expected.as_bytes(),
        "当てた本文が行ごとの答えとバイト一致しない"
    );
    // 行ごとの差（2 バイト）+ 箇所の表（1 か所 3 語 = 24 バイト）× 10 万 + 打鍵の塊ぶんの余裕
    let per_line_sum = 100_000 * (2 + 3 * std::mem::size_of::<usize>()) + 4096;
    assert!(
        bytes <= per_line_sum,
        "undo の履歴が行ごとの差の合計を超えた（{bytes} > {per_line_sum}）"
    );
    assert!(bytes <= UNDO_BYTE_LIMIT);
    assert_eq!(applied.changes, 100_000, "行ごとへ割った箇所の数");
    assert_eq!(depth, 2, "打鍵の塊 + 整形 1 件（どちらも捨てられていない）");
    assert!(buffer.undo());
    assert_eq!(
        buffer.text().as_bytes(),
        typed.as_bytes(),
        "undo 1 回で整形の前とバイト一致"
    );
    assert!(buffer.undo());
    assert_eq!(buffer.text(), original, "その前の打鍵も戻れる");
    assert!(buffer.redo());
    assert!(buffer.redo());
    assert_eq!(buffer.text().as_bytes(), expected.as_bytes());
}

/// 受け入れ条件 4: 境界（空のファイル・末尾の改行の有無・CRLF・整形しても同じ）でも、全文 1 つの
/// 答えを当てた本文は行ごとの答えとバイト一致し、undo 1 回で戻る（変わらなければ何もしない）
#[test]
fn 境界でも全文1つの答えは行ごとの答えとバイト一致する() {
    let cases = [
        ("空のファイル", ""),
        ("末尾に改行が無い", "fn a() {  \n    x();  \n}  "),
        ("末尾に改行がある", "fn a() {  \n    x();  \n}\n"),
        ("CRLF", "fn a() { \r\n    x(); \r\n}\r\n"),
        ("整形しても同じ", "fn a() {\n    x();\n}\n"),
        ("多バイト", "// 名前😀  \nfn 名前() {\t\n}\n"),
    ];
    for (name, text) in cases {
        let (per_line, whole) = both_answers("edge", text);
        let mut expected = TextBuffer::from_text(PathBuf::from("a.rs"), text.to_string());
        let expected_applied = apply(&mut expected, &per_line);
        let mut buffer = TextBuffer::from_text(PathBuf::from("b.rs"), text.to_string());
        let version = buffer.version();
        let applied = apply(&mut buffer, &whole);
        assert_eq!(
            buffer.text().as_bytes(),
            expected.text().as_bytes(),
            "{name}: 当てた本文"
        );
        // 割るのは割って undo の履歴が軽くなるときだけ（数行の小さな直しは割らなくてよい）なので、
        // 箇所の数は「変えたか変えないか」だけ揃う
        assert_eq!(
            applied.changes == 0,
            expected_applied.changes == 0,
            "{name}: 変えたかどうか"
        );
        if applied.changes == 0 {
            assert_eq!(
                buffer.version(),
                version,
                "{name}: 変わらなければ版も進まない"
            );
            assert_eq!(buffer.undo_depth(), 0, "{name}");
            continue;
        }
        assert_eq!(buffer.undo_depth(), 1, "{name}");
        assert!(buffer.undo(), "{name}");
        assert_eq!(
            buffer.text().as_bytes(),
            text.as_bytes(),
            "{name}: undo 1 回"
        );
    }
}

/// rustfmt で整形した本文（`TAKO_1899_RUSTFMT`。無ければ PATH の `rustfmt`）
fn rustfmt(path: &Path) -> String {
    let rustfmt = std::env::var("TAKO_1899_RUSTFMT").unwrap_or_else(|_| "rustfmt".into());
    let out = std::process::Command::new(&rustfmt)
        .args(["--edition", "2021", "--emit", "stdout", "--quiet"])
        .arg(path)
        .output()
        .expect("rustfmt を起こせる");
    assert!(out.status.success(), "rustfmt が失敗: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// 桁 0 の関数の頭の行ごとに区切った塊（頭の判定は #1496 の共有ヘルパ）
fn fn_chunks(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.split_inclusive('\n') {
        if tako_core::source_scan::is_top_level_fn_head(line) || out.is_empty() {
            out.push(String::new());
        }
        out.last_mut().unwrap().push_str(line);
    }
    out
}

/// 旧腕の子だけが立てる目印（値は入出力を置くディレクトリ）
const LEGACY_ARM_DIR: &str = "TAKO_1899_LEGACY_ARM_DIR";

/// 旧腕の子が起こし直すテスト（実の rust-analyzer の 1 本）
const REAL_RA_TEST: &str = "実の_rust_analyzer_の整形は割っても同じ本文で_undo_1_回で戻る";

/// 旧腕（`TAKO_1899_LEGACY=1` = 割らない）で当てた結果
struct LegacyArm {
    text: String,
    bytes: usize,
    changes: usize,
}

/// 答えたちを旧腕で当てた結果（答えの順）。旧腕は `TAKO_1899_LEGACY=1` を組んだ子（このテストを
/// `--exact` で起こし直したもの）で当てる。プロセスの env を `set_var` で切り替えると、並列に走る
/// ほかのテスト（10 万行の検査など）まで旧腕で当ててしまうのでやらない（#2400。子の起こし方は
/// `tests/common/detached_env.rs` と同じ形 = その 1 本を通したことまで見る）
fn legacy_arms(dir: &Path, path: &Path, text: &str, answers: &[&FormatAnswer]) -> Vec<LegacyArm> {
    let answers: Vec<Value> = answers
        .iter()
        .map(|answer| {
            answer
                .changes
                .iter()
                .map(|c| json!({ "start": c.range.start, "end": c.range.end, "text": c.text }))
                .collect()
        })
        .collect();
    let input = json!({ "path": path, "text": text, "answers": answers });
    std::fs::write(dir.join("legacy-in.json"), input.to_string()).unwrap();
    let exe = std::env::current_exe().expect("テストバイナリのパス");
    let out = std::process::Command::new(exe)
        .args([
            "--exact",
            REAL_RA_TEST,
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env(LEGACY_ARM_DIR, dir)
        .env("TAKO_1899_LEGACY", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("旧腕の子を起こせる");
    let log = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "旧腕の子が失敗した:\n{log}");
    assert!(
        log.contains("test result: ok. 1 passed"),
        "旧腕の子が {REAL_RA_TEST} を走らせていない（改名漏れ？）:\n{log}"
    );
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("legacy-out.json")).unwrap())
            .unwrap();
    result
        .as_array()
        .expect("旧腕の結果の列")
        .iter()
        .map(|r| LegacyArm {
            text: r["text"].as_str().unwrap().to_string(),
            bytes: r["bytes"].as_u64().unwrap() as usize,
            changes: r["changes"].as_u64().unwrap() as usize,
        })
        .collect()
}

/// 旧腕の子の側: 親が置いた答えたちを当て、本文・undo の履歴・当てた箇所の数を書いて返す
fn legacy_arm_child(dir: &Path) {
    assert_eq!(
        std::env::var("TAKO_1899_LEGACY").as_deref(),
        Ok("1"),
        "旧腕の子は TAKO_1899_LEGACY=1 で起こされる"
    );
    let input: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("legacy-in.json")).unwrap())
            .unwrap();
    let path = PathBuf::from(input["path"].as_str().unwrap());
    let text = input["text"].as_str().unwrap();
    let out: Vec<Value> = input["answers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|changes| {
            let answer = FormatAnswer {
                server: "rust-analyzer",
                changes: changes
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| tako_core::text_edit::TextChange {
                        range: c["start"].as_u64().unwrap() as usize
                            ..c["end"].as_u64().unwrap() as usize,
                        text: c["text"].as_str().unwrap().to_string(),
                    })
                    .collect(),
                dropped: 0,
            };
            let mut buffer = TextBuffer::from_text(path.clone(), text.to_string());
            let applied = apply(&mut buffer, &answer);
            json!({
                "text": buffer.text(),
                "bytes": buffer.undo_history_bytes(),
                "changes": applied.changes,
            })
        })
        .collect();
    std::fs::write(dir.join("legacy-out.json"), Value::from(out).to_string()).unwrap();
}

/// 受け入れ条件 3（実の rust-analyzer）: 整形が rustfmt とバイト一致し、割っても割らなくても
/// （旧腕 = `TAKO_1899_LEGACY=1` の子）当てた本文は同じで、undo の履歴は重くならず、undo 1 回で戻る。
/// 同じ rustfmt の出力を全文 1 つの書き換えとして受けた形（全文を返すサーバ）も測る。
/// `TAKO_1899_REAL_RA=<実行ファイルの実体>` で `--ignored` のときだけ（CI では走らない）
#[test]
#[ignore = "実の rust-analyzer が要る（TAKO_1899_REAL_RA）"]
fn 実の_rust_analyzer_の整形は割っても同じ本文で_undo_1_回で戻る() {
    if let Some(dir) = std::env::var_os(LEGACY_ARM_DIR) {
        legacy_arm_child(Path::new(&dir));
        return;
    }
    let Some(ra) = std::env::var_os("TAKO_1899_REAL_RA") else {
        eprintln!("TAKO_1899_REAL_RA が無いので測らない");
        return;
    };
    let ra = ra.to_string_lossy().into_owned();
    let scratch = Scratch::new("real");
    // 字下げ・空白・長い行の折り返し・空行の詰めが要る関数を 200 個。整形済みの形は rustfmt から採り、
    // 5 つに 1 つだけ崩れたまま残す（ほとんど整形済みのファイルの一部を直す = ふつうの整形）
    let raw: String = (0..200)
        .map(|i| {
            format!(
                "fn f{i}(a:u32,b:u32)->u32{{\nlet x=a+b;  \n  if x>3{{return x*2;}}\n\n\n    let long_name_{i} = some_function(alpha_value_{i}, beta_value_{i}, gamma_value_{i}, delta_value_{i});\nx+long_name_{i}\n}}\n"
            )
        })
        .chain(std::iter::once(
            "fn some_function(a:u32,b:u32,c:u32,d:u32)->u32{a+b+c+d}\nfn main(){}\n".to_string(),
        ))
        .collect();
    let raw_fns = fn_chunks(&raw);
    let formatted_fns = fn_chunks(&rustfmt(&scratch.write("raw.rs", &raw)));
    assert_eq!(raw_fns.len(), formatted_fns.len(), "関数の数");
    let text: String = raw_fns
        .iter()
        .zip(&formatted_fns)
        .enumerate()
        .map(|(i, (raw, formatted))| if i % 5 == 2 { raw } else { formatted }.clone())
        .collect();
    let path = scratch.write("src/main.rs", &text);
    let formatted = rustfmt(&path);
    let manager = LspManager::new(launcher_config(ra, Vec::new()));
    let answer = manager.format(&request(&path, &text)).expect("整形の答え");
    manager.shutdown_all(Duration::from_secs(5));
    assert!(
        !answer.changes.is_empty(),
        "整形の答えが空（rustfmt が読めなかった）"
    );
    let multi_line = answer
        .changes
        .iter()
        .filter(|c| text[c.range.clone()].contains('\n') && c.text.contains('\n'))
        .count();
    // 同じ rustfmt の出力を全文 1 つの書き換えとして受けた形（全文を返すサーバの形）
    let whole = FormatAnswer {
        server: "rust-analyzer",
        changes: vec![tako_core::text_edit::TextChange {
            range: 0..text.len(),
            text: formatted.clone(),
        }],
        dropped: 0,
    };
    let mut split = TextBuffer::from_text(path.clone(), text.clone());
    apply(&mut split, &answer);
    let mut whole_split = TextBuffer::from_text(path.clone(), text.clone());
    let whole_applied = apply(&mut whole_split, &whole);
    let [legacy, whole_legacy]: [LegacyArm; 2] =
        legacy_arms(&scratch.0, &path, &text, &[&answer, &whole])
            .try_into()
            .unwrap_or_else(|_| panic!("旧腕の結果は答えと同じ 2 つ"));
    assert_eq!(
        whole_legacy.changes, 1,
        "旧腕の子は全文 1 つの答えを割らずに当てる（TAKO_1899_LEGACY が子に効いている）"
    );
    assert!(
        whole_applied.changes > 1,
        "親は割る（親のプロセスの env は旧腕になっていない）"
    );
    eprintln!(
        "#1899 実の rust-analyzer: 答え {} 件（改行をまたぐ {multi_line}）/ undo の履歴 割る {} B・割らない {} B",
        answer.changes.len(),
        split.undo_history_bytes(),
        legacy.bytes
    );
    assert_eq!(
        split.text().as_bytes(),
        legacy.text.as_bytes(),
        "割っても割らなくても当てた本文は同じ"
    );
    assert_eq!(
        split.text(),
        formatted,
        "rust-analyzer の整形が rustfmt とバイト一致"
    );
    assert!(
        split.undo_history_bytes() <= legacy.bytes,
        "割って undo の履歴が重くなった"
    );
    assert!(split.undo());
    assert_eq!(split.text().as_bytes(), text.as_bytes(), "undo 1 回で戻る");

    eprintln!(
        "#1899 rustfmt の出力を全文 1 つで: 当てた箇所 割る {}・割らない {} / undo の履歴 割る {} B・割らない {} B",
        whole_applied.changes,
        whole_legacy.changes,
        whole_split.undo_history_bytes(),
        whole_legacy.bytes
    );
    assert_eq!(
        whole_legacy.text.as_bytes(),
        formatted.as_bytes(),
        "旧腕でも全文 1 つは rustfmt とバイト一致"
    );
    assert_eq!(
        whole_split.text(),
        formatted,
        "全文 1 つでも rustfmt とバイト一致"
    );
    assert!(
        whole_split.undo_history_bytes() < whole_legacy.bytes,
        "整形済みの関数を挟む全文の書き換えなのに割って軽くならない"
    );
    assert!(whole_split.undo());
    assert_eq!(whole_split.text().as_bytes(), text.as_bytes());
}
