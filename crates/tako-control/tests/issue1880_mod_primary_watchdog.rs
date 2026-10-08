//! tako mod S2（#1880。FR-2.42.9〜）の番犬: mod の報告を一次ソースにする配線の構造
//!
//! # なぜ要るか
//!
//! ctx% は 4 経路（`orchestrator self` / `worker_status` / #749 の自動ハンドオフの tick /
//! チャットヘッダの残量バー）が読む。どれか 1 経路だけ mod の段を通らなくなっても、その経路の
//! 応答は今までどおり画面の値で埋まるので、**単体テストも見た目も壊れない**（#1021 が
//! 「4 経路のうち 1 本だけ黙って全滅していた」で踏んだのと同じ形）。また、使用制限の値は
//! mod から取るが**停止の判定は画面のまま**という #813 の安全条件は、判定関数の順序 1 つで
//! 崩れる。ここは**構造**を縛って file:line で名指す。
//!
//! # 何を縛るか
//!
//! 1. 4 経路が同じ 1 実装を通る: mod の引き当て（`claude_mod::lookup` / `lookup_pane` /
//!    `ModSnapshot::capture`）と、解決（`ctx_usage::resolve_full` か、それを呼ぶ
//!    `claude_ctx::resolve`）が各経路の関数にある
//! 2. 引き当ては `lookup` の 1 本: tako-control / tako-app が `fresh_report(` を直に呼ばない
//!    （直に呼ぶと A/B の `TAKO_1877_S2_LEGACY` と落ちた理由の語彙を素通りする）
//! 3. A/B の env の名前を文字列で読むのは `tako_core::claude_mod` の 1 か所だけ
//! 4. 上限での停止は画面でしか判定しない: `detect_limit_stop_with` は画面の判定
//!    （`detect_limit_stop_from_screen(…)?`）を先に通し、mod / codex の手がかりは時刻にだけ効く
//!
//! # 見逃す側へ倒れないための作り
//!
//! [`走査が空振りしていない`] で窓が採れていることを固定し、[`逆戻りを名指しできる`] で
//! **現行ソースから作り直した注入**が file:line で名指しされることを確かめる。

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
const CLAUDE_CTX: &str = "crates/tako-control/src/claude_ctx.rs";
const LIMIT_STOP: &str = "crates/tako-control/src/limit_stop.rs";
const MAIN: &str = "crates/tako-app/src/main.rs";
const CHAT_VIEW: &str = "crates/tako-app/src/chat_view.rs";
const CORE_MOD: &str = "crates/tako-core/src/claude_mod.rs";
/// `fresh_report(` を直に呼んではいけない側（[`CORE_MOD`] の外のソース）
const SOURCE_DIRS: &[&str] = &[
    "crates/tako-control/src",
    "crates/tako-app/src",
    "crates/tako-cli/src",
];
const S2_ENV: &str = "\"TAKO_1877_S2_LEGACY\"";

fn read(rel: &str) -> String {
    std::fs::read_to_string(workspace_root().join(rel))
        .unwrap_or_else(|e| panic!("{rel} が読める: {e}"))
}

fn report(file: &str, line: usize, why: &str) -> String {
    format!("{file}:{line}: {why}")
}

/// `needle` で始まる行から、`close` と一致する最初の行までの窓（1 始まりの行番号つき）
fn window(src: &str, needle: &str, close: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines.iter().position(|l| l.contains(needle))?;
    let end = lines[start..]
        .iter()
        .position(|l| *l == close)
        .map(|i| start + i)?;
    Some((start + 1, lines[start..=end].join("\n")))
}

/// 1 経路ぶんの縛り: 窓（関数）の中に、引き当てと解決の両方がある
struct Route {
    file: &'static str,
    name: &'static str,
    open: &'static str,
    close: &'static str,
    /// どれか 1 つがあればよい（引き当て）
    lookup: &'static [&'static str],
    /// どれか 1 つがあればよい（解決）
    resolve: &'static [&'static str],
}

const ROUTES: &[Route] = &[
    Route {
        file: DISPATCH,
        name: "orchestrator self",
        open: "fn dispatch_orchestrator_self(",
        close: "}",
        lookup: &["claude_mod::lookup_pane("],
        resolve: &["claude_ctx::resolve("],
    },
    Route {
        file: DISPATCH,
        name: "worker_status（UI スレッドの写し）",
        open: "fn collect_worker_status_ctx(",
        close: "}",
        lookup: &["ModSnapshot::capture("],
        resolve: &["ModSnapshot::capture("],
    },
    Route {
        file: DISPATCH,
        name: "worker_status",
        open: "fn finish_worker_status(",
        close: "}",
        lookup: &["mod_state.view()"],
        resolve: &["claude_ctx::resolve("],
    },
    Route {
        file: MAIN,
        name: "#749 の自動ハンドオフの tick",
        open: "    fn drive_handoff_nudge(",
        close: "    }",
        lookup: &["claude_mod::lookup("],
        resolve: &["ctx_usage::resolve_full("],
    },
    Route {
        file: CHAT_VIEW,
        name: "チャットヘッダ（材料の収集）",
        open: "    pub(crate) fn collect_chat_targets(",
        close: "    }",
        lookup: &["claude_mod::lookup("],
        resolve: &["claude_mod::ctx_input("],
    },
    Route {
        file: CHAT_VIEW,
        name: "チャットヘッダ（解決）",
        open: "pub(crate) fn load_chat_refresh(",
        close: "}",
        lookup: &["target.mod_ctx"],
        resolve: &["ctx_usage::resolve_full("],
    },
    Route {
        file: CLAUDE_CTX,
        name: "claude_ctx::resolve（self / worker_status の共通口）",
        open: "pub fn resolve_with(",
        close: "}",
        lookup: &["mod_ctx"],
        resolve: &["ctx_usage::resolve_full("],
    },
];

