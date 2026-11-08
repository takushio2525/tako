//! 実経路: `tako setup --answers @<file>` / `--answers -` が、Windows PowerShell 5.1 の書き出す
//! 答えのファイル（`Out-File` の既定 = BOM 付きの UTF-16LE / `-Encoding utf8` = BOM 付きの UTF-8）を
//! 素の UTF-8 と同じ答えとして読み、読めない形は何の形と見たかと直し方を表示言語で言って断る（Issue #2789）。
//!
//! 修正前（コード読み・macOS で実測）: 読み手が `read_to_string` → `serde_json::from_str` だったので、
//! UTF-16 は読み取りの段で `stream did not contain valid UTF-8`、UTF-8 の BOM は
//! `setup answers JSON が不正: expected value at line 1 column 1` で落ちた。
//!
//! **ビルドした `tako` をそのまま**叩く（GUI 無し）。答えは `launch_agent` だけを不正な日本語の値にして
//! おくので、setup は**何も書かずに**検証の断りで止まる。その断りが値（日本語）を名指していれば、
//! 復号 → JSON の解析 → 検証まで同じ答えで届いた証拠になる（符号化を取り違えると文字化けか、
//! 読み取り / JSON の断りになる）。置き場は使い捨て（`TAKO_DATA_DIR` = 表示言語の settings.json だけ・
//! `HOME` / `USERPROFILE` も使い捨て）で、断った後に置き場へ何も増えていないことも見る。
//!
//! 両 OS で走らせる（Windows の CI は PowerShell 5.1 の `Out-File` の実物を
//! `scripts/windows/test-setup-windows-2767.ps1` でも通す）。
//! A/B: `TAKO_2789_CLI_BIN=<修正前の tako>` で別のバイナリを叩ける（修正前は 6 ケースで落ちる）

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tako_core::test_residue::ScratchDir;

/// 叩くバイナリ（既定はこの木でビルドした `tako`。A/B は `TAKO_2789_CLI_BIN`）
fn tako() -> PathBuf {
    std::env::var_os("TAKO_2789_CLI_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_tako")))
}

/// 検証で止まる答え（`launch_agent` だけが不正。値は日本語 = 復号の取り違えが文字化けで出る）
const BOGUS_VALUE: &str = "起動しない🎉";

fn answers_text() -> String {
    format!(
        "{{\r\n  \"selected_agent\": \"claude\",\r\n  \"provider_plans\": {{ \"claude\": \"max\" }},\r\n  \
         \"install\": false,\r\n  \"launch_agent\": \"{BOGUS_VALUE}\"\r\n}}\r\n"
    )
}

/// 符号化の名前とバイト列（UTF-16 は BOM 付き = PowerShell の書き出しと同じ）
fn encodings(text: &str) -> Vec<(&'static str, Vec<u8>)> {
    let utf16 = |bom: [u8; 2], unit: fn(u16) -> [u8; 2]| {
        let mut out = bom.to_vec();
        out.extend(text.encode_utf16().flat_map(unit));
        out
    };
    let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
    utf8_bom.extend(text.as_bytes());
    vec![
        ("utf8", text.as_bytes().to_vec()),
        ("utf8-bom", utf8_bom),
        ("utf16le-bom", utf16([0xff, 0xfe], u16::to_le_bytes)),
        ("utf16be-bom", utf16([0xfe, 0xff], u16::to_be_bytes)),
    ]
}

struct Sandbox {
    dir: ScratchDir,
}

impl Sandbox {
    fn new(lang: &str) -> Self {
        let dir = ScratchDir::new("issue2789-answers");
        assert!(
            dir.path().starts_with(std::env::temp_dir()),
            "一時 dir の外に書こうとしている: {}",
            dir.path().display()
        );
        let sb = Self { dir };
        for d in [sb.data(), sb.home(), sb.files()] {
            std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{} を作れる: {e}", d.display()));
        }
        // 書かないと system = 実行環境のロケールで決まり、CI と手元で答えが変わる
        std::fs::write(
            sb.data().join("settings.json"),
            format!("{{\"language\":\"{lang}\"}}"),
        )
        .expect("settings.json を置ける");
        sb
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }
    fn home(&self) -> PathBuf {
        self.dir.path().join("home")
    }
    /// 答えのファイルの置き場（data / home の外 = 断った後の「何も増えていない」の比べに混ざらない）
    fn files(&self) -> PathBuf {
        self.dir.path().join("answers")
    }

