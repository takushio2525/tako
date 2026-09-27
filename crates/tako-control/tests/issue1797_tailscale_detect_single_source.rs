//! **#1797 の回帰テストと番犬**: `tako setup` の依存チェック段と remote の段が、tailscale を
//! **同じ 1 つの検出**（`tailscale::detect_tailscale`）で探す。
//!
//! ## 何が起きていたのか
//!
//! 依存段（`setup_deps::resolve`）は tailscale を **PATH だけ**（`exe::find`）で探していた。
//! CLI が PATH の外にある App Store 版 / GUI 版（`/Applications/Tailscale.app/Contents/MacOS/Tailscale`）
//! を「見つかりません」と判定し、`--yes` では確認を省いて `brew install tailscale` まで進んだ。
//! GUI 版と brew 版の 2 系統が同居する状態は #1038（GUI 版がノード実体だと tako remote が 502）
//! の条件そのもの。一方、同じ setup の末尾の 1 行（#1507）は `remote_setup::check_status` →
//! `tailscale::find_tailscale` を通すので正しく「ログインしていません」と出ており、
//! **1 回の実行の中で 2 つの検出が食い違っていた**。
//!
//! 併せて、tailscale コマンドの待ち（`run_tailscale_within`）が自前の実装で、子が終わった後に
//! 吸い出しスレッドを **join していた**（子が残した孫がパイプを握ると上限を持たずに固まる）。
//! #1503 の規約どおり `tako_core::probe` の 1 実装へ寄せた。
//!
//! ## ここで止めるもの
//!
//! 1. [`依存段とremoteの答えが揃う`] — 振る舞い（PATH 外 / 動かない / 無い / 導入の判断）
//! 2. [`孫がパイプを握ってもtailscaleの待ちは上限で返る`] — join で固まる形へ戻らない
//! 3. [`依存段のtailscaleは検出の正本を通る`] — `fn resolve` が PATH で決める形へ戻らない
//! 4. [`remoteの検出も同じ正本を通る`] — 正本が 2 つに割れない（#1496 の 2 本立て）
//! 5. [`tailscaleの待ちは上限つきの1実装を通る`] — 自前の待ち・自前の spawn が戻らない
//! 6. [`remote_setupは在るが動かないcliを未導入と言わない`] — `tako remote setup` の [1/5] が
//!    「在るが動かない」を「未導入」と言って導入口へ渡す形へ戻らない（導入口は在るものを黙って
//!    飛ばすので「確認を省略してインストールします」と言ったまま何もせずに終わる）
//!
//! 3〜6 は落ちるとき **file:line で名指し**する。実経路（隔離 HOME + tailscale / brew スタブで
//! `tako setup --yes` を走らせ、brew の呼び出しを数える）は `scripts/test-setup-tailscale-detect-1797.sh`。

use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Mutex;

#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view;

const SETUP_DEPS: &str = "crates/tako-control/src/setup_deps.rs";
const TAILSCALE: &str = "crates/tako-control/src/tailscale.rs";
const REMOTE_SETUP: &str = "crates/tako-control/src/remote_setup.rs";

/// `TAKO_TAILSCALE_BIN` を書き換える / スタブを起こすテストを直列にする。
/// env の読み書きを重ねないためと、孫を残すテストの spawn を他と重ねないため（#1748）
#[cfg(unix)]
static SERIAL: Mutex<()> = Mutex::new(());

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番領域（テスト領域を潰した全文。長さと行番号は保たれる）
fn production(rel: &str) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::production(&src, rel)
}

/// コメントだけを潰した眺め（文字列リテラルは残る）
fn non_comment(rel: &str) -> String {
    code_view::without_comments_checked(&production(rel), rel)
}

fn line_of(src: &str, at: usize) -> usize {
    src[..at].matches('\n').count() + 1
}

