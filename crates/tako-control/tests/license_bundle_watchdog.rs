//! 番犬: 配布物の組み立てが、ライセンス本文と第三者の告知の 3 本を同梱している（Issue #1845）
//!
//! GPL-3.0 第 4 条・Apache-2.0 第 4 条 (a)・MIT / BSD の表示義務は、バイナリの受け取り手へ
//! 本文と著作権表示を渡すことを求める。配布物は OS ごとに別々の場所で組み立てるので、
//! 1 か所で 1 本抜けても残りは緑のまま出荷される（v0.8.24 がそうだった: macOS の .app は
//! #1713 で 3 本入ったが、Windows の zip は `LICENSE.txt` だけ）。そこで組み立ての全箇所を
//! ここで 1 つの表 [`TABLE`]（`scripts/lib/license-bundle.txt`）へ突き合わせる:
//!
//! - macOS: `scripts/build-app.sh` が `tako.app/Contents/Resources/` へ**署名より前に**置く
//!   （リリース zip・Homebrew cask・アプリ内更新はすべてこの .app を配る）
//! - Windows のインストーラー: `installer/windows/tako.iss` の `[Files]` が `{app}` へ置く
//! - Windows のポータブル zip: `installer/windows/build-installer.ps1` が zip の `tako/` へ置く
//! - Windows のリリース検査: `installer/windows/lib/verify-assets.ps1` の `$TakoLicenseBundle`
//!   （zip の展開とインストーラーの無人インストールで、3 本が元ファイルと一致するかを見る表）
//! - 公開後の検査（#1850）: `scripts/release.sh --check-assets` が Release の zip を取得し、
//!   同じ表を `scripts/lib/license-bundle.sh` 経由で読んで確かめる。**表はこの 1 か所だけ**で、
//!   シェル側に名前を並べ直すと file:line で名指しして落ちる
//!
//! 抜けたときは、置くべき場所の file:line を名指しして落ちる。
//! 実物の中身は、リリースの作業流れ（`.github/workflows/release-windows.yml`。ブランチから
//! dispatch するとドライラン）が zip の展開と無人インストールで確かめ、公開後は
//! `scripts/release.sh --check-assets` が Release の zip で確かめる。
//!
//! 表を Rust の定数ではなくデータファイルに置くのは、シェルの検査と番犬が同じ 1 つを読むため
//! （シェルが Rust のソースを解析すると rustfmt の整形で壊れうるうえ、それを番犬から確かめる
//! には Windows の CI でも bash を起こす必要がある。データファイルなら両側とも数行で読める）。

use std::path::{Path, PathBuf};

/// 同梱表の正本（1 行 1 本: 元ファイル / Windows での名前）
const TABLE: &str = "scripts/lib/license-bundle.txt";

/// 同梱する 1 本。`source` はリポジトリ直下の元ファイルで、macOS の .app へはこの名前のまま置く
struct Entry {
    source: String,
    /// Windows の配布物（zip の `tako/`・インストール先の `{app}`）での名前
    windows_name: String,
    /// 表の行（1 始まり。名指しに使う）
    line: usize,
}

/// 表 [`TABLE`] を読む。崩れた行・重複は file:line で名指しして落ちる
fn bundle() -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for (i, raw) in read(TABLE).lines().enumerate() {
        let t = raw.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = t.split_whitespace().collect();
        let [source, windows_name] = cols.as_slice() else {
            panic!(
                "{TABLE}:{}: 「元ファイル Windows での名前」の 2 列になっていない: {t}",
                i + 1
            );
        };
        if let Some(dup) = entries
            .iter()
            .find(|e| e.source == *source || e.windows_name == *windows_name)
        {
            panic!(
                "{TABLE}:{}: {source} / {windows_name} が {TABLE}:{} と重なっている",
                i + 1,
                dup.line
            );
        }
        entries.push(Entry {
            source: source.to_string(),
            windows_name: windows_name.to_string(),
            line: i + 1,
        });
    }
    assert!(!entries.is_empty(), "{TABLE} に 1 本も無い");
    entries
}