    /// 隔離した `tako setup --yes --answers <answers>`（標準入力へ `stdin` を流す）。
    /// 戻り値は（終了コード, stdout + stderr）
    fn setup(&self, answers: &str, stdin: &[u8]) -> (Option<i32>, String) {
        let mut command = Command::new(tako());
        command
            .args(["setup", "--yes", "--answers", answers])
            .current_dir(self.dir.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("TAKO_") {
                command.env_remove(key);
            }
        }
        command
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env("TAKO_ISOLATED", "1")
            .env("TAKO_DATA_DIR", self.data())
            .env("TAKO_DISCOVERY_DIR", self.dir.path().join("disc"))
            .env("TAKO_SOCKET", self.dir.path().join("none.sock"))
            .env("HOME", self.home())
            .env("USERPROFILE", self.home());
        let mut child = command.spawn().expect("tako を起こせる");
        // 読み元がファイル・引数のときは標準入力を読まないので、閉じた口への書き込みの失敗は無視する
        let _ = child.stdin.take().expect("stdin").write_all(stdin);
        let out = child.wait_with_output().expect("tako の終わりを待てる");
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        (out.status.code(), text)
    }

    /// 置き場（data / home）のファイルの一覧（断った後に増えていないことを見る）
    fn written(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for root in [self.data(), self.home()] {
            walk(&root, &mut out);
        }
        out.sort();
        out
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// 受け入れ条件 1 / 2 の CLI の口: 4 つの符号化 × `@<file>` / `-` が、同じ答え（`launch_agent` の
/// 日本語の値まで）として検証へ届き、何も書かずに断る
#[test]
fn issue2789_4つの符号化の答えはファイルと標準入力で同じ答えとして検証へ届く() {
    let sb = Sandbox::new("ja");
    let before = sb.written();
    let want = format!("launch_agent は claude / codex / agy / none のいずれかです: {BOGUS_VALUE}");
    let mut failures = Vec::new();
    for (name, bytes) in encodings(&answers_text()) {
        let file = sb.files().join(format!("{name}.json"));
        std::fs::write(&file, &bytes).expect("答えのファイルを書ける");
        for (via, answers, stdin) in [
            ("@file", format!("@{}", file.display()), Vec::new()),
            ("-", "-".to_string(), bytes.clone()),
        ] {
            let (code, out) = sb.setup(&answers, &stdin);
            if code != Some(1) || !out.contains(&want) {
                failures.push(format!(
                    "{name} を --answers {via} で渡すと、同じ答えの検証の断りにならない\
                     （終了コード {code:?}）:\n{out}"
                ));
            }
        }
    }
    assert_eq!(
        sb.written(),
        before,
        "断ったのに置き場のファイルが増えた（書く前に断る）"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// 受け入れ条件 3 の CLI の口: BOM の無い UTF-16・空・BOM だけは、何の形と見たかと直し方を
/// 表示言語で言って断る（英語表示の断りに日本語が混ざらない）
#[test]
fn issue2789_読めない形は表示言語で何の形と見たかと直し方を言って断る() {
    let le_no_bom: Vec<u8> = answers_text()
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let cases: [(&str, &str, &str, Vec<u8>, &[&str]); 6] = [
        (
            "en",
            "BOM の無い UTF-16LE のファイル",
            "file",
            le_no_bom.clone(),
            &[
                "cannot read the setup answers (file ",
                "it looks like UTF-16LE without a byte order mark",
                "Save it again as UTF-8 (in PowerShell: `Out-File -Encoding utf8`)",
            ],
        ),
        (
            "ja",
            "BOM の無い UTF-16LE のファイル",
            "file",
            le_no_bom.clone(),
            &[
                "setup answers（ファイル ",
                "BOM の無い UTF-16LE に見えます",
                "UTF-8 で保存し直してください（PowerShell なら `Out-File -Encoding utf8`）",
            ],
        ),
        (
            "en",
            "BOM の無い UTF-16LE の標準入力",
            "-",
            le_no_bom,
            &[
                "cannot read the setup answers (standard input)",
                "Pipe it as UTF-8, or save it to a file and pass `--answers @<file>`",
            ],
        ),
        (
            "en",
            "空ファイル",
            "file",
            Vec::new(),
            &["the setup answers (file ", "are empty"],
        ),
        (
            "ja",
            "BOM だけのファイル",
            "file",
            vec![0xef, 0xbb, 0xbf],
            &["setup answers（ファイル ", "が空です"],
        ),
        (
            "en",
            "奇数バイトの UTF-16LE の標準入力",
            "-",
            vec![0xff, 0xfe, 0x7b, 0x00, 0x7d],
            &["UTF-16LE with a byte order mark, but its length is odd (5 bytes)"],
        ),
    ];
    let mut failures = Vec::new();
    for (lang, name, via, bytes, wants) in cases {
        let sb = Sandbox::new(lang);
        let before = sb.written();
        let (answers, stdin) = if via == "file" {
            let file = sb.files().join("answers.json");
            std::fs::write(&file, &bytes).expect("答えのファイルを書ける");
            (format!("@{}", file.display()), Vec::new())
        } else {
            ("-".to_string(), bytes)
        };
        let (code, out) = sb.setup(&answers, &stdin);
        let missing: Vec<&&str> = wants.iter().filter(|w| !out.contains(**w)).collect();
        if code != Some(1) || !missing.is_empty() {
            failures.push(format!(
                "[{lang}] {name}: 終了コード {code:?}・足りない文 {missing:?}:\n{out}"
            ));
        }
        if lang == "en" {
            if let Some(c) = out
                .chars()
                .find(|c| matches!(*c as u32, 0x3040..=0x30ff | 0x4e00..=0x9fff))
            {
                failures.push(format!(
                    "[en] {name}: 英語表示の断りに日本語（{c}）が混ざる:\n{out}"
                ));
            }
        }
        if sb.written() != before {
            failures.push(format!(
                "[{lang}] {name}: 断ったのに置き場のファイルが増えた"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

/// 受け入れ条件 2 の番犬: Windows の CI の通し検査（`scripts/windows/test-setup-windows-2767.ps1`。
/// macOS では 1 行も動かない）が、Windows PowerShell 5.1 の `Out-File` の既定と `-Encoding utf8` で
/// 書いた答えを `--answers @<file>` に渡す段を持ち続ける
#[test]
fn issue2789_windowsのciは51のout_fileで書いた答えを通す段を持つ() {
    const SCRIPT: &str = "scripts/windows/test-setup-windows-2767.ps1";
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let text = std::fs::read_to_string(root.join(SCRIPT))
        .unwrap_or_else(|e| panic!("{SCRIPT} を読めない: {e}"));
    for (needle, why) in [
        (
            r"System32\WindowsPowerShell\v1.0\powershell.exe",
            "pwsh 7 ではなく Windows PowerShell 5.1 の実物で書く",
        ),
        ("Encoding = ''", "Out-File の既定（BOM 付きの UTF-16LE）の段"),
        ("Encoding = ' -Encoding utf8'", "-Encoding utf8（BOM 付きの UTF-8）の段"),
        ("Head = 'FF-FE'", "既定の書き出しが UTF-16LE の BOM で始まる前提を確かめる"),
        ("Head = 'EF-BB-BF'", "-Encoding utf8 が UTF-8 の BOM で始まる前提を確かめる"),
        (
            "Invoke-Tako @('setup', '--yes', '--answers', \"@$ps51File\")",
            "5.1 で書いた答えを --answers @<file> に渡す",
        ),
        ("BOM の無い UTF-16LE に見えます", "読めない形の断りを確かめる"),
    ] {
        assert!(
            text.contains(needle),
            "{SCRIPT} から #2789 の段が外れた: 「{needle}」が無い（{why}）"
        );
    }
}