/// 関数 `name` の本体（`{` から対応する `}` まで）のバイト範囲。
///
/// 波括弧は**コメントと文字列を潰した眺め**で数える（`format!("{x}")` の括弧を数えない）。
/// 眺めはバイト長を保つので、同じ範囲をコメントだけ潰した眺めへそのまま当てられる
fn fn_range(rel: &str, name: &str) -> std::ops::Range<usize> {
    let code = code_view::code_view(&production(rel));
    let needle = format!("fn {name}(");
    let head = code
        .match_indices(needle.as_str())
        .map(|(at, _)| at)
        .find(|at| *at == 0 || !code.as_bytes()[at - 1].is_ascii_alphanumeric())
        .unwrap_or_else(|| {
            panic!("{rel}: `fn {name}` が見つからない（改名したならこの番犬も直す）")
        });
    let open = head + code[head..].find('{').expect("fn の本体の `{`");
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open..open + offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!("{rel}: `fn {name}` の本体が閉じていない");
}

/// 関数本体（コメントだけ潰した眺め）と、その先頭行番号
fn fn_body(rel: &str, name: &str) -> (String, usize) {
    let view = non_comment(rel);
    let range = fn_range(rel, name);
    let start_line = line_of(&view, range.start);
    (view[range].to_string(), start_line)
}

/// `needle` が出る行を `rel:line` で並べる（`base_line` は本体の先頭行）
fn hits(body: &str, base_line: usize, rel: &str, needle: &str) -> Vec<String> {
    body.match_indices(needle)
        .map(|(at, _)| {
            let line = base_line + body[..at].matches('\n').count();
            format!("{rel}:{line}: `{needle}`")
        })
        .collect()
}

/// 実行できるスタブを置いてパスを返す
#[cfg(unix)]
fn stub(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).expect("スタブの置き場");
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("スタブ");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path.to_str().expect("UTF-8").to_string()
}

/// **1**: 依存段（`setup_deps::status_of`）と remote（`tailscale::find_tailscale`）の答えが揃う。
///
/// `TAKO_TAILSCALE_BIN` を**必ず一時 dir の中へ向ける**ので、実機の候補（/Applications・
/// /opt/homebrew/bin・/usr/local/bin）は 1 度も試されない
#[cfg(unix)]
#[test]
fn 依存段とremoteの答えが揃う() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tako_core::test_residue::ScratchDir::new("tako-1797-detect");
    // PATH の外（App Store 版の .app 同梱 CLI を模す）。PATH は触らない = PATH からは見えない
    let apps = stub(
        &dir.path().join("Applications/Tailscale.app/Contents/MacOS"),
        "Tailscale",
        "echo 1.80.0",
    );
    let broken = stub(&dir.path().join("broken"), "tailscale", "exit 1");
    let missing = dir.path().join("missing/tailscale");
    let missing = missing.to_str().expect("UTF-8");

    // (TAKO_TAILSCALE_BIN, 依存段の答え, remote の答え, 状況)
    let cases: [(&str, Option<&str>, Option<&str>, &str); 3] = [
        (
            &apps,
            Some(&apps),
            Some(&apps),
            "PATH の外の動く CLI（App Store 版）",
        ),
        // 在るが動かない CLI は「導入済み」（入れ直させない）。remote は使わない
        (
            &broken,
            Some(&broken),
            None,
            "--version が非 0 で終わる CLI",
        ),
        (missing, None, None, "どこにも無い"),
    ];
    for (bin, want_dep, want_remote, what) in cases {
        std::env::set_var("TAKO_TAILSCALE_BIN", bin);
        let dep = tako_control::setup_deps::status_of("tailscale").expect("依存表に tailscale");
        let remote = tako_control::tailscale::find_tailscale();
        let dry_run = tako_control::setup_deps::install(
            Some("tailscale"),
            tako_control::setup_deps::DepInstallOptions {
                dry_run: true,
                interactive: false,
            },
        );
        std::env::remove_var("TAKO_TAILSCALE_BIN");

        assert_eq!(
            dep.found.as_deref(),
            want_dep,
            "{what}: 依存段の答えが remote の検出と揃っていない。\n\
             → {SETUP_DEPS} の `fn resolve` が tailscale を PATH（`exe::find`）で決めていないか。\
             tailscale は `resolve_tailscale`（= `tailscale::detect_tailscale`）だけで決める（#1797）"
        );
        assert_eq!(remote.as_deref(), want_remote, "{what}: remote の答え");
        if want_dep.is_some() {
            // 在るものは導入の計画に載らない = `--yes` でも brew を起こさない
            let value = dry_run.expect("install（dry-run）");
            assert_eq!(
                value["planned"].as_array().map(Vec::len),
                Some(0),
                "{what}: 導入済みなのに導入の計画に載った: {value}"
            );
            assert_eq!(
                value["skipped"][0]["reason"].as_str(),
                Some("already_installed"),
                "{what}: 導入済みとして飛ばしていない: {value}"
            );
        }
    }
}

