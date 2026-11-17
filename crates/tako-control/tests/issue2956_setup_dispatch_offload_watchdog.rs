//! **#2956 の番犬**: `dispatch` の同期の腕（`prepare_offload` が background のジョブにしない要求）から、
//! 外部プロセスの起動・待ちへ届かないようにする。
//!
//! ## 何が起きていたのか
//!
//! GUI の IPC の受け口（MCP / CLI / PWA）は、要求をまず `prepare_offload` に渡し、background のジョブに
//! ならなかった要求を UI スレッドの `dispatch` で答える。MCP の `tako_setup_bootstrap`（認証の問い合わせ =
//! `claude auth status` ほか・ログインシェルの PATH の読み取り・公式インストーラ）と `tako_setup_models`
//! （`codex debug models` / `agy models`）は `prepare_offload` の腕を持たず、外部プロセスを UI スレッドで
//! 待っていた。隔離 GUI の perf.log で `メインスレッド専有: dispatch:SetupBootstrap が 3089ms`（#2942 の修正後・
//! 上限 3 秒の注入）・`29817ms`（修正前 = 返らない claude で無期限）。
//!
//! #1984 の番犬（`issue1984_ui_thread_offload_watchdog.rs`）は UI スレッドの入口から呼び出しを辿るが、
//! 集約点の `dispatch` / `dispatch_inner` は「要求ごとに UI スレッドか background かが分かれる」ので辿らない
//! （同じ番犬の `GRAPH_STOPS`）。そのため dispatch の同期の腕の中の待ちは誰も見ていなかった。
//!
//! ## ここで止めるもの
//!
//! 1. [`同期の腕は外部プロセスの待ちへ届かない`] — `dispatch_inner` の `match request` の腕のうち、
//!    `prepare_offload` が background へ出さない要求の腕を入口にして呼び出しを辿り（`common/call_graph.rs`）、
//!    #1984 の番犬の葉（[`leaves`] = 同じ番犬の `RAW` + `GRAPH_LEAVES` を**本文から読む**。写さないので
//!    葉が増えればここも追う）へ届けば、腕の中で次の段を呼んだ行を file:line で名指す。
//!    今ある残り（setup 以外の同期の腕）は [`KNOWN`] に理由つきで宣言する（直したら外す。外したのに
//!    残れば名指し、宣言したのに届かなくなれば「外すこと」と落ちる）
//! 2. [`直した要求はbackgroundから答える`] — 禁止だけでなく寄せ先も縛る（#1496 の 2 本立て）。
//!    直した要求（[`TARGETS`]）ごとに、`prepare_offload` の腕があって A/B の門を持ち、`run_reply` と
//!    同期の腕が同じ応答の関数で答える。#2956 は `SetupBootstrap` / `SetupModels`（門
//!    `setup_query_offload_legacy()`）、#2961 は `SetupMcp` / `SetupRun` / `ConfigShare`（門
//!    `sync_arm_offload_legacy()` = `TAKO_2961_LEGACY`）。直した要求が [`KNOWN`] に残っていないことも見る
//! 3. [`修正前の形を名指しできる`] — 修正前の形の注入（`prepare_offload` の腕を消す）を 1 / 2 が
//!    file:line で名指しすること（同じテストの中の A/B）
//!
//! ## 読み方の約束（見逃す側へ倒さない）
//!
//! - 腕を出す側（`prepare_offload` の腕の本体が `None` でない）は、ガードつき（`if action == "usage"` など
//!   = 一部の形だけ出す）でもその要求を「出す側」として数える。ガードつきの腕の要求は [`PARTIAL`] に
//!   宣言する（増えたら名指す = 一部だけ出した要求の同期の残りを黙って見なくなることはない）
//! - 辿り方（何を辿り・何を辿らないか）は `common/call_graph.rs` の 1 実装（先取りの外側の腕
//!   `Probed::Outside` と A/B の旧経路の腕・別スレッドへ渡すクロージャは辿らない）
//!
//! ## 相方
//!
//! 実経路（返らない偽の claude / codex / agy の隔離 GUI で、perf.log の `メインスレッド専有: dispatch:SetupBootstrap`
//! の有無を A/B で見る）は PR #2956 の本文と `scripts/test-setup-offload-2956.sh`、#2961 は
//! `scripts/test-sync-arm-offload-2961.sh`。応答の同値（background と同期の腕）と A/B は dispatch.rs の単体テスト
//! `issue2956_*` / `issue2961_*`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[path = "common/production_range.rs"]
mod production_range;

#[path = "common/call_graph.rs"]
mod call_graph;

const DISPATCH: &str = "crates/tako-control/src/dispatch.rs";
/// 葉の正本（#1984 の番犬）
const WATCHDOG_1984: &str = "crates/tako-control/tests/issue1984_ui_thread_offload_watchdog.rs";

/// 呼び出しを辿る範囲（#1984 の番犬の `GRAPH_ROOTS` と同じ）
const ROOTS: &[&str] = &["crates/tako-control/src", "crates/tako-core/src"];

/// 辿らない集約点（`dispatch` を呼び直す腕 = バッチなどは、その先の要求ごとにここで見る）
const STOPS: &[&str] = &["dispatch", "dispatch_inner"];

/// 辿らない A/B の旧経路の関数（`(関数名, 理由)`）。門が `if …legacy() {` の形でない（env を直に読む・
/// 名前が `legacy()` で終わらない）ので `common/call_graph.rs` が腕として除けないもの。製品の既定の経路では
/// 呼ばれない。関数が消えたら [`走査が空振りしていない`] が落ちる（直したら外すこと）
const AB_ONLY_FNS: &[(&str, &str)] = &[
    (
        "legacy_running_via_status",
        "#2539 の A/B（`TAKO_2539_LEGACY=1`）のときだけ `remote::daemon_running` が呼ぶ（daemon の状態を丸ごと引く旧経路）",
    ),
    (
        "scrollback_legacy",
        "#972 の A/B（`TAKO_972_LEGACY=1`）のときだけ `remote::scrollback_session` が呼ぶ（#972 前の実装）",
    ),
];

