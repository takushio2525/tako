//! 番犬: remote daemon の HTTP 受信が**直列に戻らない** / IPC の直列化が**散らばらない**（#1403）
//!
//! ## なぜ止めるのか
//!
//! daemon の受信ループは長らく「受け取ったリクエストを同じスレッドで最後まで処理する」
//! 形だった。そのため遅い 1 本が全リクエストを塞ぐ。隔離 daemon の実測（2026-09-12・
//! 偽 tailscale の `whois` を 3 秒眠らせた）:
//!
//! ```text
//! ## ベースライン（XFF 無し = whois を呼ばない 401）
//!   no-xff  401 0.000607s
//! ## A を背後で撃ちつつ B を測る
//!   B(no-xff) 401 2.708897s   ← A が終わるまで待たされている
//!   A(with-xff) 200 3.025153s
//! ```
//!
//! 塞がれる側には `/api/health`・`/api/v2/panes` のポーリング・`/ws` のアップグレードが
//! 入る。実運用で遅くなるのは `/api/files` 系（daemon → app の IPC read timeout 10 秒 →
//! sftp）・`/api/v2/panes`（tmux 5 秒）・層① の `whois`（サブプロセス）。
//!
//! 直し方は**少数のワーカーが同じ `tiny_http::Server` から recv する**こと（#1403 の
//! 候補 1）。ただし並列化には 3 つの不変条件がぶら下がるので、どれが欠けても
//! 別の壊れ方に化ける:
//!
//! 1. **合流**（`shutdown` で全ワーカーが畳まれてから後始末が走る）
//! 2. **1 本の panic で daemon を落とさない**（直列の頃は `run_daemon` まで抜けて死んだ）
//! 3. **daemon → app の IPC は同時に 1 本だけ**（ロックを往復の間ずっと握る）
//!
//! ## 7 本立て（どれか 1 つでは穴が残る）
//!
//! 1. [`受信ループが直列に戻っていない`] — `run_daemon` が `serve_http_requests` を通る
//! 2. [`ワーカーの合流とpanic耐性が消えていない`] — `thread::scope` / `catch_unwind`
//! 3. [`ipcのロックはwith_app_ipcの中だけで取る`] — 直列化が呼び出し側へ散らばらない
//! 4. [`with_app_ipcが往復の間ロックを握る`] — ガードを早く落とす形に戻らない
//! 5. [`abの口が消えていない`] — 回帰を隠さない注入（`TAKO_1403_LEGACY`）
//! 6. [`監査ログの追記が1回のwriteで出る`] — 書く者が増えたぶん行が混ざらない
//! 7. [`planに並列化とipc直列化の不変条件が書いてある`] — 次に触る人へ理由が残る

use std::path::{Path, PathBuf};

const REMOTE_RS: &str = "crates/tako-control/src/remote.rs";
const REMOTE_SSH_RS: &str = "crates/tako-control/src/remote_ssh.rs";
const PLAN_MD: &str = ".agent/plans/tako-remote-plan.md";
const REMOTE_AUTH_RS: &str = "crates/tako-control/src/remote_auth.rs";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

/// インデント 0 の `fn` 宣言行から、インデント 0 の閉じ括弧までを本体として切り出す
/// （`rustfmt` が形を保証する）。戻りは `(開始行 1-indexed, 本文)`
fn top_fn_block(src: &str, signature: &str) -> Option<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let at = lines.iter().position(|l| l.starts_with(signature))?;
    let mut end = at + 1;
    while end < lines.len() && lines[end] != "}" {
        end += 1;
    }
    Some((at + 1, lines[at..=end.min(lines.len() - 1)].join("\n")))
}

/// 行コメント（`//` 以降）を落とす。文字列リテラル中の `//` は素朴に見ないが、
/// この番犬が探す needle はどれもコード側の形なので誤検知しない
fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(i) => &line[..i],
        None => line,
    }
}

