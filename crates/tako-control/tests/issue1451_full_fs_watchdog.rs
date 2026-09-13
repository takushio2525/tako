//! **#1451 の番犬**: Finder 風の全体閲覧とショートカットの「壊れ方」を止める。
//!
//! ## 何を止めたいのか
//!
//! #1451 は**読める範囲を広げる**変更なので、壊れ方は素直に「読めてはいけない
//! ものが読める」へ倒れる。#1079 が置いた基準（リモートから読めるのは tako が
//! 意図的に見せているものだけ）を捨てたのではなく、
//! **「意図的に見せている」の主語をツリーから role へ移した**のが設計の中身で、
//! その主語が消えた瞬間に「全 role が全部読める」になる。
//!
//! だから止めるのは次の 6 つ:
//!
//! 1. [`認可の門が1か所に在る`] — `fs` ルートを一覧へ足すのが [`local_roots_for`]
//!    以外の場所から起きる / role を見ない形へ戻る
//! 2. [`pwaが呼ぶapiは全部経路表に在る`] — 表に無い `/api/files…` を PWA が呼ぶ
//!    （= `required_role` の判断からこぼれる。#1405 の懸念）
//! 3. [`ショートカットの永続は冪等で正本が1つ`] — 同じパスで件数が増える /
//!    正本（`tako_core::remote_shortcuts`）を通さない写しが生える
//! 4. [`読めないディレクトリは理由を返す`] — 空一覧 200 で無言になる /
//!    エントリの `unreadable` が落ちる（#1399 系の「黙って失敗する UI」）
//! 5. [`脅威モデルがコードの既定を書いている`] — 文書とコードの role がズレる
//!    （#1406 と同型。文章は機械検査が無いと黙って嘘になる）
//! 6. [`cliとmcpは1対1`] / [`abの逃げ道は1つ`] — 設計原則 5 と A/B 軸の維持
//!
//! ## 相方
//!
//! 「PWA でどう見えるか」は実 DOM の e2e（`web/tako-remote/e2e/files-1451.spec.js`）、
//! 「role ごとに 200 / 403 か」は実経路テスト（`scripts/test-remote-fs-1451.sh`）が見る。
//! ここは**宣言と実装の一致**だけを、`file:line` の名指しで押さえる。

use std::path::{Path, PathBuf};

use tako_control::remote_auth::DeviceRole;
use tako_control::remote_files::{self, RootKind, FILE_ROUTES};

// 本番コードだけの眺めは 1 実装を通す（#1420）
#[path = "common/production_range.rs"]
mod production_range;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("リポジトリルート")
        .to_path_buf()
}

const FILES_REL: &str = "crates/tako-control/src/remote_files.rs";
const REMOTE_REL: &str = "crates/tako-control/src/remote.rs";
const CORE_REL: &str = "crates/tako-core/src/remote_shortcuts.rs";
const DISPATCH_REL: &str = "crates/tako-control/src/dispatch.rs";
const PROTOCOL_REL: &str = "crates/tako-control/src/protocol.rs";
const CLI_REL: &str = "crates/tako-cli/src/main.rs";
const MCP_REQUEST_REL: &str = "crates/tako-control/src/mcp/request.rs";
const MCP_CATALOG_REL: &str = "crates/tako-control/src/mcp/catalog.rs";
const PWA_REL: &str = "web/tako-remote/src/pages/files.jsx";
const THREAT_REL: &str = ".agent/threat-model-remote.md";

fn read(rel: &str) -> String {
    std::fs::read_to_string(repo_root().join(rel)).unwrap_or_else(|e| panic!("{rel} を読む: {e}"))
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    // `*` を綴じ込みの印にしない: Rust では `*LEGACY.get_or_init(..)` のように
    // **本番コードの行頭が `*` になる**（実装中にこれで A/B の検査が空振りした）
    t.starts_with("//") || t.starts_with("///")
}

/// テスト領域だけを空白へ潰した眺め（バイト長と行番号は保たれる）
fn production(rel: &str) -> String {
    production_range::production(&read(rel), rel)
}

