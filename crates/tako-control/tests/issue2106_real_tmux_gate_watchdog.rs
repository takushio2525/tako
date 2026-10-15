//! 番犬: 実 tmux のテストが CI で黙って skip しない（Issue #2106）
//!
//! 実 tmux を使うテストは tmux が無いと skip して `ok` と出る。cargo test は通ったテストの
//! stderr を捨てるので skip はログに残らず、macOS の CI ランナー（image に tmux が無い）では
//! 実 tmux の番犬が 1 度も走っていなかった。「前提が揃わないので飛ばす」判断は
//! `tako_core::tmux_test_gate` の 1 実装へ寄せ、CI の macOS ジョブは tmux を入れたうえで
//! `TAKO_TEST_REQUIRE_TMUX=1`（skip を panic に変える）で走らせる。ここではその 3 点を見張る:
//!
//! 1. 門を通らない skip（`eprintln!("skip: tmux …")` の直書き）が戻っていない
//! 2. env の効き目: 必須の env の下では tmux が無いと落ち、env が無ければ従来どおり skip で
//!    通る。門を通ったテストは台帳に名前が載る（このテストバイナリを子の役で起こし直して確かめる）
//! 3. CI の配線: macOS ジョブは tmux を入れる段 → 必須の env つきの `cargo test` → 件数の報告の
//!    順に並び、Windows ジョブは必須の env を立てない。報告スクリプトは 0 件で落ちる

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tako_core::tmux_test_gate::{LEDGER_ENV, REQUIRE_ENV};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

// ---------------------------------------------------------------------------
// 1. 門を通らない skip の直書き
// ---------------------------------------------------------------------------

/// 門の実装そのもの（skip の文言を組む唯一の場所）
const GATE: &str = "crates/tako-core/src/tmux_test_gate.rs";

/// この番犬（検出器の見本として skip の文言を持つ）
const SELF: &str = "crates/tako-control/tests/issue2106_real_tmux_gate_watchdog.rs";

/// 門を通さない理由があるファイルと、その中の件数（増えたら落とす）
const KNOWN: &[(&str, usize, &str)] = &[
    (
        "crates/tako-core/tests/shell_integration_powershell.rs",
        2,
        "`#![cfg(windows)]` の psmux のテスト。「既に tmux の中で走っている」= 入れ子の検出で、tmux の不在ではない",
    ),
    (
        "crates/tako-core/tests/issue1940_launch_e2e.rs",
        1,
        "`#[ignore]` の実測用（CI では走らない）。OS が Windows のときの断り書き",
    ),
    (
        "crates/tako-app/src/main.rs",
        1,
        "セルフテスト（`TAKO_SELF_TEST=1` の GUI）の項目。cargo test ではない",
    ),
];

/// `eprintln!(` の直後の文字列リテラルの中身（改行を挟んだ形も拾う）と、その行番号
fn eprintln_literals(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut rest = src;
    let mut offset = 0;
    while let Some(at) = rest.find("eprintln!(") {
        let start = offset + at;
        let after = &src[start + "eprintln!(".len()..];
        let trimmed = after.trim_start();
        if let Some(body) = trimmed.strip_prefix('"') {
            let mut lit = String::new();
            let mut escaped = false;
            for c in body.chars() {
                if escaped {
                    escaped = false;
                    lit.push(c);
                    continue;
                }
                match c {
                    '\\' => escaped = true,
                    '"' => break,
                    _ => lit.push(c),
                }
            }
            let line = src[..start].matches('\n').count() + 1;
            out.push((line, lit));
        }
        offset = start + "eprintln!(".len();
        rest = &src[offset..];
    }
    out
}