/// `prepare_offload` がガードつきで一部だけ background へ出す要求（右は出す形）。
/// ここに挙げた要求の同期の腕は 1 で見ない（出した形の待ちまで同期の残りとして名指してしまうため）。
/// 増減したら [`ガードつきで出す要求は宣言どおり`] が落ちる
const PARTIAL: &[(&str, &str)] = &[
    ("LspResults", "ツリーの 1 段の展開（`action` = expand）だけ"),
    (
        "Mod",
        "status・報告・on / off / 帯（A/B の門つき）。`tako mod ui` / install は UI スレッドのまま（#2663）",
    ),
    ("Queue", "UI の模型を書かない要求（`!queue::needs_ui`）だけ"),
    ("Send", "添付つき（`attachments` が空でない）だけ（#2743）"),
    ("Sessions", "使用量（`action` = usage）だけ"),
    ("StaleBinary", "状態（`action` = status。A/B の門つき）だけ"),
    (
        "TmuxCleanup",
        "器の後片付けのうち、ソケット指定の servers を除く形（A/B の門つき）",
    ),
    ("TreeFolder", "git の状態（`action` = git-status）だけ"),
    ("UserTask", "`user_task_offloads` が出すと決めた action だけ"),
    (
        "WorkspaceSearch",
        "検索・置換・取り消し（`action` = search / replace / undo）だけ",
    ),
];

/// 既知の残り: `prepare_offload` が出さない同期の腕のうち、外部プロセスの待ちへ届くもの
/// （`(要求の名前, 理由)`）。#2956 の範囲外（setup の導入・モデル一覧以外）なので #2961 で扱う。
/// 直したら外すこと（届かなくなると [`既知の残りはまだ届く`] が落ちる）。
/// `SetupMcp` / `SetupRun` / `ConfigShare` は #2961 の 1 本目、`RemoteSetup` / `RemoteScrollback` は 2 本目で
/// 外した（[`TARGETS`]）。`OpenRemote` は 2 本目で先取り（[`PROBE_TARGETS`]）へ寄せた。`RemoteFolder` は 3 本目で
/// action ごとの段（[`STAGED_TARGETS`]）へ寄せた。
/// `SleepGuard` は 4 本目で段（[`STAGED_TARGETS`]）へ寄せた = #2961 で残りは無くなった
const KNOWN: &[(&str, &str)] = &[];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/tako-control の 2 つ上がワークスペースルート")
        .to_path_buf()
}

/// テスト領域を空白へ潰した本文（行番号は保たれる。#1420）
fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    let src = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} が読める: {e}"));
    production_range::scan(&src).text
}

/// [`ROOTS`] の `.rs` を全部（テスト領域を潰した本文と、その眺め）。読むのは 1 回だけ
fn graph_sources() -> &'static [(String, call_graph::Source)] {
    static CACHE: OnceLock<Vec<(String, call_graph::Source)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let entries =
                std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?} が読める: {e}"));
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    out.push(path);
                }
            }
        }
        let root = workspace_root();
        let mut paths = Vec::new();
        for dir in ROOTS {
            walk(&root.join(dir), &mut paths);
        }
        paths.sort();
        paths
            .iter()
            .map(|path| {
                let rel = path
                    .strip_prefix(&root)
                    .expect("ワークスペースの中")
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = read(&rel);
                let source = call_graph::Source::new(&rel, &text);
                (text, source)
            })
            .collect()
    })
}

/// 本番の dispatch.rs の本文（テスト領域を潰したもの = 注入の元）
fn dispatch_text() -> &'static str {
    graph_sources()
        .iter()
        .find(|(_, source)| source.rel == DISPATCH)
        .map(|(text, _)| text.as_str())
        .expect("dispatch.rs が走査の範囲にある")
}

/// `overrides`（`(相対パス, 本文)` = 注入）で差し替えたグラフ
fn graph_with(overrides: &[(&str, &str)]) -> call_graph::Graph {
    let sources = graph_sources()
        .iter()
        .map(
            |(text, source)| match overrides.iter().find(|(rel, _)| *rel == source.rel) {
                Some((rel, injected)) if injected != text => call_graph::Source::new(rel, injected),
                _ => source.clone(),
            },
        )
        .collect();
    call_graph::Graph::build(sources)
}

/// dispatch.rs の関数 `name`（ちょうど 1 つでなければ理由つきで落ちる）
fn one_fn(graph: &call_graph::Graph, name: &str) -> usize {
    match graph.find(DISPATCH, name).as_slice() {
        [f] => *f,
        [] => {
            panic!("{DISPATCH} に `fn {name}` が無い（走査が空振り。改名したなら番犬も追うこと）")
        }
        many => panic!("{DISPATCH} に `fn {name}` が {} 個ある", many.len()),
    }
}

/// 走査の範囲全体で名前が `name` の関数
fn fns_named(graph: &call_graph::Graph, name: &str) -> Vec<usize> {
    graph
        .fns
        .iter()
        .enumerate()
        .filter(|(_, f)| f.name == name)
        .map(|(i, _)| i)
        .collect()
}

// ===== #1984 の番犬の葉を本文から読む =====

/// #1984 の番犬の葉に足す、メソッド呼び出しの葉（`(綴り, 名指しの理由)`。#2961）。
///
/// 呼び出しのグラフは名前で関数を引くので、トレイトのオブジェクト越しのメソッド呼び出し
/// （`capture.capture_scrollback(…)`）の先へは辿れない。その先が外部プロセスを待つ形をここで葉にする
/// （#2961 で `RemoteScrollback` の既定の経路 = 器の `capture-pane` が番犬から見えていなかった）
const METHOD_LEAVES: &[(&str, &str)] = &[(
    ".capture_scrollback(",
    "器の `capture-pane` を待つ（`DetachedCapture` のメソッド = グラフが辿らない形。#2961）",
)];

/// #1984 の番犬の `RAW` と `GRAPH_LEAVES`（`(綴り, 名指しの理由)`）に [`METHOD_LEAVES`] を足したもの。
///
/// 写して持つと葉が増えたときにここだけ古くなるので、あちらの本文の配列を読む。読み方は
/// 「配列の頭から `\n];` まで・行コメントを飛ばし・`( "綴り", "理由" )` の組を拾う」だけ
fn leaves() -> &'static [(String, String)] {
    static CACHE: OnceLock<Vec<(String, String)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let path = workspace_root().join(WATCHDOG_1984);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{WATCHDOG_1984} が読める: {e}"));
        let mut out = Vec::new();
        for head in [
            "const RAW: &[(&str, &str)] = &[",
            "const GRAPH_LEAVES: &[(&str, &str)] = &[",
        ] {
            let at = text.find(head).unwrap_or_else(|| {
                panic!("{WATCHDOG_1984} に `{head}` が無い（改名したなら追うこと）")
            });
            let body = &text[at + head.len()..];
            let end = body
                .find("\n];")
                .unwrap_or_else(|| panic!("{WATCHDOG_1984} の `{head}` の終わり（`\\n];`）が無い"));
            out.extend(tuples_of(&body[..end]));
        }
        out.extend(
            METHOD_LEAVES
                .iter()
                .map(|(needle, why)| (needle.to_string(), why.to_string())),
        );
        out
    })
}