/// **#1403 の本体**: `run_daemon` の受信が直列ループへ戻っていない。
///
/// 注入（`serve_http_requests(...)` を `while ... server.recv_timeout ... dispatch_one`
/// へ戻す）で、この行を名指しで落とす
#[test]
fn 受信ループが直列に戻っていない() {
    let src = read(REMOTE_RS);
    let (line, body) = top_fn_block(&src, "pub fn run_daemon() -> io::Result<()> {")
        .unwrap_or_else(|| panic!("{REMOTE_RS}: run_daemon が見つからない"));

    assert!(
        body.contains("serve_http_requests("),
        "{REMOTE_RS}:{line}: run_daemon が `serve_http_requests` を通っていない。\
         受信が 1 スレッドへ戻ると、遅い 1 本（/api/files の SSH 先・/api/v2/panes の tmux・\
         層① の whois）が health / 一覧 / WS のアップグレードを塞ぐ（#1403 の実測: B が 2.71 秒）"
    );
    assert!(
        body.contains("http_worker_count()"),
        "{REMOTE_RS}:{line}: ワーカー数が `http_worker_count()` 由来でない。\
         本数の決め方（既定 / env / A/B）が 2 か所に散る"
    );
    // 直列ループの形そのものが復活していないこと（自己疎通チェック中の 1 件処理は
    // `rx.try_recv()` を持つ別の形なので当たらない）
    for (offset, raw) in body.lines().enumerate() {
        let code = strip_line_comment(raw);
        assert!(
            !(code.contains("Ok(Some(request)) => dispatch_one(request)")),
            "{REMOTE_RS}:{}: 直列の受信ループが復活している",
            line + offset
        );
    }
}

/// 並列化にぶら下がる 2 つの不変条件（合流 / panic 耐性）が消えていない。
///
/// - `thread::scope`: `shutdown` で全ワーカーが畳まれてから後始末が走る
/// - `catch_unwind`: 1 本の panic で daemon ごと死なない（直列の頃の後退を戻さない）
#[test]
fn ワーカーの合流とpanic耐性が消えていない() {
    let src = read(REMOTE_RS);
    let (line, body) = top_fn_block(&src, "fn serve_http_requests<D, P>(")
        .unwrap_or_else(|| panic!("{REMOTE_RS}: serve_http_requests が見つからない"));

    assert!(
        body.contains("std::thread::scope("),
        "{REMOTE_RS}:{line}: `std::thread::scope` が消えている。\
         合流を忘れられる形（detach）にすると、後始末（serve 解除・state 削除）が\
         リクエストを捌いている最中に走る"
    );
    assert!(
        body.contains("catch_unwind"),
        "{REMOTE_RS}:{line}: `catch_unwind` が消えている。\
         ワーカー 1 本の panic で daemon 全体が落ちる（直列の頃の挙動へ後退する）"
    );
    assert!(
        body.contains("fatal.store(true"),
        "{REMOTE_RS}:{line}: recv が壊れたときに全ワーカーへ伝えていない。\
         1 本だけ抜けて残りが回り続けると「終了したのに終了しない」状態になる"
    );
}