/// **2**: tailscale が孫を残して終わっても（孫がパイプの書き手のまま）、待ちは上限内に返る。
///
/// #1797 前の `run_tailscale_within` は子が終わった後に吸い出しスレッドを join したので、
/// ここで孫が眠っている 30 秒ぶん固まった。寄せ先（`tako_core::probe`）は読み切りの猶予
/// （2 秒）で頭打ちにする。固まる回帰を**固まらずに落とす**ため、別スレッドで待って締め切る
#[cfg(unix)]
#[test]
fn 孫がパイプを握ってもtailscaleの待ちは上限で返る() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tako_core::test_residue::ScratchDir::new("tako-1797-orphan");
    let pidfile = dir.path().join("grandchild.pid");
    let cli = stub(
        dir.path(),
        "tailscale",
        &format!(
            "/bin/sleep 30 &\necho $! > '{}'\necho '{{\"BackendState\":\"NeedsLogin\"}}'\nexit 0",
            pidfile.display()
        ),
    );

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(tako_control::tailscale::setup_status_on(Some(cli), None));
    });
    let got = rx.recv_timeout(std::time::Duration::from_secs(8));
    // 孫を止める（テストプロセスより長生きさせない = #1748）。止めてから判定する
    if let Ok(pid) = std::fs::read_to_string(&pidfile) {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-9", pid.trim()])
            .status();
    }

    let status = got.unwrap_or_else(|_| {
        panic!(
            "tailscale の待ちが 8 秒で返らなかった（孫がパイプを握ったまま = join で固まっている）。\n\
             → {TAILSCALE} の `fn run_tailscale_within` は `tako_core::probe::output_with_timeout` を\
             通すこと（#1503 の「読み切りにも同じ予算」/ #1797）"
        )
    });
    assert!(status.daemon_running, "終わった子の出力を読めていない");
    assert_eq!(status.backend_state.as_deref(), Some("NeedsLogin"));
    assert!(
        status.timed_out.is_none(),
        "子は終わっているので打ち切りではない"
    );
}

/// **3**: `fn resolve` は tailscale を PATH より先に正本へ回し、正本の答えだけで決める
#[test]
fn 依存段のtailscaleは検出の正本を通る() {
    let (resolve, base) = fn_body(SETUP_DEPS, "resolve");
    let branch = resolve.find("resolve_tailscale()").unwrap_or_else(|| {
        panic!(
            "{SETUP_DEPS}:{base}: `fn resolve` が tailscale を `resolve_tailscale()` へ回していない。\n\
             PATH（`exe::find`）で決めると、PATH の外にある App Store 版を「見つかりません」と判定し \
             `--yes` で brew 版まで入れる（2 系統の同居 = #1038。#1797）"
        )
    });
    if let Some(path_lookup) = resolve.find("exe::find(") {
        assert!(
            branch < path_lookup,
            "{SETUP_DEPS}:{}: `fn resolve` が tailscale の分岐より前に PATH を引いている（#1797）",
            base + resolve[..path_lookup].matches('\n').count()
        );
    }
    assert!(
        resolve.contains("is_tailscale(dep)"),
        "{SETUP_DEPS}:{base}: `fn resolve` の tailscale の分岐が `is_tailscale(dep)` で判定していない"
    );

    let (body, base) = fn_body(SETUP_DEPS, "resolve_tailscale");
    assert!(
        body.contains("tailscale::detect_tailscale()"),
        "{SETUP_DEPS}:{base}: `fn resolve_tailscale` が検出の正本（`tailscale::detect_tailscale`）を\
         通っていない（#1797）"
    );
    let offenders = hits(&body, base, SETUP_DEPS, "exe::find");
    assert!(
        offenders.is_empty(),
        "tailscale の「在るか」を PATH で決めている:\n  {}\n\
         → 判定は正本の答えだけで決める。表示用の絶対パス化は `display_path` に閉じる（#1797）",
        offenders.join("\n  ")
    );

    let (is_ts, base) = fn_body(SETUP_DEPS, "is_tailscale");
    assert!(
        is_ts.contains("\"tailscale\""),
        "{SETUP_DEPS}:{base}: `fn is_tailscale` が依存表の名前 `tailscale` を見ていない"
    );
}

