//! テストが `$TMPDIR` 直下に独自名の置き場を後始末なしで作る形の番犬（#2561 / #2566）
//!
//! # なぜ要るか
//!
//! #1312 で「テスト本体の使い捨てはスコープで消える器（`ScratchDir`）で作る」と決めたが、
//! **作る補助が素の `PathBuf` / `String` を返す**形は後始末を呼び手に預けたままになる。10/31 の実測で
//! `$TMPDIR` に 4 種類が消え残っていた（テストはどれも緑のまま）:
//!
//! - `dispatch.rs` の MCP 設定のテストの `mcp_test_dir` が `tako-mcp-test-<名前>-<pid>` を作って返し、
//!   7 本の呼び手が**どれも消していなかった**。`cargo test` 1 回で 7 個ずつ積もり 957 個
//! - `test_write_isolation.rs` の `scratch` は呼び手が子の直後に消していたが、旧挙動の腕で子が
//!   起こした実 agent CLI（agy の更新器）が**子の終了後も**空の `HOME` へ
//!   `.gemini/antigravity-cli/updater/` を書き戻し、消した後に作り直された dir が残った（12 個）
//! - tako-cli の `ipc.rs` のテストの `serve` が unix ソケット `tk2123-<pid>-<タグ>.sock` を `bind` して
//!   接続先の `String` を返し、3 本の呼び手がどれも消していなかった（#2566。333 個）
//! - dispatch のテストの補助 3 か所が、orchestrator の設定の置き場の `OnceLock` を
//!   `tako-dispatch-test-config-<pid>` で**先に**初期化していた。`config_dir` 側の初期化子
//!   （`tako-test-orchestrator-<pid>` + 終了時の後始末）より先に走った回だけ、後始末の無い dir が勝つ
//!   （#2566。テストの実行順しだいで 14 個）
//!
//! どちらも `$TMPDIR` 直下の独自名なので、#1296 の 2 段構え（終了時の `atexit` + 次回起動時の
//! pid 回収）にも `tako test-residue` の数え上げにも載らず、誰も回収しない。
//!
//! # 縛ること
//!
//! テストの補助 `fn …(…) -> PathBuf | String` が `std::env::temp_dir()` の下へ dir かソケットを作って返す
//! （`create_dir*` / `UnixListener::bind(` を呼び、`ScratchDir` / `process_scratch` / `arm_self_cleanup` を
//! 通さない）とき:
//!
//! - A. 同じファイルの呼び手のうち**消さないもの**（`remove_dir_all(` / `remove_file(` も、それを呼ぶ
//!   補助も呼ばない）があれば、補助の行と呼び手の行を file:line で名指す
//! - B. そのファイルが子プロセスへ `HOME` を差し替えて渡す（`.env("HOME"`）なら、呼び手が
//!   消していても名指す（子の終了後に孫が書き戻すと、呼び手の削除の後に作り直される）
//!
//! もう 1 つ、`OnceLock` の初期化子（`get_or_init(…)`）が `temp_dir()` の下へ dir を作るのに
//! 器も後始末も通さないとき（C）も名指す。プロセスの間ずっと使う置き場はスコープで消えないので、
//! 終了時の `atexit` も次回起動時の pid 回収も無いと必ず残る。
//!
//! 直し方は補助が `tako_core::test_residue::ScratchDir::new("<タグ>")` を返す形にする
//! （`OnceLock` に持つ置き場は `process_scratch("<タグ>")`。既に後始末を張った初期化子があるならそれに任せる）。
//! 器はスコープで消え（panic の巻き戻しでも）、消し損ねても親の `tako-test-scratch-<pid>` ごと
//! 終了時・次回起動時に回収され、`tako test-residue` からも数えられる（#1312）。
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で「作って返す補助」が現行ソースに実在し（器を返す形へ直した
//! 3 か所を含む）、消す呼び手を消すと見ていること・後始末を張った初期化子を見つけていることを固定し、
//! [`逆戻りを名指しできる`] で**現行ソースから作り直した注入**（#2561 / #2566 の直す前の形）が
//! file:line で名指しされることを確かめる
//! （注入は `cargo test` の中で完結し、ファイルは触らない）。

use std::path::{Path, PathBuf};

#[path = "common/code_view.rs"]
mod code_view;

/// この番犬自身（走査の語を文字列で持つので外す）
const SELF_NAME: &str = "issue2561_temp_dir_helper_watchdog.rs";

