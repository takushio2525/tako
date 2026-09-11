//! 番犬: tako 自身が開いた SSH ペインを tako 自身の自動検知が見送らない（#1411）
//!
//! ## なぜ止めるのか
//!
//! 「リモート接続…」（#1006）と `remote-folder open`（#1041）が開くペインの `ssh` は
//! tako が組む。そこに載る `-p <port>` は `~/.ssh/config` の `Port` の書き写し
//! （`ssh_config::SshHost::ssh_command`）でしかないのに、#976 の自動検知は
//! 「22 以外のポートは別のマシンかもしれない」で見送っていた = **自分が書いた `-p` を
//! 自分で疑っていた**。非既定ポートの Host はツリーへ自動で並ばない（#1411 の実測）。
//!
//! 同じ症状のもう 1 つの原因が**引用つきオプション値の平坦化**。tako は空白を含む値を
//! `-o ControlPath="…"` と引用して渡すので（`remote_fs::control_path_option`）、
//! macOS の既定 data_dir（`~/Library/Application Support/tako`）では `ps` の 1 行が
//! 空白で割れ、**続きの語が宛先に見えて** `RemoteCommand` で見送られる（ポートが 22 でも
//! 起きるので、既定構成では tako 自身のペインがそもそも 1 枚も検知されない）。
//!
//! ## 何を違反とするか（**誤検知しない形**）
//!
//! 1. [`ポートの判断はssh_detectの1箇所だけ`] — `Err(SkipReason::PortOverride)` を返す
//!    行が製品コードに 1 つだけ。オプションの解釈の途中で返す形（= 宛先が分かる前に
//!    見送る形）へ戻すと数が増えて落ちる
//! 2. [`宛先の名前だけで行けるかを問う実装が1本ある`] — その 1 箇所が
//!    `port_reachable_by_name`（`ConfiguredPorts` を引く純粋関数）を通っていること
//! 3. [`引用つきの値を戻す実装がオプション値に効いている`] — `quote_unbalanced` が
//!    `-o` / `-p` の値を組む枝で使われていること
//! 4. [`走査層はconfigを材料として渡す`] — `tako-control` 側が材料なしの
//!    `parse_ssh_command` を呼んでいないこと（呼ぶと材料が黙って落ちる）
//! 5. [`材料を読むのは走査が起きる_tickだけ`] — `DetectContext::current()` が
//!    間引きの早期復帰より**後ろ**にあること（前に出すと 2 秒ごとに config を読む）
//! 6. [`tako自身のペインが検知される`] / [`手打ちの_pは従来どおり見送る`] — 機構ではなく
//!    **答え**を固定する（公開 API の挙動なので、どちらの原因を戻しても落ちる）
//! 7. [`手順書とplanが新しい挙動を書いている`] — `guides/remote.md` と fixture が
//!    byte 一致のまま新挙動を説明し、古い断言（自動検知に出てこない）が残っていないこと

use std::path::{Path, PathBuf};

use tako_core::ssh_detect::{
    parse_ssh_command, parse_ssh_command_with, ConfiguredPorts, DetectContext, SkipReason,
};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const CORE_RS: &str = "crates/tako-core/src/ssh_detect.rs";
const CONTROL_RS: &str = "crates/tako-control/src/ssh_detect.rs";
const GUIDE_MD: &str = "crates/tako-control/src/orchestrator/guides/remote.md";
const FIXTURE_MD: &str = "crates/tako-control/tests/fixtures/guides_added_after_1154.md";
const PLAN_MD: &str = ".agent/plans/2026-08-remote-folder.md";

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// テストモジュールより手前だけを見る（テストの期待値に当たらない）
fn production(src: &str) -> &str {
    match src.find("\n#[cfg(test)]\nmod tests") {
        Some(i) => &src[..i],
        None => src,
    }
}

/// needle を含む行を `rel:line` の形で拾う（**コメント行は数えない**）
fn hits(rel: &str, src: &str, needle: &str) -> Vec<String> {
    src.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|(i, l)| format!("{rel}:{} {}", i + 1, l.trim()))
        .collect()
}

