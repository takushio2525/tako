//! Code Runner: ファイル内宣言パーサ + コマンド解決器（FR-3.18）
//!
//! ファイル先頭の `tako:run:` 等の宣言を解析し、実行コマンドを解決する。
//! GPUI 非依存・純関数。設計: `.agent/plans/2026-07-code-runner.md`

use std::collections::BTreeMap;
use std::path::Path;

use crate::shell::quote_for_shell;

// --- 定数 ---

const SCAN_MAX_LINES: usize = 64;
const SCAN_MAX_BYTES: usize = 16 * 1024;
const PREFIX_MAX_LEN: usize = 16;
const PROFILE_NAME_MAX_LEN: usize = 32;

const MARKER: &str = "tako:";

// 行末クローザ（ブロックコメント等の終端記号）
const CLOSERS: &[&str] = &["-->", "*/", "#}", "--}}"];

/// 組み込み拡張子既定マップ（設計 §2.2）
pub fn builtin_defaults() -> &'static [(&'static str, &'static str)] {
    &[
        ("command", "bash ${fileBase}"),
        ("sh", "bash ${fileBase}"),
        ("bash", "bash ${fileBase}"),
        ("zsh", "zsh ${fileBase}"),
        ("py", "python3 ${fileBase}"),
        ("js", "node ${fileBase}"),
        ("mjs", "node ${fileBase}"),
        ("ts", "npx tsx ${fileBase}"),
        ("rb", "ruby ${fileBase}"),
        ("pl", "perl ${fileBase}"),
        ("php", "php ${fileBase}"),
        ("lua", "lua ${fileBase}"),
        ("c", "cc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        ("cpp", "c++ ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        ("cc", "c++ ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        ("cxx", "c++ ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        ("rs", "rustc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}"),
        ("go", "go run ${fileBase}"),
        ("java", "java ${fileBase}"),
        ("swift", "swift ${fileBase}"),
        ("tex", "latexmk -pdf -interaction=nonstopmode ${fileBase}"),
    ]
}

// --- 型定義 ---

/// コマンドの出典
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunSource {
    /// ファイル内宣言（`tako:run:` 系）
    Declaration,
    /// 拡張子既定（settings / 組み込み）
    ExtensionDefault,
    /// CLI / MCP の明示 `--command` オーバーライド
    Override,
}

/// 1 プロファイル分の解決済み実行計画
#[derive(Debug, Clone)]
pub struct RunPlan {
    pub profile: String,
    pub command: String,
    pub cwd: std::path::PathBuf,
    /// `tako:shell` 指定（None = ログインシェル）
    pub shell: Option<String>,
    pub source: RunSource,
}

/// パース結果: 1 プロファイル分の宣言内容（未展開）
#[derive(Debug, Clone)]
pub struct ProfileDecl {
    pub name: String,
    pub run: Option<String>,
    pub cwd: Option<String>,
    pub shell: Option<String>,
}

/// `parse_declarations` の戻り値
#[derive(Debug, Clone)]
pub struct Declarations {
    /// 出現順で保持（宣言順がドロップダウンの表示順）
    pub profiles: Vec<ProfileDecl>,
    /// 重複宣言等の警告
    pub warnings: Vec<String>,
}

/// `resolve` の戻り値
#[derive(Debug, Clone)]
pub struct Resolution {
    /// 指定されたプロファイルの実行計画（実行可能な場合）
    pub plan: RunPlan,
    /// 全検出プロファイル一覧（ドロップダウン用）
    pub all_profiles: Vec<RunPlan>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    #[error("実行コマンドが見つからない: {0}")]
    NoCommand(String),
    #[error("指定されたプロファイル '{0}' はファイル内に宣言されていない")]
    ProfileNotFound(String),
    #[error("cwd が存在しない: {0}")]
    CwdNotFound(String),
}

// --- パーサ ---

/// ファイル先頭テキストから宣言を抽出（64 行 / 16 KiB ウィンドウ）
pub fn parse_declarations(head: &str) -> Declarations {
    let mut profiles: Vec<ProfileDecl> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut bytes_consumed: usize = 0;

    for (line_idx, line) in head.lines().enumerate() {
        if line_idx >= SCAN_MAX_LINES {
            break;
        }
        bytes_consumed += line.len() + 1; // +1 for newline
        if bytes_consumed > SCAN_MAX_BYTES {
            break;
        }

        // BOM 除去（先頭行のみ）
        let line = if line_idx == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };

        // 行内の `tako:` を探す
        let Some((prefix_len, rest)) = find_marker(line) else {
            continue;
        };

        // 接頭辞の長さ制限
        if prefix_len > PREFIX_MAX_LEN {
            continue;
        }

        // キーとプロファイル名のパース
        let Some((key, profile_name, value)) = parse_directive(rest) else {
            continue;
        };

        let profile_name = profile_name.unwrap_or("default");

        // プロファイルを探すか新規作成
        let decl = if let Some(pos) = profiles.iter().position(|p| p.name == profile_name) {
            &mut profiles[pos]
        } else {
            profiles.push(ProfileDecl {
                name: profile_name.to_string(),
                run: None,
                cwd: None,
                shell: None,
            });
            profiles.last_mut().unwrap()
        };

        match key {
            "run" => {
                if decl.run.is_some() {
                    warnings.push(format!(
                        "プロファイル '{}' の run が重複宣言（後勝ち、{}行目）",
                        profile_name,
                        line_idx + 1
                    ));
                }
                decl.run = Some(value.to_string());
            }
            "cwd" => {
                if decl.cwd.is_some() {
                    warnings.push(format!(
                        "プロファイル '{}' の cwd が重複宣言（後勝ち、{}行目）",
                        profile_name,
                        line_idx + 1
                    ));
                }
                decl.cwd = Some(value.to_string());
            }
            "shell" => {
                if decl.shell.is_some() {
                    warnings.push(format!(
                        "プロファイル '{}' の shell が重複宣言（後勝ち、{}行目）",
                        profile_name,
                        line_idx + 1
                    ));
                }
                decl.shell = Some(value.to_string());
            }
            _ => {}
        }
    }

    Declarations { profiles, warnings }
}

