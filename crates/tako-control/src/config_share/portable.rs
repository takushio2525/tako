//! 可搬化変換（Issue #513 要件 3）とローカルフィールドの切り分け（要件 1・2）
//!
//! ## パス可搬性の設計判断
//!
//! mac は `~/Library/Application Support/tako`、Windows は `%APPDATA%\tako` と
//! **置き場所そのものが違う**。置き場所の差はカタログの相対パスで吸収できるが、
//! 設定の**中身**に書かれた絶対パス（`projects.yaml` の `cwd` 等）は吸収できない。
//!
//! 3 案を比べて **ホームディレクトリのトークン化**（`~` 表記）を採った。
//!
//! | 案 | 判定 | 理由 |
//! |---|---|---|
//! | ホームのトークン化（採用） | ◎ | 実際に差が出るのはホーム配下のパスだけ。`~` は tako がすでに `expand_tilde` で解釈している既存表記なので、共有しない運用のユーザーにも影響がない |
//! | プラットフォーム別 overlay ファイル | △ | ファイルが 2 倍になり必ずドリフトする。`platform_parity` の T6 が「正本を複製しない」と決めているのと同じ理由で退けた |
//! | 環境変数参照（`${HOME}/...`） | △ | 既存の設定ファイル表記を変えることになり、共有していないユーザーの設定まで書き換わる |
//!
//! ホーム配下でない絶対パス（`/opt/homebrew/...` 等）は可搬化できない。
//! これは共有せずデバイス側に残すべき値なので、そのまま出力し `tako config status` で
//! 「可搬でない絶対パス」として報告する。
//!
//! ## 適用範囲の非対称性（意図的）
//!
//! - **書き出し（push）は全共有ファイルに適用**する。ホームパスにはユーザー名が入るため、
//!   markdown であっても共有リポジトリへ実パスを持ち込まない（リポジトリ規約の PII 対策）
//! - **取り込み（pull）は構造化設定（yaml / json）にだけ適用**する。markdown の `~/...` は
//!   人と AI が読む文章であり、`~` のままが正しい表記だから

use std::path::Path;

/// 区切り文字。Windows 実機が無くても両方を検証できるよう、引数で受ける純粋関数にする
/// （`platform::support` の「macOS 上から Windows 側を検証できる」と同じ考え方）
pub const UNIX_SEP: char = '/';
pub const WINDOWS_SEP: char = '\\';

/// このプラットフォームの区切り文字
pub fn native_sep() -> char {
    if cfg!(windows) {
        WINDOWS_SEP
    } else {
        UNIX_SEP
    }
}

/// パストークンの終端になる文字（YAML / JSON / markdown の引用符と区切り）
fn is_path_terminator(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | ',' | ';' | ')' | ']' | '}' | '>' | '|'
        )
}

/// ホーム表記の直前に来てよい文字（パスの途中に埋まった誤検出を防ぐ）
fn is_boundary_before(c: char) -> bool {
    !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\' | '~'))
}