/// 空白（改行を含む）を 1 個へ畳む。**折り返しで割れた文を探すため**
fn flat(src: &str) -> String {
    src.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// needle を含む最初の行番号（1 始まり。コメント行は数えない）
fn first_line(src: &str, needle: &str) -> Option<usize> {
    src.lines()
        .enumerate()
        .find(|(_, l)| !l.trim_start().starts_with("//") && l.contains(needle))
        .map(|(i, _)| i + 1)
}

#[test]
fn ポートの判断はssh_detectの1箇所だけ() {
    let src = read(CORE_RS);
    let prod = production(&src);
    let found = hits(CORE_RS, prod, "Err(SkipReason::PortOverride)");
    assert_eq!(
        found.len(),
        1,
        "`PortOverride` を返す箇所が {} 個ある（#1411 は宛先が分かってから 1 回だけ判断する）:\n{}",
        found.len(),
        found.join("\n")
    );
}

#[test]
fn 宛先の名前だけで行けるかを問う実装が1本ある() {
    let src = read(CORE_RS);
    let prod = production(&src);
    assert!(
        prod.contains("fn port_reachable_by_name("),
        "{CORE_RS}: `port_reachable_by_name`（宛先の名前だけでそのポートへ行けるかの 1 実装）が無い"
    );
    // 唯一の見送りがその判定を通っていること
    let guard = hits(CORE_RS, prod, "port_reachable_by_name(");
    assert!(
        guard.len() >= 2,
        "{CORE_RS}: `port_reachable_by_name` が定義だけで使われていない:\n{}",
        guard.join("\n")
    );
    let decision = first_line(prod, "Err(SkipReason::PortOverride)").expect("判断の 1 箇所");
    let call = prod
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("!port_reachable_by_name("))
        .map(|(i, _)| i + 1)
        .next()
        .unwrap_or_else(|| panic!("{CORE_RS}: 見送りが `port_reachable_by_name` を通っていない"));
    assert!(
        call < decision,
        "{CORE_RS}:{decision} の見送りが {CORE_RS}:{call} の判定より前に出ている"
    );
    // 材料（config の `Port`）を引いていること
    assert!(
        prod.contains(".port_of("),
        "{CORE_RS}: `ConfiguredPorts::port_of` を引いていない（材料を見ずに判断している）"
    );
}

#[test]
fn 引用つきの値を戻す実装がオプション値に効いている() {
    let src = read(CORE_RS);
    let prod = production(&src);
    assert!(
        prod.contains("fn quote_unbalanced("),
        "{CORE_RS}: `quote_unbalanced`（`ps` の平坦化を戻す判定）が無い"
    );
    for needle in ["quote_unbalanced(&value)", "ctx.join_quoted"] {
        assert!(
            !hits(CORE_RS, prod, needle).is_empty(),
            "{CORE_RS}: オプション値を組む枝に `{needle}` が無い\
             （`-o ControlPath=\"…\"` の続きが宛先に見える形へ戻っている）"
        );
    }
}

#[test]
fn 走査層はconfigを材料として渡す() {
    let src = read(CONTROL_RS);
    let prod = production(&src);
    assert!(
        prod.contains("parse_ssh_command_with(argv, ctx)"),
        "{CONTROL_RS}: 走査が材料つきの `parse_ssh_command_with` を通っていない"
    );
    // 材料なしの入口を呼ぶと config が黙って落ちる
    let strict = hits(CONTROL_RS, prod, "parse_ssh_command(");
    assert!(
        strict.is_empty(),
        "{CONTROL_RS}: 材料なしの `parse_ssh_command` を呼んでいる:\n{}",
        strict.join("\n")
    );
}

#[test]
fn 材料を読むのは走査が起きる_tickだけ() {
    let src = read(CONTROL_RS);
    let prod = production(&src);
    let current = first_line(prod, "DetectContext::current()").expect("実行時の材料");
    let carried = first_line(prod, "return carried_over(").expect("間引きの早期復帰");
    assert!(
        carried < current,
        "{CONTROL_RS}:{current} の `DetectContext::current()` が \
         {CONTROL_RS}:{carried} の早期復帰より前にある（間引いた tick でも config を読む）"
    );
}