/// 行内の `tako:` マーカーを探し、(接頭辞の長さ, マーカー以降の文字列) を返す。
/// `tako:` の直前が行頭・空白・非英数字のいずれかであることを検証する
fn find_marker(line: &str) -> Option<(usize, &str)> {
    let mut search_from = 0;
    loop {
        let pos = line[search_from..].find(MARKER)?;
        let abs_pos = search_from + pos;

        // `tako:` の直前チェック: 行頭・空白・非英数字
        if abs_pos == 0 {
            return Some((0, &line[MARKER.len()..]));
        }

        let prev_char = line[..abs_pos].chars().next_back().unwrap();
        if prev_char.is_whitespace() || !prev_char.is_alphanumeric() {
            return Some((abs_pos, &line[abs_pos + MARKER.len()..]));
        }

        // `mytako:run` のように英数字が直前にある場合はスキップして次を探す
        search_from = abs_pos + MARKER.len();
        if search_from >= line.len() {
            return None;
        }
    }
}

/// `tako:` 以降の文字列からキー・プロファイル名・値を抽出。
/// 戻り値: (key, Option<profile_name>, value)
fn parse_directive(rest: &str) -> Option<(&str, Option<&str>, &str)> {
    // キー名（小文字英字）を取得
    let key_end = rest.find(|c: char| !c.is_ascii_lowercase())?;
    if key_end == 0 {
        return None;
    }
    let key = &rest[..key_end];

    // `run` / `cwd` / `shell` のみ
    if !matches!(key, "run" | "cwd" | "shell") {
        return None;
    }

    let after_key = &rest[key_end..];

    // プロファイル名（`[name]` 部分、省略可）
    let (profile_name, after_profile) = if after_key.starts_with('[') {
        let bracket_end = after_key.find(']')?;
        let name = &after_key[1..bracket_end];
        // プロファイル名の検証: [A-Za-z0-9_-]{1,32}
        if name.is_empty()
            || name.len() > PROFILE_NAME_MAX_LEN
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return None;
        }
        (Some(name), &after_key[bracket_end + 1..])
    } else {
        (None, after_key)
    };

    // `:` の後ろが値
    let after_profile = after_profile.strip_prefix(':')?;
    let value = strip_closers(after_profile.trim());

    Some((key, profile_name, value))
}

