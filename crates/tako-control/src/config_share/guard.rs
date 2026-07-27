//! 秘匿情報の最終防壁（Issue #513 要件 1 の多層防御）
//!
//! 一次防御はカタログ（ホワイトリスト）。ここは**二次防御**で、共有対象と分類した
//! ファイルの中身に秘匿情報らしき文字列が入っていないかを書き出し直前に検査する。
//! 引っかかったら push を**止める**（黙って除外すると同期が壊れたことに気付けない）。
//!
//! 誤検出で push が止まると実害が大きいので、パターンは
//! 「まず間違いなく秘匿」と言えるものだけに絞る。

/// 検出した 1 件
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// 共有リポジトリ内のパス
    pub path: String,
    /// 1 始まりの行番号
    pub line: u32,
    /// 何を検出したか（値そのものは絶対に含めない）
    pub kind: &'static str,
}

/// 値を持たない前置パターン。この接頭辞で始まる token はそれ自体が資格情報
const PREFIXES: &[(&str, &str)] = &[
    ("sk-ant-", "Anthropic API key"),
    ("sk-proj-", "OpenAI API key"),
    ("sk-or-v1-", "OpenRouter API key"),
    ("ghp_", "GitHub personal access token"),
    ("gho_", "GitHub OAuth token"),
    ("ghs_", "GitHub server token"),
    ("github_pat_", "GitHub fine-grained token"),
    ("xoxb-", "Slack bot token"),
    ("xoxp-", "Slack user token"),
    ("AKIA", "AWS access key id"),
    ("ASIA", "AWS temporary access key id"),
    ("AIza", "Google API key"),
    ("-----BEGIN", "private key block"),
];

/// `key: value` 形式で値が十分長ければ秘匿とみなすキー名
const SECRET_KEYS: &[&str] = &[
    "token",
    "access_token",
    "refresh_token",
    "api_key",
    "apikey",
    "secret",
    "client_secret",
    "password",
    "passwd",
];

/// プレースホルダ（実値でないと判断できるもの）
fn is_placeholder(value: &str) -> bool {
    let v = value.trim().trim_matches(['"', '\'']);
    v.is_empty()
        || v == "null"
        || v == "~"
        || v.starts_with('<')
        || v.starts_with("${")
        || v.starts_with("$(")
        || v.chars().all(|c| c == '*' || c == 'x' || c == 'X')
        || v.starts_with("REDACTED")
        || v.starts_with("changeme")
}

/// AWS のキー ID は接頭辞のあと大文字英数が続く形。`AKIA` を含む普通の英単語と区別する
fn looks_like_aws_key(token: &str, prefix: &str) -> bool {
    let rest = &token[prefix.len()..];
    rest.len() >= 16
        && rest
            .chars()
            .take(16)
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn looks_like_google_key(token: &str) -> bool {
    let rest = &token["AIza".len()..];
    rest.len() >= 30
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 1 ファイルを走査する。値そのものは戻り値にもログにも出さない
pub fn scan(path: &str, content: &str) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx as u32 + 1;
        for token in line.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',')) {
            let token = token.trim_matches(|c: char| matches!(c, '(' | ')' | '[' | ']' | '`'));
            for (prefix, kind) in PREFIXES {
                if !token.starts_with(prefix) {
                    continue;
                }
                let hit = match *prefix {
                    "AKIA" | "ASIA" => looks_like_aws_key(token, prefix),
                    "AIza" => looks_like_google_key(token),
                    "-----BEGIN" => line.contains("PRIVATE KEY"),
                    // それ以外は接頭辞のあとに実体が続いていれば十分
                    _ => token.len() > prefix.len() + 8,
                };
                if hit {
                    findings.push(Finding {
                        path: path.to_string(),
                        line: line_no,
                        kind,
                    });
                }
            }
        }
        if let Some(kind) = secret_assignment(line) {
            findings.push(Finding {
                path: path.to_string(),
                line: line_no,
                kind,
            });
        }
    }
    findings.dedup_by(|a, b| a.line == b.line && a.kind == b.kind);
    findings
}

/// `token: xxxxx` / `"api_key": "xxxxx"` のような代入行を見る。
/// 値が 16 文字以上の実体ならヒット
fn secret_assignment(line: &str) -> Option<&'static str> {
    let (raw_key, value) = line.split_once(':').or_else(|| line.split_once('='))?;
    let key = raw_key
        .trim()
        .trim_matches(['"', '\'', '-', ' '])
        .to_ascii_lowercase();
    if !SECRET_KEYS.contains(&key.as_str()) {
        return None;
    }
    let value = value.trim().trim_end_matches(',');
    if is_placeholder(value) {
        return None;
    }
    let plain = value.trim_matches(['"', '\'']);
    // 「参照」や「説明」は値として扱わない（パス・文章・真偽値）。
    // 資格情報は ASCII の連続した token なので、非 ASCII を含む行は日本語の説明文とみなす
    // （`- password: は書かないこと` のような文章で push を止めない）
    if plain.chars().count() < 16
        || !plain.chars().all(|c| c.is_ascii_graphic())
        || plain.starts_with('/')
        || plain.starts_with('~')
        || matches!(plain, "true" | "false")
    {
        return None;
    }
    Some("secret-looking assignment")
}

/// 検出結果を人が読めるエラー文にする（値は出さない）
pub fn describe(findings: &[Finding]) -> String {
    let mut lines = vec![
        "秘匿情報らしき内容が共有対象に含まれています。取り除いてから再実行してください:"
            .to_string(),
    ];
    for f in findings {
        lines.push(format!("  - {}:{} ({})", f.path, f.line, f.kind));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 代表的な資格情報を検出する() {
        // テストにも実在しうる値は書かない。形だけを模した文字列を使う
        let cases = [
            "key: sk-ant-EXAMPLEEXAMPLEEXAMPLE",
            "gh: ghp_EXAMPLEEXAMPLEEXAMPLE",
            "aws: AKIAEXAMPLEEXAMPLE12",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
            "token: EXAMPLEEXAMPLEEXAMPLEEXAMPLE",
        ];
        for case in cases {
            assert!(!scan("f", case).is_empty(), "検出できなかった: {case}");
        }
    }

    #[test]
    fn 通常の設定は誤検出しない() {
        let benign = "\
effort: max
cwd: ~/dev/tako
description: 大学のアカウント
worker_agent: claude
model: claude-opus-5
token: null
api_key: <your key here>
password: \"\"
config_dir: ~/.claude-univ
url: https://example.com/docs
secret: ${MY_SECRET}
";
        let found = scan("f", benign);
        assert!(found.is_empty(), "誤検出: {found:?}");
    }

    #[test]
    fn 日本語の説明文を誤検出しない() {
        let md = "\
# 開発ルール
- token は共有しない（`~/Library/Application Support/tako/token`）
- password: は書かないこと
- secret: 秘匿情報はコミットしないこと
";
        let found = scan("f", md);
        assert!(found.is_empty(), "誤検出: {found:?}");
    }

    #[test]
    fn 検出結果に値そのものを含めない() {
        let findings = scan("f", "token: EXAMPLEEXAMPLEEXAMPLEEXAMPLE");
        let text = describe(&findings);
        assert!(!text.contains("EXAMPLE"), "値が漏れている: {text}");
    }

    #[test]
    fn 行番号が1始まりで出る() {
        let findings = scan("f", "a: 1\nb: 2\ntoken: EXAMPLEEXAMPLEEXAMPLEEXAMPLE\n");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 3);
    }
}
