//! テスト本体を切り出して眺める（番犬テストの共有部品）
//!
//! 待ちの形を見張る番犬（#1252 の `mouse_report_wait_watchdog` /
//! #1265 の `osc7_wait_watchdog`）は、どれも
//!
//! 1. 対象のテスト関数の**本体だけ**を切り出す
//! 2. A/B の旧経路アーム（`// … 開始` 〜 `// … 終了` で囲んだ範囲）を落とす
//!
//! という同じ前処理をする。切り出しを番犬ごとに書き直すと、片方だけ
//! アームの除去に失敗して**旧経路を本体扱いで誤検知する**ので 1 実装にする。

/// `fn <name>(` から、インデント 4 の閉じ括弧までを関数本体として切り出す。
/// テストモジュールの中の関数は必ずこの形（`rustfmt` が保証する）
pub fn body_of(src: &str, name: &str) -> Option<String> {
    let head = format!("    fn {name}() {{");
    let start = src.find(&head)?;
    let rest = &src[start..];
    let end = rest.find("\n    }\n")?;
    Some(rest[..end].to_string())
}

/// A/B の旧経路アーム（`begin` 〜 `end` のマーカーで囲んだ範囲）を落とす。
///
/// 落とした件数も返す（マーカーの綴り違いで**何も見ていない番犬**になるのを防ぐ。
/// 呼び出し側はこれが 0 でないことを別のテストで固定すること）
pub fn strip_arms(body: &str, begin: &str, end: &str) -> (String, usize) {
    let mut out = String::new();
    let mut dropped = 0usize;
    let mut inside = false;
    for line in body.lines() {
        if line.contains(begin) {
            inside = true;
            dropped += 1;
            continue;
        }
        if line.contains(end) {
            inside = false;
            continue;
        }
        if !inside {
            out.push_str(line);
            out.push('\n');
        }
    }
    (out, dropped)
}