/// 関数 1 本の本体（署名の行から、同じ字下げの `}` まで）。
/// **見つからないことも FAILED**（走査範囲が空だとどんな回帰でも通る）
fn fn_body(rel: &str, src: &str, signature: &str) -> (Vec<(usize, String)>, usize) {
    fn_body_min(rel, src, signature, 3)
}

/// [`fn_body`] の下限を明示する版。1 行本体の純粋関数（`allows_full_browse`）は
/// 署名 + 1 行しかないので、そこだけ 2 を渡す（**0 行を許さない**のが下限の役目）
fn fn_body_min(rel: &str, src: &str, signature: &str, min: usize) -> (Vec<(usize, String)>, usize) {
    let lines: Vec<&str> = src.lines().collect();
    let start = lines
        .iter()
        .position(|l| l.contains(signature))
        .unwrap_or_else(|| panic!("{rel}: 目印 {signature:?} が消えている（走査範囲を作れない）"));
    let indent = " ".repeat(lines[start].len() - lines[start].trim_start().len());
    let close = format!("{indent}}}");
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, l)| **l == close)
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("{rel}: {signature:?} の本体を閉じる `}}` が見つからない"));
    let body: Vec<(usize, String)> = (start..end)
        .filter(|i| !is_comment(lines[*i]))
        .map(|i| (i + 1, lines[i].to_string()))
        .collect();
    assert!(
        body.len() >= min,
        "{rel}:{}: {signature:?} の走査範囲が {} 行しかない（範囲取りが壊れている）",
        start + 1,
        body.len()
    );
    (body, start + 1)
}

/// ファイル中で `needle` が最初に出る行（1 起点）。無ければ `0`
fn line_of(src: &str, needle: &str) -> usize {
    src.lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or(0)
}

fn joined(body: &[(usize, String)]) -> String {
    body.iter()
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

const ROOTS_FN: &str = "fn current_roots(deps: &FilesDeps)";
const GATE_FN: &str = "pub fn local_roots_for(";
const ALLOWS_FN: &str = "pub fn allows_full_browse(";
const LIST_FN: &str = "pub fn list_directory(";
const ENTRY_FN: &str = "fn entry_json(";

// --------------------------------------------- 1. 認可の門

/// 注入 ①: `fs` ルートを role を見ずに足す / 門を通らず組む
#[test]
fn 認可の門が1か所に在る() {
    let src = production(FILES_REL);

    // (a) 門そのものが role と A/B の両方を見る
    let (gate, gate_at) = fn_body(FILES_REL, &src, GATE_FN);
    let gate_text = joined(&gate);
    assert!(
        gate_text.contains("allows_full_browse("),
        "{FILES_REL}:{gate_at}: {GATE_FN} が `allows_full_browse` を通っていない \
         = 全体閲覧の門が role を見ずに開く（#1451）"
    );
    let (allows, allows_at) = fn_body_min(FILES_REL, &src, ALLOWS_FN, 2);
    let allows_text = joined(&allows);
    assert!(
        allows_text.contains("role >= FULL_BROWSE_ROLE"),
        "{FILES_REL}:{allows_at}: {ALLOWS_FN} が `role >= FULL_BROWSE_ROLE` を見ていない \
         = 昇格していない端末に全体閲覧が開く（#1451 の一番まずい壊れ方）"
    );
    assert!(
        allows_text.contains("!legacy"),
        "{FILES_REL}:{allows_at}: {ALLOWS_FN} が A/B（legacy）を見ていない \
         = TAKO_1451_LEGACY で #1079 の挙動へ戻せなくなる"
    );

    // (b) 認可の門を通る経路が 1 本（`current_roots`）
    let (roots, roots_at) = fn_body(FILES_REL, &src, ROOTS_FN);
    let roots_text = joined(&roots);
    assert!(
        roots_text.contains("local_roots_for("),
        "{FILES_REL}:{roots_at}: {ROOTS_FN} が {GATE_FN} を通っていない \
         = 一覧・プレビュー・ダウンロード・書き込みが別々の門を持つ（#1451）"
    );
    assert!(
        roots_text.contains("deps.role"),
        "{FILES_REL}:{roots_at}: {ROOTS_FN} がリクエストの role を渡していない"
    );

    // (c) `fs_roots()` を呼ぶのは門だけ（門を迂回する写しが生えていない）
    let callers: Vec<String> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("fs_roots()") && !is_comment(l))
        .filter(|(_, l)| !l.contains("pub fn fs_roots"))
        .map(|(i, l)| format!("{FILES_REL}:{}: {}", i + 1, l.trim()))
        .collect();
    assert_eq!(
        callers.len(),
        1,
        "`fs_roots()` を呼ぶ場所は認可の門（{GATE_FN}）の 1 か所だけにする \
         = 門を通らずに全体閲覧のルートを組む経路を作らない（#1451）:\n{}",
        callers.join("\n")
    );
    assert!(
        callers[0].contains("out.extend(fs_roots())"),
        "`fs_roots()` の唯一の呼び出しが門の中に無い（#1451）:\n{}",
        callers[0]
    );

    // (d) 門の強さ（コードの正）
    assert_eq!(
        remote_files::FULL_BROWSE_ROLE,
        DeviceRole::Manage,
        "{FILES_REL}:{}: 全体閲覧の下限 role が Manage から動いた。\
         動かすなら脅威モデル（{THREAT_REL}）と docs も同じコミットで直すこと（#1451）",
        line_of(&src, "pub const FULL_BROWSE_ROLE")
    );
    for role in [DeviceRole::Observe, DeviceRole::Interact] {
        assert!(
            !remote_files::allows_full_browse(role, false),
            "{role:?} に全体閲覧が開いている（#1451 の認可は manage 以上）"
        );
    }
    for role in [DeviceRole::Manage, DeviceRole::Admin] {
        assert!(
            remote_files::allows_full_browse(role, false),
            "{role:?} で全体閲覧が開かない（#1451 の受け入れ条件）"
        );
        assert!(
            !remote_files::allows_full_browse(role, true),
            "{role:?} の legacy 腕で全体閲覧が開いている（A/B が効いていない）"
        );
    }
}

