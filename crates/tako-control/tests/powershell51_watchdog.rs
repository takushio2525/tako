//! 番犬: リポジトリの .ps1 は Windows PowerShell 5.1 で「読める」か「理由を出して止まる」かを
//! 先頭で決めている（Issue #2329）
//!
//! 5.1 は BOM の無い `.ps1` を ANSI コードページ（英語版 = CP1252 / 日本語版 = CP932）として読む。
//! BOM の無い UTF-8 で日本語を書くと、5.1 では文字列やコメントの境目が崩れて構文エラーになる
//! （#2329 の着手時の `installer/windows/lib/verify-assets.ps1` は CP1252 で 22 件 / CP932 で 27 件）。
//! リリースの経路は pwsh 7 だけなので、手元でも CI でも気づけなかった。そこで .ps1 はどれも
//! 次のどちらかを宣言する:
//!
//! - **(A) 5.1 でも読める**: UTF-8 with BOM で保存する。利用者が自分の手で走らせうるものはこちら。
//!   5.1（.NET Framework 4.x）に無い API（[`PWSH7_ONLY`]）も呼ばない
//! - **(B) pwsh 7 必須**: 1 行目を `#Requires -Version 7.0` にする。開発・CI・リリースだけが
//!   走らせるものはこちら。5.1 は構文解析のエラーより先に #Requires を確かめ、理由を出して止まる。
//!   1 行目に置くのは、それより前に非 ASCII があると ANSI の復号で行ごと食われうるため
//!
//! どちらでもない .ps1 はファイルを名指しして落ちる（新しく足したものにも同じ規則が当たる）。
//! 例外は [`EXEMPT`] の表だけで、理由と前提（ASCII だけ）をここで確かめる。
//! ファイルごとの判断と根拠は `.agent/conventions.md` の #2329 の節。
//!
//! 実際の読み方は `scripts/windows/check-powershell51.ps1` が確かめる。pwsh があればこの番犬が
//! 5.1 の読み方を写して（BOM が無ければ CP1252 と CP932 で復号して構文解析）走らせ、CI の
//! windows ジョブは Windows PowerShell 5.1 の実物で走らせる（複製を起こして止まるところまで見る）。

use std::path::{Path, PathBuf};
use std::process::Command;

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// 実際の読み方を確かめるスクリプト（pwsh 7 では 5.1 の読み方を写し、5.1 では実物で読む）
const CHECKER: &str = "scripts/windows/check-powershell51.ps1";

/// 走査で外す dir（生成物・依存物。`scripts/windows/check-powershell51.ps1` の `$TakoSkipDirs` と同じ）
const SKIP_DIRS: &[&str] = &["target", ".git", "node_modules", "dist", ".wrangler"];

/// BOM も #Requires も持たないことを認める .ps1（リポジトリのルートからの相対パス, 理由）
const EXEMPT: &[(&str, &str)] = &[(
    "crates/tako-core/shell-integration/tako.ps1",
    "本体へ include_str! で埋め込むテンプレートで、利用者の手元へは write_powershell_script が \
     BOM 付き UTF-8 で書き出す（scripts/test-setup-shell-integration-1504.sh が確かめる）。\
     リポジトリの写しに BOM を付けると書き出しで BOM が二重になる",
)];

/// 5.1 に無い API・引数（綴り, 理由と 5.1 での代わり）。(A) の側が呼んだら名指しする
const PWSH7_ONLY: &[(&str, &str)] = &[
    (
        "GetRelativePath",
        "[System.IO.Path]::GetRelativePath は .NET Core 2.0 から（.NET Framework 4.x に無い）。\
         根の FullName の長さぶんを Substring で落とす",
    ),
    (
        "-AsHashtable",
        "ConvertFrom-Json -AsHashtable は PowerShell 6.0 から",
    ),
    (
        "-AsByteStream",
        "-AsByteStream は PowerShell 6.0 から（5.1 は -Encoding Byte）",
    ),
    (
        "-Parallel",
        "ForEach-Object -Parallel は PowerShell 7.0 から",
    ),
    ("$PSStyle", "$PSStyle は PowerShell 7.2 から"),
];