/// macOS の既定 data_dir（**空白を含む**）で tako が組む SSH ペインのコマンド行
fn self_opened_argv(host: &str, port: u16) -> String {
    let dir = Path::new("/Users/testuser/Library/Application Support/tako");
    let cp = tako_core::remote_fs::control_path_option(&tako_core::remote_fs::control_path_in(
        Some(dir),
        host,
    ));
    format!(
        "/usr/bin/ssh -o {cp} -o ControlMaster=auto -o ControlPersist=600 \
         -o ConnectTimeout=10 -o ServerAliveInterval=5 -o ServerAliveCountMax=3 -p {port} {host}"
    )
}

#[test]
fn tako自身のペインが検知される() {
    let ctx = DetectContext::with_configured(ConfiguredPorts::from_pairs(&[("win", 2222)]));
    let line = self_opened_argv("win", 2222);
    let got = parse_ssh_command_with(&line, &ctx)
        .unwrap_or_else(|e| panic!("tako 自身のペインを見送った: {}（{line}）", e.label()));
    assert_eq!(got.destination, "win");
    // 引用つきの値が空白で割れる側だけを戻しても落ちる形（ポートは既定）
    let line22 = self_opened_argv("win", 22);
    let got = parse_ssh_command(&line22)
        .unwrap_or_else(|e| panic!("既定ポートのペインを見送った: {}（{line22}）", e.label()));
    assert_eq!(got.destination, "win");
}

#[test]
fn 手打ちの_pは従来どおり見送る() {
    let ctx = DetectContext::with_configured(ConfiguredPorts::from_pairs(&[("win", 2222)]));
    // config が `Port` を持たない Host への `-p`（= 名前だけでは同じ相手へ行けない）
    assert_eq!(
        parse_ssh_command_with("ssh -p 2222 box", &ctx),
        Err(SkipReason::PortOverride)
    );
    // config の宣言と違うポート
    assert_eq!(
        parse_ssh_command_with("ssh -p 2223 win", &ctx),
        Err(SkipReason::PortOverride)
    );
    // 別の config を使っている行（宛先の解決が食い違いうる）
    assert_eq!(
        parse_ssh_command_with("ssh -F /tmp/other -p 2222 win", &ctx),
        Err(SkipReason::PortOverride)
    );
    // 経路や実ホストの書き換えは引用つきでも見送る
    assert_eq!(
        parse_ssh_command_with("ssh -o ProxyCommand=\"nc %h %p\" win", &ctx),
        Err(SkipReason::RouteOverride)
    );
}

#[test]
fn 手順書とplanが新しい挙動を書いている() {
    let guide = read(GUIDE_MD);
    let fixture = read(FIXTURE_MD);
    // #1004 の道（本文は fixture へ 1 文字も変えずに貼る）を保つ
    assert!(
        fixture.contains(&guide),
        "{FIXTURE_MD} が {GUIDE_MD} の本文と byte 一致していない（#1004 の宣言）"
    );
    // 古い断言が残っていないこと
    for (rel, src) in [(GUIDE_MD, &guide), (FIXTURE_MD, &fixture)] {
        let stale = hits(rel, src, "will not appear by itself");
        assert!(
            stale.is_empty(),
            "古い挙動（非既定ポートは自動検知に出てこない）が残っている:\n{}",
            stale.join("\n")
        );
        // 折り返しを変えて復活させても落とす（行単位の検査だけだと抜ける）
        assert!(
            !flat(src).contains("will not appear by itself"),
            "{rel}: 古い挙動が折り返しを変えて残っている"
        );
    }
    // 折り返しの位置で落ちないよう、空白を畳んでから探す
    assert!(
        flat(&guide).contains("does the name alone reach that port"),
        "{GUIDE_MD}: 新しい物差し（名前だけでそのポートへ行けるか）が書かれていない"
    );
    assert!(
        guide.contains("#1411"),
        "{GUIDE_MD}: 出所（#1411）が書かれていない"
    );
    let plan = read(PLAN_MD);
    for needle in [
        "宛先の名前だけでそのポートへ行けるか",
        "ConfiguredPorts",
        "TAKO_1411_LEGACY=1",
        "引用つきオプション値の平坦化を戻す",
    ] {
        assert!(
            plan.contains(needle),
            "{PLAN_MD}: `{needle}` が無い（#1411 の設計が plan に残っていない）"
        );
    }
}
