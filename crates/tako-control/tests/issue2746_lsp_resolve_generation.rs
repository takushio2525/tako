//! #2746 の e2e と番犬: 言語サーバの manager の解決（`Shared::resolve`）が、restart をまたいで古い答えを
//! 解決の覚え（`resolved`）へ戻さないこと。
//!
//! 解決はログインシェルを待つのでロックの外で走る。restart（`tako lsp restart`）はその間に覚えを捨てるが、
//! restart の前に始まった解決が restart の後に着地すると、捨てたはずの古い答えがもう一度覚えに入る
//! （入れて restart したサーバが「未導入」のまま起きない）。修正は覚えを捨てるたびに ID ごとの世代を進め、
//! 引く直前に読んだ世代と違えば書かない（`remember_resolved` / `forget_resolved`）。A/B は `TAKO_2746_LEGACY=1`。
//!
//! **注入**: テストの launcher が、止めると決めたサーバの 1 回目の解決を「呼ばれた時点の答え」を持ったまま止め、
//! restart の後に放す（ログインシェルが遅い機で解決が restart をまたぐ形。env も CPU の負荷も使わない）。
//! 答えは呼ばれた時点で決まる = 本番の解決が PATH を引いた時点の答えを返すのと同じ
//!
//! **番犬**（末尾）: 世代を見ずに `resolved` へ書く口（書く口 `remember_resolved` / 捨てる口 `forget_resolved` の
//! 外の書き込み・世代を比べない書く口・世代を進めない捨てる口・引いた後に世代を読む解決）を file:line で名指す

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::{legacy_2746, DEFAULT_IDLE_GRACE};
use tako_control::lsp::{DocLink, Launch, LspConfig, LspManager};
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::platform::child_cmd::ChildCmd;
use tako_core::test_residue::ScratchDir;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 止めると決めたサーバの 1 回目の解決だけを止める launcher の持ち物
struct Gate {
    calls: AtomicUsize,
    /// 止めるサーバ（その 1 回目だけ）
    block_id: &'static str,
    blocked: AtomicBool,
    /// 利用者がサーバを入れたか（解決は呼ばれた時点のこの値で答える）
    installed: AtomicBool,
    entered: Mutex<mpsc::Sender<()>>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl Gate {
    fn new(block_id: &'static str) -> (Arc<Self>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let gate = Arc::new(Self {
            calls: AtomicUsize::new(0),
            block_id,
            blocked: AtomicBool::new(false),
            installed: AtomicBool::new(false),
            entered: Mutex::new(entered_tx),
            release: Mutex::new(release_rx),
        });
        (gate, entered_rx, release_tx)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

/// 表の先頭から `n` 個（先頭は rust-analyzer）
fn table(n: usize) -> &'static [ServerSpec] {
    &servers::SERVERS[..n]
}

fn gated(gate: Arc<Gate>, table: &'static [ServerSpec]) -> LspConfig {
    // 初回 exec の検査を時計の外で済ませる（#2228）
    tako_core::fake_exec::warm_binary(FAKE);
    LspConfig {
        table,
        launcher: Arc::new(move |spec: &ServerSpec| {
            gate.calls.fetch_add(1, Ordering::SeqCst);
            let answer = if gate.installed.load(Ordering::SeqCst) {
                Launch::Found {
                    plan: ChildCmd {
                        program: FAKE.to_string(),
                        args: Vec::new(),
                    },
                    program_path: FAKE.to_string(),
                }
            } else {
                Launch::NotFound {
                    program: spec.program.to_string(),
                    override_env: None,
                }
            };
            if spec.id == gate.block_id && !gate.blocked.swap(true, Ordering::SeqCst) {
                // 注入: 答えを持ったまま、テストが放すまで着地しない
                let _ = gate.entered.lock().unwrap().send(());
                let _ = gate
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(20));
            }
            answer
        }),
        request_timeout: Duration::from_secs(10),
        shutdown_timeout: Duration::from_secs(5),
        idle_grace: DEFAULT_IDLE_GRACE,
        restart: RestartPolicy {
            max_restarts: 3,
            base_delay_ms: 50,
        },
        raw_log_dir: None,
    }
}

fn row(servers: &Value, i: usize) -> &Value {
    &servers["servers"][i]
}