const BUILD_APP: &str = "scripts/build-app.sh";
const ISS: &str = "installer/windows/tako.iss";
const BUILD_INSTALLER: &str = "installer/windows/build-installer.ps1";
const VERIFY_ASSETS: &str = "installer/windows/lib/verify-assets.ps1";
const RELEASE_WINDOWS_PS1: &str = "installer/windows/release-windows.ps1";
const RELEASE_WINDOWS_YML: &str = ".github/workflows/release-windows.yml";
const RELEASE_SH: &str = "scripts/release.sh";
const LICENSE_LIB: &str = "scripts/lib/license-bundle.sh";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()))
}

/// コメント行を空行に置き換えた本文（**行番号は保つ**）。
///
/// 説明のコメントに書いた同じ綴りで真にならないよう、存在確認はこの眺めで行う（#1609）。
/// `marker` は行コメントの記号。PowerShell は `<# … #>` のブロックコメントも潰す
fn code_lines(rel: &str, marker: &str) -> Vec<String> {
    let powershell = rel.ends_with(".ps1");
    let mut in_block = false;
    read(rel)
        .lines()
        .map(|line| {
            let t = line.trim_start();
            if powershell && in_block {
                in_block = !t.contains("#>");
                return String::new();
            }
            if powershell && t.starts_with("<#") {
                in_block = !t.contains("#>");
                return String::new();
            }
            if t.starts_with(marker) {
                return String::new();
            }
            line.to_string()
        })
        .collect()
}

/// 述語に合う最初の行（1 始まり）
fn find_line(lines: &[String], pred: impl Fn(&str) -> bool) -> Option<usize> {
    lines.iter().position(|l| pred(l)).map(|i| i + 1)
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"').trim_matches('\'')
}

fn assert_no_problems(problems: Vec<String>) {
    assert!(
        problems.is_empty(),
        "ライセンスの同梱が崩れている（Issue #1845 / #1850）:\n{}",
        problems.join("\n")
    );
}

#[test]
fn 同梱する3本がリポジトリ直下にある() {
    let mut problems = Vec::new();
    for e in &bundle() {
        let path = repo_root().join(&e.source);
        let at = format!("{TABLE}:{}", e.line);
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() > 0 => {}
            Ok(_) => problems.push(format!("{at}: {} が空、またはファイルではない", e.source)),
            Err(err) => problems.push(format!("{at}: {} を読めない（{err}）", e.source)),
        }
    }
    assert_no_problems(problems);
}

#[test]
fn macosのappが3本をresourcesへ署名より前に置く() {
    let lines = code_lines(BUILD_APP, "#");
    let anchor = find_line(&lines, |l| {
        l.trim_start().starts_with("mkdir -p") && l.contains("$APP/Contents/Resources")
    })
    .unwrap_or_else(|| panic!("{BUILD_APP} に Contents/Resources を作る mkdir が無い"));
    // `security find-identity -p codesigning` は署名ではないので、行頭の `codesign` だけを数える
    let first_sign = find_line(&lines, |l| l.trim_start().starts_with("codesign "))
        .unwrap_or_else(|| panic!("{BUILD_APP} に codesign が無い"));

    let mut problems = Vec::new();
    for e in &bundle() {
        let dest_file = format!("$APP/Contents/Resources/{}", e.source);
        let placed = find_line(&lines, |l| {
            let words: Vec<&str> = l.split_whitespace().collect();
            words.len() >= 3
                && words[0] == "cp"
                && unquote(words[words.len() - 2]) == e.source
                && [
                    dest_file.as_str(),
                    "$APP/Contents/Resources",
                    "$APP/Contents/Resources/",
                ]
                .contains(&unquote(words[words.len() - 1]))
        });
        match placed {
            None => problems.push(format!(
                "{BUILD_APP}:{anchor}: tako.app/Contents/Resources へ {src} を置く cp が無い\
                 （例: cp {src} \"{dest_file}\"）",
                src = e.source
            )),
            Some(n) if n < anchor || n > first_sign => problems.push(format!(
                "{BUILD_APP}:{n}: {src} を置く cp が .app の組み立て（{anchor} 行）から\
                 署名（{first_sign} 行）の間に無い。署名より後に置くと封印が壊れる",
                src = e.source
            )),
            Some(_) => {}
        }
    }
    assert_no_problems(problems);
}

/// `.iss` の 1 行（`Key: Value; Key: "Value"; …`）を分解する。キーは小文字へ揃える
fn iss_params(line: &str) -> Vec<(String, String)> {
    line.split(';')
        .filter_map(|seg| {
            let (k, v) = seg.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), unquote(v).to_string()))
        })
        .collect()
}

