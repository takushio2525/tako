//! 番犬: 昇格ゲート（#1452）を足したあと、承認を撃つ隔離テストを赤いまま置き去りにしない（#1493）
//!
//! ## 何が起きたのか
//!
//! #1452 が `POST /api/admin/pair/approve` に「呼び出し元が tako-app か」のゲートを足し、
//! 隔離テストのための逃し口 [`TRUSTED_ADMIN_NAMES_ENV`] を同時に用意した。
//! そのとき既存の実経路テスト 2 本には宣言を足したが、**同じ日に着地したばかりの
//! `scripts/test-remote-fs-1451.sh` だけが漏れ**、承認が 403 `upgrade_requires_gui` で
//! 通らなくなった。症状は「端末の role が `null` / 一覧が 403」で **OK=18 NG=55**。
//! 落ちているのがテストの前提なのか製品の回帰なのか、1 週間後には誰にも分からなくなっていた。
//!
//! ## ここで止める 2 つ
//!
//! 1. **宣言漏れ**（#1493 そのもの）。昇格経路（`ROLE_ROUTES` の `grants_upgrade`）を
//!    叩く `scripts/*.sh` は、逃し口の env を**実際に export している**こと。
//!    経路表が正本なので、昇格経路が増えたら検査対象も自動で増える
//! 2. **逃し口の越境**。この env は「隔離テストが GUI を名乗る」ためだけのもので、
//!    本番の経路（リリース・セットアップ・製品コード）に現れてはいけない
//!    （`.agent/threat-model-remote.md` が受容しているのは**テストに限る**前提）
//!
//! 「承認の応答を捨てない」（`-o /dev/null` にすると断られた事実が下流の数十件の NG に
//! 化ける）は #1493 の 2 つめの教訓だが、機械で縛ると既存 4 本の書き換えが要るので
//! ここでは縛っていない。直したのは霧が出た `test-remote-fs-1451.sh` の `pair_as` だけ
//!
//! 落ちるときは **file:line で名指し**する（直す場所が分からない番犬は直されない）。
//!
//! ## 相方
//!
//! ゲートそのものの不変条件（昇格経路は 2 本・どちらも `gui_only`・CLI / MCP から
//! 叩けない）は `issue1452_role_grant_watchdog.rs`。実際に 403 が返ることは
//! `scripts/test-remote-role-1452.sh`。ここは**隔離テスト側の作法**だけを見る。

use std::path::{Path, PathBuf};

use tako_control::remote_role::{ROLE_ROUTES, TRUSTED_ADMIN_NAMES_ENV};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリのルートを解決できる")
}

/// `scripts/` 配下の .sh を集める（再帰）
fn shell_scripts() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "sh") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo_root().join("scripts"), &mut out);
    out.sort();
    assert!(!out.is_empty(), "scripts/ 配下に .sh が 1 つも見つからない");
    out
}

fn rel(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{} を読めない: {e}", rel(path)))
}

/// `needle` を含む**コメントでない**行を `(行番号, 行)` で拾う。
/// コメントを外すのは、規約や理由を書いた行で落ちると説明を書けなくなるから
fn hits(text: &str, needle: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            !t.starts_with('#') && !t.starts_with("//") && !t.starts_with('*')
        })
        .filter(|(_, l)| l.contains(needle))
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .collect()
}

/// 昇格しうる経路の具体形（`/api/admin/pair/approve` / `/api/admin/devices/role`）
fn upgrade_paths() -> Vec<&'static str> {
    ROLE_ROUTES
        .iter()
        .filter(|r| r.grants_upgrade)
        .map(|r| r.sample_path)
        .collect()
}

/// その .sh が昇格経路を**叩いている**行を `(経路, 行番号, 行)` で返す。
///
/// `echo` / `printf` で経路名を出しているだけの行は外す（診断メッセージに経路を書くと
/// 「叩いている」と誤って数えられ、宣言を要求されてしまう）
fn upgrade_calls(text: &str) -> Vec<(&'static str, usize, String)> {
    let mut out = Vec::new();
    for path in upgrade_paths() {
        for (line, src) in hits(text, path) {
            if src.starts_with("echo ") || src.starts_with("printf ") {
                continue;
            }
            out.push((path, line, src));
        }
    }
    out.sort_by_key(|(_, line, _)| *line);
    out
}

/// 逃し口を**実際に置いている**行（`NAME=…` の形）。
///
/// 「名前が出てくるか」では見ない: 診断メッセージで env 名を案内している行を
/// 宣言と取り違えると、宣言を落としても番犬が緑のままになる（注入で実測した穴）
fn declares_escape_hatch(text: &str) -> Vec<(usize, String)> {
    hits(text, &format!("{TRUSTED_ADMIN_NAMES_ENV}="))
}

// --- 1. 宣言漏れ（#1493 そのもの） -------------------------------------------