/// 1 本の .ps1 が 5.1 に対して何を宣言しているか
#[derive(Debug, Clone, Copy, PartialEq)]
enum Stance {
    /// (A) UTF-8 with BOM = 5.1 も UTF-8 として読む
    Bom,
    /// (B) 1 行目の `#Requires -Version 7` 以上 = 5.1 は理由を出して止まる
    Requires7,
    /// [`EXEMPT`] の表にある
    Exempt,
    /// どちらでもない（5.1 では黙って化けうる）
    Neither,
}

impl Stance {
    /// `check-powershell51.ps1` が返すべき判定
    fn expected_verdict(self) -> &'static str {
        match self {
            Stance::Bom | Stance::Exempt => "readable",
            Stance::Requires7 => "stops",
            Stance::Neither => "broken",
        }
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// リポジトリの .ps1 / .psm1 を集める（ルートからの相対パス・区切りは `/`, 中身）
fn ps1_files() -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skip = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| SKIP_DIRS.contains(&n));
                if !skip {
                    walk(&path, out);
                }
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("ps1") || e.eq_ignore_ascii_case("psm1"))
            {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut paths = Vec::new();
    walk(&root, &mut paths);
    let mut out: Vec<(String, Vec<u8>)> = paths
        .iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap_or(p)
                .display()
                .to_string()
                .replace('\\', "/");
            let bytes =
                std::fs::read(p).unwrap_or_else(|e| panic!("{} を読めない: {e}", p.display()));
            (rel, bytes)
        })
        .collect();
    out.sort();
    assert!(!out.is_empty(), "リポジトリに .ps1 が 1 つも見つからない");
    out
}

/// `#Requires -Version N[.M]` の主版数（その形でなければ None）
fn requires_major(line: &str) -> Option<u32> {
    let mut words = line.split_whitespace();
    if !words.next()?.eq_ignore_ascii_case("#requires") {
        return None;
    }
    while let Some(w) = words.next() {
        if w.eq_ignore_ascii_case("-version") {
            return words.next()?.split('.').next()?.parse().ok();
        }
    }
    None
}

fn stance(rel: &str, bytes: &[u8]) -> Stance {
    let has_bom = bytes.starts_with(BOM);
    let body = if has_bom { &bytes[BOM.len()..] } else { bytes };
    let first = body.split(|&b| b == b'\n').next().unwrap_or_default();
    let first = String::from_utf8_lossy(first);
    if requires_major(first.trim_end_matches('\r')).is_some_and(|m| m >= 7) {
        return Stance::Requires7;
    }
    if has_bom {
        return Stance::Bom;
    }
    if EXEMPT.iter().any(|(p, _)| *p == rel) {
        return Stance::Exempt;
    }
    Stance::Neither
}

/// 規則に反していれば、ファイルを名指しした 1 行を返す
fn violation(rel: &str, bytes: &[u8]) -> Option<String> {
    match stance(rel, bytes) {
        Stance::Bom | Stance::Requires7 => None,
        Stance::Exempt if bytes.is_ascii() => None,
        Stance::Exempt => Some(format!(
            "{rel}: 例外の表（EXEMPT）にあるが ASCII 以外が混ざった。例外の前提が崩れたので、\
             BOM の無いまま 5.1 で化ける"
        )),
        Stance::Neither => Some(format!(
            "{rel}: UTF-8 with BOM でも、1 行目が #Requires -Version 7 でもない\
             （Windows PowerShell 5.1 は BOM の無い .ps1 を ANSI として読み、日本語で構文が崩れる）"
        )),
    }
}

/// コメント（行頭の `#` と `<# … #>`）を空にした本文（行番号は保つ）
fn code_lines(text: &str) -> Vec<String> {
    let mut in_block = false;
    text.lines()
        .map(|line| {
            let t = line.trim_start_matches('\u{feff}').trim_start();
            if in_block {
                in_block = !t.contains("#>");
                return String::new();
            }
            if t.starts_with("<#") {
                in_block = !t.contains("#>");
                return String::new();
            }
            if t.starts_with('#') {
                return String::new();
            }
            line.to_string()
        })
        .collect()
}

