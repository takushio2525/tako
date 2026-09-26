//! 番犬: PWA の e2e（Playwright）が**ユーザーのホームへ書かない**・モックの版が古びない（#1749）。
//!
//! ## 何を止めたいのか
//!
//! #1749 の修正前、`cd web/tako-remote && npm run e2e` を回すだけで、一時 `HOME` の実測で
//! **PNG 80 枚**がホーム配下へ書かれていた（`~/Desktop/tako-28{4,5}-evidence/` に 16 枚、
//! `~/dev/tako-evidence/<Issue 番号>/` の 10 dir に 64 枚）。spec ごとに
//! `process.env.HOME + '/Desktop/…'` を組み立てていたのが原因で、スクショには環境の情報が
//! 写りうる。直した後の置き場は `e2e/support.js` の `evidencePath()` の 1 実装で、既定は
//! Playwright の outputDir（`test-results/`。run の開始時に Playwright が消す）。
//!
//! もう 1 つは版。PWA は `/api/me` の版とビルドに埋め込んだ版（`__TAKO_VERSION__`）を比べて
//! 「アプリの表示が古い可能性があります」のバナーを出す。モックが `version: '0.8.12'` を
//! 手で書いていたので、版を上げるたびにモックの画面すべてへバナーが写った。
//!
//! ここはそれをソース走査で押さえる。**落ちるときは file:line で名指し**。
//!
//! 1. e2e がホームを組み立てない（`process.env.HOME` / `homedir()` / `Desktop`）
//! 2. スクショは `evidencePath(` を通し、ファイルを書く口を spec に生やさない
//! 3. `evidencePath` の既定が outputDir（`test.info().outputPath(`）のまま
//! 4. モックの版を数字のリテラルで書かず、ビルドの版と同じ 1 実装から取る
//! 5. `TAKO_EVIDENCE_DIR` の下へ spec 名のサブ dir を挟む（#1775: 平置きだと `01-list.png` を
//!    撮る spec 同士が上書きし合い、80 回撮って 76 枚しか残らなかった）
//! 6. 同じ spec の中で同じ名前を 2 回撮らない（5 のサブ dir は spec をまたぐ衝突しか防げない）
//!
//! ## 相方（実挙動）
//!
//! 一時 `HOME` で `npm run e2e` を回し、`find "$HOME" -mindepth 1` が空であることを実測する
//! （手順は `web/tako-remote/README.md`「スクリーンショットの出力先」）。
//!
//! ## A/B（検出力の確かめ方）
//!
//! 下の注入を 1 つずつ入れると、対応するテストが file:line を名指して FAILED になる:
//! ① どれかの spec に `const D = process.env.HOME + '/Desktop/x';` を足す → 1 /
//! ② `path: evidencePath('01.png')` を `` path: `/tmp/x/01.png` `` へ → 2 /
//! ③ `support.js` の `test.info().outputPath(name)` を `join('/tmp', name)` へ → 3 /
//! ④ モックの `version: TAKO_VERSION` を `version: '0.8.12'` へ → 4 /
//! ⑤ `support.js` の `join(dir, specDir(), name)` を `join(dir, name)` へ → 5 /
//! ⑥ `specDir()` の本体を `return 'evidence';` へ → 5 /
//! ⑦ どれかの spec の 2 つの `evidencePath('…')` を同じ名前にする → 6

use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const E2E_REL: &str = "web/tako-remote/e2e";
const SUPPORT_REL: &str = "web/tako-remote/e2e/support.js";
const VITE_REL: &str = "web/tako-remote/vite.config.js";
const VERSION_REL: &str = "web/tako-remote/workspace-version.js";
const PLAYWRIGHT_REL: &str = "web/tako-remote/playwright.config.js";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with('*') || t.starts_with("/*")
}

fn is_js(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "mjs" | "cjs" | "ts" | "jsx")
    )
}

/// `e2e/` 配下の JS（fixtures の画像・動画は拾わない）+ playwright.config.js。
/// **1 本も拾えないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn e2e_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} を読む: {e}"));
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if is_js(&path) {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join(E2E_REL), &mut files);
    files.push(root.join(PLAYWRIGHT_REL));
    files.sort();
    let sources: Vec<(String, String)> = files
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .expect("リポジトリ配下")
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{rel} を読む: {e}"));
            (rel, src)
        })
        .collect();
    let specs = sources
        .iter()
        .filter(|(rel, _)| rel.ends_with(".spec.js"))
        .count();
    assert!(
        specs >= 10,
        "{E2E_REL}: spec が {specs} 本しか拾えない（走査範囲が壊れている）"
    );
    assert!(
        sources.iter().any(|(rel, _)| rel == SUPPORT_REL),
        "{SUPPORT_REL} を拾えない（走査範囲が壊れている）"
    );
    sources
}