/// 走査しないファイル（ワークスペースルートからの相対パス, 理由）
const EXEMPT: &[(&str, &str)] = &[(
    "crates/tako-core/src/test_residue.rs",
    "器そのものの実装。A/B（TAKO_1312_LEGACY=1）の腕だけが修正前の形を $TMPDIR 直下へ作る",
)];

/// 名指さないと決めた「作って返す補助」（ファイル, 関数, 理由）。増やすときは
/// 「なぜ消えなくてよいか / なぜ消せないか」を書くこと
const EXEMPT_HELPERS: &[(&str, &str, &str)] = &[(
    "crates/tako-core/src/tmux_backend.rs",
    "injected_shell",
    "注入（#1265）を立てた回だけ作る包みのシェル。実行中のシェルが握っているので消せない（doc に明記。\
     通常の実行では 1 つも作らない）",
)];

/// 器を通す目印（どれかを通していれば「作って返す補助」ではない）
const MANAGED: &[&str] = &["ScratchDir", "process_scratch", "arm_self_cleanup"];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// `dir` の下の `.rs` を再帰で集める
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// `crates/*/{src,tests}/**/*.rs`（この番犬と [`EXEMPT`] を除く）を名前順に（相対パス, 本文）
fn current() -> Vec<(String, String)> {
    let root = workspace_root();
    let mut paths = Vec::new();
    for krate in std::fs::read_dir(root.join("crates"))
        .expect("crates/ が読める")
        .flatten()
    {
        for sub in ["src", "tests"] {
            collect_rs(&krate.path().join(sub), &mut paths);
        }
    }
    let mut files: Vec<(String, String)> = paths
        .into_iter()
        .filter_map(|path| {
            let rel = path
                .strip_prefix(&root)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            if rel.ends_with(SELF_NAME) || EXEMPT.iter().any(|(f, _)| *f == rel) {
                return None;
            }
            let src =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
            Some((rel, src))
        })
        .collect();
    files.sort();
    files
}

/// 関数 1 つ（コードだけの眺めでの位置）
#[derive(Debug)]
struct Func {
    name: String,
    /// 頭の行（1-based）
    line: usize,
    /// 頭から本体の閉じ `}` まで（コードだけの眺め）
    text: String,
}

/// コードだけの眺めから関数を全部（入れ子も含む）。本体は `{}` の対応で取る
/// （文字列とコメントは潰してあるので、中の `{` を数えない）
fn functions(code: &str) -> Vec<Func> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    for (idx, line) in code.split('\n').enumerate() {
        let start = offset;
        offset += line.len() + 1;
        let Some(name) = tako_core::source_scan::fn_head_name(line) else {
            continue;
        };
        // 本体の `{`（宣言だけの `fn x();` は `;` が先に来る）
        let tail = &code[start..];
        let Some(open) = tail
            .find(['{', ';'])
            .filter(|&i| tail.as_bytes()[i] == b'{')
        else {
            continue;
        };
        let mut depth = 0usize;
        let mut end = tail.len();
        for (i, c) in tail[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push(Func {
            name: name.to_string(),
            line: idx + 1,
            text: tail[..end].to_string(),
        });
    }
    out
}

/// `text` の中に `name(` の**素の呼び出し**（`::name(` / `.name(` / `fn name(` ではない）があるか
fn calls(text: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    text.match_indices(&needle).any(|(at, _)| {
        let before = text[..at].trim_end_matches(' ');
        let prev = before.chars().next_back();
        !matches!(prev, Some(c) if c.is_alphanumeric() || c == '_' || c == '.' || c == ':')
            && !before.ends_with("fn")
    })
}

/// 本体の頭（宣言の `{` まで）が素のパス（`PathBuf` / 接続先の `String`）を返すか
fn returns_bare_path(func: &Func) -> bool {
    let head = func.text.split('{').next().unwrap_or("");
    head.split_once("->")
        .is_some_and(|(_, ret)| matches!(ret.trim(), "PathBuf" | "std::path::PathBuf" | "String"))
}

/// `$TMPDIR` の下に置き場を作る呼び出し（dir と unix ソケット。#2566 でソケットを足した）
const MAKES: &[&str] = &["create_dir", "UnixListener::bind("];

/// 置き場を消す呼び出し
const REMOVES: &[&str] = &["remove_dir_all(", "remove_file("];