/// tmux の skip を告げる文言か（tmux に触れ、かつ skip / スキップを言う）
fn is_tmux_skip(lit: &str) -> bool {
    let lower = lit.to_ascii_lowercase();
    lower.contains("tmux") && (lower.contains("skip") || lit.contains("スキップ"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn 検出器は1行と改行を挟んだ形のtmuxのskipを拾う() {
    let src = "fn a() {\n    eprintln!(\"skip: tmux が無い環境\");\n}\nfn b() {\n    eprintln!(\n        \"[e2e] 本物の tmux が無いのでスキップ\"\n    );\n    eprintln!(\"skip: psmux が無い環境\");\n    eprintln!(\"tmux を起動した\");\n}\n";
    let hits: Vec<_> = eprintln_literals(src)
        .into_iter()
        .filter(|(_, lit)| is_tmux_skip(lit))
        .map(|(line, _)| line)
        .collect();
    assert_eq!(
        hits,
        vec![2, 5],
        "1 行の形と複数行の形だけを拾い（行番号は eprintln! の行）、psmux と skip でない行は拾わない"
    );
    assert!(
        is_tmux_skip("SKIP: 本物の tmux が無い"),
        "大文字の SKIP も拾う"
    );
}

#[test]
fn 門を通らないtmuxのskipが無い() {
    let root = repo_root();
    let mut files = Vec::new();
    rust_files(&root.join("crates"), &mut files);
    assert!(
        files.len() > 200,
        "走査が空振りしている（crates の .rs が {} 本しか見えない）",
        files.len()
    );
    let mut violations = Vec::new();
    let mut known_hits: Vec<(&str, usize)> = KNOWN.iter().map(|(f, _, _)| (*f, 0)).collect();
    let mut gate_seen = false;
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel == GATE {
            gate_seen = true;
            continue;
        }
        if rel == SELF {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        for (line, lit) in eprintln_literals(&src) {
            if !is_tmux_skip(&lit) {
                continue;
            }
            if let Some(slot) = known_hits.iter_mut().find(|(f, _)| *f == rel) {
                slot.1 += 1;
                continue;
            }
            violations.push(format!("{rel}:{line}: {lit}"));
        }
    }
    assert!(
        gate_seen,
        "門の実装 {GATE} が走査に出てこない（置き場が変わった？）"
    );
    assert!(
        violations.is_empty(),
        "実 tmux のテストの skip を門を通さずに書いている（CI で黙って skip して ok になる = #2106）。\
         `if !tako_core::tmux_test_gate::container() {{ return; }}`（器）/ `real_tmux()`（本物の tmux）/ \
         `require(..)` / `skip(..)` を通す:\n{}",
        violations.join("\n")
    );
    for ((file, want, why), (_, got)) in KNOWN.iter().zip(&known_hits) {
        assert_eq!(
            got, want,
            "{file} の門を通さない skip が {got} 件（宣言は {want} 件。理由: {why}）。\
             増えたなら門を通す、減ったなら KNOWN を直す"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. env の効き目（このテストバイナリを子の役で起こし直す）
// ---------------------------------------------------------------------------

const ROLE_ENV: &str = "TAKO_2106_ROLE";

/// 子の役。親が env で選んだ門を 1 回通るだけ（直接 `cargo test` したときは何もしない）
#[test]
#[ignore = "issue2106 の番犬が自分を起こし直したときだけ動く"]
fn 子の役() {
    match std::env::var(ROLE_ENV).as_deref() {
        Ok("container") => {
            if !tako_core::tmux_test_gate::container() {
                return;
            }
            println!("ROLE_RAN");
        }
        Ok("real") => {
            if !tako_core::tmux_test_gate::real_tmux() {
                return;
            }
            println!("ROLE_RAN");
        }
        Ok("available") => {
            assert!(tako_core::tmux_test_gate::require(
                true,
                "（揃っている前提）"
            ));
            println!("ROLE_RAN");
        }
        _ => {}
    }
}

/// 子の役を起こす。`absent` なら tmux を見えなくする（PATH から外し、`TAKO_TMUX_BIN` を
/// 実在しない場所へ向ける = tmux の解決順の全段を塞ぐ）
fn run_role(role: &str, absent: bool, require: bool, ledger: Option<&Path>) -> Output {
    let exe = std::env::current_exe().expect("テストバイナリ");
    let mut cmd = Command::new(exe);
    cmd.args(["--exact", "子の役", "--ignored", "--nocapture"])
        .env(ROLE_ENV, role)
        .env_remove(REQUIRE_ENV)
        .env_remove(LEDGER_ENV)
        .env_remove("TAKO_BACKEND")
        .env_remove("TAKO_PSMUX_BIN")
        .stdin(Stdio::null());
    if absent {
        let missing =
            std::env::temp_dir().join(format!("tako-2106-no-tmux-{}", std::process::id()));
        cmd.env("TAKO_TMUX_BIN", missing.join("tmux"))
            .env("PATH", "/usr/bin:/bin")
            // ログインシェル経由の解決（`command -v tmux`）も塞ぐ
            .env("SHELL", "/bin/sh");
    }
    if require {
        cmd.env(REQUIRE_ENV, "1");
    }
    if let Some(path) = ledger {
        cmd.env(LEDGER_ENV, path);
    }
    cmd.output().expect("子の役を起こす")
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
#[cfg(unix)]
fn 必須のenvの下ではtmuxが無いと落ちる() {
    for role in ["container", "real"] {
        let out = run_role(role, true, true, None);
        let all = text(&out);
        assert!(
            !out.status.success(),
            "[{role}] {REQUIRE_ENV}=1 なのに tmux 無しで通った（CI で黙って skip する形）:\n{all}"
        );
        assert!(
            all.contains("skip を許さない") && all.contains(REQUIRE_ENV),
            "[{role}] 落ち方が門の panic ではない:\n{all}"
        );
        assert!(
            !all.contains("ROLE_RAN"),
            "[{role}] tmux が無いのに本体まで進んだ:\n{all}"
        );
    }
}

#[test]
#[cfg(unix)]
fn envが無ければtmuxが無くても従来どおりskipで通る() {
    for role in ["container", "real"] {
        let out = run_role(role, true, false, None);
        let all = text(&out);
        assert!(
            out.status.success(),
            "[{role}] {REQUIRE_ENV} 無しで落ちた（tmux の無い手元の cargo test が赤くなる）:\n{all}"
        );
        assert!(
            all.contains("skip: "),
            "[{role}] skip の印が出ていない:\n{all}"
        );
        assert!(
            !all.contains("ROLE_RAN"),
            "[{role}] tmux が無いのに本体まで進んだ:\n{all}"
        );
    }
}

#[test]
fn 門を通ったテストは台帳に名前が載る() {
    // スコープを抜けたら（panic でも）消える置き場（#1312）
    let dir = tako_core::test_residue::ScratchDir::new("tako-2106-ledger");
    let ledger = dir.path().join("ledger.tsv");
    let out = run_role("available", false, true, Some(&ledger));
    let all = text(&out);
    let written = std::fs::read_to_string(&ledger).unwrap_or_default();
    assert!(out.status.success(), "揃っている前提で落ちた:\n{all}");
    assert!(all.contains("ROLE_RAN"), "本体まで進んでいない:\n{all}");
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines.len(), 1, "門を 1 回通ったら台帳は 1 行: {written:?}");
    let (name, at) = lines[0].split_once('\t').expect("台帳の行はタブ区切り");
    assert_eq!(
        name, "子の役",
        "台帳のテスト名がスレッド名（= テスト名）でない: {written:?}"
    );
    assert!(
        at.replace('\\', "/")
            .contains("issue2106_real_tmux_gate_watchdog.rs:"),
        "台帳の場所が門を呼んだ行でない: {written:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. CI の配線
// ---------------------------------------------------------------------------

/// ci.yml の 1 ジョブの本文（`  <id>:` の行から次の 2 字下げのジョブの手前まで）
fn job_block(yml: &str, id: &str) -> String {
    let head = format!("  {id}:");
    let mut lines = yml.lines().skip_while(|l| *l != head);
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("ci.yml にジョブ {id} が無い"));
    let mut out = format!("{first}\n");
    for line in lines {
        let top_level = !line.is_empty() && !line.starts_with(' ');
        let next_job = line.len() > 2
            && line.starts_with("  ")
            && !line[2..].starts_with(' ')
            && !line[2..].starts_with('#');
        if top_level || next_job {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// ジョブの本文を steps（`      - ` で始まる塊）へ割る
fn steps(block: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in block.lines() {
        if line.starts_with("      - ") {
            out.push(String::new());
        }
        if let Some(cur) = out.last_mut() {
            // コメント行は配線ではない（コメントにだけ env 名が残った形で緑にしない）
            if !line.trim_start().starts_with('#') {
                cur.push_str(line);
                cur.push('\n');
            }
        }
    }
    out
}

#[test]
fn ciのmacosジョブはtmuxを入れて必須のenvでテストし件数を出す() {
    let yml = read(".github/workflows/ci.yml");
    let macos = steps(&job_block(&yml, "macos"));
    let find = |pred: &dyn Fn(&str) -> bool, what: &str| -> usize {
        macos
            .iter()
            .position(|s| pred(s))
            .unwrap_or_else(|| panic!("macOS ジョブに {what} の段が無い（#2106）"))
    };
    let install = find(&|s: &str| s.contains("brew install tmux"), "tmux を入れる");
    let test = find(
        &|s: &str| s.contains("run: cargo test --workspace"),
        "cargo test --workspace",
    );
    let report = find(
        &|s: &str| s.contains("scripts/ci-real-tmux-report.sh"),
        "件数の報告",
    );
    assert!(
        install < test && test < report,
        "段の順が tmux を入れる（{install}）→ テスト（{test}）→ 報告（{report}）でない"
    );
    let test_step = &macos[test];
    assert!(
        test_step.contains(&format!("{REQUIRE_ENV}: \"1\"")),
        "テストの段に {REQUIRE_ENV}: \"1\" が無い（tmux が消えても黙って skip する）:\n{test_step}"
    );
    let ledger_line = test_step
        .lines()
        .find(|l| l.trim_start().starts_with(&format!("{LEDGER_ENV}:")))
        .unwrap_or_else(|| panic!("テストの段に {LEDGER_ENV} が無い:\n{test_step}"));
    let ledger_file = ledger_line
        .rsplit('/')
        .next()
        .expect("台帳のファイル名")
        .trim();
    assert!(
        macos[report].contains(ledger_file),
        "報告の段が読む台帳（{}）がテストの段の台帳（{ledger_file}）と違う",
        macos[report].trim()
    );

    let windows = job_block(&yml, "windows");
    assert!(
        !windows.contains(REQUIRE_ENV),
        "Windows ジョブが {REQUIRE_ENV} を立てている（本物の tmux が無いので実 tmux のテストが全部落ちる）"
    );
}

#[test]
fn ciの段の切り出しはジョブとstepsの境界を取り違えない() {
    let yml = "jobs:\n  a:\n    steps:\n      # コメント\n      - name: x\n        run: echo a\n      - name: y\n        env:\n          K: \"1\"\n        run: cargo test\n  b:\n    steps:\n      - run: echo b\n";
    let a = job_block(yml, "a");
    assert!(a.contains("echo a") && !a.contains("echo b"), "{a}");
    let s = steps(&a);
    assert_eq!(s.len(), 2, "{s:?}");
    assert!(
        s[1].contains("K: \"1\"") && s[1].contains("run: cargo test"),
        "{s:?}"
    );
    assert!(job_block(yml, "b").contains("echo b"));
}

#[test]
#[cfg(unix)]
fn 報告スクリプトは件数と名前を出し0件で落ちる() {
    // スコープを抜けたら（panic でも）消える置き場（#1312）
    let scratch = tako_core::test_residue::ScratchDir::new("tako-2106-report");
    let dir = scratch.path();
    let script = repo_root().join("scripts/ci-real-tmux-report.sh");
    // CI の macOS ランナーの既定ロケール（en_US.UTF-8）で走らせる。この照合順序では
    // 日本語のテスト名どうしが「等しい」扱いになり、`sort -u` が別のテストを 1 本へ潰した
    // （#2106 の初回の CI で 52 回 → 51 本と数え、issue1293 の 1 本が名前ごと消えた）
    let run = |ledger: &Path| {
        Command::new("bash")
            .arg(&script)
            .arg(ledger)
            .env("LC_ALL", "en_US.UTF-8")
            .env("LANG", "en_US.UTF-8")
            .output()
            .expect("bash を起こせる")
    };

    let filled = dir.join("filled.tsv");
    std::fs::write(
        &filled,
        "a::t1\tcrates/x/src/a.rs:10\na::t1\tcrates/x/src/a.rs:10\nt2\tcrates/y/tests/b.rs:5\n\
         issue1_会話ログの上の下見\tcrates/y/tests/c.rs:7\nissue1_番号指定は選択肢へ\tcrates/y/tests/c.rs:9\n",
    )
    .unwrap();
    let ok = run(&filled);
    let ok_text = text(&ok);
    assert!(ok.status.success(), "台帳があるのに落ちた:\n{ok_text}");
    assert!(
        ok_text.contains("4 本（門を通った回数 5"),
        "同じテストの重複だけを 1 本に数え、日本語の別のテストは潰さない件数が出ていない:\n{ok_text}"
    );
    for name in [
        "a::t1",
        "t2",
        "issue1_会話ログの上の下見",
        "issue1_番号指定は選択肢へ",
    ] {
        assert!(
            ok_text.contains(name),
            "テスト名 {name} が出ていない:\n{ok_text}"
        );
    }

    let empty = dir.join("empty.tsv");
    std::fs::write(&empty, "").unwrap();
    let missing = dir.join("missing.tsv");
    for ledger in [&empty, &missing] {
        let out = run(ledger);
        assert_eq!(
            out.status.code(),
            Some(1),
            "台帳が空 / 無いのに exit 1 でない（配線切れが緑のまま見えなくなる）:\n{}",
            text(&out)
        );
    }
}