fn iss_param<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn windowsのインストーラーが3本をappへ置く() {
    let lines = code_lines(ISS, ";");
    let section = find_line(&lines, |l| l.trim().eq_ignore_ascii_case("[Files]"))
        .unwrap_or_else(|| panic!("{ISS} に [Files] 節が無い"));
    let section_end = lines
        .iter()
        .enumerate()
        .skip(section)
        .find(|(_, l)| l.trim_start().starts_with('['))
        .map(|(i, _)| i)
        .unwrap_or(lines.len());

    let mut problems = Vec::new();
    for e in &bundle() {
        let want_source = format!("{{#RepoRoot}}\\{}", e.source);
        let hit = (section..section_end).find_map(|i| {
            let params = iss_params(&lines[i]);
            (iss_param(&params, "source") == Some(want_source.as_str())).then_some((i + 1, params))
        });
        let Some((n, params)) = hit else {
            // 元と同じ名前なら DestName は要らない
            let dest_name = if e.windows_name == e.source {
                String::new()
            } else {
                format!(" DestName: \"{}\";", e.windows_name)
            };
            problems.push(format!(
                "{ISS}:{section}: [Files] に {src} を {{app}}\\{name} へ置く行が無い\
                 （例: Source: \"{want_source}\"; DestDir: \"{{app}}\";{dest_name} Flags: ignoreversion）",
                src = e.source,
                name = e.windows_name
            ));
            continue;
        };
        let get = |key: &str| iss_param(&params, key);
        let name = get("destname").unwrap_or(e.source.as_str());
        if get("destdir") != Some("{app}") || name != e.windows_name {
            problems.push(format!(
                "{ISS}:{n}: {src} の置き先が違う（期待: DestDir \"{{app}}\" / 名前 {want}。\
                 実際: DestDir {dir} / 名前 {name}）",
                src = e.source,
                want = e.windows_name,
                dir = get("destdir").unwrap_or("（無し）"),
            ));
        }
        // ライセンスは常に置く。タスク・コンポーネント・条件で外せる形にしない
        for key in ["tasks", "components", "check", "languages"] {
            if get(key).is_some() {
                problems.push(format!(
                    "{ISS}:{n}: {src} を {key} 付きで置いている（条件に関わらず必ず置くこと）",
                    src = e.source
                ));
            }
        }
    }
    assert_no_problems(problems);
}

#[test]
fn windowsのzipが3本をtakoへ置く() {
    let lines = code_lines(BUILD_INSTALLER, "#");
    let anchor = find_line(&lines, |l| {
        l.contains("$payload = Join-Path $staging 'tako'")
    })
    .unwrap_or_else(|| panic!("{BUILD_INSTALLER} に zip の置き場 $payload の定義が無い"));
    let compress = find_line(&lines, |l| l.contains("Compress-Archive"))
        .unwrap_or_else(|| panic!("{BUILD_INSTALLER} に Compress-Archive が無い"));

    let mut problems = Vec::new();
    for e in &bundle() {
        let from = format!("(Join-Path $repoRoot '{}')", e.source);
        let to = format!("-Destination (Join-Path $payload '{}')", e.windows_name);
        let copies = |l: &str| l.contains("Copy-Item") && l.contains(&from);
        match find_line(&lines, copies) {
            None => problems.push(format!(
                "{BUILD_INSTALLER}:{anchor}: zip の tako/ へ {src} を置く Copy-Item が無い\
                 （例: Copy-Item -LiteralPath {from} {to}）",
                src = e.source
            )),
            Some(n) => {
                let line = &lines[n - 1];
                let same_name_ok = e.source == e.windows_name
                    && line.trim_end().ends_with("-Destination $payload");
                if !(line.contains(&to) || same_name_ok) {
                    problems.push(format!(
                        "{BUILD_INSTALLER}:{n}: {src} の zip 内の名前が {want} になっていない: {}",
                        line.trim(),
                        src = e.source,
                        want = e.windows_name
                    ));
                }
                if n < anchor || n > compress {
                    problems.push(format!(
                        "{BUILD_INSTALLER}:{n}: {src} を置く Copy-Item が置き場の用意（{anchor} 行）から\
                         zip を固める（{compress} 行）までの間に無い",
                        src = e.source
                    ));
                }
            }
        }
    }
    assert_no_problems(problems);
}