/// 絶対パス表記を可搬表記（`~/…`、区切りは `/`）へ置き換える。
///
/// `home` にマッチした箇所だけを対象にし、続くパストークンの区切りを `/` に正規化する。
/// 文書全体の `\` を触らないので、markdown のエスケープや正規表現を壊さない。
pub fn to_portable(text: &str, home: &str) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let home_chars: Vec<char> = home.chars().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        if matches_at(&bytes, i, &home_chars)
            && (i == 0 || is_boundary_before(bytes[i - 1]))
            && bytes
                .get(i + home_chars.len())
                .is_none_or(|c| *c == '/' || *c == '\\' || is_path_terminator(*c))
        {
            out.push('~');
            i += home_chars.len();
            // 続きのパストークンだけ区切りを正規化する
            while let Some(&c) = bytes.get(i) {
                if is_path_terminator(c) {
                    break;
                }
                out.push(if c == '\\' { '/' } else { c });
                i += 1;
            }
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// 可搬表記（`~/…`）をこのデバイスの絶対パスへ戻す。
/// 構造化設定にだけ使う（markdown には使わない。モジュール冒頭の設計判断を参照）
pub fn from_portable(text: &str, home: &str, sep: char) -> String {
    let home = home.trim_end_matches(['/', '\\']);
    if home.is_empty() {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < chars.len() {
        let next = chars.get(i + 1).copied();
        if chars[i] == '~'
            && matches!(next, Some('/') | Some('\\'))
            && (i == 0 || is_boundary_before(chars[i - 1]))
        {
            out.push_str(home);
            i += 1;
            while let Some(&c) = chars.get(i) {
                if is_path_terminator(c) {
                    break;
                }
                out.push(if c == '/' || c == '\\' { sep } else { c });
                i += 1;
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn matches_at(haystack: &[char], at: usize, needle: &[char]) -> bool {
    haystack.len() >= at + needle.len() && haystack[at..at + needle.len()] == *needle
}

/// ホーム配下でない絶対パス（可搬化できない値）を拾う。
/// 共有しても別デバイスで解決できないので、status で警告するために使う
pub fn non_portable_absolute_paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let is_unix_root = chars[i] == '/' && (i == 0 || is_boundary_before(chars[i - 1]));
        let is_win_root = chars[i].is_ascii_alphabetic()
            && matches!(chars.get(i + 1), Some(':'))
            && matches!(chars.get(i + 2), Some('\\') | Some('/'))
            && (i == 0 || is_boundary_before(chars[i - 1]));
        if is_unix_root || is_win_root {
            let start = i;
            while let Some(&c) = chars.get(i) {
                if is_path_terminator(c) {
                    break;
                }
                i += 1;
            }
            let token: String = chars[start..i].iter().collect();
            // 単独の `/` や URL の一部は拾わない
            if token.len() > 3 && !token.starts_with("//") && !found.contains(&token) {
                found.push(token);
            }
            continue;
        }
        i += 1;
    }
    found
}

// --- ローカルフィールドの切り分け ---------------------------------------------

/// フィールドパス（`accounts.*.config_dir`）を分解する
fn split_field_path(path: &str) -> Vec<&str> {
    path.split('.').filter(|s| !s.is_empty()).collect()
}

/// ファイル名から構造化フォーマットを判定する。
/// markdown 等は構造を持たないので `None`（フィールド操作の対象外）
pub fn format_of(path: &Path) -> Option<Format> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("yaml") | Some("yml") => Some(Format::Yaml),
        Some("json") => Some(Format::Json),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Yaml,
    Json,
}

/// 共有コピーからローカルフィールドを取り除く。
/// パースできない内容は**そのまま返さず失敗させる**（壊れた設定を無検査で共有に載せない）
pub fn strip_local_fields(
    content: &str,
    format: Format,
    fields: &[&str],
) -> Result<String, String> {
    if fields.is_empty() {
        return Ok(content.to_string());
    }
    match format {
        Format::Yaml => {
            let mut value: serde_yaml::Value =
                serde_yaml::from_str(content).map_err(|e| format!("YAML のパースに失敗: {e}"))?;
            for field in fields {
                yaml_remove(&mut value, &split_field_path(field));
            }
            serde_yaml::to_string(&value).map_err(|e| format!("YAML の出力に失敗: {e}"))
        }
        Format::Json => {
            let mut value: serde_json::Value =
                serde_json::from_str(content).map_err(|e| format!("JSON のパースに失敗: {e}"))?;
            for field in fields {
                json_remove(&mut value, &split_field_path(field));
            }
            serde_json::to_string_pretty(&value).map_err(|e| format!("JSON の出力に失敗: {e}"))
        }
    }
}

/// 取り込み時に、ローカルの値を共有内容へ戻す。
/// ローカルに値が無ければ何もしない（= 共有側の「欠けている」状態が残る）
pub fn restore_local_fields(
    shared: &str,
    local: Option<&str>,
    format: Format,
    fields: &[&str],
) -> Result<String, String> {
    if fields.is_empty() {
        return Ok(shared.to_string());
    }
    let Some(local) = local else {
        return Ok(shared.to_string());
    };
    match format {
        Format::Yaml => {
            let mut target: serde_yaml::Value =
                serde_yaml::from_str(shared).map_err(|e| format!("YAML のパースに失敗: {e}"))?;
            let source: serde_yaml::Value = match serde_yaml::from_str(local) {
                Ok(v) => v,
                // ローカルが壊れていても取り込みは進める（ローカル値の復元だけ諦める）
                Err(_) => return Ok(shared.to_string()),
            };
            for field in fields {
                yaml_restore(&mut target, &source, &split_field_path(field));
            }
            serde_yaml::to_string(&target).map_err(|e| format!("YAML の出力に失敗: {e}"))
        }
        Format::Json => {
            let mut target: serde_json::Value =
                serde_json::from_str(shared).map_err(|e| format!("JSON のパースに失敗: {e}"))?;
            let source: serde_json::Value = match serde_json::from_str(local) {
                Ok(v) => v,
                Err(_) => return Ok(shared.to_string()),
            };
            for field in fields {
                json_restore(&mut target, &source, &split_field_path(field));
            }
            serde_json::to_string_pretty(&target).map_err(|e| format!("JSON の出力に失敗: {e}"))
        }
    }
}

/// ローカルフィールドが**埋まっていない**具体パスを列挙する。
///
/// 別デバイスから取り込んだ直後は、資格情報の場所（`accounts.*.config_dir`）や
/// `env` がこのデバイスに無い。放置すると起動時まで気付けないので、
/// pull の結果で「このデバイスで埋める必要がある値」として報告する。
///
/// ワイルドカードを含まないフィールドは「取り込み側に元から無くて当然」なので
/// 列挙しない（`profiles/*.yaml` の `env` は無いのが普通）。
pub fn missing_local_fields(content: &str, format: Format, field: &str) -> Vec<String> {
    let path = split_field_path(field);
    if !path.contains(&"*") {
        return Vec::new();
    }
    let mut out = Vec::new();
    match format {
        Format::Yaml => {
            if let Ok(v) = serde_yaml::from_str::<serde_yaml::Value>(content) {
                yaml_collect_missing(&v, &path, &mut Vec::new(), &mut out);
            }
        }
        Format::Json => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
                json_collect_missing(&v, &path, &mut Vec::new(), &mut out);
            }
        }
    }
    out
}

fn yaml_collect_missing(
    value: &serde_yaml::Value,
    path: &[&str],
    prefix: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if *head == "*" {
        for key in yaml_keys(value) {
            let Some(child) = value.get(key.as_str()) else {
                continue;
            };
            prefix.push(key);
            yaml_collect_missing(child, rest, prefix, out);
            prefix.pop();
        }
        return;
    }
    if rest.is_empty() {
        if value.get(*head).is_none() {
            let mut parts = prefix.clone();
            parts.push((*head).to_string());
            out.push(parts.join("."));
        }
        return;
    }
    if let Some(child) = value.get(*head) {
        prefix.push((*head).to_string());
        yaml_collect_missing(child, rest, prefix, out);
        prefix.pop();
    }
}

fn json_collect_missing(
    value: &serde_json::Value,
    path: &[&str],
    prefix: &mut Vec<String>,
    out: &mut Vec<String>,
) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if *head == "*" {
        for key in json_keys(value) {
            let Some(child) = value.get(&key) else {
                continue;
            };
            prefix.push(key);
            json_collect_missing(child, rest, prefix, out);
            prefix.pop();
        }
        return;
    }
    if rest.is_empty() {
        if value.get(*head).is_none() {
            let mut parts = prefix.clone();
            parts.push((*head).to_string());
            out.push(parts.join("."));
        }
        return;
    }
    if let Some(child) = value.get(*head) {
        prefix.push((*head).to_string());
        json_collect_missing(child, rest, prefix, out);
        prefix.pop();
    }
}

