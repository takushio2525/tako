//! 言語サーバの本番の解決を子プロセスで走らせる e2e の番犬（#2551）
//!
//! # なぜ要るか
//!
//! 言語サーバの本番の解決（`default_launch`。`LspManager::from_env` / `LspConfig::from_env` の
//! launcher）は unix で `$SHELL -l -c 'exec …'` を起こす。差し替えの env（`TAKO_LSP_BIN_<ID>`）で
//! 偽サーバを指しても同じ。e2e がこれを子プロセスで走らせ、子が開発者の `$SHELL` と HOME を継ぐと、
//! 偽サーバの起動より前に開発者の rc が走る。rc の `pyenv init --path` が出す `pyenv rehash` は
//! 他の rehash の残したロックを最大 60 秒待つので、全体実行でだけ initialize の待ちを使い切る
//! （#2551: `issue2035_lsp_init_options` の子が「1 回目の initialize が 15s 以内に起きなかった」で
//! 落ち、単独では通った。取得つきの manager の側の同型は #2509 / #2514 = `issue1944_lsp_fetch_watchdog`）。
//!
//! # 縛ること
//!
//! - `crates/tako-control/tests/*.rs` で自分のテストバイナリを子として起こす箇所のうち、`--exact` で
//!   名指した子のテスト関数が本番の解決を呼ぶもの（直接か、同じファイルの関数を 1 段挟んで）は、
//!   同じ起こし方の中で子へ `SHELL` を渡す（rc を読まない `/bin/sh` か、主題に合わせた偽のシェル）
//! - 渡す値が `/bin/sh` なら HOME も渡す（`sh -l` は `~/.profile` を読む）
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で既知の組（本番の解決を呼ぶ子 6 つと、偽サーバを直接起こす子 1 つ）が
//! その通りに分類されることを固定し、[`逆戻りを名指しできる`] で**現行ソースから作り直した注入**が
//! file:line で名指しされることを確かめる（注入は `cargo test` の中で完結し、ファイルは触らない）。

use std::path::{Path, PathBuf};

const TESTS_DIR: &str = "crates/tako-control/tests";

/// この番犬自身（走査の語を文字列で持つので外す）
const SELF_NAME: &str = "issue2551_lsp_child_shell_watchdog.rs";

/// 本番の解決（unix はログインシェル越しに起こす）へ至る呼び出し
const PRODUCTION_RESOLVERS: &[&str] = &[
    "default_launch",
    "LspManager::from_env(",
    "LspConfig::from_env(",
];

/// 自分のテストバイナリを子として起こす形
const SPAWN: &str = "Command::new(std::env::current_exe()";

/// `--exact` の引数（リテラルの `"--exact"` をこのファイルに書かない = #2248 の番犬の走査対象に入らない）
const EXACT: &str = "\"--exact\"";

const SHELL_ENV: &str = "env(\"SHELL\",";
const PLAIN_SHELL_ENV: &str = "env(\"SHELL\", \"/bin/sh\")";
const HOME_ENV: &str = "env(\"HOME\",";

/// 本番の解決を呼ぶ子（ファイル名, 子のテスト関数）
const KNOWN_PRODUCTION: &[(&str, &str)] = &[
    (
        "issue2035_lsp_init_options.rs",
        "本番の口で送った値を出す子プロセス",
    ),
    (
        "issue1769_lsp_followup.rs",
        "子プロセス_既定の解決を1回走らせて結果を書く",
    ),
    (
        "issue1956_lsp_servers_wave2.rs",
        "子プロセス_既定の解決を1回走らせて結果を書く",
    ),
    (
        "issue1951_lsp_arduino.rs",
        "子プロセス_既定の解決で_ino_を開いて状態を書く",
    ),
    (
        "issue1951_lsp_arduino.rs",
        "子プロセス_実機の同梱物で問い合わせる",
    ),
    (
        "issue2113_lsp_sketch_headers.rs",
        "子プロセス_実機の同梱物でスケッチの_h_と_cpp_を問い合わせる",
    ),
];

/// 偽サーバを直接起こす子（本番の解決を呼ばない = SHELL を求めない）
const KNOWN_DIRECT: &[(&str, &str)] = &[(
    "issue2113_lsp_sketch_headers.rs",
    "子プロセス_スケッチの_h_を開いて受け持つ器を書く",
)];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// `crates/tako-control/tests/*.rs`（この番犬を除く）を名前順に（相対パス, 本文）
fn current() -> Vec<(String, String)> {
    let dir = workspace_root().join(TESTS_DIR);
    let mut files: Vec<(String, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{TESTS_DIR} が読める: {e}"))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?.to_string();
            let is_rs = path.extension().and_then(|e| e.to_str()) == Some("rs");
            (is_rs && name != SELF_NAME).then(|| {
                let src = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("{name} が読める: {e}"));
                (format!("{TESTS_DIR}/{name}"), src)
            })
        })
        .collect();
    files.sort();
    files
}

