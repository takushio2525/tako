//! #2034 の e2e: tako の外で変わったファイルを言語サーバへ知らせる（`workspace/didChangeWatchedFiles`）。
//!
//! 偽の言語サーバ（`tako-lsp-fake`）を `--disk-model` で起こす = **実の rust-analyzer と同じ持ち方**
//! （開いていないファイルは読み込んだ時点の中身を持ち、監視の知らせを受けたファイルだけ読み直す）。
//! 外で `cargo fmt` 相当の書き換え・`git checkout` 相当の差し替えをして、
//!
//! - 知らせの中身（URI・種別）が偽サーバへ届く（OS の監視 → まとめ送り → 登録の照合 → 送信の実経路）
//! - 参照・定義の位置が新しい中身と一致する（抜粋は tako がディスクから読む = ずれていれば別の行が出る）
//! - A/B の旧腕（`TAKO_2034_LEGACY=1`）では同じ手順で古い位置のまま（= 修正前の症状）
//!
//! を測る。登録に当たらないファイル・`kind` で外した種別・解除の後・アイドルは送らないことも見る。
//!
//! OS の監視（macOS の FSEvents）は監視を始めてから流れ出すまでに間がある（#233 のテストと同じ事情）。
//! 実時間で待たず、**温め用のファイルを書いて、その知らせが偽サーバへ届くまで**待ってから測る。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tako_control::lsp::manager::DEFAULT_IDLE_GRACE;
use tako_control::lsp::{
    DocLink, GotoRequest, Launch, LspConfig, LspManager, XrefAnswer, XrefRequest,
};
use tako_core::lsp::goto::GotoKind;
use tako_core::lsp::servers::{self, ServerSpec};
use tako_core::lsp::state::RestartPolicy;
use tako_core::lsp::xref::XrefKind;
use tako_core::platform::child_cmd::ChildCmd;

#[path = "common/lsp_fake_e2e.rs"]
mod lsp_fake_e2e;

use lsp_fake_e2e::wait_running;

const FAKE: &str = env!("CARGO_BIN_EXE_tako-lsp-fake");

/// 状態待ちの上限（止まったときの保険。届けば待たずに進む）
const LIMIT: Duration = Duration::from_secs(30);

/// まとめ送りの上限の 2 倍（旧腕で「待っても直らない」を見る・アイドルの区切り）
const MAX_DELAY_2X: Duration = Duration::from_secs(2);