/// コメント行を除いた (行番号, 行) の列
fn code_lines(src: &str) -> impl Iterator<Item = (usize, &str)> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l))
        .map(|(i, l)| (i + 1, l))
}

// --------------------------------------------- 1. ホームを組み立てない

/// ホームを指す材料。`HOME` という名前の**モック定数**（`const HOME = '/Users/testuser'`）は
/// 実環境を読まないので拾わない（読むのは `process.env` 経由の値だけ）。
/// `'~/…'` も拾わない: Node は `~` を展開しないので書き込み先にならず、e2e では
/// 画面に出す表示用の文字列（`display_path: '~/dev/tako'` / `'~/.ssh/config'`）にしか現れない
const HOME_NEEDLES: &[&str] = &[
    "process.env.HOME",
    "env.HOME",
    "env['HOME']",
    "env[\"HOME\"]",
    "USERPROFILE",
    "homedir(",
    "/Desktop",
    "Desktop/",
];

/// 注入 ①: spec にホーム（デスクトップ）を組み立てる行を戻す
#[test]
fn e2eはホームを組み立てない() {
    let mut hits = Vec::new();
    for (rel, src) in e2e_sources() {
        for (n, line) in code_lines(&src) {
            if let Some(needle) = HOME_NEEDLES.iter().find(|nd| line.contains(*nd)) {
                hits.push(format!("{rel}:{n}: `{needle}` — {}", line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "PWA の e2e がホーム配下の置き場を組み立てている（#1749: `npm run e2e` を回すだけで \
         ユーザーのデスクトップ / ホームへ PNG が書かれる）:\n  {}\n\
         → スクショは `evidencePath('<名前>.png')`（{SUPPORT_REL}。既定は Playwright の outputDir）を通す。\
         リポの外へ残したいときは実行する人が `TAKO_EVIDENCE_DIR` を渡す",
        hits.join("\n  ")
    );
}

// --------------------------------------------- 2. 書く口は evidencePath だけ

/// `.screenshot(` の呼び出し 1 つ分の本文（開き括弧から対応する閉じ括弧まで。複数行も拾う）
fn call_text(src: &str, open: usize) -> &str {
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open..open + i + 1];
                }
            }
            _ => {}
        }
    }
    &src[open..]
}

fn line_at(src: &str, byte: usize) -> usize {
    src[..byte].matches('\n').count() + 1
}

/// spec にファイルを書く口を生やさない（スクショ以外の経路でホームへ書かないため）
const WRITE_NEEDLES: &[&str] = &[
    "writeFile",
    "appendFile",
    "mkdir",
    "createWriteStream",
    "copyFile",
    "recordVideo",
    "recordHar",
    ".saveAs(",
];

/// 注入 ②: スクショの `path:` を `evidencePath(` 以外で組み立てる
#[test]
fn スクショの置き場はevidencepathだけ() {
    let mut shots = 0usize;
    let mut hits = Vec::new();
    for (rel, src) in e2e_sources() {
        let commented: Vec<bool> = src.lines().map(is_comment).collect();
        let mut from = 0;
        while let Some(at) = src[from..].find(".screenshot(") {
            let open = from + at + ".screenshot".len();
            from = open;
            let n = line_at(&src, open);
            if commented[n - 1] {
                continue;
            }
            let call = call_text(&src, open);
            let Some(p) = call.find("path:") else {
                continue; // バッファで受け取るだけの呼び出しは書かない
            };
            shots += 1;
            let value = call[p + "path:".len()..].trim_start();
            if !value.starts_with("evidencePath(") {
                let line = src.lines().nth(n - 1).unwrap_or_default().trim();
                hits.push(format!("{rel}:{n}: {line}"));
            }
        }
        if rel == SUPPORT_REL {
            continue;
        }
        for (n, line) in code_lines(&src) {
            if let Some(needle) = WRITE_NEEDLES.iter().find(|nd| line.contains(*nd)) {
                hits.push(format!("{rel}:{n}: `{needle}` — {}", line.trim()));
            }
        }
    }
    assert!(
        shots >= 10,
        "{E2E_REL}: `path:` 付きの `.screenshot(` が {shots} 件しか拾えない（走査範囲が壊れている）"
    );
    assert!(
        hits.is_empty(),
        "PWA の e2e が `evidencePath(` を通らずにファイルを書いている（#1749）:\n  {}\n\
         → `page.screenshot({{ path: evidencePath('<名前>.png') }})` の形にする（{SUPPORT_REL}）",
        hits.join("\n  ")
    );
}