#[test]
fn windowsのリリース検査の表が3本と一致する() {
    let lines = code_lines(VERIFY_ASSETS, "#");
    let anchor = find_line(&lines, |l| {
        l.trim_start().starts_with("$TakoLicenseBundle =")
    })
    .unwrap_or_else(|| panic!("{VERIFY_ASSETS} に $TakoLicenseBundle の定義が無い"));
    let close = (anchor..lines.len())
        .find(|&i| lines[i].trim() == "}")
        .unwrap_or_else(|| panic!("{VERIFY_ASSETS}:{anchor}: $TakoLicenseBundle が閉じていない"));

    let mut problems = Vec::new();
    // 表の中身: `'配布物の中の名前' = '元ファイル'`
    let mut table: Vec<(usize, String, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate().take(close).skip(anchor) {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (unquote(k), unquote(v));
        if k.is_empty() {
            continue;
        }
        table.push((i + 1, k.to_string(), v.to_string()));
    }
    let entries = bundle();
    for e in &entries {
        match table.iter().find(|(_, k, _)| *k == e.windows_name) {
            None => problems.push(format!(
                "{VERIFY_ASSETS}:{anchor}: $TakoLicenseBundle に '{name}' = '{src}' が無い\
                 （{TABLE}:{line} の行）",
                name = e.windows_name,
                src = e.source,
                line = e.line
            )),
            Some((n, _, v)) if *v != e.source => problems.push(format!(
                "{VERIFY_ASSETS}:{n}: '{name}' の元ファイルが {v}（期待 {src} = {TABLE}:{line}）",
                name = e.windows_name,
                src = e.source,
                line = e.line
            )),
            Some(_) => {}
        }
    }
    for (n, k, _) in &table {
        if !entries.iter().any(|e| e.windows_name == *k) {
            problems.push(format!(
                "{VERIFY_ASSETS}:{n}: '{k}' は同梱表 {TABLE} に無い（足すなら表と組み立ての全箇所へ）"
            ));
        }
    }

    // 表が検査に繋がっていること: zip は Test-TakoWindowsAssets が（実機・CI の両経路）、
    // インストール先は CI だけが見る
    let assets_fn = find_line(&lines, |l| l.contains("function Test-TakoWindowsAssets"))
        .unwrap_or_else(|| panic!("{VERIFY_ASSETS} に Test-TakoWindowsAssets が無い"));
    let assets_end = (assets_fn..lines.len())
        .find(|&i| lines[i].starts_with('}'))
        .unwrap_or(lines.len());
    if !lines[assets_fn..assets_end]
        .iter()
        .any(|l| l.contains("Test-TakoLicenseBundle"))
    {
        problems.push(format!(
            "{VERIFY_ASSETS}:{assets_fn}: Test-TakoWindowsAssets が zip の同梱（Test-TakoLicenseBundle）を検査していない"
        ));
    }
    let yml = code_lines(RELEASE_WINDOWS_YML, "#");
    if find_line(&yml, |l| l.contains("Test-TakoInstalledPayload")).is_none() {
        // 名指しは配布物の検査の行（この後ろに無人インストールの検査を置く）
        let verify = find_line(&yml, |l| l.contains("Test-TakoWindowsAssets")).unwrap_or(1);
        problems.push(format!(
            "{RELEASE_WINDOWS_YML}:{verify}: 配布物の検査の後に、インストール先の同梱の検査\
             （Test-TakoInstalledPayload）が無い"
        ));
    }
    // 無人インストールは実インストールを上書きするので、実機の経路からは呼ばない
    let real = code_lines(RELEASE_WINDOWS_PS1, "#");
    if let Some(n) = find_line(&real, |l| l.contains("Test-TakoInstalledPayload")) {
        problems.push(format!(
            "{RELEASE_WINDOWS_PS1}:{n}: 実機の経路が無人インストールの検査を呼んでいる（実インストールを上書きする）"
        ));
    }
    assert_no_problems(problems);
}

/// `name` が 1 語として現れるか（前後がファイル名の文字ではない。`TAKO_LICENSE_…` や
/// `THIRD-PARTY-LICENSES.md` の中の `LICENSE` は数えない）
fn has_name_token(line: &str, name: &str) -> bool {
    let name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    let step = name.chars().next().map_or(1, char::len_utf8);
    let mut from = 0;
    while let Some(off) = line[from..].find(name) {
        let start = from + off;
        let end = start + name.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        if !before.is_some_and(name_char) && !after.is_some_and(name_char) {
            return true;
        }
        from = start + step;
    }
    false
}

/// `start` 行（1 始まり）から始まる塊の終わり（行頭が `end` の最初の行。1 始まり）
fn block_end(lines: &[String], start: usize, end: &str) -> usize {
    (start..lines.len())
        .find(|&i| lines[i].trim_end() == end)
        .map_or(lines.len(), |i| i + 1)
}

#[test]
fn 公開後のzipの検査は同じ表を読み名前を直書きしない() {
    let entries = bundle();
    let mut problems = Vec::new();

    // 読み手が表 TABLE を指している
    let lib = code_lines(LICENSE_LIB, "#");
    let quoted_table = format!("\"{TABLE}\"");
    if find_line(&lib, |l| l.contains(&quoted_table)).is_none() {
        problems.push(format!(
            "{LICENSE_LIB}:1: 同梱表 {TABLE} を読んでいない（TAKO_LICENSE_BUNDLE_TABLE={quoted_table} が無い）"
        ));
    }

    // 2 か所目の表を作らない: シェル側のコード（コメントを除く）に名前を直書きしない
    for rel in [LICENSE_LIB, RELEASE_SH] {
        for (i, l) in code_lines(rel, "#").iter().enumerate() {
            for e in &entries {
                let mut names = vec![e.source.as_str()];
                if e.windows_name != e.source {
                    names.push(e.windows_name.as_str());
                }
                for name in names {
                    if has_name_token(l, name) {
                        problems.push(format!(
                            "{rel}:{}: 同梱するライセンスの名前 {name} を直書きしている。表は {TABLE}:{} の 1 か所だけ\
                             （tako_license_bundle で読む）: {}",
                            i + 1,
                            e.line,
                            l.trim()
                        ));
                    }
                }
            }
        }
    }

    // release.sh --check-assets が読み手を source し、公開済みの zip の検査を呼んでいる
    let rel = code_lines(RELEASE_SH, "#");
    if find_line(&rel, |l| {
        l.trim_start().starts_with("source ") && l.contains("scripts/lib/license-bundle.sh")
    })
    .is_none()
    {
        problems.push(format!(
            "{RELEASE_SH}:1: {LICENSE_LIB} を source していない"
        ));
    }
    match find_line(&rel, |l| l.contains("if [[ -n \"$CHECK_ASSETS_TAG\" ]]")) {
        None => problems.push(format!("{RELEASE_SH}:1: --check-assets の分岐が無い")),
        Some(start) => {
            let end = block_end(&rel, start, "fi");
            if !rel[start..end]
                .iter()
                .any(|l| l.contains("report_release_licenses"))
            {
                problems.push(format!(
                    "{RELEASE_SH}:{start}: --check-assets が公開済みの zip のライセンス検査（report_release_licenses）を呼んでいない"
                ));
            }
        }
    }
    match find_line(&rel, |l| l.starts_with("report_release_licenses()")) {
        None => problems.push(format!(
            "{RELEASE_SH}:1: report_release_licenses が定義されていない"
        )),
        Some(start) => {
            let end = block_end(&rel, start, "}");
            if !rel[start..end]
                .iter()
                .any(|l| l.contains("tako_license_check_zip"))
            {
                problems.push(format!(
                    "{RELEASE_SH}:{start}: report_release_licenses が zip を表と照合していない（tako_license_check_zip）"
                ));
            }
        }
    }
    assert_no_problems(problems);
}

#[test]
fn 語の照合はファイル名の一部を数えない() {
    assert!(has_name_token("cp LICENSE \"$x\"", "LICENSE"));
    assert!(has_name_token(
        "x=$APP/Contents/Resources/LICENSE",
        "LICENSE"
    ));
    assert!(has_name_token("'LICENSE.txt'", "LICENSE.txt"));
    assert!(!has_name_token("TAKO_LICENSE_BUNDLE_TABLE=x", "LICENSE"));
    assert!(!has_name_token("THIRD-PARTY-LICENSES.md", "LICENSE"));
    assert!(!has_name_token("LICENSE.txt", "LICENSE"));
    assert!(!has_name_token("LICENSES", "LICENSE"));
}