/// 配列の本文から `("綴り", "理由")` の組を拾う（行コメント・文字列のエスケープを読む小さな字句読み）
fn tuples_of(body: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut literals: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '"' => {
                let mut lit = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        match chars.get(i + 1) {
                            // 行末の `\` は改行と次の行の頭の空白を読み飛ばす
                            Some('\n') => {
                                i += 2;
                                while i < chars.len() && chars[i].is_whitespace() {
                                    i += 1;
                                }
                                continue;
                            }
                            Some(c) => {
                                lit.push(*c);
                                i += 2;
                                continue;
                            }
                            None => break,
                        }
                    }
                    lit.push(chars[i]);
                    i += 1;
                }
                if depth == 1 {
                    literals.push(lit);
                }
            }
            '(' => {
                depth += 1;
                if depth == 1 {
                    literals.clear();
                }
            }
            ')' => {
                if depth == 1 && literals.len() == 2 {
                    out.push((literals[0].clone(), literals[1].clone()));
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        i += 1;
    }
    out
}

// ===== `match request` の腕を読む =====

/// 1 つの腕（バイト位置はコードの眺め = 原文と同じ）
#[derive(Debug, Clone)]
struct Arm {
    /// 腕の頭（パターン）の開始
    start: usize,
    /// 腕の終わり（含まない）
    end: usize,
    /// パターンの `Request::名前` の名前（`|` で並べたものは全部）
    variants: Vec<String>,
    /// `=>` の後ろ（本体）の開始
    body_start: usize,
    /// パターンのガード（`if …` の中身。無ければ `None`）
    guard: Option<String>,
}

/// 対応する閉じ括弧の位置（`open` は開き括弧）
fn matching(code: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, &b) in code.iter().enumerate().skip(open) {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("{open} の括弧が閉じていない");
}

fn ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// 関数 `f` の本体の最初の `match request {` の腕（入れ子の match の腕は数えない）
fn arms_of(graph: &call_graph::Graph, f: usize) -> Vec<Arm> {
    let def = &graph.fns[f];
    let code = &graph.sources[def.file].code;
    let bytes = code.as_bytes();
    let body = &code[def.body.0..=def.body.1];
    let at = def.body.0
        + body
            .find("match request {")
            .unwrap_or_else(|| panic!("`fn {}` に `match request {{` が無い", def.name));
    let open = at + "match request ".len();
    let close = matching(bytes, open);
    let mut arms = Vec::new();
    let mut p = open + 1;
    loop {
        while p < close && (bytes[p].is_ascii_whitespace() || bytes[p] == b',') {
            p += 1;
        }
        if p >= close {
            break;
        }
        let start = p;
        // パターンの終わり（深さ 0 の `=>`）
        let mut q = p;
        let mut depth = 0usize;
        while q < close {
            match bytes[q] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'=' if depth == 0 && bytes.get(q + 1) == Some(&b'>') => break,
                _ => {}
            }
            q += 1;
        }
        let pattern = &code[start..q];
        let mut r = q + 2;
        while r < close && bytes[r].is_ascii_whitespace() {
            r += 1;
        }
        let body_start = r;
        // 本体の終わり: `{` で始まればその塊、それ以外は深さ 0 の `,` か match の終わり
        let end = if bytes[r] == b'{' {
            matching(bytes, r) + 1
        } else {
            let mut s = r;
            let mut depth = 0usize;
            while s < close {
                match bytes[s] {
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' => depth -= 1,
                    b',' if depth == 0 => break,
                    _ => {}
                }
                s += 1;
            }
            s
        };
        let variants = pattern
            .match_indices("Request::")
            .map(|(i, _)| {
                pattern[i + "Request::".len()..]
                    .bytes()
                    .take_while(|b| ident_byte(*b))
                    .map(char::from)
                    .collect::<String>()
            })
            .collect();
        // ガード: 深さ 0 の `if `（パターンの `{ .. }` の中の語は数えない）
        let pb = pattern.as_bytes();
        let mut depth = 0usize;
        let mut guard = None;
        for i in 0..pb.len() {
            match pb[i] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b'i' if depth == 0
                    && pattern[i..].starts_with("if ")
                    && (i == 0 || !ident_byte(pb[i - 1])) =>
                {
                    guard = Some(pattern[i + 3..].trim().to_string());
                    break;
                }
                _ => {}
            }
        }
        arms.push(Arm {
            start,
            end,
            variants,
            body_start,
            guard,
        });
        p = end;
    }
    arms
}

/// ガードが A/B の門だけ（`!…legacy…()` = 名前に `legacy` を持つ関数の否定 1 つ）か
fn guard_is_ab_gate(guard: &str) -> bool {
    let g = guard.trim();
    let Some(path) = g.strip_prefix('!').and_then(|g| g.strip_suffix("()")) else {
        return false;
    };
    let name = path.rsplit("::").next().unwrap_or(path);
    name.contains("legacy") && path.bytes().all(|b| ident_byte(b) || b == b':')
}

/// `prepare_offload` が出す要求（`(名前, ガードつきか)`。本体が `None` の腕は出さない）
fn offloaded(graph: &call_graph::Graph) -> BTreeMap<String, bool> {
    let f = one_fn(graph, "prepare_offload");
    let code = &graph.sources[graph.fns[f].file].code;
    let mut out: BTreeMap<String, bool> = BTreeMap::new();
    for arm in arms_of(graph, f) {
        if code[arm.body_start..arm.end].trim() == "None" {
            continue;
        }
        let partial = arm.guard.as_deref().is_some_and(|g| !guard_is_ab_gate(g));
        for v in arm.variants {
            // 同じ要求に完全な腕とガードつきの腕があれば、完全な腕を採る
            let e = out.entry(v).or_insert(partial);
            *e = *e && partial;
        }
    }
    out
}

// ===== 1. 同期の腕から外部プロセスの待ちへ届く形 =====

/// 1 つの名指し（`dispatch.rs:行（Request::名前 の腕）…`）
#[derive(Debug)]
struct Finding {
    variant: String,
    line: usize,
    text: String,
}