/// 行末クローザ（`-->`, `*/`, `#}`, `--}}`）を除去
fn strip_closers(value: &str) -> &str {
    let mut v = value;
    for closer in CLOSERS {
        if let Some(stripped) = v.strip_suffix(closer) {
            v = stripped.trim_end();
            break;
        }
    }
    v
}

// --- 変数展開 ---

/// コマンド・cwd の値中の変数を展開。展開値はシングルクオートで自動エスケープ
pub fn expand_variables(template: &str, path: &Path) -> String {
    let file_str = path.to_string_lossy();
    let file_dir = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_base = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_no_ext = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let mut result = String::with_capacity(template.len() * 2);
    let mut chars = template.char_indices().peekable();

    while let Some((i, c)) = chars.next() {
        if c == '$' && template[i + 1..].starts_with('{') {
            // ${...} パターンの検出
            if let Some(end) = template[i + 2..].find('}') {
                let var_name = &template[i + 2..i + 2 + end];
                let replacement = match var_name {
                    "file" => Some(quote_for_shell(&file_str)),
                    "fileDir" => Some(quote_for_shell(&file_dir)),
                    "fileBase" => Some(quote_for_shell(&file_base)),
                    "fileNoExt" => Some(quote_for_shell(&file_no_ext)),
                    "ext" => Some(quote_for_shell(&ext)),
                    _ => None, // 未知は展開せずそのまま
                };
                if let Some(rep) = replacement {
                    result.push_str(&rep);
                    // ${...} の分だけスキップ
                    for _ in 0..end + 2 {
                        chars.next();
                    }
                    continue;
                }
            }
        }
        result.push(c);
    }

    result
}

// --- 解決器 ---

/// 組み込み既定と settings 由来のユーザー定義をマージ。
/// ユーザー定義が優先。空文字列は無効化
pub fn merged_defaults(user_defaults: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for &(ext, cmd) in builtin_defaults() {
        map.insert(ext.to_string(), cmd.to_string());
    }
    for (ext, cmd) in user_defaults {
        if cmd.is_empty() {
            map.remove(ext);
        } else {
            map.insert(ext.clone(), cmd.clone());
        }
    }
    map
}

/// 宣言 + 拡張子既定 + オーバーライドから解決
pub fn resolve(
    path: &Path,
    head: &str,
    ext_defaults: &BTreeMap<String, String>,
    profile: Option<&str>,
    command_override: Option<&str>,
) -> Result<Resolution, RunnerError> {
    let file_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let decls = parse_declarations(head);
    let warnings = decls.warnings;

    // 全プロファイルの RunPlan を構築（ドロップダウン用）
    let mut all_profiles: Vec<RunPlan> = Vec::new();

    // 1. コマンドオーバーライド
    if let Some(cmd_override) = command_override {
        let plan = RunPlan {
            profile: profile.unwrap_or("default").to_string(),
            command: expand_variables(cmd_override, path),
            cwd: file_dir.clone(),
            shell: None,
            source: RunSource::Override,
        };
        all_profiles.push(plan.clone());
        return Ok(Resolution {
            plan,
            all_profiles,
            warnings,
        });
    }

    // 2. 宣言プロファイル
    // 共通 cwd（無添字 tako:cwd）
    let common_cwd = decls
        .profiles
        .iter()
        .find(|p| p.name == "default")
        .and_then(|p| p.cwd.as_deref());

    for decl in &decls.profiles {
        if let Some(run_cmd) = &decl.run {
            // cwd 解決: プロファイル別 → 共通 → ファイルのディレクトリ
            let raw_cwd = decl
                .cwd
                .as_deref()
                .or(if decl.name != "default" {
                    common_cwd
                } else {
                    None
                })
                .or(common_cwd);

            let resolved_cwd = if let Some(cwd_str) = raw_cwd {
                let expanded = expand_variables(cwd_str, path);
                // シングルクオート除去（expand_variables がクオートするが cwd はパスとして使う）
                let cleaned = strip_quotes(&expanded);
                let cwd_path = Path::new(&cleaned);
                if cwd_path.is_absolute() {
                    cwd_path.to_path_buf()
                } else {
                    file_dir.join(cwd_path)
                }
            } else {
                file_dir.clone()
            };

            all_profiles.push(RunPlan {
                profile: decl.name.clone(),
                command: expand_variables(run_cmd, path),
                cwd: resolved_cwd,
                shell: decl.shell.clone(),
                source: RunSource::Declaration,
            });
        }
    }

    // 3. 拡張子既定（宣言がない場合のフォールバック）
    if all_profiles.is_empty() && !ext.is_empty() {
        if let Some(default_cmd) = ext_defaults.get(&ext) {
            all_profiles.push(RunPlan {
                profile: "default".to_string(),
                command: expand_variables(default_cmd, path),
                cwd: file_dir.clone(),
                shell: None,
                source: RunSource::ExtensionDefault,
            });
        }
    }

    // 指定されたプロファイルの選択
    if let Some(profile_name) = profile {
        // 明示指定時は宣言からのみ探す（拡張子既定へフォールバックしない）
        let plan = all_profiles
            .iter()
            .find(|p| p.profile == profile_name)
            .cloned()
            .ok_or_else(|| RunnerError::ProfileNotFound(profile_name.to_string()))?;
        return Ok(Resolution {
            plan,
            all_profiles,
            warnings,
        });
    }

    // 既定選択: 無添字 `tako:run:` があればそれ、なければ最初に宣言されたプロファイル
    let plan = all_profiles
        .iter()
        .find(|p| p.profile == "default")
        .or_else(|| all_profiles.first())
        .cloned()
        .ok_or_else(|| {
            let hint = if ext.is_empty() {
                "ファイル先頭に `tako:run: <コマンド>` を書くか、\
                     `tako run-default <拡張子> \"<コマンド>\"` で拡張子既定を設定してください"
                    .to_string()
            } else {
                format!(
                    "ファイル先頭に `tako:run: <コマンド>` を書くか、\
                     `tako run-default {ext} \"<コマンド>\"` で拡張子既定を設定してください"
                )
            };
            RunnerError::NoCommand(hint)
        })?;

    Ok(Resolution {
        plan,
        all_profiles,
        warnings,
    })
}