/// `$TMPDIR` の下へ dir かソケットを作って素のパスで返す補助か
fn makes_bare_temp_dir(func: &Func) -> bool {
    returns_bare_path(func)
        && func.text.contains("temp_dir()")
        && MAKES.iter().any(|m| func.text.contains(m))
        && !MANAGED.iter().any(|m| func.text.contains(m))
}

/// `text` が置き場を消す呼び出しを含むか
fn removes(text: &str) -> bool {
    REMOVES.iter().any(|r| text.contains(r))
}

/// 作って返す補助 1 つと、その診断
#[derive(Debug)]
struct Helper {
    file: String,
    line: usize,
    name: String,
    /// 消さない呼び手（`file:line fn 名前`）
    leaking: Vec<String>,
    /// 消す呼び手の数
    cleaning: usize,
    /// 子プロセスへ `HOME` を差し替えて渡すファイルか
    child_home: bool,
}

impl Helper {
    fn exempt(&self) -> bool {
        EXEMPT_HELPERS
            .iter()
            .any(|(file, name, _)| self.file == *file && self.name == *name)
    }

    fn violation(&self) -> Option<String> {
        if self.exempt() {
            return None;
        }
        if !self.leaking.is_empty() {
            return Some(format!(
                "{}:{} — fn {} が $TMPDIR の下へ置き場を作って素のパスを返し、呼び手 {} 本が消さない\
                 （cargo test のたびに $TMPDIR 直下へ積もり、終了時の後始末・次回起動時の pid 回収・\
                 tako test-residue のどれにも載らない。#2561 / #2566）: {}。\
                 直し方: 補助が tako_core::test_residue::ScratchDir::new(\"<タグ>\") を返す形にする（#1312）",
                self.file,
                self.line,
                self.name,
                self.leaking.len(),
                self.leaking.join(" / ")
            ));
        }
        if self.child_home {
            return Some(format!(
                "{}:{} — fn {} が $TMPDIR の下へ置き場を作って素のパスを返し、このファイルは子プロセスへ \
                 HOME を差し替えて渡す（子が起こした孫 = 実 agent CLI の更新器などが子の終了後に書き戻すと、\
                 呼び手が消した後に作り直されて $TMPDIR 直下に消え残る。#2561）。\
                 直し方: 補助が tako_core::test_residue::ScratchDir::new(\"<タグ>\") を返す形にする（#1312）",
                self.file, self.line, self.name
            ));
        }
        None
    }
}

/// 全ファイルの「作って返す補助」
fn scan_helpers(sources: &[(String, String)]) -> Vec<Helper> {
    let mut out = Vec::new();
    for (file, src) in sources {
        // 生のソースに無ければコードの眺めにも無い（潰す前に安く絞る）
        if !src.contains("temp_dir()") {
            continue;
        }
        let code = code_view::code_view(src);
        if !code.contains("temp_dir()") {
            continue;
        }
        let funcs = functions(&code);
        let makers: Vec<&Func> = funcs.iter().filter(|f| makes_bare_temp_dir(f)).collect();
        if makers.is_empty() {
            continue;
        }
        // 消す補助（作る補助の「作る前に消す」は数えない）
        let cleaners: Vec<&str> = funcs
            .iter()
            .filter(|f| removes(&f.text) && !makes_bare_temp_dir(f))
            .map(|f| f.name.as_str())
            .collect();
        let child_home = code_view::without_comments(src).contains(".env(\"HOME\"");
        for maker in makers {
            let mut leaking = Vec::new();
            let mut cleaning = 0usize;
            for caller in funcs.iter().filter(|f| f.name != maker.name) {
                if !calls(&caller.text, &maker.name) {
                    continue;
                }
                // 入れ子の関数は外側の本体にも含まれるので、呼び手は「呼び出しを含む最も内側」だけにする
                let inner = funcs.iter().any(|g| {
                    g.line > caller.line
                        && g.name != maker.name
                        && caller.text.contains(&g.text)
                        && calls(&g.text, &maker.name)
                });
                if inner {
                    continue;
                }
                let cleans =
                    removes(&caller.text) || cleaners.iter().any(|c| calls(&caller.text, c));
                if cleans {
                    cleaning += 1;
                } else {
                    leaking.push(format!("{file}:{} fn {}", caller.line, caller.name));
                }
            }
            out.push(Helper {
                file: file.clone(),
                line: maker.line,
                name: maker.name.clone(),
                leaking,
                cleaning,
                child_home,
            });
        }
    }
    out
}