/// A/B の env（`TAKO_2034_LEGACY`）を触るので、このファイルのテストは 1 本ずつ走る。
/// 新腕のテストは env に触らない = `TAKO_2034_LEGACY=1 cargo test …` で回すと名指しで落ちる（検出力の確認）
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 使い捨ての置き場（固定名を使わない = 並行する cargo test 同士で消し合わない。#1666）
struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "tako-2034-{label}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj").join("src")).unwrap();
        std::fs::write(
            dir.join("proj").join("Cargo.toml"),
            "[package]\nname = \"x\"\n",
        )
        .unwrap();
        Self(dir)
    }

    /// プロジェクトの根（偽サーバの置き場とは分ける = ログの書き込みを監視が拾わない）
    fn root(&self) -> PathBuf {
        self.0.join("proj")
    }

    fn path(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.root(), |p, part| p.join(part))
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.path(rel);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn log(&self) -> PathBuf {
        self.0.join("received.jsonl")
    }

    fn file(&self, name: &str, body: &Value) -> String {
        let path = self.0.join(name);
        std::fs::write(&path, body.to_string()).unwrap();
        path.display().to_string()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 根の配下の `.rs` を見る登録（rust-analyzer が根ごとに登録する形）
fn rs_watchers() -> Value {
    json!([{ "globPattern": { "baseUri": "{root}", "pattern": "**/*.rs" } }])
}

fn config(scratch: &Scratch, watchers: &Value, extra: &[&str]) -> LspConfig {
    let mut args = vec![
        "--scenario".to_string(),
        "normal".to_string(),
        "--log".to_string(),
        scratch.log().display().to_string(),
        "--disk-model".to_string(),
        "--watchers".to_string(),
        scratch.file("watchers.json", watchers),
        // 参照は模型でその語が現れる所すべて・定義は最初に現れる `fn target`
        "--xref".to_string(),
        scratch.file(
            "xref.json",
            &json!([{ "method": "textDocument/references", "find": "target" }]),
        ),
        "--goto".to_string(),
        scratch.file(
            "goto.json",
            &json!([{ "method": "textDocument/definition", "find": "fn target" }]),
        ),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    LspConfig {
        table: servers::SERVERS,
        launcher: Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: FAKE.to_string(),
                args: args.clone(),
            },
            program_path: FAKE.to_string(),
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

fn received(scratch: &Scratch) -> Vec<Value> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// 偽サーバが受けた `workspace/didChangeWatchedFiles` の変化（URI, 種別）を届いた順に
fn watched_changes(scratch: &Scratch) -> Vec<(String, u64)> {
    received(scratch)
        .iter()
        .filter(|m| m["method"] == json!("workspace/didChangeWatchedFiles"))
        .flat_map(|m| {
            m["params"]["changes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .map(|c| {
            (
                c["uri"].as_str().unwrap_or_default().to_string(),
                c["type"].as_u64().unwrap_or_default(),
            )
        })
        .collect()
}

fn uri(path: &Path) -> String {
    tako_core::file_uri::from_path(path)
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool, detail: impl Fn() -> String) {
    let deadline = Instant::now() + LIMIT;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "{what} が {LIMIT:?} 以内に起きなかった: {}",
            detail()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn status(manager: &LspManager) -> Value {
    manager.status(None)
}

fn server(manager: &LspManager) -> Value {
    status(manager)["servers"][0].clone()
}

/// 編集モードで開いた状態にする（GUI の編集セッションと同じ = `sync` の 1 回目で didOpen）
fn open_editing(manager: &LspManager, path: &Path) -> DocLink {
    let text = std::fs::read_to_string(path).unwrap();
    let mut link = DocLink::default();
    manager.sync(&mut link, true, path, &text, 1);
    assert!(matches!(link, DocLink::Open(_)), "文書が開く");
    link
}

/// 登録が届いて監視が動くまで待ち、温め用のファイルの知らせが偽サーバへ届くまで書き続ける
/// （OS の監視が流れ出すまでの間を実時間で待たない）
fn wait_watching(manager: &LspManager, scratch: &Scratch) {
    wait_running(manager);
    wait_for(
        "監視の登録",
        || {
            server(manager)["watched_files"]["registrations"] == json!(1)
                && status(manager)["file_watch"]["active"] == json!(true)
        },
        || status(manager).to_string(),
    );
    let warm = scratch.path("src/warm.rs");
    let want = uri(&warm);
    let deadline = Instant::now() + LIMIT;
    let mut n = 0;
    while !watched_changes(scratch).iter().any(|(u, _)| *u == want) {
        assert!(
            Instant::now() < deadline,
            "温め用のファイルの知らせが届かない: {}",
            status(manager)
        );
        std::fs::write(&warm, format!("// warm {n}\n")).unwrap();
        n += 1;
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn references(
    manager: &LspManager,
    path: &Path,
    line: usize,
    column: usize,
) -> Vec<(String, usize, String)> {
    let answer = manager
        .xref(&XrefRequest {
            kind: XrefKind::References,
            path: path.to_path_buf(),
            line,
            column,
            timeout: Duration::from_secs(10),
            document: None,
        })
        .expect("参照の答えが来る");
    let XrefAnswer::List { items, .. } = answer else {
        panic!("参照は一覧で返る");
    };
    let mut out: Vec<(String, usize, String)> = items
        .into_iter()
        .map(|place| {
            let name = place
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, place.line, place.excerpt)
        })
        .collect();
    out.sort();
    out
}

fn definition(manager: &LspManager, path: &Path, line: usize, column: usize) -> (String, usize) {
    let answer = manager
        .goto(&GotoRequest {
            kind: GotoKind::Definition,
            path: path.to_path_buf(),
            line,
            column,
            timeout: Duration::from_secs(10),
            document: None,
        })
        .expect("定義の答えが来る");
    let target = answer.targets.first().expect("定義が 1 か所返る");
    (
        target
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        target.line,
    )
}

const MAIN: &str = "mod lib;\nfn main() {\n    lib::target();\n}\n";
const LIB: &str = "pub fn target() {}\n";
/// `cargo fmt` 相当の書き換え: 宣言の前に 3 行増える
const LIB_FORMATTED: &str = "//! 整形で増えた行\n\n#[inline]\npub fn target() {}\n";

/// 書き換える前の参照（lib.rs の 1 行目と main.rs の呼び出し）
fn references_before(manager: &LspManager, main: &Path) {
    assert_eq!(
        references(manager, main, 2, 9),
        vec![
            ("lib.rs".into(), 0, "pub fn target() {}".into()),
            ("main.rs".into(), 2, "lib::target();".into()),
        ],
        "書き換える前は lib.rs の 1 行目"
    );
}

/// 受け入れ条件 1: 外で `cargo fmt` 相当の書き換えをした後、参照・定義の位置が新しい中身と一致する。
/// 知らせの URI は tako がサーバへ渡した綴り（macOS の FSEvents は `/private/var/…` で知らせる）
#[test]
fn 外で整形した開いていないファイルの参照と定義が新しい行で返る() {
    let _serial = serial();
    let scratch = Scratch::new("fmt");
    let main = scratch.write("src/main.rs", MAIN);
    let lib = scratch.write("src/lib.rs", LIB);
    let manager = LspManager::new(config(&scratch, &rs_watchers(), &[]));
    let _link = open_editing(&manager, &main);
    wait_watching(&manager, &scratch);
    let initialize = received(&scratch)
        .into_iter()
        .find(|m| m["method"] == json!("initialize"))
        .unwrap();
    assert_eq!(
        initialize["params"]["capabilities"]["workspace"]["didChangeWatchedFiles"],
        json!({ "dynamicRegistration": true, "relativePatternSupport": true }),
        "監視の動的登録を申告する"
    );

    let lib_uri = uri(&lib);
    let notified = || watched_changes(&scratch).iter().any(|(u, _)| *u == lib_uri);
    references_before(&manager, &main);
    // 外で書き換える → 知らせが届くのを待ってから問い直す
    std::fs::write(&lib, LIB_FORMATTED).unwrap();
    wait_for("lib.rs の知らせ", notified, || {
        format!("{:?} / {}", watched_changes(&scratch), status(&manager))
    });
    let kinds: Vec<u64> = watched_changes(&scratch)
        .into_iter()
        .filter(|(u, _)| *u == lib_uri)
        .map(|(_, k)| k)
        .collect();
    assert!(
        kinds.iter().all(|k| matches!(k, 1 | 2)),
        "書き換えは作成か変更で知らせる（削除にしない）: {kinds:?}"
    );
    assert_eq!(
        references(&manager, &main, 2, 9),
        vec![
            ("lib.rs".into(), 3, "pub fn target() {}".into()),
            ("main.rs".into(), 2, "lib::target();".into()),
        ],
        "参照は整形後の 4 行目（0 起点で 3）"
    );
    assert_eq!(
        definition(&manager, &main, 2, 9),
        ("lib.rs".into(), 3),
        "定義も整形後の行"
    );
    let watched = &server(&manager)["watched_files"];
    assert!(watched["notifications"].as_u64().unwrap() >= 2, "{watched}");
    assert_eq!(watched["dropped"], json!(0), "{watched}");
    assert_eq!(
        watched["patterns"].as_array().map(Vec::len),
        Some(1),
        "{watched}"
    );
}

/// 受け入れ条件 1: `git checkout` 相当の差し替え（rename で置き換え・新しいファイル・消えたファイル）を
/// 作成 / 変更 / 削除で知らせ、参照が差し替えた後の中身で返る
#[test]
fn git_checkout_相当の差し替えを種別つきで知らせる() {
    let _serial = serial();
    let scratch = Scratch::new("checkout");
    let main = scratch.write("src/main.rs", MAIN);
    let lib = scratch.write("src/lib.rs", LIB);
    let gone = scratch.write("src/gone.rs", "fn uses() { target(); }\n");
    let manager = LspManager::new(config(&scratch, &rs_watchers(), &[]));
    let _link = open_editing(&manager, &main);
    wait_watching(&manager, &scratch);
    assert_eq!(
        references(&manager, &main, 2, 9).len(),
        3,
        "差し替える前は lib.rs / main.rs / gone.rs"
    );

    // git checkout は書き換えるファイルを一時ファイル + rename で置き換え、増えたものを作り、無いものを消す
    let tmp = scratch.path("src/.lib.rs.tmp");
    std::fs::write(&tmp, "\n\npub fn target() {}\n").unwrap();
    std::fs::rename(&tmp, &lib).unwrap();
    let added = scratch.write("src/added.rs", "fn more() {\n    target();\n}\n");
    std::fs::remove_file(&gone).unwrap();
    let wanted = [uri(&lib), uri(&added), uri(&gone)];
    wait_for(
        "3 つの知らせ",
        || {
            let got = watched_changes(&scratch);
            wanted.iter().all(|u| got.iter().any(|(g, _)| g == u))
        },
        || format!("{:?}", watched_changes(&scratch)),
    );
    let got = watched_changes(&scratch);
    let kinds = |u: &str| -> Vec<u64> {
        got.iter()
            .filter(|(g, _)| g == u)
            .map(|(_, k)| *k)
            .collect()
    };
    // OS は作成と書き込みを別々に届けうる（作成の後に変更が続く）ので、種別は「どこかで」を見る
    assert!(
        kinds(&uri(&lib)).iter().all(|k| matches!(k, 1 | 2)),
        "置き換えたファイルは作成か変更（途中で消えたことにしない）: {got:?}"
    );
    assert!(
        kinds(&uri(&added)).contains(&1),
        "増えたファイルは作成: {got:?}"
    );
    assert!(!kinds(&uri(&added)).contains(&3), "{got:?}");
    assert_eq!(
        kinds(&uri(&gone)).last(),
        Some(&3),
        "消えたファイルは削除: {got:?}"
    );
    assert!(
        !got.iter().any(|(g, _)| g.ends_with(".lib.rs.tmp")),
        "登録に当たらない一時ファイルは知らせない: {got:?}"
    );
    assert_eq!(
        references(&manager, &main, 2, 9),
        vec![
            ("added.rs".into(), 1, "target();".into()),
            ("lib.rs".into(), 2, "pub fn target() {}".into()),
            ("main.rs".into(), 2, "lib::target();".into()),
        ],
        "差し替えた後の中身（gone.rs は消え、added.rs が増え、lib.rs は 3 行目）"
    );
}

/// 受け入れ条件 2（A/B）: 旧腕（`TAKO_2034_LEGACY=1`）は申告せず、登録を受けても見ない =
/// 外で整形した後も古い行で答え、tako がディスクから読む抜粋は別の行（= 位置がずれる）
#[test]
fn 旧腕では外で整形した後も古い位置のまま() {
    let _serial = serial();
    // 外から渡された A/B の値は終わったら戻す（`TAKO_2034_LEGACY=1` で全体を回すと新腕のテストが落ちる）
    let outer = std::env::var_os("TAKO_2034_LEGACY");
    std::env::set_var("TAKO_2034_LEGACY", "1");
    let scratch = Scratch::new("legacy");
    let main = scratch.write("src/main.rs", MAIN);
    scratch.write("src/lib.rs", LIB);
    let lib = scratch.path("src/lib.rs");
    let manager = LspManager::new(config(&scratch, &rs_watchers(), &[]));
    let _link = open_editing(&manager, &main);
    wait_running(&manager);
    let initialize = received(&scratch)
        .into_iter()
        .find(|m| m["method"] == json!("initialize"))
        .unwrap();
    let declared = initialize["params"]["capabilities"]["workspace"].clone();
    references_before(&manager, &main);
    std::fs::write(&lib, LIB_FORMATTED).unwrap();
    // 新腕なら知らせが届く時間（まとめ送りの上限の 2 倍）を待ってから問う = 競争で古い答えを
    // 拾っているのではなく、待っても直らないことを見る
    std::thread::sleep(MAX_DELAY_2X);
    let (refs, def) = (
        references(&manager, &main, 2, 9),
        definition(&manager, &main, 2, 9),
    );
    // 偽サーバは申告に関係なく登録を送る。旧腕は受けても見ない（登録の返事は null のまま）
    let watch = status(&manager)["file_watch"].clone();
    let registrations = server(&manager)["watched_files"]["registrations"].clone();
    match outer {
        Some(value) => std::env::set_var("TAKO_2034_LEGACY", value),
        None => std::env::remove_var("TAKO_2034_LEGACY"),
    }
    assert_eq!(declared, Value::Null, "旧腕は workspace の能力を申告しない");
    assert_eq!(watch["active"], json!(false), "{watch}");
    assert_eq!(registrations, json!(0));
    assert_eq!(
        refs,
        vec![
            ("lib.rs".into(), 0, "//! 整形で増えた行".into()),
            ("main.rs".into(), 2, "lib::target();".into()),
        ],
        "旧腕: サーバは古い中身の 1 行目を返し、その行は今は別の中身（#2034 の症状）"
    );
    assert_eq!(def, ("lib.rs".into(), 0), "旧腕: 定義も古い行");
    assert!(watched_changes(&scratch).is_empty());
}

/// 登録に当たらないファイル・`kind` で外した種別は知らせない（削除だけを見る登録）
#[test]
fn 登録に当たらないファイルと外した種別は知らせない() {
    let _serial = serial();
    let scratch = Scratch::new("kind");
    let main = scratch.write("src/main.rs", MAIN);
    let lib = scratch.write("src/lib.rs", LIB);
    let doomed = scratch.write("src/doomed.rs", "// doomed\n");
    // 削除だけ（kind = 4）
    let watchers =
        json!([{ "globPattern": { "baseUri": "{root}", "pattern": "**/*.rs" }, "kind": 4 }]);
    let manager = LspManager::new(config(&scratch, &watchers, &[]));
    let _link = open_editing(&manager, &main);
    wait_running(&manager);
    wait_for(
        "監視の登録",
        || status(&manager)["file_watch"]["active"] == json!(true),
        || status(&manager).to_string(),
    );
    // 温め: 削除だけを見るので、消しては作り直す
    let warm = scratch.path("src/warm.rs");
    let deadline = Instant::now() + LIMIT;
    while !watched_changes(&scratch)
        .iter()
        .any(|(u, k)| *u == uri(&warm) && *k == 3)
    {
        assert!(Instant::now() < deadline, "温めの削除が届かない");
        std::fs::write(&warm, "x").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let _ = std::fs::remove_file(&warm);
        std::thread::sleep(Duration::from_millis(200));
    }
    let base = watched_changes(&scratch).len();
    std::fs::write(&lib, LIB_FORMATTED).unwrap();
    std::fs::write(scratch.path("notes.txt"), "not rust\n").unwrap();
    // 区切り: 変更と txt の後に消す。消した知らせが届いた時点で、前の 2 つはもう振り分け済み
    std::thread::sleep(Duration::from_millis(300));
    std::fs::remove_file(&doomed).unwrap();
    wait_for(
        "削除の知らせ",
        || {
            watched_changes(&scratch)[base..]
                .iter()
                .any(|(u, _)| *u == uri(&doomed))
        },
        || format!("{:?}", watched_changes(&scratch)),
    );
    std::thread::sleep(Duration::from_millis(400));
    let after: Vec<(String, u64)> = watched_changes(&scratch)[base..].to_vec();
    assert!(
        after
            .iter()
            .all(|(u, k)| *k == 3 && !u.ends_with("lib.rs") && !u.ends_with("notes.txt")),
        "削除だけを知らせ、変更と登録に当たらないファイルは知らせない: {after:?}"
    );
}

/// 解除（`client/unregisterCapability`）の後は知らせず、どのサーバも登録していなければ監視を落とす。
/// サーバを止めても登録を捨てて監視を落とす
#[test]
fn 解除や停止の後は知らせず監視を落とす() {
    let _serial = serial();
    let scratch = Scratch::new("unwatch");
    let main = scratch.write("src/main.rs", MAIN);
    let lib = scratch.write("src/lib.rs", LIB);
    let manager = LspManager::new(config(
        &scratch,
        &rs_watchers(),
        &["--unwatch-on", "unwatch.rs"],
    ));
    let _link = open_editing(&manager, &main);
    wait_watching(&manager, &scratch);
    let starts = status(&manager)["file_watch"]["starts"].clone();
    let trigger = scratch.write("src/unwatch.rs", "// bye\n");
    wait_for(
        "解除",
        || server(&manager)["watched_files"]["registrations"] == json!(0),
        || status(&manager).to_string(),
    );
    assert!(watched_changes(&scratch)
        .iter()
        .any(|(u, _)| *u == uri(&trigger)));
    assert_eq!(
        status(&manager)["file_watch"]["active"],
        json!(false),
        "登録が無くなったら監視ごと落とす（スレッドも残さない）"
    );
    let base = watched_changes(&scratch).len();
    std::fs::write(&lib, LIB_FORMATTED).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(
        watched_changes(&scratch).len(),
        base,
        "解除の後は知らせない"
    );
    assert_eq!(status(&manager)["file_watch"]["starts"], starts);

    // 起こし直す → 新しいプロセスが登録し直す → 止める → 登録を捨てて監視を落とす
    manager.restart(None);
    wait_for(
        "起こし直した後の登録",
        || {
            server(&manager)["watched_files"]["registrations"] == json!(1)
                && status(&manager)["file_watch"]["active"] == json!(true)
        },
        || status(&manager).to_string(),
    );
    manager.stop(None);
    wait_for(
        "止めた後に監視を落とす",
        || {
            server(&manager)["state"] == json!("stopped")
                && status(&manager)["file_watch"]["active"] == json!(false)
        },
        || status(&manager).to_string(),
    );
    assert_eq!(server(&manager)["watched_files"]["registrations"], json!(0));
}

/// アイドルでは何も送らない（OS が知らせてこない限り、監視もまとめ送りも動かない）
#[test]
fn アイドルでは何も送らない() {
    let _serial = serial();
    let scratch = Scratch::new("idle");
    let main = scratch.write("src/main.rs", MAIN);
    scratch.write("src/lib.rs", LIB);
    let manager = LspManager::new(config(&scratch, &rs_watchers(), &[]));
    let _link = open_editing(&manager, &main);
    wait_watching(&manager, &scratch);
    // 温めで書いた残りの知らせが振り分け終わるのを待ってから数える
    std::thread::sleep(MAX_DELAY_2X);
    let messages = received(&scratch).len();
    let batches = status(&manager)["file_watch"]["batches"].clone();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        received(&scratch).len(),
        messages,
        "アイドルでは何も送らない"
    );
    assert_eq!(status(&manager)["file_watch"]["batches"], batches);
}

/// 実の rust-analyzer（`TAKO_2034_REAL_RA=<実行ファイルの実体>` で `--ignored` のときだけ。CI では走らない）。
///
/// 根が**シンボリックリンクを通る**とき（macOS の一時 dir = `/var` → `/private/var`）、旧腕は rust-analyzer
/// 自前の監視が OS の知らせ（実パス）と根を照合できず整形前の行のまま・新腕は tako が渡した綴りへ戻して
/// 知らせるので整形後の行（tako の GUI は文書を実パスで開くので、この差は根がリンクを通る経路でだけ出る）
#[test]
#[ignore = "実の rust-analyzer が要る（TAKO_2034_REAL_RA）"]
fn 実の_rust_analyzer_でリンクを通る根でも整形後の行で返る() {
    let _serial = serial();
    let Some(ra) = std::env::var_os("TAKO_2034_REAL_RA") else {
        eprintln!("TAKO_2034_REAL_RA が無いので測らない");
        return;
    };
    let ra = ra.to_string_lossy().into_owned();
    let outer = std::env::var_os("TAKO_2034_LEGACY");
    let mut lines = Vec::new();
    for legacy in [false, true] {
        if legacy {
            std::env::set_var("TAKO_2034_LEGACY", "1");
        } else {
            std::env::remove_var("TAKO_2034_LEGACY");
        }
        let scratch = Scratch::new(if legacy { "real-legacy" } else { "real-new" });
        std::fs::write(
            scratch.root().join("Cargo.toml"),
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        )
        .unwrap();
        let main = scratch.write(
            "src/main.rs",
            "mod util;\nfn main() {\n    let _ = util::helper();\n}\n",
        );
        let util = scratch.write("src/util.rs", "pub fn helper() -> u32 {\n    1\n}\n");
        let ra = ra.clone();
        let mut config = config(&scratch, &rs_watchers(), &[]);
        config.launcher = Arc::new(move |_spec: &ServerSpec| Launch::Found {
            plan: ChildCmd {
                program: ra.clone(),
                args: Vec::new(),
            },
            program_path: ra.clone(),
        });
        config.request_timeout = Duration::from_secs(120);
        let manager = LspManager::new(config);
        let _link = open_editing(&manager, &main);
        let util_line = |manager: &LspManager| {
            references(manager, &main, 2, 18)
                .into_iter()
                .find(|(name, _, _)| name == "util.rs")
                .map(|(_, line, _)| line)
        };
        assert_eq!(util_line(&manager), Some(0), "整形の前は util.rs の 1 行目");
        if !legacy {
            wait_for(
                "rust-analyzer の監視の登録",
                || status(&manager)["file_watch"]["active"] == json!(true),
                || status(&manager).to_string(),
            );
            // 温め: OS の監視が流れ出して振り分けが 1 回走るまで書く（実サーバのログは読めないので数で見る）
            let warm = scratch.path("src/warm.rs");
            let deadline = Instant::now() + LIMIT;
            let mut n = 0;
            while status(&manager)["file_watch"]["batches"]
                .as_u64()
                .unwrap_or(0)
                == 0
            {
                assert!(Instant::now() < deadline, "温めの振り分けが走らない");
                std::fs::write(&warm, format!("// warm {n}\n")).unwrap();
                n += 1;
                std::thread::sleep(Duration::from_millis(200));
            }
        }
        // `cargo fmt` 相当: 宣言の前に 3 行（`a` を整形すると 3 行増える形）
        std::fs::write(
            &util,
            "pub fn a() {\n    let _x = 1;\n}\npub fn helper() -> u32 {\n    1\n}\n",
        )
        .unwrap();
        // 新腕は知らせが届いて rust-analyzer が読み直すまで・旧腕は同じだけ問い直す（10 秒）
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = util_line(&manager);
        while got != Some(3) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
            got = util_line(&manager);
        }
        lines.push((
            legacy,
            got,
            status(&manager)["file_watch"]["active"].clone(),
        ));
        manager.stop(None);
    }
    match outer {
        Some(value) => std::env::set_var("TAKO_2034_LEGACY", value),
        None => std::env::remove_var("TAKO_2034_LEGACY"),
    }
    eprintln!("実の rust-analyzer（legacy, util.rs の行, 監視）: {lines:?}");
    assert_eq!(lines[0].1, Some(3), "新腕: 整形後の行（{lines:?}）");
    if cfg!(target_os = "macos")
        && tako_core::platform::path::canonicalize_or_self(&std::env::temp_dir())
            != std::env::temp_dir()
    {
        assert_eq!(
            lines[1].1,
            Some(0),
            "旧腕: リンクを通る根では整形前の行のまま（{lines:?}）"
        );
    }
}