/// (A) の側が 5.1 に無い API を呼んでいる箇所（file:line と理由）
fn pwsh7_only_calls(rel: &str, bytes: &[u8]) -> Vec<String> {
    if !matches!(stance(rel, bytes), Stance::Bom | Stance::Exempt) {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    for (i, line) in code_lines(&text).iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        for (word, why) in PWSH7_ONLY {
            if lower.contains(&word.to_ascii_lowercase()) {
                out.push(format!("{rel}:{}: {word} — {why}", i + 1));
            }
        }
    }
    out
}

#[test]
fn ps1は5_1で読めるか理由を出して止まるかを先頭で決めている() {
    let files = ps1_files();
    let mut problems: Vec<String> = files
        .iter()
        .filter_map(|(rel, bytes)| violation(rel, bytes))
        .collect();
    // 消えた・名前の変わったファイルを例外の表に残さない（表が規則の抜け道にならないように）
    for (path, _) in EXEMPT {
        if !files.iter().any(|(rel, _)| rel == path) {
            problems.push(format!(
                "{path}: 例外の表（EXEMPT）にあるがリポジトリに無い"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "Windows PowerShell 5.1 に対する宣言が無い .ps1 がある（#2329）。\n\
         利用者が自分の手で走らせうるものは UTF-8 with BOM で保存し、開発・CI・リリースだけが\n\
         走らせるものは 1 行目を `#Requires -Version 7.0` にする（.agent/conventions.md の #2329 の節）:\n{}",
        problems.join("\n")
    );
}

/// 検出力: 実物の BOM / #Requires を外す・例外のファイルへ日本語を足すと、そのファイルを名指しする
#[test]
fn ps1の番犬はbomや_requiresを外す注入でそのファイルを名指しする() {
    let mut exercised = [0usize; 3];
    for (rel, bytes) in ps1_files() {
        let injected: Option<Vec<u8>> = match stance(&rel, &bytes) {
            Stance::Bom => {
                exercised[0] += 1;
                Some(bytes[BOM.len()..].to_vec())
            }
            Stance::Requires7 => {
                exercised[1] += 1;
                let cut = bytes
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |n| n + 1);
                // BOM ごと 1 行目を落とすと BOM も消えるので、BOM があれば残す
                let mut v = if bytes.starts_with(BOM) {
                    BOM.to_vec()
                } else {
                    Vec::new()
                };
                v.extend_from_slice(&bytes[cut..]);
                Some(v)
            }
            Stance::Exempt => {
                exercised[2] += 1;
                let mut v = bytes.clone();
                v.extend_from_slice("# 日本語のコメント\n".as_bytes());
                Some(v)
            }
            Stance::Neither => None,
        };
        let Some(injected) = injected else { continue };
        // BOM と #Requires の両方を持つファイルは片方を外しても規則を満たすので対象外
        if matches!(stance(&rel, &injected), Stance::Bom | Stance::Requires7) {
            continue;
        }
        let got = violation(&rel, &injected);
        assert!(
            got.as_deref().is_some_and(|m| m.starts_with(&format!("{rel}:"))),
            "{rel} の BOM / #Requires を外した（例外なら日本語を足した）のに名指ししなかった: {got:?}"
        );
    }
    assert!(
        exercised.iter().all(|&n| n > 0),
        "注入の対象が足りない（BOM / #Requires / 例外 = {exercised:?}）"
    );

    // 新しく足した .ps1 にも同じ規則が当たる（例外の表は名前で引くので、別名は素通りさせない）
    let fresh = "scripts/windows/new-tool.ps1";
    let got = violation(fresh, "# 新しい道具\nWrite-Output 'x'\n".as_bytes());
    assert!(
        got.as_deref()
            .is_some_and(|m| m.starts_with(&format!("{fresh}:"))),
        "宣言の無い新しい .ps1 を名指ししなかった: {got:?}"
    );
    // 2 行目以降の #Requires は 1 行目より前の非 ASCII で食われうるので認めない
    let late = "# 説明\n#Requires -Version 7.0\n";
    assert_eq!(stance(fresh, late.as_bytes()), Stance::Neither);
    // pwsh 7 より前の版を求める #Requires は 5.1 を止めない
    assert_eq!(stance(fresh, b"#Requires -Version 5.1\n"), Stance::Neither);
    assert_eq!(
        stance(fresh, b"#requires -version 7\r\n"),
        Stance::Requires7
    );
}

#[test]
fn bomの側のps1は5_1に無いapiを呼ばない() {
    let problems: Vec<String> = ps1_files()
        .iter()
        .flat_map(|(rel, bytes)| pwsh7_only_calls(rel, bytes))
        .collect();
    assert!(
        problems.is_empty(),
        "5.1 でも読む側（UTF-8 with BOM / 例外の表）の .ps1 が 5.1 に無い API を呼んでいる（#2329）。\n\
         5.1 での代わりを使うか、pwsh 7 必須の側（1 行目に #Requires -Version 7.0）へ移す:\n{}",
        problems.join("\n")
    );

    // 検出力: コードの行は名指しし、コメントの行は見ない。#Requires の側は対象外
    let fake = "scripts/windows/fake.ps1";
    let mut a = BOM.to_vec();
    a.extend_from_slice(
        b"# GetRelativePath in a comment\n$r = [System.IO.Path]::GetRelativePath($a, $b)\n",
    );
    let got = pwsh7_only_calls(fake, &a);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].starts_with(&format!("{fake}:2: GetRelativePath")),
        "{got:?}"
    );
    let b = b"#Requires -Version 7.0\n$r = [System.IO.Path]::GetRelativePath($a, $b)\n";
    assert!(pwsh7_only_calls(fake, b).is_empty());
}