/// `OnceLock` の初期化子 1 つ（`$TMPDIR` の下へ dir を作るもの）
#[derive(Debug)]
struct OnceInit {
    file: String,
    /// `get_or_init(` の行（1-based）
    line: usize,
    /// 器か後始末を通しているか
    managed: bool,
}

/// `get_or_init(` の引数（対応する `)` まで）が `$TMPDIR` の下へ dir を作るものを全部
fn scan_once_inits(sources: &[(String, String)]) -> Vec<OnceInit> {
    const HEAD: &str = "get_or_init(";
    let mut out = Vec::new();
    for (file, src) in sources {
        if !(src.contains(HEAD) && src.contains("temp_dir()")) {
            continue;
        }
        let code = code_view::code_view(src);
        for (at, _) in code.match_indices(HEAD) {
            let from = at + HEAD.len();
            let mut depth = 1usize;
            let mut end = code.len();
            for (i, c) in code[from..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = from + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let init = &code[from..end];
            if !(init.contains("temp_dir()") && init.contains("create_dir")) {
                continue;
            }
            out.push(OnceInit {
                file: file.clone(),
                line: code[..at].matches('\n').count() + 1,
                managed: MANAGED.iter().any(|m| init.contains(m)),
            });
        }
    }
    out
}

/// `file:line — 理由` の一覧（違反が無ければ空）
fn scan(sources: &[(String, String)]) -> Vec<String> {
    let mut out: Vec<String> = scan_helpers(sources)
        .iter()
        .filter_map(Helper::violation)
        .collect();
    out.extend(scan_once_inits(sources).iter().filter(|o| !o.managed).map(|o| {
        format!(
            "{}:{} — OnceLock の初期化子（get_or_init）が $TMPDIR の下へ dir を作るのに、器も後始末も通さない\
             （プロセスの間ずっと使う置き場はスコープで消えず、終了時の atexit も次回起動時の pid 回収も無いので\
             必ず残る。同じ OnceLock を後始末つきで初期化する口が別にあると、実行順しだいでこちらが勝つ。#2566）。\
             直し方: tako_core::test_residue::process_scratch(\"<タグ>\") を通すか、後始末を張った初期化子に任せる",
            o.file, o.line
        )
    }));
    out
}

#[test]
fn テストの補助は作ったdirを消さない呼び手へ素のpathbufで渡さない() {
    let reports = scan(&current());
    assert!(reports.is_empty(), "違反:\n{}", reports.join("\n"));
}

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const WRITE_ISOLATION: &str = "crates/tako-control/src/test_write_isolation.rs";
const CLI_IPC: &str = "crates/tako-cli/src/ipc.rs";
const ORCHESTRATOR: &str = "crates/tako-control/src/orchestrator/mod.rs";

#[test]
fn 走査が空振りしていない() {
    let sources = current();
    let helpers = scan_helpers(&sources);
    // 「作って返す補助」は現行ソースに多数ある（呼び手が自分で消す形）。0 なら走査が壊れている
    assert!(
        helpers.len() >= 20,
        "作って返す補助を {} 本しか拾えない = 走査が壊れている: {helpers:#?}",
        helpers.len()
    );
    // 消す呼び手を「消す」と見ている（消す補助を呼ぶ形も含む）
    let find = |file: &str, name: &str| {
        helpers
            .iter()
            .find(|h| h.file == file && h.name == name)
            .unwrap_or_else(|| panic!("{file} の fn {name} を拾えない: {helpers:#?}"))
    };
    // links.rs は消す補助（cleanup_test_dir）を呼んで消す形
    let links = find("crates/tako-core/src/links.rs", "setup_test_dir");
    assert!(
        links.leaking.is_empty() && links.cleaning >= 5,
        "消す補助を呼ぶ呼び手を「消す」と見ていない: {links:#?}"
    );
    // 生文字列の中の `}` で本体を早く切らない（run_configs の旧い形の JSON を含むテスト）
    let sandbox = find("crates/tako-control/src/run_configs/tests.rs", "sandbox");
    assert!(
        sandbox.leaking.is_empty(),
        "生文字列の中の }} で本体を切っている: {sandbox:#?}"
    );
    // #2561 / #2566 で器を返す形へ直した 3 か所は「作って返す補助」ではない
    let code = |file: &str| {
        let src = &sources
            .iter()
            .find(|(f, _)| f == file)
            .unwrap_or_else(|| panic!("{file} が無い"))
            .1;
        code_view::code_view(src)
    };
    for (file, name) in [
        (DISPATCH, "mcp_test_dir"),
        (WRITE_ISOLATION, "scratch"),
        (CLI_IPC, "serve"),
    ] {
        let funcs = functions(&code(file));
        let func = funcs
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{file} の fn {name} が無い"));
        assert!(
            func.text.contains("ScratchDir"),
            "{file} の fn {name} が器を返していない: {}",
            func.text
        );
    }
    // 免除は今も「作って返す補助」として拾えている（直ったら EXEMPT_HELPERS から外す）
    for (file, name, _) in EXEMPT_HELPERS {
        let helper = find(file, name);
        assert!(
            helper.exempt(),
            "{file} の fn {name} を免除と見ていない: {helper:#?}"
        );
    }
    // 後始末を張った OnceLock の初期化子（orchestrator の test_config_dir）を見つけ、器・後始末を
    // 通していると見ている。dispatch のテストには `$TMPDIR` の下へ作る初期化子が残っていない（#2566）
    let inits = scan_once_inits(&sources);
    assert!(
        inits.iter().any(|o| o.file == ORCHESTRATOR && o.managed),
        "orchestrator の後始末つきの初期化子を拾えない = 走査が壊れている: {inits:#?}"
    );
    assert!(
        !inits
            .iter()
            .any(|o| o.file == DISPATCH || (o.file == ORCHESTRATOR && !o.managed)),
        "dispatch / orchestrator のテストに後始末の無い初期化子が戻っている: {inits:#?}"
    );
}

/// `from` を `to` へ 1 回置き換える（注入元が現行ソースに無ければ落ちる）
fn replaced(label: &str, src: &str, from: &str, to: &str) -> String {
    assert_eq!(
        src.matches(from).count(),
        1,
        "{label}: 注入元 {from:?} が現行ソースにちょうど 1 つない"
    );
    src.replacen(from, to, 1)
}

/// 注入した行の目印（コメント = 走査の眺めでは潰れる）
const MARK: &str = "// 注入 #2561";

/// 注入 1 つぶん: `file` の本文を `inject` で変えたソースで、目印の行が `why` の理由で名指しされること
fn assert_named(label: &str, file: &str, why: &str, inject: impl Fn(&str) -> String) {
    let mut sources = current();
    let slot = sources
        .iter_mut()
        .find(|(f, _)| f == file)
        .unwrap_or_else(|| panic!("{label}: {file} が無い"));
    slot.1 = inject(&slot.1);
    let line = slot
        .1
        .lines()
        .position(|l| l.contains(MARK))
        .map(|i| i + 1)
        .unwrap_or_else(|| panic!("{label}: 目印が無い"));
    let expected = format!("{}:{line} ", slot.0);
    let reports = scan(&sources);
    let named = reports
        .iter()
        .find(|r| r.starts_with(&expected) && r.contains(why));
    assert!(
        named.is_some(),
        "{label}: {expected} を「{why}」で名指ししていない: {reports:?}"
    );
    println!("{label}: FAILED {}", named.unwrap_or(&String::new()));
}

/// 修正前の `mcp_test_dir`（#2561 の直す前の形）
const MCP_BEFORE: &str = "    fn mcp_test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(\"tako-mcp-test-{name}-{}\", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }";

/// 修正前の `scratch`（#2561 の直す前の形）
const SCRATCH_BEFORE: &str = "    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            \"tako-944-{tag}-{}-{}\",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect(\"一時ディレクトリを作れる\");
        dir
    }";