/// 門が実際に「一覧へ載るか」を決めていること（純粋関数の挙動）
#[test]
fn 門を閉じるとfsルートは一覧に載らない() {
    let tree = Vec::new();
    let open = remote_files::local_roots_for(DeviceRole::Manage, false, tree.clone());
    assert!(
        open.iter().any(|r| r.kind == RootKind::Fs),
        "manage で全体閲覧の入口が出ない"
    );
    for (role, legacy) in [
        (DeviceRole::Interact, false),
        (DeviceRole::Observe, false),
        (DeviceRole::Manage, true),
    ] {
        let closed = remote_files::local_roots_for(role, legacy, tree.clone());
        assert!(
            !closed.iter().any(|r| r.kind == RootKind::Fs),
            "{role:?}（legacy={legacy}）で全体閲覧の入口が一覧に出ている"
        );
    }
    // id は他の 2 系統（12 桁 16 進 / `s-`）と構造的に衝突しない
    assert!(remote_files::is_fs_root_id(remote_files::FS_ROOT_ID));
    assert!(remote_files::is_fs_root_id("fs-c"));
    for other in ["0123456789ab", "s-abc", "", "fsx"] {
        assert!(
            !remote_files::is_fs_root_id(other),
            "{other} が全体閲覧の id と誤認されている"
        );
    }
}

// --------------------------------------------- 2. PWA の呼び口 ↔ 経路表