/// `dispatch_inner` の腕のうち、`prepare_offload` が出さない要求の腕から葉へ届くもの
fn scan(graph: &call_graph::Graph) -> Vec<Finding> {
    let inner = one_fn(graph, "dispatch_inner");
    let mut stops: Vec<usize> = STOPS.iter().map(|name| one_fn(graph, name)).collect();
    stops.extend(
        AB_ONLY_FNS
            .iter()
            .flat_map(|(name, _)| fns_named(graph, name)),
    );
    let offloaded = offloaded(graph);
    let leaves = leaves();
    let mut out = Vec::new();
    for arm in arms_of(graph, inner) {
        let sync: Vec<&String> = arm
            .variants
            .iter()
            .filter(|v| !offloaded.contains_key(*v))
            .collect();
        let Some(variant) = sync.first() else {
            continue;
        };
        let src = &graph.sources[graph.fns[inner].file];
        // 腕の中に直に書いた葉
        let (arm_first, arm_last) = (src.line_of(arm.start), src.line_of(arm.end));
        let mut hits: Vec<(usize, usize, String)> = Vec::new();
        for (needle, why) in leaves {
            for line in graph.hits(inner, needle) {
                if (arm_first..=arm_last).contains(&line) {
                    hits.push((
                        0,
                        line,
                        format!("腕の中の {DISPATCH}:{line} `{needle}`（{why}）"),
                    ));
                }
            }
        }
        // 腕から呼んだ関数を入口に辿る
        let calls = graph.callees_in(inner, arm.start, arm.end);
        let entries: Vec<usize> = calls
            .iter()
            .map(|(g, _)| *g)
            .filter(|g| !stops.contains(g))
            .collect();
        let reach = graph.reach(&entries, &|_, to| stops.contains(&to));
        let mut reached: Vec<usize> = reach.keys().copied().collect();
        reached.sort();
        for g in reached {
            let rel = &graph.sources[graph.fns[g].file].rel;
            let mut found: Vec<(usize, String)> = Vec::new();
            for (needle, why) in leaves {
                for line in graph.hits(g, needle) {
                    found.push((line, format!("{rel}:{line} `{needle}`（{why}）")));
                }
            }
            found.sort();
            let Some((_, first)) = found.first() else {
                continue;
            };
            let path = graph.path_to(&reach, g);
            let entry = path[0].0;
            let call_line = calls
                .iter()
                .find(|(h, _)| *h == entry)
                .map(|(_, line)| *line)
                .unwrap_or(arm_first);
            let route: String = path
                .iter()
                .map(|(h, _)| format!(" → `{}`", graph.fns[*h].name))
                .collect();
            hits.push((path.len(), call_line, format!("{first}。道筋: 腕{route}")));
        }
        if hits.is_empty() {
            continue;
        }
        // 腕ごとに 1 件（道筋の短い順に 3 つまで見せる）
        hits.sort();
        let line = hits[0].1;
        let shown: Vec<&str> = hits.iter().take(3).map(|(_, _, t)| t.as_str()).collect();
        out.push(Finding {
            variant: (*variant).clone(),
            line,
            text: format!(
                "{DISPATCH}:{line}（`Request::{variant}` の腕。`prepare_offload` が background へ出さない = \
                 UI スレッドで答える）が外部プロセスの起動・待ちへ届く: {}{}",
                shown.join(" / "),
                if hits.len() > 3 {
                    format!("（ほか {} 件）", hits.len() - 3)
                } else {
                    String::new()
                }
            ),
        });
    }
    out
}