/// 修正前の unix の `serve`（#2566 の直す前の形）
const SERVE_BEFORE: &str = "        #[cfg(unix)]
        fn serve(tag: &str, fake: Fake) -> String {
            use std::os::unix::net::UnixListener;
            let path =
                std::env::temp_dir().join(format!(\"tk2123-{}-{tag}.sock\", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener = UnixListener::bind(&path).expect(\"受け口を作れる\");
            std::thread::spawn(move || {
                if let Ok((stream, _)) = listener.accept() {
                    behave(stream.try_clone().expect(\"複製できる\"), stream, fake);
                }
            });
            path.to_string_lossy().into_owned()
        }";

/// 現行ソースの `head` で始まる関数の定義（頭から、頭と同じ字下げの閉じ `}` まで）を
/// `replacement` へ差し替える
fn swap_fn(label: &str, src: &str, head: &str, replacement: &str) -> String {
    assert_eq!(
        src.matches(head).count(),
        1,
        "{label}: {head:?} が現行ソースにちょうど 1 つない"
    );
    let start = src.find(head).expect("数えた");
    let indent = &head[..head.len() - head.trim_start().len()];
    let close = format!("\n{indent}}}\n");
    let end = start
        + src[start..]
            .find(&close)
            .unwrap_or_else(|| panic!("{label}: {head:?} の終わりが無い"))
        + close.len()
        - 1;
    format!("{}{}{}", &src[..start], replacement, &src[end..])
}

#[test]
fn 逆戻りを名指しできる() {
    // A. MCP 設定のテストの補助を直す前の形へ（7 本の呼び手が消さない）
    let label = "A mcp_test_dir を素の PathBuf へ";
    assert_named(label, DISPATCH, "呼び手 7 本が消さない", |src| {
        let before = MCP_BEFORE.replacen(
            "-> std::path::PathBuf {",
            &format!("-> std::path::PathBuf {{ {MARK}"),
            1,
        );
        swap_fn(label, src, "    fn mcp_test_dir(", &before)
    });
    // B. 子へ HOME として渡す置き場の補助を、呼び手ごと直す前の形へ（呼び手は子の直後に
    //    `remove_dir_all` で消す = 規則 A は通る。孫の書き戻しで消え残る形を規則 B が名指す）
    let label = "B scratch と呼び手を直す前の形へ";
    assert_named(
        label,
        WRITE_ISOLATION,
        "HOME を差し替えて渡す",
        |src| {
            let before =
                SCRATCH_BEFORE.replacen("-> PathBuf {", &format!("-> PathBuf {{ {MARK}"), 1);
            let mut src = swap_fn(label, src, "    fn scratch(", &before);
            for var in ["fake_home", "home", "base"] {
                let drop = format!("        drop({var});\n");
                assert!(src.contains(&drop), "{label}: {drop:?} が現行ソースに無い");
                src = src.replace(
                    &drop,
                    &format!("        let _ = std::fs::remove_dir_all(&{var});\n"),
                );
            }
            src
        },
    );
    // C. 新しく足した補助が $TMPDIR 直下へ作って素の PathBuf を返す（呼び手が消さない）
    let label = "C 新しい補助が temp_dir 直下を返す";
    assert_named(label, DISPATCH, "呼び手 1 本が消さない", |src| {
        replaced(
            label,
            src,
            "    fn mcp_test_dir(",
            &format!(
                "    fn new_helper_2561() -> PathBuf {{ {MARK}\n        \
                 let dir = std::env::temp_dir().join(format!(\"tako-2561-{{}}\", std::process::id()));\n        \
                 std::fs::create_dir_all(&dir).unwrap();\n        dir\n    }}\n\n    \
                 #[test]\n    fn uses_new_helper_2561() {{\n        let _ = new_helper_2561();\n    }}\n\n    \
                 fn mcp_test_dir("
            ),
        )
    });
    // D. IPC のテストの偽の受け口を直す前の形へ（ソケットを $TMPDIR 直下へ bind して
    //    接続先の String を返し、3 本の呼び手が消さない。#2566）
    let label = "D serve を素の String へ";
    assert_named(label, CLI_IPC, "呼び手 3 本が消さない", |src| {
        let before = SERVE_BEFORE.replacen("-> String {", &format!("-> String {{ {MARK}"), 1);
        swap_fn(
            label,
            src,
            "        #[cfg(unix)]\n        fn serve(",
            &before,
        )
    });
    // E. dispatch のテストの補助が orchestrator の置き場の OnceLock を後始末なしの dir で
    //    先に初期化する形へ（#2566 の直す前の形）
    let label = "E with_test_project が OnceLock を先に初期化";
    assert_named(label, DISPATCH, "OnceLock の初期化子", |src| {
        replaced(
            label,
            src,
            "        let _ = orchestrator::ensure_defaults();\n",
            &format!(
                "        orchestrator::test_config_dir_override().get_or_init(|| {{ {MARK}\n            \
                 let dir = std::env::temp_dir()\n                \
                 .join(format!(\"tako-dispatch-test-config-{{}}\", std::process::id()));\n            \
                 let _ = std::fs::create_dir_all(&dir);\n            dir\n        }});\n        \
                 let _ = orchestrator::ensure_defaults();\n"
            ),
        )
    });
}