/// 行コメントを落とす（doc と注記に名前が出てくるので、実行されるコードだけを見る）
fn code_only(text: &str) -> String {
    text.lines()
        .map(|l| match l.find("//") {
            Some(i) if !l[..i].ends_with(':') => &l[..i],
            _ => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 宣言行 `start`（0-based）の関数の終わり（0-based）。宣言行と同じ字下げの `}`
fn fn_end(lines: &[&str], start: usize) -> usize {
    let indent = lines[start].len() - lines[start].trim_start().len();
    let close = format!("{}}}", " ".repeat(indent));
    lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map_or(lines.len() - 1, |(i, _)| i)
}

/// 関数ごとの（名前, 宣言行, 終わり）（0-based）。関数の頭は共有ヘルパで読む
/// （`pub(crate) fn` / `async fn` も頭と見る = #1496）
fn functions(lines: &[&str]) -> Vec<(String, usize, usize)> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| {
            tako_core::source_scan::fn_head_name(l).map(|n| (n.to_string(), i, fn_end(lines, i)))
        })
        .collect()
}

/// `code` が関数 `name` を呼んでいるか（`fake_config(` を `config(` と取り違えない）
fn calls(code: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    code.match_indices(&needle).any(|(i, _)| {
        !code[..i]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

fn has_resolver(code: &str) -> bool {
    PRODUCTION_RESOLVERS.iter().any(|r| code.contains(r))
}

/// `--exact` の次の文字列リテラル（子のテストの名前）
fn exact_child(segment: &str) -> Option<String> {
    let rest = &segment[segment.find(EXACT)? + EXACT.len()..];
    let rest = &rest[rest.find('"')? + 1..];
    Some(rest[..rest.find('"')?].to_string())
}

/// 子プロセスの起こし方 1 つ
#[derive(Debug)]
struct Spawn {
    file: String,
    /// `Command::new(` の行（1-based）
    line: usize,
    child: Option<String>,
    /// 子のテスト関数が見つかった
    child_found: bool,
    /// 子が本番の解決を呼ぶ（直接か、同じファイルの関数 1 段を挟んで）
    production: bool,
    /// 起こし方の範囲（`Command::new(` から、同じ関数の次の起こし方か関数の終わりまで）のコード
    segment: String,
}

/// 本番の解決を呼ぶファイルの、子プロセスの起こし方を全部
fn scan_spawns(sources: &[(String, String)]) -> Vec<Spawn> {
    let mut out = Vec::new();
    for (file, src) in sources {
        let code = code_only(src);
        if !has_resolver(&code) {
            continue;
        }
        let lines: Vec<&str> = code.lines().collect();
        let fns = functions(&lines);
        // 本番の解決を直接呼ぶ関数（子がこれを呼べば 1 段の間接）
        let helpers: Vec<&str> = fns
            .iter()
            .filter(|(_, s, e)| has_resolver(&lines[*s..=*e].join("\n")))
            .map(|(n, _, _)| n.as_str())
            .collect();
        let spawn_lines: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains(SPAWN))
            .map(|(i, _)| i)
            .collect();
        for &at in &spawn_lines {
            let Some(&(_, _, end)) = fns
                .iter()
                .filter(|(_, s, e)| *s < at && at <= *e)
                .max_by_key(|(_, s, _)| *s)
            else {
                continue;
            };
            let stop = spawn_lines
                .iter()
                .copied()
                .find(|&next| next > at && next <= end)
                .map_or(end, |next| next - 1);
            let segment = lines[at..=stop].join("\n");
            let child = exact_child(&segment);
            let child_fn = child
                .as_deref()
                .and_then(|c| fns.iter().find(|(n, _, _)| n == c));
            let production = child_fn.is_some_and(|(name, s, e)| {
                let body = lines[*s..=*e].join("\n");
                has_resolver(&body)
                    || helpers
                        .iter()
                        .any(|h| *h != name.as_str() && calls(&body, h))
            });
            out.push(Spawn {
                file: file.clone(),
                line: at + 1,
                child,
                child_found: child_fn.is_some(),
                production,
                segment,
            });
        }
    }
    out
}

/// `file:line — 理由` の一覧（違反が無ければ空）
fn scan(sources: &[(String, String)]) -> Vec<String> {
    let mut out = Vec::new();
    for spawn in scan_spawns(sources) {
        let at = format!("{}:{}", spawn.file, spawn.line);
        let Some(child) = &spawn.child else {
            out.push(format!(
                "{at} — 子のテストを `--exact` の文字列リテラルで名指していない（走査できない）"
            ));
            continue;
        };
        if !spawn.child_found {
            out.push(format!(
                "{at} — 子のテスト `{child}` の関数が見つからない（走査が空振り）"
            ));
            continue;
        }
        if !spawn.production {
            continue;
        }
        if !spawn.segment.contains(SHELL_ENV) {
            out.push(format!(
                "{at} — 本番の解決を走らせる子 `{child}` が開発者の `$SHELL` を継ぐ（`$SHELL -l -c` で rc が\
                 偽サーバより先に走り、`pyenv rehash` のロック待ちで initialize の待ちを使い切る = #2551。\
                 子へ `SHELL` を `/bin/sh` か偽のシェルで渡す）"
            ));
        } else if spawn.segment.contains(PLAIN_SHELL_ENV) && !spawn.segment.contains(HOME_ENV) {
            out.push(format!(
                "{at} — 子 `{child}` へ `SHELL=/bin/sh` を渡すが HOME が開発者のまま（`sh -l` は\
                 `~/.profile` を読む = #2551。子へ一時の HOME を渡す）"
            ));
        }
    }
    out
}

#[test]
fn 本番の解決を走らせる子は開発者のシェルを継がない() {
    let reports = scan(&current());
    assert!(reports.is_empty(), "違反:\n{}", reports.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let spawns = scan_spawns(&current());
    let find = |file: &str, child: &str| {
        spawns
            .iter()
            .find(|s| s.file.ends_with(file) && s.child.as_deref() == Some(child))
            .unwrap_or_else(|| panic!("{file} の子 `{child}` の起こし方が採れない: {spawns:#?}"))
    };
    for (file, child) in KNOWN_PRODUCTION {
        let spawn = find(file, child);
        assert!(
            spawn.production,
            "{file} の子 `{child}` を本番の解決を呼ぶ子と見ていない"
        );
        assert!(
            spawn.segment.contains(SHELL_ENV),
            "{file} の子 `{child}` の範囲に `SHELL` の差し替えが入っていない（範囲の取り方が狭い）"
        );
    }
    for (file, child) in KNOWN_DIRECT {
        assert!(
            !find(file, child).production,
            "{file} の子 `{child}`（偽サーバを直接起こす）を本番の解決を呼ぶ子と見ている"
        );
    }
}

/// `from` を `to` へ 1 回置き換える（注入元が現行ソースに無ければ落ちる）
fn replaced(label: &str, src: &str, from: &str, to: &str) -> String {
    assert!(
        src.contains(from),
        "{label}: 注入元 {from:?} が現行ソースに無い"
    );
    src.replacen(from, to, 1)
}

/// 注入 1 つぶん: `file` の本文を `inject` で変えたソースで、`needle` の行が名指しされること
fn assert_named(label: &str, file: &str, inject: impl Fn(&str) -> String, needle: &str) {
    let mut sources = current();
    let slot = sources
        .iter_mut()
        .find(|(f, _)| f.ends_with(file))
        .unwrap_or_else(|| panic!("{label}: {file} が無い"));
    slot.1 = inject(&slot.1);
    let line = slot
        .1
        .lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("{label}: 名指し先 {needle:?} が無い"));
    let expected = format!("{}:{line} ", slot.0);
    let reports = scan(&sources);
    let named = reports.iter().find(|r| r.starts_with(&expected));
    assert!(
        named.is_some(),
        "{label}: {expected} を名指ししていない: {reports:?}"
    );
    println!("{label}: FAILED {}", named.unwrap_or(&String::new()));
}

const E2E_2035: &str = "issue2035_lsp_init_options.rs";
const PROBE_SPAWN: &str = "let mut command = Command::new(std::env::current_exe().unwrap());";
const PROBE_SHELL: &str =
    "    if cfg!(unix) {\n        command.env(\"SHELL\", \"/bin/sh\");\n    }\n";

#[test]
fn 逆戻りを名指しできる() {
    // A. #2551 の直す前: 2035 の子が開発者の `$SHELL` を継ぐ
    let label = "A 2035 の子が $SHELL を継ぐ";
    assert_named(
        label,
        E2E_2035,
        |src| replaced(label, src, PROBE_SHELL, ""),
        PROBE_SPAWN,
    );
    // B. `/bin/sh` は渡すが HOME が開発者のまま（`~/.profile` を読む）
    let label = "B 2035 の子の HOME が開発者のまま";
    assert_named(
        label,
        E2E_2035,
        |src| replaced(label, src, "        .env(\"HOME\", &home)\n", ""),
        PROBE_SPAWN,
    );
    // C. 1951 の子（既定の解決で .ino を開く）が開発者の `$SHELL` を継ぐ
    let label = "C 1951 の子が $SHELL を継ぐ";
    assert_named(
        label,
        "issue1951_lsp_arduino.rs",
        |src| {
            replaced(
                label,
                src,
                "    if cfg!(unix) {\n        child.env(\"SHELL\", \"/bin/sh\");\n    }\n",
                "",
            )
        },
        "let mut child = std::process::Command::new(std::env::current_exe().unwrap());",
    );
    // D. 子が本番の解決を同じファイルの関数 1 段の向こうで呼び、親が `$SHELL` を継がせる
    let label = "D 1 段の間接";
    assert_named(
        label,
        E2E_2035,
        |src| {
            let src = replaced(
                label,
                src,
                "    let manager = LspManager::from_env();\n",
                "    let manager = production_manager();\n",
            );
            replaced(label, &src, PROBE_SHELL, "")
                + "\nfn production_manager() -> LspManager {\n    LspManager::from_env()\n}\n"
        },
        PROBE_SPAWN,
    );
}