/// `a.b.c` の最後の要素だけを差し替えた兄弟パスを作る（`a.b.inherit`）
pub fn sibling_path(concrete: &str, sibling: &str) -> String {
    match concrete.rfind('.') {
        Some(at) => format!("{}.{sibling}", &concrete[..at]),
        None => sibling.to_string(),
    }
}

/// 指定パスの値が真か（bool の true、または文字列 "true"）
pub fn is_truthy_at(content: &str, format: Format, path: &str) -> bool {
    let parts = split_field_path(path);
    match format {
        Format::Yaml => serde_yaml::from_str::<serde_yaml::Value>(content)
            .ok()
            .and_then(|v| yaml_get_path(&v, &parts).cloned())
            .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true")),
        Format::Json => serde_json::from_str::<serde_json::Value>(content)
            .ok()
            .and_then(|v| json_get_path(&v, &parts).cloned())
            .is_some_and(|v| v.as_bool() == Some(true) || v.as_str() == Some("true")),
    }
}

fn yaml_get_path<'a>(value: &'a serde_yaml::Value, path: &[&str]) -> Option<&'a serde_yaml::Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn json_get_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

/// 指定フィールドが内容に残っていないか検査する（テストと push 前検査で使う）
pub fn contains_field(content: &str, format: Format, field: &str) -> bool {
    let path = split_field_path(field);
    match format {
        Format::Yaml => serde_yaml::from_str::<serde_yaml::Value>(content)
            .ok()
            .is_some_and(|v| yaml_has(&v, &path)),
        Format::Json => serde_json::from_str::<serde_json::Value>(content)
            .ok()
            .is_some_and(|v| json_has(&v, &path)),
    }
}