// --------------------------------------------- 3. 既定は outputDir

/// `support.js` のトップレベル関数 1 つ分（`header` を含む行から、行頭の `}` まで）。
/// 返すのは (開始行の番号, 本文)
fn support_function(src: &str, header: &str) -> (usize, String) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(header))
        .unwrap_or_else(|| panic!("{SUPPORT_REL}: `{header}` が無い"));
    let end = lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, l)| **l == "}")
        .map(|(i, _)| i)
        .unwrap_or_else(|| {
            panic!(
                "{SUPPORT_REL}:{}: `{header}` を閉じる `}}` が無い",
                start + 1
            )
        });
    (start + 1, lines[start..=end].join("\n"))
}

/// 注入 ③: `evidencePath` の既定を outputDir 以外へ向ける
#[test]
fn evidencepathの既定はplaywrightのoutputdir() {
    let src = read(SUPPORT_REL);
    let (start, body) = support_function(&src, "export function evidencePath(");
    assert!(
        body.contains("test.info().outputPath(name)"),
        "{SUPPORT_REL}:{start}: evidencePath の既定が Playwright の outputDir \
         （`test.info().outputPath(name)`）ではなくなった（#1749）:\n{body}"
    );
    assert!(
        body.contains("process.env.TAKO_EVIDENCE_DIR"),
        "{SUPPORT_REL}:{start}: 置き場を変える口は実行する人が渡す `TAKO_EVIDENCE_DIR` だけ（#1749）:\n{body}"
    );
}

// --------------------------------------------- 4. モックの版はビルドの版

/// 行の中に `version` キーへ数字の文字列を直書きした箇所があれば、その列を返す。
/// `version: '0.8.12'` / `"version":"0.5.5"` の両方を拾い、`api_version: 2` は拾わない
fn literal_version(line: &str) -> Option<usize> {
    let b = line.as_bytes();
    let mut from = 0;
    while let Some(at) = line[from..].find("version") {
        let start = from + at;
        from = start + "version".len();
        if start > 0 && (b[start - 1] == b'_' || b[start - 1].is_ascii_alphanumeric()) {
            continue;
        }
        let mut i = from;
        if i < b.len() && matches!(b[i], b'\'' | b'"') {
            i += 1;
        }
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        if i >= b.len() || b[i] != b':' {
            continue;
        }
        i += 1;
        while i < b.len() && b[i] == b' ' {
            i += 1;
        }
        if i + 1 < b.len() && matches!(b[i], b'\'' | b'"' | b'`') && b[i + 1].is_ascii_digit() {
            return Some(start + 1);
        }
    }
    None
}