/// シングルクオートの除去（expand_variables がパスをクオートするが cwd では不要）
fn strip_quotes(s: &str) -> String {
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        // エスケープされたクオートも元に戻す
        s[1..s.len() - 1].replace("'\\''", "'")
    } else {
        s.to_string()
    }
}

// --- テスト ---

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // --- parse_declarations ---

    #[test]
    fn 接頭辞バリエーション_ハッシュ() {
        let head = "# tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("echo hello"));
    }

    #[test]
    fn 接頭辞バリエーション_ダブルスラッシュ() {
        let head = "// tako:run: gcc main.c\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("gcc main.c"));
    }

    #[test]
    fn 接頭辞バリエーション_パーセント() {
        let head = "% tako:run: latexmk -pdf report.tex\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn 接頭辞バリエーション_ダブルダッシュ() {
        let head = "-- tako:run: ghci main.hs\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn 接頭辞バリエーション_セミコロン() {
        let head = "; tako:run: clisp main.lisp\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn 接頭辞バリエーション_html() {
        let head = "<!-- tako:run: open index.html -->\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("open index.html"));
    }

    #[test]
    fn 接頭辞バリエーション_css_block_comment() {
        let head = "/* tako:run: sass style.scss */\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("sass style.scss"));
    }

    #[test]
    fn 誤検知拒否_mytako() {
        let head = "mytako:run: should not match\n";
        let decls = parse_declarations(head);
        assert!(decls.profiles.is_empty());
    }

    #[test]
    fn 誤検知拒否_65行目は範囲外() {
        let mut head = String::new();
        for i in 1..=64 {
            head.push_str(&format!("// line {i}\n"));
        }
        head.push_str("# tako:run: should be ignored\n");
        let decls = parse_declarations(&head);
        assert!(decls.profiles.is_empty());
    }

    #[test]
    fn 誤検知拒否_大文字は無視() {
        let head = "# TAKO:RUN: echo hello\n";
        let decls = parse_declarations(head);
        assert!(decls.profiles.is_empty());
    }

    #[test]
    fn プロファイル複数_順序保持() {
        let head = "\
// tako:run: echo default
// tako:run[test]: echo test
// tako:run[build]: echo build
";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 3);
        assert_eq!(decls.profiles[0].name, "default");
        assert_eq!(decls.profiles[1].name, "test");
        assert_eq!(decls.profiles[2].name, "build");
    }

    #[test]
    fn 後勝ち_警告あり() {
        let head = "\
# tako:run: first
# tako:run: second
";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("second"));
        assert_eq!(decls.warnings.len(), 1);
        assert!(decls.warnings[0].contains("重複"));
    }

    #[test]
    fn cwd宣言() {
        let head = "\
# tako:run: echo hello
# tako:cwd: ../build
";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles[0].cwd.as_deref(), Some("../build"));
    }

    #[test]
    fn プロファイル別cwd() {
        let head = "\
// tako:run[build]: make
// tako:cwd[build]: ../..
// tako:run: cc main.c
";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 2);
        let build = decls.profiles.iter().find(|p| p.name == "build").unwrap();
        assert_eq!(build.cwd.as_deref(), Some("../.."));
        let default = decls.profiles.iter().find(|p| p.name == "default").unwrap();
        assert!(default.cwd.is_none());
    }

    #[test]
    fn shell宣言() {
        let head = "# tako:shell: fish\n# tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles[0].shell.as_deref(), Some("fish"));
    }

    #[test]
    fn crlf許容() {
        let head = "# tako:run: echo hello\r\n# tako:cwd: .\r\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("echo hello"));
    }

    #[test]
    fn bom許容() {
        let head = "\u{feff}# tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn jinja_closer() {
        let head = "{# tako:run: python3 app.py #}\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
        assert_eq!(decls.profiles[0].run.as_deref(), Some("python3 app.py"));
    }

    // --- expand_variables ---

    #[test]
    fn 変数展開_基本() {
        let path = PathBuf::from("/Users/a/src/main.c");
        let result = expand_variables("cc ${fileBase} -o ${fileNoExt}", &path);
        assert_eq!(result, "cc main.c -o main");
    }

    #[test]
    fn 変数展開_フルパス() {
        let path = PathBuf::from("/Users/a/src/main.c");
        let result = expand_variables("${file}", &path);
        assert_eq!(result, "/Users/a/src/main.c");
    }

    #[test]
    fn 変数展開_拡張子() {
        let path = PathBuf::from("/tmp/test.PY");
        let result = expand_variables("${ext}", &path);
        assert_eq!(result, "py");
    }

    #[test]
    fn 変数展開_空白パスはクオート() {
        let path = PathBuf::from("/Users/a/my project/main tool.c");
        let result = expand_variables("cc ${fileBase}", &path);
        assert_eq!(result, "cc 'main tool.c'");
    }

    #[test]
    fn 変数展開_日本語パス() {
        let path = PathBuf::from("/Users/a/ドキュメント/テスト.py");
        let result = expand_variables("python3 ${fileBase}", &path);
        assert_eq!(result, "python3 'テスト.py'");
    }

    #[test]
    fn 変数展開_シングルクオート含み() {
        let path = PathBuf::from("/tmp/it's.sh");
        let result = expand_variables("bash ${fileBase}", &path);
        assert_eq!(result, "bash 'it'\\''s.sh'");
    }

    #[test]
    fn 変数展開_未知変数はそのまま() {
        let path = PathBuf::from("/tmp/test.sh");
        let result = expand_variables("echo ${HOME} ${fileBase}", &path);
        assert_eq!(result, "echo ${HOME} test.sh");
    }

    #[test]
    #[allow(non_snake_case)]
    fn 変数展開_fileDir() {
        let path = PathBuf::from("/Users/a/src/main.c");
        let result = expand_variables("${fileDir}", &path);
        assert_eq!(result, "/Users/a/src");
    }

    // --- resolve ---

    #[test]
    fn 解決_宣言優先() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "# tako:run: python3 -u ${fileBase}\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.source, RunSource::Declaration);
        assert_eq!(res.plan.command, "python3 -u test.py");
    }

    #[test]
    fn 解決_拡張子既定フォールバック() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "#!/usr/bin/env python3\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.source, RunSource::ExtensionDefault);
        assert_eq!(res.plan.command, "python3 test.py");
    }

    #[test]
    fn 解決_オーバーライド最優先() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "# tako:run: python3 ${fileBase}\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, Some("echo overridden")).unwrap();
        assert_eq!(res.plan.source, RunSource::Override);
        assert_eq!(res.plan.command, "echo overridden");
    }

    #[test]
    fn 解決_プロファイル指定() {
        let path = PathBuf::from("/tmp/main.c");
        let head = "\
// tako:run: cc ${fileBase} -o ${fileNoExt}
// tako:run[test]: make test
";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, Some("test"), None).unwrap();
        assert_eq!(res.plan.profile, "test");
        assert_eq!(res.plan.command, "make test");
    }

    #[test]
    fn 解決_プロファイル不一致はエラー() {
        let path = PathBuf::from("/tmp/main.c");
        let head = "// tako:run: cc main.c\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let err = resolve(&path, head, &defaults, Some("nonexistent"), None).unwrap_err();
        assert!(matches!(err, RunnerError::ProfileNotFound(_)));
    }

    #[test]
    fn 解決_宣言なし_既定なし_エラー() {
        let path = PathBuf::from("/tmp/Makefile");
        let head = "all:\n\techo hello\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let err = resolve(&path, head, &defaults, None, None).unwrap_err();
        assert!(matches!(err, RunnerError::NoCommand(_)));
    }

    #[test]
    fn 解決_cwd_ファイルディレクトリ既定() {
        let path = PathBuf::from("/tmp/src/main.py");
        let head = "# tako:run: python3 ${fileBase}\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/src"));
    }

    #[test]
    fn 解決_cwd_相対パス() {
        let path = PathBuf::from("/tmp/src/main.c");
        let head = "// tako:run[build]: make\n// tako:cwd[build]: ..\n";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, Some("build"), None).unwrap();
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/src/.."));
    }

    #[test]
    fn 解決_ユーザー既定が組み込みを上書き() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "";
        let mut user = BTreeMap::new();
        user.insert("py".to_string(), "python ${fileBase}".to_string());
        let defaults = merged_defaults(&user);
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.plan.command, "python test.py");
    }

    #[test]
    fn 解決_空文字列で無効化() {
        let path = PathBuf::from("/tmp/test.py");
        let head = "";
        let mut user = BTreeMap::new();
        user.insert("py".to_string(), String::new());
        let defaults = merged_defaults(&user);
        let err = resolve(&path, head, &defaults, None, None).unwrap_err();
        assert!(matches!(err, RunnerError::NoCommand(_)));
    }

    #[test]
    fn 全プロファイル一覧() {
        let path = PathBuf::from("/tmp/main.c");
        let head = "\
// tako:run: cc ${fileBase} -o ${fileNoExt} && ./${fileNoExt}
// tako:run[build]: make
// tako:run[test]: make test
";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, None, None).unwrap();
        assert_eq!(res.all_profiles.len(), 3);
        assert_eq!(res.all_profiles[0].profile, "default");
        assert_eq!(res.all_profiles[1].profile, "build");
        assert_eq!(res.all_profiles[2].profile, "test");
    }

    // --- merged_defaults ---

    #[test]
    fn 組み込み既定の存在確認() {
        let defaults = merged_defaults(&BTreeMap::new());
        assert!(defaults.contains_key("py"));
        assert!(defaults.contains_key("c"));
        assert!(defaults.contains_key("command"));
        assert!(defaults.contains_key("tex"));
    }

    // --- builtin_defaults ---

    #[test]
    fn 組み込み既定テーブル_重複なし() {
        let builtins = builtin_defaults();
        let mut seen = std::collections::HashSet::new();
        for (ext, _) in builtins {
            assert!(seen.insert(ext), "拡張子 '{ext}' が重複");
        }
    }

    // --- 接頭辞のバウンダリ ---

    #[test]
    fn 接頭辞_行頭直接() {
        let head = "tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert_eq!(decls.profiles.len(), 1);
    }

    #[test]
    fn 接頭辞_長すぎは無視() {
        // 17 文字の接頭辞（制限は 16）
        let head = "0123456789ABCDEFG tako:run: echo hello\n";
        let decls = parse_declarations(head);
        assert!(decls.profiles.is_empty());
    }

    #[test]
    fn 共通cwdはプロファイル別にもフォールバック() {
        let path = PathBuf::from("/tmp/src/main.c");
        let head = "\
// tako:cwd: /tmp/build
// tako:run: cc ${fileBase}
// tako:run[test]: make test
";
        let defaults = merged_defaults(&BTreeMap::new());
        let res = resolve(&path, head, &defaults, Some("test"), None).unwrap();
        // test プロファイルには個別 cwd がないので共通 cwd が使われる
        assert_eq!(res.plan.cwd, PathBuf::from("/tmp/build"));
    }
}