/// 注入 ②: 表に無い `/api/files…` を PWA が呼ぶ
#[test]
fn pwaが呼ぶapiは全部経路表に在る() {
    let pwa = read(PWA_REL);
    let declared: Vec<&str> = FILE_ROUTES.iter().map(|r| r.path).collect();
    let mut stray: Vec<String> = Vec::new();
    for (i, line) in pwa.lines().enumerate() {
        if is_comment(line) {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("/api/files") {
            let tail = &rest[at..];
            // パスの終わり（引用符・クエリ・テンプレートの境目）まで
            let path: String = tail
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-'))
                .collect();
            if !declared.contains(&path.as_str()) {
                stray.push(format!("{PWA_REL}:{}: {}", i + 1, path));
            }
            rest = &rest[at + "/api/files".len()..];
        }
    }
    assert!(
        stray.is_empty(),
        "PWA が経路表（`remote_files::FILE_ROUTES`）に無い API を呼んでいる。\n\
         表に載せるまで role の判断からこぼれる（未知の GET は Observe へ落ちる = #1405）:\n{}",
        stray.join("\n")
    );
    // 表そのものの健全性（宣言が自分の判定に当たる / role が緩んでいない）
    for r in FILE_ROUTES {
        assert_eq!(
            remote_files::required_role_for_method(r.method, r.path),
            Some(r.role),
            "{} {} の role が表から引けない",
            r.method,
            r.path
        );
        assert!(remote_files::owns_path(r.path), "{} が担当外", r.path);
    }

    // **宣言した監査種別が実際に記録されている**（表が飾りにならない）。
    // 経路を足して監査を忘れると、何をどれだけ持ち出したかが残らない（#287 P2-2）
    let src = production(FILES_REL);
    for r in FILE_ROUTES {
        let needle = format!("audit_payload(\"{}\"", r.audit_kind);
        assert!(
            src.contains(&needle),
            "{FILES_REL}: {} {} が宣言している監査種別 {:?} を誰も記録していない \
             （`{}` の呼び出しが無い = 持ち出しの記録が残らない。#1451 / #287 P2-2）",
            r.method,
            r.path,
            r.audit_kind,
            needle
        );
    }
}

/// 注入 ⑥: ショートカットの role が Manage から緩む
#[test]
fn ショートカットの経路はmanage以上() {
    for m in ["GET", "POST", "DELETE"] {
        assert_eq!(
            remote_files::required_role_for_method(m, "/api/files/shortcuts"),
            Some(DeviceRole::Manage),
            "{m} /api/files/shortcuts が Manage 未満へ緩んだ。\
             絶対パスは画面に映らない在庫情報で、全体閲覧できない端末へ配らない（#1451 / #1080）"
        );
    }
    // 読み出し（#1079）は Interact のまま = 既存端末の見え方を変えない
    for p in ["/api/files", "/api/files/content", "/api/files/download"] {
        assert_eq!(
            remote_files::required_role_for_method("GET", p),
            Some(DeviceRole::Interact),
            "{p} の読み出し role が動いた（#1079 の互換が壊れる）"
        );
    }
    // 表に無い `/api/files…` は安全側（#1405）
    assert_eq!(
        remote_files::required_role_for_method("GET", "/api/files/unknown"),
        Some(DeviceRole::Manage),
        "表に無い経路が弱い role へこぼれている"
    );

    // `remote.rs` のルータが表を通ること（メソッド列挙へ戻ると DELETE が落ちる）
    let remote = production(REMOTE_REL);
    assert!(
        remote.contains("crate::remote_files::owns_path(p)"),
        "{REMOTE_REL}:{}: ルータが `owns_path` を通っていない \
         = 表に足した経路が配線されない（表が飾りになる。#1451）",
        line_of(&remote, "remote_files::FilesDeps {")
    );
    assert!(
        remote.contains("role: device.role"),
        "{REMOTE_REL}:{}: `FilesDeps` に確定した role を渡していない \
         = 全体閲覧の門が開きっぱなしになる（#1451）",
        line_of(&remote, "remote_files::FilesDeps {")
    );
    assert!(
        remote.contains("required_role_for_method(\"POST\", path)"),
        "{REMOTE_REL}: `required_role` が経路表（メソッド込み）を引いていない"
    );
}

// --------------------------------------------- 3. ショートカットの正本

/// 注入 ③: 永続が非冪等になる / 正本を通さない写しが生える
#[test]
fn ショートカットの永続は冪等で正本が1つ() {
    use tako_core::remote_shortcuts::ShortcutsFile;

    let dir = std::env::temp_dir().join(format!("tako-1451-wd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let target = dir.join("projects");
    std::fs::create_dir_all(&target).expect("作れる");

    let mut file = ShortcutsFile::default();
    file.add(&target, None, 1).expect("足せる");
    file.add(&target, Some("別名"), 2).expect("足せる");
    assert_eq!(
        file.user_shortcuts().len(),
        1,
        "{CORE_REL}:{}: 同じパスの追加で件数が増えている（冪等でない = #1451）",
        line_of(&read(CORE_REL), "pub fn add(")
    );
    // 保存 → 読み直しで同じ（版数フィールドが落ちると移行機構が版を読めない）
    let path = dir.join("shortcuts.json");
    file.save_to(&path).expect("書ける");
    let back = ShortcutsFile::load_from(&path);
    assert_eq!(back.user_shortcuts().len(), 1, "読み直しで件数が変わった");
    let text = std::fs::read_to_string(&path).expect("読める");
    assert!(
        text.contains("\"version\""),
        "{CORE_REL}: 永続ファイルに版数が無い（#916 の移行機構が版を読めない）"
    );
    assert!(
        !text.contains("\"builtin\""),
        "{CORE_REL}: 既定の印がファイルへ書かれている \
         = 「消したのに復活する / 消したまま戻らない」が起きる（#1451）"
    );
    let _ = std::fs::remove_dir_all(&dir);

    // 正本は 1 つ: HTTP / dispatch とも `remote_shortcuts::ShortcutsFile` を通る
    let files = production(FILES_REL);
    assert!(
        files.contains("use tako_core::remote_shortcuts::"),
        "{FILES_REL}: HTTP 側が正本（tako_core::remote_shortcuts）を通っていない"
    );
    let dispatch = production(DISPATCH_REL);
    assert!(
        dispatch.contains("tako_core::remote_shortcuts::ShortcutsFile"),
        "{DISPATCH_REL}: dispatch が正本を通っていない（PWA と CLI の答えが割れる）"
    );
    // 移行の番地に載っている（永続ファイルを足したら #916 の登録が要る）
    assert!(
        tako_control::migrations::spec(tako_core::migration::SchemaId::RemoteShortcuts).is_some(),
        "shortcuts.json が移行の番地に無い（#916）"
    );
}

/// 設計原則 5: PWA でできることは CLI / MCP でもできる
#[test]
fn cliとmcpは1対1() {
    let protocol = production(PROTOCOL_REL);
    assert!(
        protocol.contains("RemoteShortcuts {"),
        "{PROTOCOL_REL}: `Request::RemoteShortcuts` が無い（dispatch を通らない操作は作らない）"
    );
    let cli = production(CLI_REL);
    assert!(
        cli.contains("RemoteCommand::Shortcuts"),
        "{CLI_REL}: `tako remote shortcuts` が無い（設計原則 5）"
    );
    assert!(
        cli.contains("dispatch_remote_shortcuts("),
        "{CLI_REL}:{}: CLI が dispatch と同じ 1 実装を通っていない",
        line_of(&cli, "fn remote_shortcuts(")
    );
    // 最簡形（#322）: サブコマンド省略で一覧できる
    let (body, at) = fn_body(CLI_REL, &cli, "fn remote_shortcuts(");
    assert!(
        joined(&body).contains("None | Some(RemoteShortcutsCommand::List)"),
        "{CLI_REL}:{at}: `tako remote shortcuts` の引数なしが一覧になっていない（#322 の最簡形）"
    );
    let mcp_req = production(MCP_REQUEST_REL);
    assert!(
        mcp_req.contains("\"tako_remote_shortcuts\""),
        "{MCP_REQUEST_REL}: MCP から呼べない（設計原則 5）"
    );
    let mcp_cat = read(MCP_CATALOG_REL);
    assert!(
        mcp_cat.contains("\"name\": \"tako_remote_shortcuts\""),
        "{MCP_CATALOG_REL}: MCP カタログに載っていない = AI からは存在しない"
    );
}

// --------------------------------------------- 4. 無言で失敗しない

/// 注入 ④: 読めないディレクトリで空一覧 200 / エントリの `unreadable` が落ちる
#[test]
fn 読めないディレクトリは理由を返す() {
    let src = production(FILES_REL);

    // (a) `read_dir` の失敗を握り潰して空一覧にしていない。
    //
    // **`Denial::Unreadable` が本体のどこかに在る**だけでは足りない（実装中に実測:
    // `read_dir` を `let Ok(..) else { return Ok(空) }` へ変えても、直前の
    // `metadata()` の `map_err` が残るので `contains` は真のまま = 見逃した）。
    // ①`read_dir` を受ける**その行**が失敗を変換していること
    // ②本体に**早期の成功 return が無い**こと、の 2 つで押さえる
    let (list, list_at) = fn_body(FILES_REL, &src, LIST_FN);
    let list_text = joined(&list);
    let read_at = list
        .iter()
        .find(|(_, l)| l.contains("read_dir(&resolved.path)"))
        .unwrap_or_else(|| panic!("{FILES_REL}:{list_at}: {LIST_FN} に `read_dir` が無い"));
    assert!(
        read_at.1.contains("map_err") || read_at.1.contains(".map_err"),
        "{FILES_REL}:{}: `read_dir` の失敗を変換していない \
         = 読めないフォルダが**空のフォルダ**に見える（#1451 / #1399 系）:\n  {}",
        read_at.0,
        read_at.1.trim()
    );
    let window = list
        .iter()
        .skip_while(|(n, _)| *n < read_at.0)
        .take(4)
        .map(|(_, l)| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        window.contains("Denial::Unreadable"),
        "{FILES_REL}:{}: `read_dir` の失敗が `Denial::Unreadable` になっていない",
        read_at.0
    );
    let silent: Vec<String> = list
        .iter()
        .filter(|(_, l)| l.contains("return Ok("))
        .map(|(n, l)| format!("{FILES_REL}:{n}: {}", l.trim()))
        .collect();
    assert!(
        silent.is_empty(),
        "{LIST_FN} に早期の成功 return が在る = 失敗を 200 で返して無言にする道ができている（#1451）:\n{}",
        silent.join("\n")
    );
    assert!(
        !list_text.contains("unwrap_or_default()") && !list_text.contains(".ok()?"),
        "{FILES_REL}:{list_at}: {LIST_FN} が失敗を既定値へ握り潰している"
    );

    // (b) エントリ単位の `unreadable` が落ちていない
    let (entry, entry_at) = fn_body(FILES_REL, &src, ENTRY_FN);
    let entry_text = joined(&entry);
    assert!(
        entry_text.contains("\"unreadable\": unreadable"),
        "{FILES_REL}:{entry_at}: エントリに `unreadable` が載っていない \
         = 読めない 1 件が 0 バイトの行として黙って混ざる（#1451）"
    );
    assert!(
        entry_text.contains("let unreadable = meta.is_none()"),
        "{FILES_REL}:{entry_at}: `unreadable` の判定が metadata の失敗を見ていない"
    );

    // (c) PWA が理由を出す（受け取っても表示しなければ無言のまま）
    let pwa = read(PWA_REL);
    for needle in [
        "e.unreadable",
        "読み取り権限がありません",
        "kind === 'unreadable'",
    ] {
        assert!(
            pwa.contains(needle),
            "{PWA_REL}: 読めないものの理由を画面へ出していない（{needle} が無い。#1451）"
        );
    }

    // (d) 理由の文言そのもの（日英）が消えていない
    let denial = tako_control::remote_files::Denial::Unreadable;
    assert_eq!(denial.kind(), "unreadable");
    assert!(!denial.message_ja().is_empty() && !denial.message_en().is_empty());
}

/// 巨大ディレクトリで実体を問う回数が件数に比例しない（#1451 の性能要件）
#[test]
fn 一覧は切ってから実体を問う() {
    let src = production(FILES_REL);
    let (body, at) = fn_body(FILES_REL, &src, LIST_FN);
    let truncate = body
        .iter()
        .position(|(_, l)| l.contains("names.truncate(MAX_ENTRIES)"))
        .unwrap_or_else(|| {
            panic!("{FILES_REL}:{at}: 名前の段階で切っていない（#1451 の「切ってから metadata」）")
        });
    let map = body
        .iter()
        .position(|(_, l)| l.contains("entry_json(n,"))
        .unwrap_or_else(|| panic!("{FILES_REL}:{at}: `entry_json` の呼び出しが見つからない"));
    assert!(
        truncate < map,
        "{FILES_REL}:{}: 実体を問う（entry_json）方が先にある \
         = 万単位のディレクトリで件数 x 3 回の syscall を撃つ（#1451）",
        body[map].0
    );
}

// --------------------------------------------- 5. 文書 ↔ コード（#1406 と同型）

/// 注入 ⑤: 脅威モデルがコードの既定と食い違う
#[test]
fn 脅威モデルがコードの既定を書いている() {
    let doc = read(THREAT_REL);
    let section = "### 全体のファイル閲覧（#1451）";
    let at = line_of(&doc, section);
    assert!(
        at > 0,
        "{THREAT_REL}: 「{section}」の節が無い。\
         読める範囲を広げた判断は脅威モデルに残す（#1406: 文章は機械検査が無いと黙って嘘になる）"
    );
    // **コードの既定を読んでから**文書がその既定を書いているかを見る
    let role = remote_files::FULL_BROWSE_ROLE.as_str();
    let body = &doc[doc.find(section).expect("節が在る")..];
    let body = body
        .split("\n### ")
        .next()
        .expect("節の本文")
        .split("\n## ")
        .next()
        .expect("節の本文");
    for needle in [role, "#1451", "interact"] {
        assert!(
            body.contains(needle),
            "{THREAT_REL}:{at}: 「{section}」の節に「{needle}」が無い \
             = コードの既定（{role} 以上だけ全体閲覧）と文書がズレている"
        );
    }
    assert!(
        body.contains("受容"),
        "{THREAT_REL}:{at}: 受容したリスクが書かれていない（何を許したかが後から読めない）"
    );
    // #1079 の節が「ツリー配下だけ」と言い切ったままになっていない
    let old = "### ファイル API（柱 3-E。#1079）";
    let old_at = line_of(&doc, old);
    let old_body = &doc[doc.find(old).expect("#1079 の節が在る")..];
    let old_body = old_body.split("\n### ").next().expect("節の本文");
    assert!(
        old_body.contains("#1451"),
        "{THREAT_REL}:{old_at}: #1079 の節が #1451 で変わった前提を書いていない \
         = 同じ文書の中で「読めるのはツリー配下だけ」と矛盾する（#1406 と同じ壊れ方）"
    );
}

// --------------------------------------------- 6. A/B の逃げ道

#[test]
fn abの逃げ道は1つ() {
    let src = production(FILES_REL);
    let envs: Vec<String> = src
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("TAKO_1451") && !is_comment(l))
        .map(|(i, l)| format!("{FILES_REL}:{}: {}", i + 1, l.trim()))
        .collect();
    assert_eq!(
        envs.len(),
        1,
        "#1451 の A/B は `TAKO_1451_LEGACY` の 1 軸だけにする（腕が増えると何を比べたか分からなくなる）:\n{}",
        envs.join("\n")
    );
    assert!(
        envs[0].contains("TAKO_1451_LEGACY"),
        "A/B の env 名が変わっている:\n{}",
        envs[0]
    );
    // PWA 側の逃げ道も 1 つ（ブラウザには env が届かないのでクエリ 1 本）
    let pwa = read(PWA_REL);
    let qs: Vec<String> = pwa
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("tako_1451_legacy") && !is_comment(l))
        .map(|(i, l)| format!("{PWA_REL}:{}: {}", i + 1, l.trim()))
        .collect();
    assert_eq!(
        qs.len(),
        1,
        "PWA 側の A/B も 1 本だけにする:\n{}",
        qs.join("\n")
    );
}