/// 注入 ④: モックの版を数字で直書きする
#[test]
fn モックの版はビルドの版から取る() {
    let mut hits = Vec::new();
    for (rel, src) in e2e_sources() {
        for (n, line) in code_lines(&src) {
            if let Some(col) = literal_version(line) {
                hits.push(format!("{rel}:{n}:{col}: {}", line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "PWA の e2e のモックが版を数字で直書きしている（#1749: 版を上げるたびに \
         「アプリの表示が古い可能性があります」のバナーがモックの画面へ写る）:\n  {}\n\
         → `import {{ TAKO_VERSION }} from './support.js'` を使う（ビルドへ埋め込む版と同じ読み方）",
        hits.join("\n  ")
    );

    // 埋め込む側（vite）と返す側（モック）が同じ 1 実装を通る
    let support = read(SUPPORT_REL);
    let vite = read(VITE_REL);
    let version = read(VERSION_REL);
    for (rel, src, needle) in [
        (
            SUPPORT_REL,
            &support,
            "import { workspaceVersion } from '../workspace-version.js';",
        ),
        (SUPPORT_REL, &support, "TAKO_VERSION = workspaceVersion()"),
        (
            VITE_REL,
            &vite,
            "import { workspaceVersion } from './workspace-version.js';",
        ),
        (
            VITE_REL,
            &vite,
            "__TAKO_VERSION__: JSON.stringify(workspaceVersion())",
        ),
        (VERSION_REL, &version, "export function workspaceVersion()"),
    ] {
        assert!(
            src.contains(needle),
            "{rel}: `{needle}` が無い（PWA に埋め込む版とモックの版が別々の読み方になる。#1749）"
        );
    }
    for (rel, src) in [(SUPPORT_REL, &support), (VITE_REL, &vite)] {
        if let Some((n, line)) = code_lines(src).find(|(_, l)| l.contains("Cargo.toml")) {
            panic!(
                "{rel}:{n}: Cargo.toml を自前で読んでいる（読み方は {VERSION_REL} の 1 実装。#1749）: {}",
                line.trim()
            );
        }
    }
}

#[test]
fn 版のリテラル検出は形を見分ける() {
    assert!(literal_version("  version: '0.8.12',").is_some());
    assert!(literal_version(r#"body: '{"status":"ok","version":"0.5.5"}'"#).is_some());
    assert!(literal_version("json(route, { status: 'ok', version: \"1.0.0\" })").is_some());
    assert!(literal_version("  version: TAKO_VERSION,").is_none());
    assert!(literal_version("{ api_version: 2, panes: [] }").is_none());
    assert!(literal_version("const apiversion = '1';").is_none());
}

// --------------------------------------------- 5. TAKO_EVIDENCE_DIR の下は spec ごとのサブ dir

/// 注入 ⑤ / ⑥: `TAKO_EVIDENCE_DIR` の下へ平置きに戻す
#[test]
fn evidence_dirの下はspecごとのサブdir() {
    let src = read(SUPPORT_REL);
    let (start, body) = support_function(&src, "export function evidencePath(");
    let joins: Vec<(usize, &str)> = body
        .lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains("join(dir"))
        .map(|(i, l)| (start + i, l.trim()))
        .collect();
    assert!(
        !joins.is_empty(),
        "{SUPPORT_REL}:{start}: evidencePath が `TAKO_EVIDENCE_DIR`（`dir`）から置き場を組んでいない（#1775）:\n{body}"
    );
    for (n, line) in joins {
        assert!(
            line.contains("join(dir, specDir(),"),
            "{SUPPORT_REL}:{n}: `TAKO_EVIDENCE_DIR` の下へ spec 名のサブ dir を挟んでいない（#1775: \
             渡した dir は全 spec で共有するので、`01-list.png` を撮る spec 同士が上書きし合う）: {line}\n\
             → `join(dir, specDir(), name)` の形にする"
        );
    }
    let (at, spec_dir) = support_function(&src, "function specDir(");
    let code: String = spec_dir
        .lines()
        .filter(|l| !is_comment(l))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("info.file") && code.contains("testDir"),
        "{SUPPORT_REL}:{at}: specDir() が走っている spec のパス（`test.info().file` の testDir からの相対）\
         から組まれていない（#1775: spec によらない名前を返すと平置きと同じく上書きし合う）:\n{spec_dir}"
    );
}

// --------------------------------------------- 6. spec の中で同じ名前を 2 回撮らない

/// 注入 ⑦: 同じ spec の 2 か所で同じ名前を撮る。5 のサブ dir は spec をまたぐ衝突しか
/// 防げないので、spec の中の重複は `TAKO_EVIDENCE_DIR` を渡した run で黙って上書きになる
#[test]
fn 同じspecで同じ名前を2回撮らない() {
    let mut calls = 0usize;
    let mut hits = Vec::new();
    for (rel, src) in e2e_sources() {
        if !rel.ends_with(".spec.js") {
            continue;
        }
        let commented: Vec<bool> = src.lines().map(is_comment).collect();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut from = 0;
        // import 行の `evidencePath` は `(` が続かないので拾わない
        while let Some(at) = src[from..].find("evidencePath(") {
            let open = from + at + "evidencePath".len();
            from = open;
            let n = line_at(&src, open);
            if commented[n - 1] {
                continue;
            }
            let call = call_text(&src, open);
            let arg = call
                .strip_prefix('(')
                .and_then(|c| c.strip_suffix(')'))
                .unwrap_or(call)
                .trim()
                .to_string();
            calls += 1;
            match seen.get(&arg) {
                Some(first) => {
                    hits.push(format!("{rel}:{first} と {rel}:{n}: evidencePath({arg})"))
                }
                None => {
                    seen.insert(arg, n);
                }
            }
        }
    }
    assert!(
        calls >= 10,
        "{E2E_REL}: `evidencePath(` の呼び出しが {calls} 件しか拾えない（走査範囲が壊れている）"
    );
    assert!(
        hits.is_empty(),
        "同じ spec の中で同じ名前のスクショを 2 回撮っている（#1775: `TAKO_EVIDENCE_DIR` を渡すと \
         spec のサブ dir の中で後の 1 枚が前の 1 枚を上書きする）:\n  {}\n→ 名前を分ける",
        hits.join("\n  ")
    );
}