/// PowerShell の単引用符の文字列（中の `'` は `''`）
fn ps_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

/// `check-powershell51.ps1` を 1 回走らせた結果
struct CheckerRun {
    /// 終了コードが 0 か（broken が 1 本でもあれば 1）
    ok: bool,
    /// `PS51 <判定> <パス> …` の行の (パス, 判定)
    rows: Vec<(String, String)>,
    /// 落ちたときに添える stdout / stderr
    context: String,
}

/// `check-powershell51.ps1` を pwsh で起こす。pwsh が無ければ None
fn run_checker(args: &[PathBuf], cache: &Path) -> Option<CheckerRun> {
    // canonicalize しない（Windows では `\\?\` 付きになり、$PSScriptRoot から組むルートとずれうる）
    let checker = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(CHECKER);
    let out = match Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&checker)
        .args(args)
        // pwsh のキャッシュをユーザーのホームへ撒かない（#944 と同じ倒し方）
        .env("XDG_CACHE_HOME", cache)
        .env("LOCALAPPDATA", cache)
        .env("POWERSHELL_TELEMETRY_OPTOUT", "1")
        .env("POWERSHELL_UPDATECHECK", "Off")
        .env("NO_COLOR", "1")
        .output()
    {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => panic!("pwsh を起こせない: {e}"),
    };
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let rows = stdout
        .lines()
        .filter_map(|l| l.trim().strip_prefix("PS51 "))
        .filter_map(|r| {
            let mut cols = r.split_whitespace();
            let verdict = cols.next()?;
            if !matches!(verdict, "readable" | "stops" | "broken") {
                return None;
            }
            Some((cols.next()?.to_string(), verdict.to_string()))
        })
        .collect();
    let context = format!("{CHECKER}\n--- stdout\n{stdout}\n--- stderr\n{stderr}");
    Some(CheckerRun {
        ok: out.status.success(),
        rows,
        context,
    })
}