/// IPC のロックは [`with_app_ipc`] の中だけで取る（#1403 の不変条件）。
///
/// 呼び出し側が自前で `app_conn.write()` すると、往復の途中でガードを落とす形が
/// 静かに戻る（= 同時に 2 本の IPC が走る）
#[test]
fn ipcのロックはwith_app_ipcの中だけで取る() {
    let remote = read(REMOTE_RS);
    let (with_line, with_body) = top_fn_block(&remote, "pub(crate) fn with_app_ipc<T>(")
        .unwrap_or_else(|| panic!("{REMOTE_RS}: with_app_ipc が見つからない"));
    let with_range = with_line..(with_line + with_body.lines().count());

    let mut offenders = Vec::new();
    for (rel, src) in [(REMOTE_RS, &remote), (REMOTE_SSH_RS, &read(REMOTE_SSH_RS))] {
        for (i, raw) in src.lines().enumerate() {
            let line = i + 1;
            if rel == REMOTE_RS && with_range.contains(&line) {
                continue; // ここが唯一の正しい取得点
            }
            let code = strip_line_comment(raw);
            if !code.contains("app_conn") {
                continue;
            }
            if code.contains(".write()") || code.contains(".read()") {
                offenders.push(format!("{rel}:{line}: {}", raw.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "IPC のロックを `with_app_ipc` の外で取っている（#1403 の直列化が散る）:\n{}",
        offenders.join("\n")
    );
}

/// [`with_app_ipc`] が**往復の間ずっと**ロックを握る形であること。
///
/// `conn.get()` だけ取り出してガードを落とす形に戻すと、同時に 2 本の IPC が走る
#[test]
fn with_app_ipcが往復の間ロックを握る() {
    let src = read(REMOTE_RS);
    let (line, body) = top_fn_block(&src, "pub(crate) fn with_app_ipc<T>(")
        .unwrap_or_else(|| panic!("{REMOTE_RS}: with_app_ipc が見つからない"));

    assert!(
        body.contains("app_conn.write()"),
        "{REMOTE_RS}:{line}: 排他ロックを取っていない（`read()` では直列化にならない）"
    );
    assert!(
        body.contains("call(&mut guard)"),
        "{REMOTE_RS}:{line}: 呼び出し側の処理をガードの下で回していない。\
         `&AppIpcClient` を返す形にすると往復の前にロックが落ちる"
    );
    assert!(
        body.contains("InflightGuard::enter()"),
        "{REMOTE_RS}:{line}: 同時実行数の観測（量を観る口）が消えている。\
         直列化が壊れたことをテストが検出できなくなる"
    );
}

/// A/B の口（`TAKO_1403_LEGACY=1` = 直列 1 本）が残っていること。
/// 回帰を隠していないことを毎回確かめられる形を保つ
#[test]
fn abの口が消えていない() {
    let src = read(REMOTE_RS);
    // **env を実際に読む形**で縛る。単なる `TAKO_1403_LEGACY` の文字列一致だと、
    // 名前を説明している doc コメントの側に当たって**注入が素通りする**（実測）
    const READS_ENV: &str = r#"env_on("TAKO_1403_LEGACY")"#;
    let (line, body) = top_fn_block(&src, "fn http_worker_count() -> usize {")
        .unwrap_or_else(|| panic!("{REMOTE_RS}: http_worker_count が見つからない"));
    assert!(
        body.contains(READS_ENV),
        "{REMOTE_RS}:{line}: A/B の口（{READS_ENV}）が `http_worker_count` から消えている。\
         回帰を隠していないことを確かめる手段が無くなる"
    );
    assert_eq!(
        src.matches(READS_ENV).count(),
        1,
        "{REMOTE_RS}:{line}: legacy の判定が 2 か所以上にある。本数の決め方は 1 実装"
    );
    assert!(
        src.contains("fn ワーカー1本では遅い1本が別のリクエストを塞ぐ() {"),
        "{REMOTE_RS}: 旧アーム（ワーカー 1 本 = 直列）で順序が反転することを\
         固定するテストが消えている。新しい方が「材料が緩くて通った」のではないことの証拠"
    );
}

/// 監査ログの 1 行が**1 回の write** で出ること（#1403）。
///
/// `writeln!` は本文と改行を別々に書くので、`O_APPEND` でも行が混ざる。
/// #1403 で受信が 4 ワーカーになり、ここへ同時に書く者が増えた
#[test]
fn 監査ログの追記が1回のwriteで出る() {
    let src = read(REMOTE_AUTH_RS);
    let (line, body) = top_fn_block(
        &src,
        "pub fn append_audit(path: &Path, event: &str, extra: Value) {",
    )
    .unwrap_or_else(|| panic!("{REMOTE_AUTH_RS}: append_audit が見つからない"));
    assert!(
        body.contains("f.write_all(line.as_bytes())"),
        "{REMOTE_AUTH_RS}:{line}: 監査ログの 1 行を 1 回の write で出していない。\
         `writeln!` は本文と改行を別々に書くので、同時に書く者がいると行が混ざる"
    );
    // **コード行だけ**を見る（理由を書いたコメントに `writeln!` が出るので、
    // 素の文字列一致だと自分の説明文に当たって常時 FAILED になる）
    for (offset, raw) in body.lines().enumerate() {
        assert!(
            !strip_line_comment(raw).contains("writeln!"),
            "{REMOTE_AUTH_RS}:{}: `writeln!` が戻っている（行が混ざる形）",
            line + offset
        );
    }
}

/// 不変条件が plan へ書いてあること（次に触る人が理由ごと読める）
#[test]
fn planに並列化とipc直列化の不変条件が書いてある() {
    let src = read(PLAN_MD);
    let needles = [
        "#1403",
        "serve_http_requests",
        "with_app_ipc",
        "TAKO_REMOTE_HTTP_WORKERS",
    ];
    let missing: Vec<&str> = needles
        .iter()
        .copied()
        .filter(|n| !src.contains(n))
        .collect();
    assert!(
        missing.is_empty(),
        "{PLAN_MD}: #1403 の不変条件（受信の並列化と IPC の直列化）の記述が欠けている: {missing:?}"
    );
}