fn wait_until(what: &str, limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + limit;
    while !done() {
        if Instant::now() >= deadline {
            println!("TAKO_2746: {what} が {limit:?} 以内に起きなかった");
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

/// (a) restart の後、誰も引き直さないうちに restart の前に始まった解決が着地する
#[test]
fn restartの前に始まった解決が後から着地しても古い答えを覚えへ戻さない() {
    let (gate, entered, release) = Gate::new(servers::SERVERS[0].id);
    let manager = LspManager::new(gated(Arc::clone(&gate), table(1)));
    std::thread::scope(|s| {
        let inflight = s.spawn(|| manager.servers());
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("1 回目の解決が始まる");
        // 解決が待っている間に利用者がサーバを入れ、`tako lsp restart` で引き直しを頼む
        gate.installed.store(true, Ordering::SeqCst);
        manager.restart(None);
        release.send(()).unwrap();
        let stale = inflight.join().unwrap();
        assert_eq!(
            row(&stale, 0)["installed"],
            json!(false),
            "restart の前に始まった解決は入れる前の答え"
        );
    });
    let after = manager.servers();
    println!(
        "TAKO_2746: (a) restart の後の servers: installed={} cached={} 解決の回数={} legacy={}",
        row(&after, 0)["installed"],
        row(&after, 0)["cached"],
        gate.calls(),
        legacy_2746()
    );
    if legacy_2746() {
        // 修正前: restart の前の答え（見つからない）が覚えに戻り、引き直さずにそれを返す
        assert_eq!(row(&after, 0)["installed"], json!(false));
        assert_eq!(row(&after, 0)["cached"], json!(true));
    } else {
        assert_eq!(
            row(&after, 0)["installed"],
            json!(true),
            "restart の後は入れた後の答え（restart の前の答えが覚えに戻っていない）"
        );
        assert_eq!(
            row(&after, 0)["cached"],
            json!(false),
            "restart の後は引き直す"
        );
    }
    manager.shutdown_all(Duration::from_secs(2));
}

/// (b) restart の後の新しい解決が先に覚えへ入り、restart の前に始まった解決がその後に着地する
#[test]
fn restartの後の新しい答えを後から着地した古い答えで上書きしない() {
    let (gate, entered, release) = Gate::new(servers::SERVERS[0].id);
    let manager = LspManager::new(gated(Arc::clone(&gate), table(1)));
    std::thread::scope(|s| {
        let inflight = s.spawn(|| manager.servers());
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("1 回目の解決が始まる");
        gate.installed.store(true, Ordering::SeqCst);
        manager.restart(None);
        let fresh = manager.servers();
        assert_eq!(
            row(&fresh, 0)["installed"],
            json!(true),
            "restart の後の解決は入れた後の答え"
        );
        assert_eq!(row(&fresh, 0)["cached"], json!(false));
        release.send(()).unwrap();
        inflight.join().unwrap();
    });
    let after = manager.servers();
    println!(
        "TAKO_2746: (b) 古い解決の着地の後の servers: installed={} cached={} 解決の回数={} legacy={}",
        row(&after, 0)["installed"],
        row(&after, 0)["cached"],
        gate.calls(),
        legacy_2746()
    );
    // 新しい答えは覚えから答える（引き直さない = #1769）。修正前は古い答えで上書きされている
    assert_eq!(row(&after, 0)["installed"], json!(!legacy_2746()));
    assert_eq!(row(&after, 0)["cached"], json!(true));
    manager.shutdown_all(Duration::from_secs(2));
}

/// (c) 利用者に見える害: 入れて restart した後に文書を開いても、サーバが「未導入」のまま起きない
#[test]
fn restartの後に開いた文書のサーバは古い答えで未導入にならず起きる() {
    let scratch = ScratchDir::new("tako-2746-doc");
    std::fs::create_dir_all(scratch.path().join("src")).unwrap();
    std::fs::write(
        scratch.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\n",
    )
    .unwrap();
    let (gate, entered, release) = Gate::new(servers::SERVERS[0].id);
    let manager = LspManager::new(gated(Arc::clone(&gate), table(1)));
    std::thread::scope(|s| {
        let inflight = s.spawn(|| manager.servers());
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("1 回目の解決が始まる");
        gate.installed.store(true, Ordering::SeqCst);
        manager.restart(None);
        release.send(()).unwrap();
        inflight.join().unwrap();
    });
    let mut link = DocLink::default();
    manager.sync(
        &mut link,
        true,
        &scratch.path().join("src/main.rs"),
        "fn main() {}\n",
        1,
    );
    let want = if legacy_2746() {
        "not_installed"
    } else {
        "running"
    };
    let state = || manager.status(None)["servers"][0]["state"].clone();
    let landed = wait_until(want, Duration::from_secs(10), || state() == json!(want));
    println!(
        "TAKO_2746: (c) restart の後に開いた文書のサーバ: state={} 解決の回数={} legacy={}",
        state(),
        gate.calls(),
        legacy_2746()
    );
    assert!(
        landed,
        "入れて restart した後のサーバは {want}（修正前は古い答えで未導入）: {}",
        state()
    );
    manager.shutdown_all(Duration::from_secs(2));
}

/// (d) 名指しの restart は名指したサーバの世代だけを進める = 別のサーバの解決の答えは捨てない
/// （#1769 の「名指しならその 1 つだけ引き直す」を保つ。修正前後で同じ）
#[test]
fn 名指しのrestartは別のサーバの解決の答えを捨てない() {
    let blocked = servers::SERVERS[0].id;
    let named = servers::SERVERS[1].id;
    let (gate, entered, release) = Gate::new(blocked);
    let manager = LspManager::new(gated(Arc::clone(&gate), table(2)));
    std::thread::scope(|s| {
        let inflight = s.spawn(|| manager.servers());
        entered
            .recv_timeout(Duration::from_secs(10))
            .expect("1 回目の解決が始まる");
        manager.restart(Some(named));
        release.send(()).unwrap();
        inflight.join().unwrap();
    });
    let calls_before = gate.calls();
    let after = manager.servers();
    println!(
        "TAKO_2746: (d) {named} を名指しで restart した後: {blocked} cached={} / {named} cached={} 解決の回数 {calls_before} → {}",
        row(&after, 0)["cached"],
        row(&after, 1)["cached"],
        gate.calls()
    );
    assert_eq!(
        row(&after, 0)["cached"],
        json!(true),
        "名指していないサーバの答えは覚えに残る"
    );
    assert_eq!(
        row(&after, 1)["cached"],
        json!(false),
        "名指したサーバは引き直す"
    );
    assert_eq!(gate.calls(), calls_before + 1);
    manager.shutdown_all(Duration::from_secs(2));
}

// ---- 番犬 ----

const MANAGER: &str = "crates/tako-control/src/lsp/manager.rs";

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**ので行番号はずれない
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードだけの眺め（テスト領域とコメントを同じバイト数の空白へ潰す）
fn read_manager() -> String {
    let src = std::fs::read_to_string(repo_root().join(MANAGER))
        .unwrap_or_else(|e| panic!("{MANAGER} を読めない: {e}"));
    production_range::code_view::without_comments(&production_range::production(&src, MANAGER))
}

/// バイト位置の行番号（1 始まり）
fn line_at(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// `open` の位置から始まる `{` に対応する `}` の位置（文字列の中の括弧は数えない）
fn block_end(src: &str, open: usize) -> Option<usize> {
    let code = production_range::code_view_of(src);
    let bytes = code.as_bytes();
    let start = open + code[open..].find('{')?;
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// `head`（`fn 名前(` まで）で始まる関数の本体（開始位置と本文）
fn fn_body<'a>(src: &'a str, head: &str) -> Option<(usize, &'a str)> {
    let at = src.find(head)?;
    let end = block_end(src, at)?;
    Some((at, &src[at..=end]))
}

/// 位置 `at` を囲む関数の名前（直前の `fn 名前`）
fn enclosing_fn(src: &str, at: usize) -> String {
    let bytes = src.as_bytes();
    let mut search = at;
    while let Some(i) = src[..search].rfind("fn ") {
        search = i;
        if i > 0 && is_ident(bytes[i - 1]) {
            continue;
        }
        return src[i + 3..]
            .bytes()
            .take_while(|b| is_ident(*b))
            .map(char::from)
            .collect();
    }
    String::new()
}

/// 覚えへ足す・書き換える呼び出し（書く口 `remember_resolved` の中だけ）
const INSERTS: &[&str] = &[
    "insert",
    "entry",
    "extend",
    "append",
    "get_mut",
    "values_mut",
    "iter_mut",
];
/// 覚えを捨てる呼び出し（捨てる口 `forget_resolved` の中だけ）
const REMOVES: &[&str] = &[
    "remove",
    "remove_entry",
    "retain",
    "clear",
    "split_off",
    "pop_first",
    "pop_last",
    "extract_if",
];

/// 世代を見ずに `resolved`（フィールド）へ書く口。改行をまたいだ `inner\n.resolved\n.retain(` も拾う
fn stray_writes(src: &str, rel: &str) -> Vec<String> {
    let bytes = src.as_bytes();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(off) = src[from..].find("resolved") {
        let at = from + off;
        from = at + "resolved".len();
        // 語の切れ目（`resolved_gen` や `apply_resolved` は別の名前）
        if (at > 0 && is_ident(bytes[at - 1])) || bytes.get(from).copied().is_some_and(is_ident) {
            continue;
        }
        // フィールド = 直前（空白を飛ばして）が `.`
        let before = src[..at].trim_end();
        let Some(receiver) = before.strip_suffix('.') else {
            continue;
        };
        let after = src[from..].trim_start();
        let write = if let Some(call) = after.strip_prefix('.') {
            let method: String = call
                .trim_start()
                .bytes()
                .take_while(|b| is_ident(*b))
                .map(char::from)
                .collect();
            if INSERTS.contains(&method.as_str()) {
                Some((method, "remember_resolved"))
            } else if REMOVES.contains(&method.as_str()) {
                Some((method, "forget_resolved"))
            } else {
                None
            }
        } else if after.starts_with('=') && !after.starts_with("==") {
            Some(("=（丸ごと置き換え）".to_string(), ""))
        } else if receiver_after_mut_borrow(receiver) {
            Some(("&mut（丸ごと渡す）".to_string(), ""))
        } else {
            None
        };
        let Some((method, allowed)) = write else {
            continue;
        };
        let owner = enclosing_fn(src, at);
        if !allowed.is_empty() && owner == allowed {
            continue;
        }
        found.push(format!(
            "{rel}:{} `{owner}` が解決の覚え `resolved` へ `{method}` で直に触る（#2746。書く口は \
             `remember_resolved`（引く直前の世代と比べる）、捨てる口は `forget_resolved`（世代を進める）だけ）",
            line_at(src, at)
        ));
    }
    found
}

/// `receiver`（`.resolved` の手前）の受け手の並び（`inner` / `self.lock()` 等）の直前が `&mut` か
fn receiver_after_mut_borrow(receiver: &str) -> bool {
    let Some(at) = receiver.rfind("&mut") else {
        return false;
    };
    let chain = &receiver[at + "&mut".len()..];
    chain.starts_with(|c: char| c.is_whitespace())
        && chain
            .bytes()
            .all(|b| is_ident(b) || matches!(b, b'.' | b'(' | b')') || b.is_ascii_whitespace())
}

/// 番犬の本体: 違反を file:line つきで返す（空なら合格）
fn check(src: &str, rel: &str) -> Vec<String> {
    let mut found = stray_writes(src, rel);
    match fn_body(src, "fn remember_resolved(") {
        Some((at, body)) => {
            let compare = body.find("resolved_generation(");
            let insert = body.find(".insert(");
            if !(compare.is_some_and(|c| insert.is_some_and(|i| c < i))
                && body.contains("legacy_2746()")
                && body.contains("return"))
            {
                found.push(format!(
                    "{rel}:{} `remember_resolved` が書く前に今の世代（`resolved_generation`）と引く直前の世代を比べて\
                     違えば書かずに返す形でない（A/B のゲート `legacy_2746()` も要る。#2746）",
                    line_at(src, at)
                ));
            }
        }
        None => found.push(format!(
            "{rel}: 書く口 `fn remember_resolved(` が無い（#2746）"
        )),
    }
    match fn_body(src, "fn forget_resolved(") {
        Some((at, body)) => {
            if !(body.contains("resolved_gen") && body.contains("wrapping_add(")) {
                found.push(format!(
                    "{rel}:{} `forget_resolved` が覚えを捨てるときに世代（`resolved_gen`）を進めない（#2746。\
                     進めないと捨てる前に始まった解決の答えが後から書かれる）",
                    line_at(src, at)
                ));
            }
        }
        None => found.push(format!(
            "{rel}: 捨てる口 `fn forget_resolved(` が無い（#2746）"
        )),
    }
    match fn_body(src, "fn resolve(&self, spec") {
        Some((at, body)) => {
            let generation = body.find("resolved_generation(");
            let launcher = body.find("(self.config.launcher)(");
            let remember = body.find("remember_resolved(");
            let ordered = matches!(
                (generation, launcher, remember),
                (Some(g), Some(l), Some(r)) if g < l && l < r
            );
            if !ordered {
                found.push(format!(
                    "{rel}:{} 解決 `resolve` が「世代を読む → 引く → `remember_resolved` へ渡す」の順でない\
                     （引いた後に世代を読むと、引いている間の restart を見逃す。#2746）",
                    line_at(src, at)
                ));
            }
        }
        None => found.push(format!(
            "{rel}: 解決 `fn resolve(&self, spec` が無い（#2746）"
        )),
    }
    match fn_body(src, "fn legacy_2746(") {
        Some((at, body)) if !body.contains("\"TAKO_2746_LEGACY\"") => found.push(format!(
            "{rel}:{} A/B のゲート `legacy_2746` が `TAKO_2746_LEGACY` を読まない",
            line_at(src, at)
        )),
        Some(_) => {}
        None => found.push(format!("{rel}: A/B のゲート `fn legacy_2746(` が無い")),
    }
    found
}

#[test]
fn 番犬_解決の覚えへ世代を見ずに書く口が無い() {
    let src = read_manager();
    let found = check(&src, MANAGER);
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// 番犬の検出力: 今の形を 1 か所ずつ崩した合成の本文を、それぞれ名指す
#[test]
fn 番犬_崩した形を名指す() {
    let good = r#"
pub fn legacy_2746() -> bool { std::env::var("TAKO_2746_LEGACY").is_ok() }
impl Shared {
    fn restart(&self, name: Option<&str>) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        self.forget_resolved(inner, name);
    }
    fn resolve(&self, spec: &'static ServerSpec) -> (Launch, bool) {
        let generation = self.lock().resolved_generation(spec.id);
        let launch = (self.config.launcher)(spec);
        self.remember_resolved(spec.id, generation, Resolved { launch: launch.clone() });
        (launch, false)
    }
    fn remember_resolved(&self, id: &'static str, generation: u64, resolved: Resolved) {
        let mut inner = self.lock();
        if !legacy_2746() && inner.resolved_generation(id) != generation {
            return;
        }
        inner.resolved.insert(id, resolved);
    }
    fn forget_resolved(&self, inner: &mut Inner, name: Option<&str>) {
        for spec in self.config.table {
            inner.resolved.remove(spec.id);
            let generation = inner.resolved_gen.entry(spec.id).or_insert(0);
            *generation = generation.wrapping_add(1);
        }
    }
    fn cached_launch(&self, spec: &ServerSpec) -> Option<Launch> {
        self.lock().resolved.get(spec.id).map(|r| r.launch.clone())
    }
}
"#;
    assert_eq!(check(good, "good.rs"), Vec::<String>::new(), "崩す前は合格");
    let cases: &[(&str, &str, &str)] = &[
        (
            "restart が覚えを直に捨てる（改行をまたぐ）",
            "        self.forget_resolved(inner, name);\n    }\n    fn resolve",
            "        inner\n            .resolved\n            .retain(|id, _| name.is_some_and(|n| n != *id));\n    }\n    fn resolve",
        ),
        (
            "解決が世代を見ずに直に書く",
            "        self.remember_resolved(spec.id, generation, Resolved { launch: launch.clone() });",
            "        self.lock().resolved.insert(spec.id, Resolved { launch: launch.clone() });",
        ),
        (
            "書く口が世代を比べない",
            "        if !legacy_2746() && inner.resolved_generation(id) != generation {\n            return;\n        }\n",
            "",
        ),
        (
            "捨てる口が世代を進めない",
            "            *generation = generation.wrapping_add(1);\n",
            "",
        ),
        (
            "引いた後に世代を読む",
            "        let generation = self.lock().resolved_generation(spec.id);\n        let launch = (self.config.launcher)(spec);",
            "        let launch = (self.config.launcher)(spec);\n        let generation = self.lock().resolved_generation(spec.id);",
        ),
        (
            "丸ごと取り替える",
            "    fn cached_launch(",
            "    fn reset(&self) { let _ = std::mem::take(&mut self.lock().resolved); }\n    fn cached_launch(",
        ),
        (
            "A/B のゲートが env を読まない",
            "std::env::var(\"TAKO_2746_LEGACY\").is_ok()",
            "false",
        ),
    ];
    for (what, from, to) in cases {
        assert!(good.contains(from), "{what}: 崩す元が見本に無い");
        let broken = good.replacen(from, to, 1);
        let found = check(&broken, "broken.rs");
        println!("TAKO_2746_WATCHDOG: {what} → {found:?}");
        assert!(
            found.iter().any(|f| f.starts_with("broken.rs:")),
            "{what} を名指さない: {found:?}"
        );
    }
}