/// 実際の読み方: 5.1 の読み方を写して（BOM が無ければ CP1252 と CP932 で復号して）構文解析し、
/// (A) は読める・(B) は #Requires で止まる、を確かめる。注入（#Requires を外す・日本語の BOM を外す）で
/// 名指しして落ちることも見る。
///
/// pwsh が無い環境（素の Linux コンテナ等）ではスキップする。macOS の CI ランナーと Windows には
/// pwsh があるので CI では必ず走る。5.1 の実物での確認は CI の windows ジョブの段が行う
#[test]
fn ps1を5_1の読み方で読むと読めるか理由を出して止まる() {
    let files = ps1_files();
    let scratch = tako_core::test_residue::ScratchDir::new("ps51");
    let root = scratch.path();
    let cache = root.join("pwsh-cache");

    let Some(CheckerRun { ok, rows, context }) = run_checker(&[], &cache) else {
        eprintln!("pwsh が無いので {CHECKER} の実行検証をスキップ（静的な検査は実施済み）");
        return;
    };
    let mut problems = Vec::new();
    for (rel, bytes) in &files {
        let want = stance(rel, bytes).expected_verdict();
        match rows.iter().find(|(p, _)| p == rel) {
            Some((_, got)) if got == want => {}
            Some((_, got)) => problems.push(format!("{rel}: 期待は {want}、実際は {got}")),
            None => problems.push(format!("{rel}: {CHECKER} の結果に無い（走査の食い違い）")),
        }
    }
    for (p, _) in &rows {
        if !files.iter().any(|(rel, _)| rel == p) {
            problems.push(format!("{p}: {CHECKER} だけが拾った（走査の食い違い）"));
        }
    }
    assert!(
        problems.is_empty() && ok,
        "5.1 の読み方で読むと宣言どおりにならない .ps1 がある（#2329）:\n{}\n{context}",
        problems.join("\n")
    );

    // 注入: (B) の #Requires を外した複製 → 5.1 では化けるので broken。日本語の BOM 付き → readable、
    // BOM を外す → broken。複製の名前で結果を引く
    let inject = root.join("inject");
    std::fs::create_dir_all(&inject).expect("注入の置き場を作る");
    let mut cases: Vec<(PathBuf, &str)> = Vec::new();
    for (i, (rel, bytes)) in files.iter().enumerate() {
        if stance(rel, bytes) != Stance::Requires7 || bytes.starts_with(BOM) {
            continue;
        }
        let cut = bytes
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |n| n + 1);
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let path = inject.join(format!("no-requires-{i}-{name}"));
        std::fs::write(&path, &bytes[cut..]).expect("注入した複製を書く");
        cases.push((path, "broken"));
    }
    assert!(!cases.is_empty(), "#Requires の側の .ps1 が 1 本も無い");
    let ja = "# 日本語のコメント（5.1 が ANSI として読むと化ける）\n$m = 'ライセンスの同梱表'\nWrite-Output $m\n";
    let with_bom = inject.join("ja-with-bom.ps1");
    let mut b = BOM.to_vec();
    b.extend_from_slice(ja.as_bytes());
    std::fs::write(&with_bom, b).expect("BOM 付きの複製を書く");
    cases.push((with_bom, "readable"));
    let without_bom = inject.join("ja-without-bom.ps1");
    std::fs::write(&without_bom, ja).expect("BOM 無しの複製を書く");
    cases.push((without_bom, "broken"));

    let args: Vec<PathBuf> = cases.iter().map(|(p, _)| p.clone()).collect();
    let CheckerRun { ok, rows, context } =
        run_checker(&args, &cache).expect("pwsh は上で見つかった");
    let mut problems = Vec::new();
    for (path, want) in &cases {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let got = rows
            .iter()
            .find(|(p, _)| p.rsplit('/').next() == Some(name))
            .map(|(_, v)| v.as_str());
        if got != Some(*want) {
            problems.push(format!("{}: 期待は {want}、実際は {got:?}", ps_quote(path)));
        }
    }
    assert!(
        problems.is_empty() && !ok,
        "注入した複製の判定が違う（broken があれば終了コードも失敗のはず）:\n{}\n{context}",
        problems.join("\n")
    );
}