struct Sources {
    files: Vec<(&'static str, String)>,
    /// `fresh_report(` / A/B の env を探す対象（パス, 中身）
    tree: Vec<(String, String)>,
}

impl Sources {
    fn get(&self, file: &str) -> &str {
        self.files
            .iter()
            .find(|(f, _)| *f == file)
            .map(|(_, s)| s.as_str())
            .unwrap_or_else(|| panic!("{file} を読んでいない"))
    }

    fn get_mut(&mut self, file: &str) -> &mut String {
        &mut self
            .files
            .iter_mut()
            .find(|(f, _)| *f == file)
            .unwrap_or_else(|| panic!("{file} を読んでいない"))
            .1
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn current() -> Sources {
    let files = [DISPATCH, CLAUDE_CTX, LIMIT_STOP, MAIN, CHAT_VIEW, CORE_MOD]
        .into_iter()
        .map(|f| (f, read(f)))
        .collect();
    let root = workspace_root();
    let mut paths = Vec::new();
    for dir in SOURCE_DIRS {
        rust_files(&root.join(dir), &mut paths);
    }
    let mut tree: Vec<(String, String)> = paths
        .into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            let body = std::fs::read_to_string(&p).unwrap_or_default();
            (rel, body)
        })
        .collect();
    tree.sort();
    Sources { files, tree }
}

fn scan_routes(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    for r in ROUTES {
        let Some((line, body)) = window(s.get(r.file), r.open, r.close) else {
            out.push(report(
                r.file,
                0,
                &format!("{} の関数（{}）が見つからない", r.name, r.open),
            ));
            continue;
        };
        if !r.lookup.iter().any(|n| body.contains(n)) {
            out.push(report(
                r.file,
                line,
                &format!(
                    "{} が mod の報告を引き当てていない（{} のどれも無い）。この経路だけ mod の段を通らなくなる",
                    r.name,
                    r.lookup.join(" / ")
                ),
            ));
        }
        if !r.resolve.iter().any(|n| body.contains(n)) {
            out.push(report(
                r.file,
                line,
                &format!(
                    "{} が ctx% を共通の 1 実装で解いていない（{} のどれも無い）",
                    r.name,
                    r.resolve.join(" / ")
                ),
            ));
        }
    }
    out
}

/// 行がコメントか（`//` で始まる）
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

fn scan_tree(s: &Sources) -> Vec<String> {
    let mut out = Vec::new();
    for (file, body) in &s.tree {
        for (i, line) in body.lines().enumerate() {
            if is_comment(line) {
                continue;
            }
            if line.contains(".fresh_report(") {
                out.push(report(
                    file,
                    i + 1,
                    "`fresh_report(` を直に呼んでいる（一次ソースの引き当ては `claude_mod::lookup` の 1 本。\
                     直に呼ぶと A/B と落ちた理由を素通りする）",
                ));
            }
            if line.contains(S2_ENV) {
                out.push(report(
                    file,
                    i + 1,
                    "A/B の env（TAKO_1877_S2_LEGACY）を文字列で読んでいる（`claude_mod::s2_legacy()` の 1 か所を通す）",
                ));
            }
        }
    }
    // tako-core の中では定義の 1 か所だけ
    let core_hits: Vec<usize> = s
        .get(CORE_MOD)
        .lines()
        .enumerate()
        .filter(|(_, l)| !is_comment(l) && l.contains(S2_ENV))
        .map(|(i, _)| i + 1)
        .collect();
    if core_hits.len() != 1 {
        out.push(report(
            CORE_MOD,
            core_hits.first().copied().unwrap_or(0),
            &format!(
                "A/B の env の名前が {} か所に書かれている（定数 1 か所だけにする）",
                core_hits.len()
            ),
        ));
    }
    out
}

fn scan_limit_stop(s: &Sources) -> Vec<String> {
    let Some((line, body)) = window(s.get(LIMIT_STOP), "pub fn detect_limit_stop_with(", "}")
    else {
        return vec![report(
            LIMIT_STOP,
            0,
            "detect_limit_stop_with が見つからない",
        )];
    };
    let screen = body.find("detect_limit_stop_from_screen(lines, observed_at, tz_offset)?");
    let hint = body.find("hint.and_then(");
    match (screen, hint) {
        (Some(a), Some(b)) if a < b => Vec::new(),
        _ => vec![report(
            LIMIT_STOP,
            line,
            "停止の判定が画面より先に手がかり（mod / codex）を見ている、または画面の判定を素通りできる\
             （#813 の安全条件: 停止は画面でしか判定しない）",
        )],
    }
}

fn all(s: &Sources) -> Vec<String> {
    let mut out = scan_routes(s);
    out.extend(scan_tree(s));
    out.extend(scan_limit_stop(s));
    out
}

#[test]
fn mod_の一次ソース化の構造が契約どおり() {
    let found = all(&current());
    assert!(found.is_empty(), "\n{}", found.join("\n"));
}

#[test]
fn 走査が空振りしていない() {
    let s = current();
    for r in ROUTES {
        let (_, body) = window(s.get(r.file), r.open, r.close)
            .unwrap_or_else(|| panic!("{} の窓が採れない（{}）", r.name, r.open));
        assert!(body.lines().count() >= 5, "{} の窓が短すぎる", r.name);
    }
    assert!(
        s.tree.len() > 50,
        "走査対象のソースが少なすぎる（{} 本）",
        s.tree.len()
    );
    assert!(
        s.tree
            .iter()
            .any(|(f, _)| f.ends_with("tako-app/src/main.rs")),
        "tako-app を走査していない"
    );
}

fn assert_named(found: &[String], file: &str, why: &str) {
    assert!(
        found.iter().any(|f| f.starts_with(file) && f.contains(why)),
        "注入を名指しできない（{file} / {why}）:\n{}",
        found.join("\n")
    );
}

#[test]
fn 逆戻りを名指しできる() {
    let base = current();
    let mutate = |f: &dyn Fn(&mut Sources)| {
        let mut s = current();
        f(&mut s);
        all(&s)
    };
    // 1. 4 経路のうち 1 本だけ mod の段を外す（tick を画面だけに戻す）
    let found = mutate(&|s| {
        let main = s.get_mut(MAIN);
        *main = main.replacen(
            "tako_core::ctx_usage::resolve_full(&mod_ctx, screen.as_ref(), None)",
            "tako_core::ctx_usage::resolve(screen.as_ref(), None)",
            1,
        );
        *main = main.replacen(
            "tako_core::claude_mod::lookup(",
            "tako_core::claude_mod::peek(",
            1,
        );
    });
    assert_named(&found, MAIN, "#749");
    // self の引き当てを外す
    let found = mutate(&|s| {
        let d = s.get_mut(DISPATCH);
        *d = d.replacen(
            "crate::claude_mod::lookup_pane(host, pane_id, now)",
            "Err(tako_core::claude_mod::ModUnavailable::Absent)",
            1,
        );
    });
    assert_named(&found, DISPATCH, "orchestrator self");
    // チャットヘッダの解決を旧式へ
    let found = mutate(&|s| {
        let c = s.get_mut(CHAT_VIEW);
        *c = c.replacen(
            "tako_core::ctx_usage::resolve_full(",
            "tako_core::ctx_usage::resolve_old(",
            1,
        );
    });
    assert_named(&found, CHAT_VIEW, "チャットヘッダ（解決）");
    // 2. fresh_report を直に呼ぶ
    let found = mutate(&|s| {
        let (_, body) = s
            .tree
            .iter_mut()
            .find(|(f, _)| f.ends_with("tako-app/src/limit_autoresume.rs"))
            .expect("limit_autoresume.rs を走査している");
        body.push_str("\nfn leak(h: &ModHub) { let _ = h.fresh_report(1, now); }\n");
    });
    assert_named(
        &found,
        "crates/tako-app/src/limit_autoresume.rs",
        "fresh_report",
    );
    // 3. A/B の env を別の場所で読む
    let found = mutate(&|s| {
        let (_, body) = s
            .tree
            .iter_mut()
            .find(|(f, _)| f.ends_with("tako-control/src/dispatch.rs"))
            .expect("dispatch.rs を走査している");
        body.push_str(
            "\nfn legacy() -> bool { std::env::var_os(\"TAKO_1877_S2_LEGACY\").is_some() }\n",
        );
    });
    assert_named(&found, DISPATCH, "TAKO_1877_S2_LEGACY");
    // 4. 停止の判定で手がかりを先に見る
    let found = mutate(&|s| {
        let l = s.get_mut(LIMIT_STOP);
        *l = l.replacen(
            "let mut stop = detect_limit_stop_from_screen(lines, observed_at, tz_offset)?;",
            "let mut stop = detect_limit_stop_from_hint(hint)?;",
            1,
        );
    });
    assert_named(&found, LIMIT_STOP, "#813");
    // 注入の前の状態は綺麗（上の名指しは注入が原因）
    assert!(all(&base).is_empty());
}
