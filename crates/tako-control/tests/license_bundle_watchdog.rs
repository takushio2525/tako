//! 番犬: 配布物の組み立てが、ライセンス本文と第三者の告知の 3 本を同梱している（Issue #1845）
//!
//! GPL-3.0 第 4 条・Apache-2.0 第 4 条 (a)・MIT / BSD の表示義務は、バイナリの受け取り手へ
//! 本文と著作権表示を渡すことを求める。配布物は OS ごとに別々の場所で組み立てるので、
//! 1 か所で 1 本抜けても残りは緑のまま出荷される（v0.8.24 がそうだった: macOS の .app は
//! #1713 で 3 本入ったが、Windows の zip は `LICENSE.txt` だけ）。そこで組み立ての全箇所を
//! ここで 1 つの表 [`BUNDLE`] へ突き合わせる:
//!
//! - macOS: `scripts/build-app.sh` が `tako.app/Contents/Resources/` へ**署名より前に**置く
//!   （リリース zip・Homebrew cask・アプリ内更新はすべてこの .app を配る）
//! - Windows のインストーラー: `installer/windows/tako.iss` の `[Files]` が `{app}` へ置く
//! - Windows のポータブル zip: `installer/windows/build-installer.ps1` が zip の `tako/` へ置く
//! - Windows のリリース検査: `installer/windows/lib/verify-assets.ps1` の `$TakoLicenseBundle`
//!   （zip の展開とインストーラーの無人インストールで、3 本が元ファイルと一致するかを見る表）
//!
//! 抜けたときは、置くべき場所の file:line を名指しして落ちる。
//! 実物の中身は、リリースの作業流れ（`.github/workflows/release-windows.yml`。ブランチから
//! dispatch するとドライラン）が zip の展開と無人インストールで確かめる。

use std::path::{Path, PathBuf};

/// 同梱する 1 本。`source` はリポジトリ直下の元ファイルで、macOS の .app へはこの名前のまま置く
struct Entry {
    source: &'static str,
    /// Windows の配布物（zip の `tako/`・インストール先の `{app}`）での名前
    windows_name: &'static str,
}

/// 同梱する 3 本（正本）。LICENSE は Windows ではメモ帳で開けるよう `.txt` を付ける
const BUNDLE: &[Entry] = &[
    Entry {
        source: "LICENSE",
        windows_name: "LICENSE.txt",
    },
    Entry {
        source: "THIRD-PARTY-NOTICES.md",
        windows_name: "THIRD-PARTY-NOTICES.md",
    },
    Entry {
        source: "THIRD-PARTY-LICENSES.md",
        windows_name: "THIRD-PARTY-LICENSES.md",
    },
];

const BUILD_APP: &str = "scripts/build-app.sh";
const ISS: &str = "installer/windows/tako.iss";
const BUILD_INSTALLER: &str = "installer/windows/build-installer.ps1";
const VERIFY_ASSETS: &str = "installer/windows/lib/verify-assets.ps1";
const RELEASE_WINDOWS_PS1: &str = "installer/windows/release-windows.ps1";
const RELEASE_WINDOWS_YML: &str = ".github/workflows/release-windows.yml";

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
        "ライセンスの同梱が崩れている（Issue #1845）:\n{}",
        problems.join("\n")
    );
}

#[test]
fn 同梱する3本がリポジトリ直下にある() {
    let mut problems = Vec::new();
    for e in BUNDLE {
        let path = repo_root().join(e.source);
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() && m.len() > 0 => {}
            Ok(_) => problems.push(format!("{}: 空、またはファイルではない", e.source)),
            Err(err) => problems.push(format!("{}: 読めない（{err}）", e.source)),
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
    for e in BUNDLE {
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
    for e in BUNDLE {
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
        let name = get("destname").unwrap_or(e.source);
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
    for e in BUNDLE {
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
    for e in BUNDLE {
        match table.iter().find(|(_, k, _)| k == e.windows_name) {
            None => problems.push(format!(
                "{VERIFY_ASSETS}:{anchor}: $TakoLicenseBundle に '{name}' = '{src}' が無い",
                name = e.windows_name,
                src = e.source
            )),
            Some((n, _, v)) if v != e.source => problems.push(format!(
                "{VERIFY_ASSETS}:{n}: '{name}' の元ファイルが {v}（期待 {src}）",
                name = e.windows_name,
                src = e.source
            )),
            Some(_) => {}
        }
    }
    for (n, k, _) in &table {
        if !BUNDLE.iter().any(|e| e.windows_name == k) {
            problems.push(format!(
                "{VERIFY_ASSETS}:{n}: '{k}' はこの番犬の表 BUNDLE に無い（足すなら BUNDLE と組み立ての全箇所へ）"
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