#[test]
fn 同期の腕は外部プロセスの待ちへ届かない() {
    let graph = graph_with(&[]);
    let findings = scan(&graph);
    let offenders: Vec<&Finding> = findings
        .iter()
        .filter(|f| !KNOWN.iter().any(|(v, _)| *v == f.variant))
        .collect();
    assert!(
        offenders.is_empty(),
        "dispatch の同期の腕（UI スレッドで答える）が外部プロセスを待つ（#2956）。\
         `prepare_offload` の腕で background のジョブにすること（`SetupDeps` = #2433 の形）:\n{}",
        offenders
            .iter()
            .map(|f| format!("  {}", f.text))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn 既知の残りはまだ届く() {
    let graph = graph_with(&[]);
    let findings = scan(&graph);
    for (variant, _) in KNOWN {
        assert!(
            findings.iter().any(|f| f.variant == *variant),
            "既知の残り `Request::{variant}` の同期の腕が外部プロセスへ届かなくなった（直したなら KNOWN から外すこと）"
        );
    }
}

#[test]
fn ガードつきで出す要求は宣言どおり() {
    let graph = graph_with(&[]);
    let partial: Vec<String> = offloaded(&graph)
        .into_iter()
        .filter(|(_, partial)| *partial)
        .map(|(v, _)| v)
        .collect();
    let declared: Vec<String> = PARTIAL.iter().map(|(v, _)| v.to_string()).collect();
    assert_eq!(
        partial, declared,
        "`prepare_offload` がガードつきで一部だけ出す要求が PARTIAL の宣言と違う（増えたなら理由を添えて足す）"
    );
}

#[test]
fn 走査が空振りしていない() {
    let leaves = leaves();
    for needle in [
        "Command::new(",
        ".output()",
        ".spawn()",
        "wait_with_output(",
        "output_with_timeout(",
        "find_with_timeout(",
    ] {
        assert!(
            leaves.iter().any(|(n, _)| n == needle),
            "#1984 の番犬の葉に `{needle}` が無い（読み方が空振り）: {} 件",
            leaves.len()
        );
    }
    let graph = graph_with(&[]);
    let inner = one_fn(&graph, "dispatch_inner");
    let arms = arms_of(&graph, inner);
    assert!(
        arms.len() > 100,
        "`dispatch_inner` の腕が {} 個しか読めない",
        arms.len()
    );
    for v in ["SetupBootstrap", "SetupModels", "SetupDeps", "Split"] {
        assert!(
            arms.iter().any(|a| a.variants.iter().any(|x| x == v)),
            "`dispatch_inner` に `Request::{v}` の腕が読めない"
        );
    }
    for (name, _) in AB_ONLY_FNS {
        assert_eq!(
            fns_named(&graph, name).len(),
            1,
            "辿らない A/B の関数 `fn {name}` がちょうど 1 つ無い（消した・改名したなら AB_ONLY_FNS も追うこと）"
        );
    }
    let offloaded = offloaded(&graph);
    for v in ["SetupDeps", "RemoteAgents", "TmuxList"] {
        assert!(
            offloaded.contains_key(v),
            "`prepare_offload` の `Request::{v}` の腕が読めない（出す側の読み方が空振り）"
        );
    }
}

// ===== 2. 寄せ先 =====

/// 直した要求の寄せ先（#2956 / #2961）。`prepare_offload` の腕（A/B の門つき）がジョブを作り、`run_reply` と
/// `dispatch_inner` の同期の腕が同じ応答の関数を通す
struct Target {
    variant: &'static str,
    issue: &'static str,
    /// A/B の門（`prepare_offload` の腕のガード）
    gate: &'static str,
    /// `prepare_offload` の腕の頭（パターン + ガード + `=>`）
    prepare_arm: &'static str,
    /// その腕が作るジョブ
    job: &'static str,
    /// 注入で消す `prepare_offload` の腕の全文（修正前の形 = 腕が無い）
    prepare_whole: &'static str,
    /// `run_reply` の腕
    run_arm: &'static str,
    /// `dispatch_inner` の同期の腕の頭と、その腕の中で応答の関数を呼ぶ字面（同じ行でもよい）
    sync_head: &'static str,
    sync_call: &'static str,
    /// 応答の関数
    reply: &'static str,
}

const TARGETS: &[Target] = &[
    Target {
        variant: "SetupBootstrap",
        issue: "#2956",
        gate: "setup_query_offload_legacy()",
        prepare_arm: "Request::SetupBootstrap { .. } if !setup_query_offload_legacy() =>",
        job: "OffloadJob::SetupBootstrap(",
        prepare_whole: "bootstrap @ Request::SetupBootstrap { .. } if !setup_query_offload_legacy() => {\n            Some(Ok(OffloadJob::SetupBootstrap(Box::new(bootstrap.clone()))))\n        }\n",
        run_arm: "OffloadJob::SetupBootstrap(request) => setup_bootstrap_reply(&request)",
        sync_head: "request @ Request::SetupBootstrap { .. } => ",
        sync_call: "setup_bootstrap_reply(&request)",
        reply: "setup_bootstrap_reply",
    },
    Target {
        variant: "SetupModels",
        issue: "#2956",
        gate: "setup_query_offload_legacy()",
        prepare_arm: "Request::SetupModels { .. } if !setup_query_offload_legacy() =>",
        job: "OffloadJob::SetupModels(",
        prepare_whole: "models @ Request::SetupModels { .. } if !setup_query_offload_legacy() => {\n            Some(Ok(OffloadJob::SetupModels(Box::new(models.clone()))))\n        }\n",
        run_arm: "OffloadJob::SetupModels(request) => setup_models_reply(&request)",
        sync_head: "request @ Request::SetupModels { .. } => ",
        sync_call: "setup_models_reply(&request)",
        reply: "setup_models_reply",
    },
    Target {
        variant: "SetupMcp",
        issue: "#2961",
        gate: "sync_arm_offload_legacy()",
        prepare_arm: "Request::SetupMcp { scope, pane, agent } if !sync_arm_offload_legacy() =>",
        job: "OffloadJob::SetupMcp(",
        prepare_whole: "Request::SetupMcp { scope, pane, agent } if !sync_arm_offload_legacy() => Some(\n            setup_mcp_prepare(host, scope.as_deref(), *pane, agent.clone())\n                .map(|job| OffloadJob::SetupMcp(Box::new(job))),\n        ),\n",
        run_arm: "OffloadJob::SetupMcp(job) => setup_mcp_reply(&job)",
        sync_head: "Request::SetupMcp { scope, pane, agent } => {",
        sync_call: "setup_mcp_reply(&job)",
        reply: "setup_mcp_reply",
    },
    Target {
        variant: "SetupRun",
        issue: "#2961",
        gate: "sync_arm_offload_legacy()",
        prepare_arm: "Request::SetupRun { .. } if !sync_arm_offload_legacy() =>",
        job: "OffloadJob::SetupRun(",
        prepare_whole: "run @ Request::SetupRun { .. } if !sync_arm_offload_legacy() => {\n            Some(Ok(OffloadJob::SetupRun(Box::new(run.clone()))))\n        }\n",
        run_arm: "OffloadJob::SetupRun(request) => setup_run_reply(&request)",
        sync_head: "request @ Request::SetupRun { .. } => ",
        sync_call: "setup_run_reply(&request)",
        reply: "setup_run_reply",
    },
    Target {
        variant: "ConfigShare",
        issue: "#2961",
        gate: "sync_arm_offload_legacy()",
        prepare_arm: "Request::ConfigShare { .. } if !sync_arm_offload_legacy() =>",
        job: "OffloadJob::ConfigShare(",
        prepare_whole: "share @ Request::ConfigShare { .. } if !sync_arm_offload_legacy() => {\n            Some(Ok(OffloadJob::ConfigShare(Box::new(share.clone()))))\n        }\n",
        run_arm: "OffloadJob::ConfigShare(request) => config_share_reply(&request)",
        sync_head: "request @ Request::ConfigShare { .. } => ",
        sync_call: "config_share_reply(&request)",
        reply: "config_share_reply",
    },
    Target {
        variant: "RemoteSetup",
        issue: "#2961",
        gate: "sync_arm_offload_legacy()",
        prepare_arm: "Request::RemoteSetup { .. } if !sync_arm_offload_legacy() =>",
        job: "OffloadJob::RemoteSetup(",
        prepare_whole: "remote_setup @ Request::RemoteSetup { .. } if !sync_arm_offload_legacy() => {\n            Some(Ok(OffloadJob::RemoteSetup(Box::new(remote_setup.clone()))))\n        }\n",
        run_arm: "OffloadJob::RemoteSetup(request) => remote_setup_reply(&request)",
        sync_head: "request @ Request::RemoteSetup { .. } => ",
        sync_call: "remote_setup_reply(&request)",
        reply: "remote_setup_reply",
    },
    Target {
        variant: "RemoteScrollback",
        issue: "#2961",
        gate: "sync_arm_offload_legacy()",
        prepare_arm: "Request::RemoteScrollback { pane_id, lines } if !sync_arm_offload_legacy() =>",
        job: "OffloadJob::RemoteScrollback {",
        prepare_whole: "Request::RemoteScrollback { pane_id, lines } if !sync_arm_offload_legacy() => {\n            Some(scrollback_target_session(host, pane_id).map(|session| {\n                OffloadJob::RemoteScrollback {\n                    session,\n                    lines: *lines,\n                }\n            }))\n        }\n",
        run_arm: "remote_scrollback_reply(&session, lines)",
        sync_head: "Request::RemoteScrollback { pane_id, lines } => {",
        sync_call: "remote_scrollback_reply(&session, lines)",
        reply: "remote_scrollback_reply",
    },
];

/// 先取り（#1984 の `ProbeNeeds` → `UiProbes`）へ寄せた要求（#2961）。UI スレッドの同期の腕は残り、待つ問い合わせだけを
/// background が先に済ませる。`(要求, probe_needs の腕の頭, その腕が立てる印, 同期の腕の頭, 読む側の関数の呼び出し,
/// 注入で戻す修正前の呼び出し)`
const PROBE_TARGETS: &[(&str, &str, &str, &str, &str, &str)] = &[(
    "OpenRemote",
    "Request::OpenRemote { .. } if !sync_arm_offload_legacy() => Some(ProbeNeeds {",
    "ssh_bin: ssh_bin_need(),",
    "        Request::OpenRemote {\n            host: ssh_host,",
    "let argv = remote_ssh_argv_probed(&ssh_host);",
    "let argv = remote_ssh_argv(&ssh_host);",
)];

#[test]
fn 直した要求はbackgroundから答える() {
    let problems = target_problems(dispatch_text());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// action ごとに段を分ける準備の関数へ寄せた要求（#2961）。`prepare_offload` の腕（A/B の門つき）が準備の関数を呼ぶ。
/// 準備・着地（UI スレッドで回る段）が外部プロセスへ届かないことは #1984 の番犬の `UI_PATHS` が見る。
/// `(要求, prepare_offload の腕の頭, 腕が呼ぶ準備の関数, 注入で消す腕の全文)`
const STAGED_TARGETS: &[(&str, &str, &str, &str)] = &[
    (
        "RemoteFolder",
        "Request::RemoteFolder { .. } if !sync_arm_offload_legacy() =>",
        "remote_folder_prepare(host, request)",
        "        Request::RemoteFolder { .. } if !sync_arm_offload_legacy() => {\n            remote_folder_prepare(host, request)\n        }\n",
    ),
    (
        "SleepGuard",
        "Request::SleepGuard { action, .. } if !sync_arm_offload_legacy() =>",
        "sleep_guard_prepare(action.as_deref())",
        "        Request::SleepGuard { action, .. } if !sync_arm_offload_legacy() => {\n            sleep_guard_prepare(action.as_deref())\n        }\n",
    ),
];

#[test]
fn 段へ寄せた要求は準備の関数を通る() {
    let problems = staged_target_problems(dispatch_text());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// [`STAGED_TARGETS`] の検査（本文の字面で見る）
fn staged_target_problems(text: &str) -> Vec<String> {
    let code = production_range::code_view::code_view(text);
    let line_of = |at: usize| code[..at].matches('\n').count() + 1;
    let mut out = Vec::new();
    let prepare = code.find("pub fn prepare_offload(");
    for (variant, arm, call, _) in STAGED_TARGETS {
        match prepare.and_then(|at| code[at..].find(arm).map(|i| at + i)) {
            None => out.push(format!(
                "{DISPATCH}:{} `prepare_offload` に `{arm}` の腕が無い。`Request::{variant}` の外部プロセスを \
                 UI スレッドで待つ（#2961）。`{call}` で action ごとに段へ分ける",
                prepare.map_or(1, line_of)
            )),
            Some(at) => {
                let from = at + arm.len();
                let arm_end = code[from..]
                    .find("Request::")
                    .map_or(code.len(), |i| from + i);
                if !code[from..arm_end].contains(call) {
                    out.push(format!(
                        "{DISPATCH}:{} `{arm}` の腕が `{call}` を呼ばない（#2961）",
                        line_of(at)
                    ));
                }
            }
        }
    }
    out
}

#[test]
fn 先取りへ寄せた要求はその答えで組む() {
    let problems = probe_target_problems(dispatch_text());
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// [`PROBE_TARGETS`] の検査: `probe_needs` に腕があって印を立て、同期の腕が先取りを読む関数を呼ぶ
fn probe_target_problems(text: &str) -> Vec<String> {
    let code = production_range::code_view::code_view(text);
    let line_of = |at: usize| code[..at].matches('\n').count() + 1;
    let mut out = Vec::new();
    let needs_at = code.find("fn probe_needs(");
    for (variant, needs_arm, flag, sync_head, call, _) in PROBE_TARGETS {
        match needs_at.and_then(|at| code[at..].find(needs_arm).map(|i| at + i)) {
            None => out.push(format!(
                "{DISPATCH}:{} `probe_needs` に `{needs_arm}` の腕が無い。`Request::{variant}` の待つ問い合わせを \
                 background で先取りしない（#2961）",
                needs_at.map_or(1, line_of)
            )),
            Some(at) => {
                let arm = &code[at..];
                let arm_end = arm[needs_arm.len()..]
                    .find("Request::")
                    .map_or(arm.len(), |i| needs_arm.len() + i);
                if !arm[..arm_end].contains(flag) {
                    out.push(format!(
                        "{DISPATCH}:{} `{needs_arm}` の腕が `{flag}` を立てない（#2961）",
                        line_of(at)
                    ));
                }
            }
        }
        match code.find(sync_head) {
            None => out.push(format!(
                "{DISPATCH}:1 `dispatch_inner` の `Request::{variant}` の腕が読めない（走査の空振り）"
            )),
            Some(at) => {
                let after = at + sync_head.len();
                let next_arm = code[after..]
                    .find("\n        Request::")
                    .map_or(code.len(), |i| after + i);
                if !code[after..next_arm].contains(call) {
                    out.push(format!(
                        "{DISPATCH}:{} `Request::{variant}` の同期の腕が先取りを読む `{call}` を通らない（UI スレッドで待つ。#2961）",
                        line_of(at)
                    ));
                }
            }
        }
    }
    out
}

#[test]
fn 直した要求はknownに残っていない() {
    for (variant, ..) in STAGED_TARGETS {
        assert!(
            !KNOWN.iter().any(|(v, _)| v == variant),
            "`{variant}` は段へ寄せた（#2961）のに KNOWN に残っている"
        );
    }
    for (variant, ..) in PROBE_TARGETS {
        assert!(
            !KNOWN.iter().any(|(v, _)| v == variant),
            "`{variant}` は先取りへ寄せた（#2961）のに KNOWN に残っている"
        );
    }
    for t in TARGETS {
        assert!(
            !KNOWN.iter().any(|(v, _)| *v == t.variant),
            "`{}` は直した（{}）のに KNOWN に残っている",
            t.variant,
            t.issue
        );
    }
}

/// `prepare_offload` の腕と `run_reply` の答え方（本文の字面で見る）
fn target_problems(text: &str) -> Vec<String> {
    let code = production_range::code_view::code_view(text);
    let line_of = |at: usize| code[..at].matches('\n').count() + 1;
    let span = |sig: &str| -> Option<(usize, usize)> {
        let at = code.find(sig)?;
        let open = at + code[at..].find('{')?;
        Some((at, matching(code.as_bytes(), open)))
    };
    let mut out = Vec::new();
    match span("pub fn prepare_offload(") {
        None => out.push(format!(
            "{DISPATCH}:1 `prepare_offload` が無い（走査の空振り）"
        )),
        Some((start, end)) => {
            let body = &code[start..end];
            for t in TARGETS {
                let (needle, job) = (t.prepare_arm, t.job);
                match body.find(needle) {
                    None => out.push(format!(
                        "{DISPATCH}:{} `prepare_offload` に `{needle}` の腕が無い。外部プロセスを UI スレッドで待つ（{}）。\
                         `{job}` へ包み、A/B の門 `{}` を置く",
                        line_of(start),
                        t.issue,
                        t.gate
                    )),
                    Some(at) => {
                        // 腕の本体（`=>` の後ろから、次の腕の頭 = 次の `Request::` まで）
                        let from = at + needle.len();
                        let arm_end = body[from..]
                            .find("Request::")
                            .map_or(body.len(), |i| from + i);
                        if !body[from..arm_end].contains(job) {
                            out.push(format!(
                                "{DISPATCH}:{} `{needle}` の腕が `{job}` を作らない（{}）",
                                line_of(start + at),
                                t.issue
                            ));
                        }
                    }
                }
            }
        }
    }
    match span("fn run_reply(self)") {
        None => out.push(format!("{DISPATCH}:1 `run_reply` が無い（走査の空振り）")),
        Some((start, end)) => {
            let body = &code[start..end];
            for t in TARGETS {
                if !body.contains(t.run_arm) {
                    out.push(format!(
                        "{DISPATCH}:{} `run_reply` が `{}` で答えていない。同期の腕と同じ関数 `{}` を通す（{}）",
                        line_of(start),
                        t.run_arm,
                        t.reply,
                        t.issue
                    ));
                }
            }
        }
    }
    // 同期の腕も同じ関数（応答の 1 実装）
    for t in TARGETS {
        if sync_call_at(&code, t).is_none() {
            out.push(format!(
                "{DISPATCH}:1 `dispatch_inner` の同期の腕（`{}`）が `{}` を通していない（background と応答が分かれる。{}）",
                t.sync_head, t.reply, t.issue
            ));
        }
    }
    out
}

/// 同期の腕の中で応答の関数を呼ぶ位置（腕の頭の後ろ・次の腕の頭より前の最初の `sync_call`）
fn sync_call_at(code: &str, t: &Target) -> Option<usize> {
    let head = code.find(t.sync_head)?;
    let after = head + t.sync_head.len();
    let next_arm = code[after..]
        .find("Request::")
        .map_or(code.len(), |i| after + i);
    code[after..next_arm].find(t.sync_call).map(|i| after + i)
}

// ===== 3. 修正前の形を名指しできる =====

#[test]
fn 修正前の形を名指しできる() {
    let text = dispatch_text();
    for t in TARGETS {
        let variant = t.variant;
        assert!(
            text.contains(t.prepare_whole),
            "注入の元の `{variant}` の腕が本文に無い（形を変えたなら注入も追うこと）"
        );
        // 修正前の形: `prepare_offload` の腕を消す（= 同期の腕で UI スレッドが待つ）
        let injected = text.replacen(t.prepare_whole, "", 1);
        let graph = graph_with(&[(DISPATCH, &injected)]);
        let findings = scan(&graph);
        let named: Vec<&Finding> = findings.iter().filter(|f| f.variant == variant).collect();
        let at = sync_call_at(&injected, t).expect("同期の腕が本文にある");
        let line = injected[..at].matches('\n').count() + 1;
        assert!(
            named.iter().any(|f| f.line == line && f.text.starts_with(&format!("{DISPATCH}:{line}"))),
            "`prepare_offload` の `{variant}` の腕を消した注入で、同期の腕（{DISPATCH}:{line}）が名指しされない: {findings:#?}"
        );
        let problems = target_problems(&injected);
        assert!(
            problems.iter().any(|p| p.contains(t.prepare_arm)),
            "`prepare_offload` の `{variant}` の腕を消した注入を 2 が名指ししない: {problems:#?}"
        );
    }
    // 段へ寄せた要求: `prepare_offload` の腕を消すと、同期の腕（全 action が UI スレッドで待つ）を 1 が、腕が無いことを 2 が名指す
    for (variant, arm, _, whole) in STAGED_TARGETS {
        assert!(
            text.contains(whole),
            "注入の元の `{variant}` の腕が本文に無い（形を変えたなら注入も追うこと）"
        );
        let injected = text.replacen(whole, "", 1);
        let graph = graph_with(&[(DISPATCH, &injected)]);
        let findings = scan(&graph);
        assert!(
            findings.iter().any(|f| f.variant == *variant),
            "`prepare_offload` の `{variant}` の腕を消した注入で、同期の腕が名指しされない: {findings:#?}"
        );
        let problems = staged_target_problems(&injected);
        assert!(
            problems.iter().any(|p| p.contains(arm)),
            "`prepare_offload` の `{variant}` の腕を消した注入を 2 が名指ししない: {problems:#?}"
        );
    }
    // 先取りへ寄せた要求: 同期の腕を修正前の呼び出し（その場で問い合わせる）へ戻すと 1 と 2 が名指す
    for (variant, _, _, _, call, legacy_call) in PROBE_TARGETS {
        assert!(
            text.contains(call),
            "注入の元の `{call}` が本文に無い（形を変えたなら注入も追うこと）"
        );
        let injected = text.replacen(call, legacy_call, 1);
        let graph = graph_with(&[(DISPATCH, &injected)]);
        let findings = scan(&graph);
        let at = injected
            .find(legacy_call)
            .expect("注入した呼び出しが本文にある");
        let line = injected[..at].matches('\n').count() + 1;
        assert!(
            findings
                .iter()
                .any(|f| f.variant == *variant && f.line == line),
            "`{variant}` の同期の腕を修正前の呼び出しへ戻した注入（{DISPATCH}:{line}）が名指しされない: {findings:#?}"
        );
        let problems = probe_target_problems(&injected);
        assert!(
            problems.iter().any(|p| p.contains(call)),
            "`{variant}` の同期の腕を修正前の呼び出しへ戻した注入を 2 が名指ししない: {problems:#?}"
        );
    }
}

// ===== 4. 呼び出しのグラフの外（tako-app）の UI スレッドの着地 =====

/// tako-app の UI スレッドで回る着地（`(ファイル, 関数の頭)`。#2961）。呼び出しのグラフは tako-control /
/// tako-core しか辿らないので、ここに挙げた関数の本文に外部プロセスの起動・待ち（#1984 の葉）と、版の読み
/// （`extract_version_from_path(` = パスに版が無ければ `claude --version`）が直に書かれていないかを字面で見る。
/// A/B の旧経路の腕（`if …legacy() {` の then 側）は除く。
/// #2969: ssh の検知の着地（`apply_ssh_scan_with` → 引き取り `adopt_ssh_connect`）も挙げる
const TAKO_APP_UI_LANDINGS: &[(&str, &str)] = &[
    ("crates/tako-app/src/main.rs", "fn apply_stale_binary_scan("),
    ("crates/tako-app/src/ssh_folders.rs", "fn apply_ssh_scan("),
    (
        "crates/tako-app/src/ssh_folders.rs",
        "fn apply_ssh_scan_with(",
    ),
    ("crates/tako-app/src/main.rs", "fn adopt_ssh_connect("),
];

/// tako-app の着地で名指す綴りのうち、#1984 の葉に無いもの（`(綴り, 理由)`）
const TAKO_APP_EXTRA_LEAVES: &[(&str, &str)] = &[
    (
        "extract_version_from_path(",
        "claude の版を読む（パスにも版リソースにも版が無ければ `claude --version` を上限まで待つ。#2961）",
    ),
    (
        "remote_ssh_argv(",
        "SSH ペインの argv を組む（ssh の実体をログインシェルの `command -v ssh` で引く = 上限 5 秒。\
         走査が引いた実体を受ける `remote_ssh_argv_with` を使う。#2969）",
    ),
    (
        "remote_fs::ssh_bin(",
        "ssh の実体を引く（ログインシェルの `command -v ssh`。#2969）",
    ),
];

/// `text`（テスト領域を潰した本文）の関数 `head` の中で、A/B の旧経路の腕の外に書かれた葉（`(行, 綴り, 理由)`）
fn ui_landing_offenders(text: &str, head: &str) -> Option<Vec<(usize, String, String)>> {
    let code = production_range::code_view::code_view(text);
    let bytes = code.as_bytes();
    let at = code.find(head)?;
    let open = at + code[at..].find('{')?;
    let close = matching(bytes, open);
    let body = &code[open..close];
    // A/B の旧経路の腕（`if …legacy() {` の then 側）の範囲
    let mut legacy: Vec<(usize, usize)> = Vec::new();
    for (i, _) in body.match_indices("legacy() {") {
        let line_start = body[..i].rfind('\n').map_or(0, |n| n + 1);
        if !body[line_start..i].trim_start().contains("if ") {
            continue;
        }
        let brace = open + i + "legacy() ".len();
        legacy.push((brace, matching(bytes, brace)));
    }
    let line_of = |pos: usize| code[..pos].matches('\n').count() + 1;
    let mut out = Vec::new();
    let leaves: Vec<(String, String)> = leaves()
        .iter()
        .cloned()
        .chain(
            TAKO_APP_EXTRA_LEAVES
                .iter()
                .map(|(n, w)| (n.to_string(), w.to_string())),
        )
        .collect();
    for (needle, why) in &leaves {
        for (i, _) in body.match_indices(needle.as_str()) {
            let pos = open + i;
            if legacy.iter().any(|(a, b)| (*a..*b).contains(&pos)) {
                continue;
            }
            out.push((line_of(pos), needle.clone(), why.clone()));
        }
    }
    out.sort();
    Some(out)
}

#[test]
fn tako_appのuiスレッドの着地は外部プロセスを待たない() {
    let mut problems = Vec::new();
    for (file, head) in TAKO_APP_UI_LANDINGS {
        match ui_landing_offenders(&read(file), head) {
            None => problems.push(format!(
                "{file}:1 `{head}` が無い（走査が空振り。改名したなら TAKO_APP_UI_LANDINGS も追うこと）"
            )),
            Some(found) => {
                for (line, needle, why) in found {
                    problems.push(format!(
                        "{file}:{line}（UI スレッドの着地 `{head}`）が `{needle}` を呼ぶ（{why}）。\
                         背景の段で済ませて結果だけを着地へ運ぶこと（#2961）"
                    ));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

#[test]
fn tako_appのsshの引き取りでログインシェルを待つ修正前の形を名指しできる() {
    // #2969: 引き取りの A/B の門を外す（着地でログインシェルの `command -v ssh` を待つ修正前の形）
    let file = "crates/tako-app/src/main.rs";
    let text = read(file);
    let fixed = "let argv = if crate::ssh_folders::issue2969_legacy() {";
    assert!(
        text.contains(fixed),
        "注入の元（A/B の門つきの argv の組み立て）が本文に無い（形を変えたなら注入も追うこと）"
    );
    let injected = text.replacen(fixed, "let argv = if true {", 1);
    let found = ui_landing_offenders(&injected, "fn adopt_ssh_connect(")
        .expect("注入した本文にも関数がある");
    assert!(
        found.iter().any(|(_, n, _)| n == "remote_ssh_argv("),
        "門を外した注入（着地でログインシェルを待つ `remote_ssh_argv`）を名指ししない: {found:?}"
    );
    // 着地の本体へ ssh の実体の解決を直に書き足す形も名指す
    let folders = "crates/tako-app/src/ssh_folders.rs";
    let text = read(folders);
    let anchor = "    ) -> Vec<SshAutoOpenJob> {\n";
    assert!(
        text.contains(anchor),
        "注入の位置（`apply_ssh_scan_with` の頭）が本文に無い（形を変えたなら注入も追うこと）"
    );
    let injected = text.replacen(
        anchor,
        "    ) -> Vec<SshAutoOpenJob> {\n        let adopt_ssh_bin = tako_core::remote_fs::ssh_bin();\n",
        1,
    );
    let found = ui_landing_offenders(&injected, "fn apply_ssh_scan_with(")
        .expect("注入した本文にも関数がある");
    assert!(
        found.iter().any(|(_, n, _)| n == "remote_fs::ssh_bin("),
        "着地で ssh の実体を引く注入を名指ししない: {found:?}"
    );
    // 今の形は名指さない
    for (f, head) in [
        (file, "fn adopt_ssh_connect("),
        (folders, "fn apply_ssh_scan_with("),
    ] {
        assert_eq!(
            ui_landing_offenders(&read(f), head),
            Some(Vec::new()),
            "{f} {head}"
        );
    }
}

#[test]
fn tako_appの着地で版を読む修正前の形を名指しできる() {
    let file = "crates/tako-app/src/main.rs";
    let text = read(file);
    let fixed = "let spawned_version = if tako_control::dispatch::sync_arm_offload_legacy() {";
    assert!(
        text.contains(fixed),
        "注入の元（A/B の門つきの版の読み）が本文に無い（形を変えたなら注入も追うこと）"
    );
    // 修正前の形: 門を外して着地で版を読む
    let injected = text.replacen(fixed, "let spawned_version = if true {", 1);
    let found = ui_landing_offenders(&injected, "fn apply_stale_binary_scan(")
        .expect("注入した本文にも関数がある");
    assert!(
        found
            .iter()
            .any(|(_, n, _)| n == "extract_version_from_path("),
        "門を外した注入（着地で `claude --version`）を名指ししない: {found:?}"
    );
    // 今の形は名指さない
    assert_eq!(
        ui_landing_offenders(&text, "fn apply_stale_binary_scan("),
        Some(Vec::new())
    );
}