#[test]
fn 昇格経路を叩く隔離テストは逃し口を宣言している() {
    let mut problems = Vec::new();
    for script in shell_scripts() {
        let text = read(&script);
        let calls = upgrade_calls(&text);
        if calls.is_empty() {
            continue;
        }
        // 「宣言している」= コメントでも案内文でもなく、実際に値を置いている行が在ること
        if declares_escape_hatch(&text).is_empty() {
            let where_ = calls
                .iter()
                .map(|(path, line, src)| {
                    format!("  {}:{line}: {path} を叩く → {src}", rel(&script))
                })
                .collect::<Vec<_>>()
                .join("\n");
            problems.push(format!(
                "{} が昇格経路を叩くのに {TRUSTED_ADMIN_NAMES_ENV} を宣言していない:\n{where_}",
                rel(&script)
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "隔離テストが curl / python で承認を撃つと、#1452 のゲートが\n\
         403 upgrade_requires_gui で断る（端末が登録されず、以降が全部 403 になる）。\n\
         この隔離環境では GUI 側を名乗ると宣言すること:\n\n    export \
         {TRUSTED_ADMIN_NAMES_ENV}=\"curl\"\n\n{}",
        problems.join("\n")
    );
}

// --- 2. 逃し口が隔離テストの外へ広がらない -----------------------------------

/// 逃し口の名前が現れてよい唯一の製品ソース（定数の宣言そのもの）
const ENV_DECL_REL: &str = "crates/tako-control/src/remote_role.rs";

#[test]
fn 逃し口は隔離テストの外に現れない() {
    let root = repo_root();
    let mut problems = Vec::new();

    // 製品ソース（crates/*/src/**.rs）: 定数の宣言ファイル以外に現れない
    fn walk_rs(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut sources = Vec::new();
    for crate_dir in ["tako-core", "tako-control", "tako-cli", "tako-app"] {
        walk_rs(
            &root.join("crates").join(crate_dir).join("src"),
            &mut sources,
        );
    }
    sources.sort();
    for source in sources {
        if rel(&source) == ENV_DECL_REL {
            continue;
        }
        for (line, src) in hits(&read(&source), TRUSTED_ADMIN_NAMES_ENV) {
            problems.push(format!("  {}:{line}: {src}", rel(&source)));
        }
    }

    // シェル: `test-` で始まる隔離テストの中だけ
    for script in shell_scripts() {
        let name = script
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with("test-") {
            continue;
        }
        for (line, src) in hits(&read(&script), TRUSTED_ADMIN_NAMES_ENV) {
            problems.push(format!("  {}:{line}: {src}", rel(&script)));
        }
    }

    assert!(
        problems.is_empty(),
        "{TRUSTED_ADMIN_NAMES_ENV} は**隔離テストが GUI を名乗るためだけ**の口で、\n\
         本番の経路（リリース・セットアップ・製品コード）に置くと昇格ゲートが\n\
         名前を知っている誰にでも開く。脅威モデルが受容しているのはテストに限る前提:\n\n{}",
        problems.join("\n")
    );
}

// --- 検出力（走査対象が消えていないか） --------------------------------------

#[test]
fn i1493_注入_検査の材料がすべて実在する() {
    // 昇格経路が空・具体形が空文字だと、上の 2 本は「空振りで緑」になる
    let paths = upgrade_paths();
    assert_eq!(
        paths.len(),
        2,
        "昇格できる経路の数が変わった（{paths:?}）。増やすならこの番犬の走査にも載ること"
    );
    for path in &paths {
        assert!(
            path.starts_with("/api/admin/"),
            "昇格経路 {path} が管理 API の形をしていない（走査の材料が崩れている）"
        );
    }

    // **判定が「名前が出てくるか」に戻っていないこと**（注入で見つけた穴）。
    // 案内文で env 名を書いている行を宣言と数えると、宣言を落としても緑のままになる
    assert!(
        declares_escape_hatch(&format!(
            "  echo \"  {TRUSTED_ADMIN_NAMES_ENV} で GUI 側を名乗る宣言が要る。\""
        ))
        .is_empty(),
        "案内文の行を「宣言」と数えている（宣言漏れを検出できない）"
    );
    assert!(
        !declares_escape_hatch(&format!("export {TRUSTED_ADMIN_NAMES_ENV}=\"curl\"")).is_empty(),
        "本物の export を「宣言」と数えられていない（全スクリプトが常時 FAILED になる）"
    );
    // 経路名を出すだけの行は「叩いている」と数えない
    assert!(
        upgrade_calls("  echo \"  POST /api/admin/pair/approve → HTTP 403\"").is_empty(),
        "診断メッセージの行を「叩いている」と数えている（宣言を不要に要求してしまう）"
    );

    // 逃し口の定数の宣言が在ること（名前だけ変わると全検査が空振りする）
    let decl = read(&repo_root().join(ENV_DECL_REL));
    assert!(
        decl.contains(TRUSTED_ADMIN_NAMES_ENV),
        "{ENV_DECL_REL} に {TRUSTED_ADMIN_NAMES_ENV} の宣言が無い"
    );

    // 実際に昇格経路を叩いている .sh が在ること（0 本なら検査 1・2 は空振り）
    let drivers: Vec<String> = shell_scripts()
        .into_iter()
        .filter(|s| !upgrade_calls(&read(s)).is_empty())
        .map(|s| rel(&s))
        .collect();
    assert!(
        drivers.len() >= 5,
        "昇格経路を叩く隔離テストが {} 本しかない（走査対象が消えた？）: {drivers:?}",
        drivers.len()
    );
    // #1493 の当事者。ここが対象から外れたら、同じ回帰がまた素通りする
    assert!(
        drivers
            .iter()
            .any(|d| d == "scripts/test-remote-fs-1451.sh"),
        "scripts/test-remote-fs-1451.sh が検査対象から外れた（#1493 の当事者）: {drivers:?}"
    );
}