/// **4**: remote の答え（`find_tailscale`）も同じ正本から出す（正本を 2 つに割らない）
#[test]
fn remoteの検出も同じ正本を通る() {
    let (body, base) = fn_body(TAILSCALE, "find_tailscale");
    assert!(
        body.contains("detect_tailscale()"),
        "{TAILSCALE}:{base}: `fn find_tailscale` が `detect_tailscale()` を通っていない。\n\
         依存段（`setup_deps::resolve_tailscale`）と remote が別々に探すと、同じ setup の中で\
         答えが割れる（#1797）"
    );
    let offenders = hits(&body, base, TAILSCALE, "TAILSCALE_CANDIDATES");
    assert!(
        offenders.is_empty(),
        "`find_tailscale` が候補を自分で探している:\n  {}\n→ 探索は `detect_tailscale` の 1 か所（#1797）",
        offenders.join("\n  ")
    );
}

/// **5**: tailscale コマンドの待ちは上限つきの 1 実装（`tako_core::probe`）を通る
#[test]
fn tailscaleの待ちは上限つきの1実装を通る() {
    let (body, base) = fn_body(TAILSCALE, "run_tailscale_within");
    assert!(
        body.contains("tako_core::probe::output_with_timeout("),
        "{TAILSCALE}:{base}: `fn run_tailscale_within` が待ちの 1 実装\
         （`tako_core::probe::output_with_timeout`）を通っていない（#1503 / #1797）"
    );
    let mut offenders = Vec::new();
    for needle in [
        ".join()",
        "try_wait",
        "Command::new",
        ".spawn()",
        "thread::sleep",
    ] {
        offenders.extend(hits(&body, base, TAILSCALE, needle));
    }
    assert!(
        offenders.is_empty(),
        "tailscale コマンドの待ちを自前で書いている:\n  {}\n\
         → 吸い出しを join すると、子が残した孫がパイプを握ったとき上限を持たずに固まる。\
         `tako_core::probe::output_with_timeout` へ寄せてください（#1503 / #1797）",
        offenders.join("\n  ")
    );

    // ファイル全体でも子プロセスを自分で起こさない（検出の `--version` も待ちも probe を通る）
    let view = non_comment(TAILSCALE);
    let offenders = hits(&view, 1, TAILSCALE, "Command::new(");
    assert!(
        offenders.is_empty(),
        "tailscale.rs が子プロセスを自分で起こしている:\n  {}\n\
         → `tako_core::probe::output_with_timeout` を通してください（上限・コンソール窓の抑止・\
         stdin の遮断をそちらが持つ。#1503 / #586 / #1797）",
        offenders.join("\n  ")
    );
}

/// **6**: `tako remote setup` の [1/5] は「在るが動かない」を「未導入」より先に言い分ける
#[test]
fn remote_setupは在るが動かないcliを未導入と言わない() {
    let (body, base) = fn_body(REMOTE_SETUP, "run_interactive");
    let unrunnable = body
        .find("tailscale::Detection::Unrunnable(")
        .unwrap_or_else(|| {
            panic!(
                "{REMOTE_SETUP}:{base}: `fn run_interactive` の [1/5] が「在るが動かない」\
             （`tailscale::Detection::Unrunnable`）を見ていない。\n\
             見ないと `未導入` → 依存の導入口 → 導入済みとして黙って飛ばす、の順で\
             「確認を省略してインストールします」と言ったまま何もせずに終わる（#1797）"
            )
        });
    let missing = body.find("\"未導入\"").unwrap_or_else(|| {
        panic!("{REMOTE_SETUP}:{base}: `fn run_interactive` に「未導入」の行が無い（改めたならこの番犬も直す）")
    });
    assert!(
        unrunnable < missing,
        "{REMOTE_SETUP}:{}: `fn run_interactive` が「未導入」と言った後で「在るが動かない」を見ている（#1797）",
        base + body[..unrunnable].matches('\n').count()
    );
}