fn yaml_keys(v: &serde_yaml::Value) -> Vec<String> {
    v.as_mapping()
        .map(|m| {
            m.keys()
                .filter_map(|k| k.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn yaml_remove(value: &mut serde_yaml::Value, path: &[&str]) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if *head == "*" {
        for key in yaml_keys(value) {
            if let Some(child) = value.get_mut(key.as_str()) {
                yaml_remove(child, rest);
            }
        }
        return;
    }
    if rest.is_empty() {
        if let Some(map) = value.as_mapping_mut() {
            map.remove(serde_yaml::Value::String((*head).to_string()));
        }
        return;
    }
    if let Some(child) = value.get_mut(*head) {
        yaml_remove(child, rest);
    }
}

fn yaml_restore(target: &mut serde_yaml::Value, source: &serde_yaml::Value, path: &[&str]) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if *head == "*" {
        for key in yaml_keys(target) {
            let Some(src_child) = source.get(key.as_str()) else {
                continue;
            };
            let src_child = src_child.clone();
            if let Some(child) = target.get_mut(key.as_str()) {
                yaml_restore(child, &src_child, rest);
            }
        }
        return;
    }
    if rest.is_empty() {
        if let Some(v) = source.get(*head) {
            let v = v.clone();
            if let Some(map) = target.as_mapping_mut() {
                map.insert(serde_yaml::Value::String((*head).to_string()), v);
            }
        }
        return;
    }
    let Some(src_child) = source.get(*head).cloned() else {
        return;
    };
    if let Some(child) = target.get_mut(*head) {
        yaml_restore(child, &src_child, rest);
    }
}

fn yaml_has(value: &serde_yaml::Value, path: &[&str]) -> bool {
    let Some((head, rest)) = path.split_first() else {
        return true;
    };
    if *head == "*" {
        return yaml_keys(value)
            .iter()
            .any(|k| value.get(k.as_str()).is_some_and(|c| yaml_has(c, rest)));
    }
    value.get(*head).is_some_and(|c| yaml_has(c, rest))
}

fn json_keys(v: &serde_json::Value) -> Vec<String> {
    v.as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn json_remove(value: &mut serde_json::Value, path: &[&str]) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if *head == "*" {
        for key in json_keys(value) {
            if let Some(child) = value.get_mut(&key) {
                json_remove(child, rest);
            }
        }
        return;
    }
    if rest.is_empty() {
        if let Some(map) = value.as_object_mut() {
            map.remove(*head);
        }
        return;
    }
    if let Some(child) = value.get_mut(*head) {
        json_remove(child, rest);
    }
}

fn json_restore(target: &mut serde_json::Value, source: &serde_json::Value, path: &[&str]) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if *head == "*" {
        for key in json_keys(target) {
            let Some(src_child) = source.get(&key).cloned() else {
                continue;
            };
            if let Some(child) = target.get_mut(&key) {
                json_restore(child, &src_child, rest);
            }
        }
        return;
    }
    if rest.is_empty() {
        if let Some(v) = source.get(*head).cloned() {
            if let Some(map) = target.as_object_mut() {
                map.insert((*head).to_string(), v);
            }
        }
        return;
    }
    let Some(src_child) = source.get(*head).cloned() else {
        return;
    };
    if let Some(child) = target.get_mut(*head) {
        json_restore(child, &src_child, rest);
    }
}

fn json_has(value: &serde_json::Value, path: &[&str]) -> bool {
    let Some((head, rest)) = path.split_first() else {
        return true;
    };
    if *head == "*" {
        return json_keys(value)
            .iter()
            .any(|k| value.get(k).is_some_and(|c| json_has(c, rest)));
    }
    value.get(*head).is_some_and(|c| json_has(c, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC_HOME: &str = "/Users/alice";
    const WIN_HOME: &str = "C:\\Users\\alice";

    #[test]
    fn macのホームパスが可搬表記になる() {
        let input = "cwd: /Users/alice/dev/tako\nother: /opt/homebrew/bin/git\n";
        let out = to_portable(input, MAC_HOME);
        assert!(out.contains("cwd: ~/dev/tako"), "{out}");
        // ホーム配下でない絶対パスは触らない
        assert!(out.contains("/opt/homebrew/bin/git"), "{out}");
    }

    /// **Windows 実機が無くても検証できる**のがこの設計の肝
    #[test]
    fn windowsのホームパスも可搬表記になり区切りが正規化される() {
        let input = "cwd: C:\\Users\\alice\\dev\\tako\n";
        let out = to_portable(input, WIN_HOME);
        assert_eq!(out, "cwd: ~/dev/tako\n", "{out}");
    }

    #[test]
    fn 可搬表記から各プラットフォームへ戻せる() {
        let portable = "cwd: ~/dev/tako\n";
        assert_eq!(
            from_portable(portable, MAC_HOME, UNIX_SEP),
            "cwd: /Users/alice/dev/tako\n"
        );
        assert_eq!(
            from_portable(portable, WIN_HOME, WINDOWS_SEP),
            "cwd: C:\\Users\\alice\\dev\\tako\n"
        );
    }

    /// mac で書いた設定が Windows で開けること = Issue #513 要件 3 の核心
    #[test]
    fn mac往復とwindows往復で同じ可搬表記に収束する() {
        let mac_local = "cwd: /Users/alice/dev/tako\n";
        let portable = to_portable(mac_local, MAC_HOME);
        // Windows デバイスで取り込む
        let win_local = from_portable(&portable, WIN_HOME, WINDOWS_SEP);
        assert_eq!(win_local, "cwd: C:\\Users\\alice\\dev\\tako\n");
        // Windows で push し直しても同じ可搬表記に戻る（無限に差分が出ない）
        assert_eq!(to_portable(&win_local, WIN_HOME), portable);
    }

    #[test]
    fn 可搬化は冪等() {
        let input = "cwd: /Users/alice/x\n";
        let once = to_portable(input, MAC_HOME);
        assert_eq!(to_portable(&once, MAC_HOME), once);
    }

    #[test]
    fn 似た名前のパスを誤って置換しない() {
        // /Users/alice2 は /Users/alice の前方一致だが別ユーザー
        let input = "a: /Users/alice2/x\nb: /Volumes/Users/alice/x\n";
        let out = to_portable(input, MAC_HOME);
        assert_eq!(out, input, "誤置換された: {out}");
    }

    #[test]
    fn 引用符で終わるパストークンを正しく切る() {
        let input = "{\"cwd\": \"/Users/alice/dev/tako\", \"n\": 1}";
        let out = to_portable(input, MAC_HOME);
        assert_eq!(out, "{\"cwd\": \"~/dev/tako\", \"n\": 1}");
    }

    #[test]
    fn チルダの誤展開を避ける() {
        // 文章中の `~` は展開しない（次が `/` のときだけ）
        let input = "約 ~10 件。~/dev は展開する\n";
        let out = from_portable(input, MAC_HOME, UNIX_SEP);
        assert!(out.contains("約 ~10 件"), "{out}");
        assert!(out.contains("/Users/alice/dev"), "{out}");
    }

    #[test]
    fn 可搬でない絶対パスを検出できる() {
        let found = non_portable_absolute_paths("a: /opt/homebrew/bin/git\nb: ~/dev/x\n");
        assert_eq!(found, vec!["/opt/homebrew/bin/git".to_string()]);
    }

    #[test]
    fn yamlのローカルフィールドを取り除ける() {
        let yaml = "effort: max\nenv:\n  API_KEY: secret-value\ncwd: ~/dev\n";
        let out = strip_local_fields(yaml, Format::Yaml, &["env"]).unwrap();
        assert!(!out.contains("secret-value"), "{out}");
        assert!(!out.contains("API_KEY"), "{out}");
        assert!(out.contains("effort: max"), "{out}");
        assert!(!contains_field(&out, Format::Yaml, "env"));
    }

    #[test]
    fn ワイルドカードでネストしたフィールドを取り除ける() {
        let yaml = "accounts:\n  univ:\n    config_dir: ~/.claude-univ\n    description: 大学\n  personal:\n    inherit: true\n";
        let out = strip_local_fields(yaml, Format::Yaml, &["accounts.*.config_dir"]).unwrap();
        assert!(!out.contains("config_dir"), "{out}");
        assert!(out.contains("description"), "{out}");
        assert!(out.contains("inherit"), "{out}");
        assert!(!contains_field(&out, Format::Yaml, "accounts.*.config_dir"));
    }

    #[test]
    fn 取り込み時にローカル値が復元される() {
        let shared = "accounts:\n  univ:\n    description: 大学（共有側で更新）\n";
        let local =
            "accounts:\n  univ:\n    config_dir: ~/.claude-univ-local\n    description: 旧\n";
        let out = restore_local_fields(
            shared,
            Some(local),
            Format::Yaml,
            &["accounts.*.config_dir"],
        )
        .unwrap();
        assert!(out.contains("~/.claude-univ-local"), "{out}");
        assert!(out.contains("大学（共有側で更新）"), "{out}");
    }

    #[test]
    fn ローカルに値がなければ復元しない() {
        let shared = "accounts:\n  univ:\n    description: 大学\n";
        let out =
            restore_local_fields(shared, None, Format::Yaml, &["accounts.*.config_dir"]).unwrap();
        assert!(!out.contains("config_dir"), "{out}");
    }

    #[test]
    fn jsonのローカルフィールドを取り除いて復元できる() {
        let json = r#"{"theme":"dark","welcome_dismissed":true}"#;
        let stripped = strip_local_fields(json, Format::Json, &["welcome_dismissed"]).unwrap();
        assert!(!stripped.contains("welcome_dismissed"), "{stripped}");
        let restored = restore_local_fields(
            &stripped,
            Some(r#"{"welcome_dismissed":false}"#),
            Format::Json,
            &["welcome_dismissed"],
        )
        .unwrap();
        assert!(
            restored.contains("\"welcome_dismissed\": false"),
            "{restored}"
        );
    }

    #[test]
    fn 壊れた構造化設定は共有に載せず失敗する() {
        let broken = "accounts:\n  - [unbalanced\n";
        assert!(strip_local_fields(broken, Format::Yaml, &["accounts.*.x"]).is_err());
    }

    #[test]
    fn 取り込み後に埋めるべき値を列挙できる() {
        let shared =
            "accounts:\n  univ:\n    description: 大学\n  personal:\n    config_dir: ~/.claude-p\n";
        let missing = missing_local_fields(shared, Format::Yaml, "accounts.*.config_dir");
        assert_eq!(missing, vec!["accounts.univ.config_dir".to_string()]);
    }

    #[test]
    fn 兄弟キーによる免除判定ができる() {
        let content = "accounts:\n  personal:\n    inherit: true\n  univ:\n    description: x\n";
        assert_eq!(
            sibling_path("accounts.personal.config_dir", "inherit"),
            "accounts.personal.inherit"
        );
        // inherit: true のアカウントは config_dir を持たないのが正しい
        assert!(is_truthy_at(
            content,
            Format::Yaml,
            "accounts.personal.inherit"
        ));
        assert!(!is_truthy_at(
            content,
            Format::Yaml,
            "accounts.univ.inherit"
        ));
    }

    #[test]
    fn ワイルドカードのないフィールドは不足として報告しない() {
        // profile の env は「無いのが普通」なので、pull のたびに警告を出さない
        let shared = "effort: max\n";
        assert!(missing_local_fields(shared, Format::Yaml, "env").is_empty());
    }

    #[test]
    fn 拡張子からフォーマットを判定する() {
        assert_eq!(format_of(Path::new("a/b.yaml")), Some(Format::Yaml));
        assert_eq!(format_of(Path::new("a/b.json")), Some(Format::Json));
        assert_eq!(format_of(Path::new("a/CLAUDE.md")), None);
    }
}
