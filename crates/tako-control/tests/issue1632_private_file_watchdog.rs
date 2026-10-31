//! **#1632 の番犬**: remote の秘密（トークン・QR・状態ファイル）の保護が Windows で no-op へ
//! 戻っていない。
//!
//! ## なぜ止めるのか
//!
//! `remote.rs` の `restrict_permissions` / `write_secret_file` / `ensure_state_dir` は unix だけ
//! 0600 / 0700 にし、Windows は `#[cfg(not(unix))] { let _ = path; }` と素の `std::fs::write` で
//! **何もしていなかった**（置き場も中身も `%APPDATA%\tako` の継承まかせ。縮退の宣言も無い）。
//! #1632 で境界 `tako_core::platform::private_file`（B32）へ寄せ、Windows は継承を切った
//! 本人 + SYSTEM の DACL を当てるようにした。
//!
//! 実際に効いているかは Windows の CI が実ファイルの ACL を読み戻して確かめる
//! （`remote::tests::issue1632_*` / `platform::private_file::tests`）が、それは Windows のジョブでしか
//! 走らない。**macOS の手元でも 1 行の戻しを落とす**ために、構造をここで固定する:
//!
//! 1. [`境界のwindowsの腕がdaclを当てる`] — Windows の腕の 3 つの口（置き場 / 作る / 絞る）が
//!    DACL を当てる 1 実装（`apply`）を通り、その実装が継承を切る形（`PROTECTED_DACL_…`）で
//!    `SetNamedSecurityInfoW` を呼ぶ。no-op（`let _ = path; Ok(())`）へ戻すと落ちる
//! 2. [`remoteの秘密の書き手が境界を通る`] — `remote.rs` の 3 つの書き手が境界を呼び、
//!    `#[cfg]` の腕（片方の OS にしか効かない形）・素の `std::fs::write` へ戻っていない。
//!    QR の保存が絞りを通ることも見る
//! 3. [`秘密を含む残りの書き手が境界を通る`]（#2581）— IPC の接続情報 control.json と `instances/`
//!    （`discovery::write_to`）・IPC / MCP の常設トークン（`load_or_create_token_at`）・usage の鍵の塩
//!    （`load_or_create_salt`）・`remote_shortcuts.json`（`ShortcutsFile::save_to`）・スマホから
//!    受け取ったファイル（`save_received_file`）・ペインの最後の出力の置き場（`ensure_pane_died_dir`）が
//!    境界を呼び、`#[cfg]` の腕へ戻っていない
//! 4. [`境界の外で本人だけのmodeを当てない`]（#2581）— **これから足される書き手**も広く見る。
//!    本番コード（全 crate の `src/`）で本人だけの mode（`0o600` / `0o700`）を境界の外で当てていたら、
//!    [`OWNER_ONLY_MODE_ALLOWED`]（秘密を含まない unix だけの実体 = ソケット・fifo など。理由つき）に
//!    載っていない限り `file:line` で落とす。unix でだけ絞って Windows を継承まかせにする形
//!    （#1632 / #2581 が直した形）を、新しいファイルでも作らせない
//!
//! 組む DACL の中身（本人 + SYSTEM だけ・`D:P`）は tako-core の単体
//! `platform::private_file::tests::本人とsystemだけの継承を切ったsddlを組む` が両 OS で固定する。

use std::path::{Path, PathBuf};

// 本番コードの範囲取りは 1 実装（#1420）
#[path = "common/production_range.rs"]
mod production_range;

const BOUNDARY: &str = "crates/tako-core/src/platform/private_file.rs";
const REMOTE: &str = "crates/tako-control/src/remote.rs";

/// Windows の腕の目印（綴りはこの 1 か所に持つ）
const WINDOWS_ARM: &str = "#[cfg(windows)]\nmod imp {";

/// 秘密を書く関数 1 本の約束（#2581）
struct SecretWriter {
    rel: &'static str,
    /// 関数のシグネチャの頭（改行 + インデントから。`fn_body` が同じインデントの `}` までを本文にする）
    signature: &'static str,
    /// 本文が通るべき境界の口
    calls: &'static [&'static str],
    /// 本文に在ってはいけない形（境界を通らずに書く・片方の OS にだけ効かせる）
    forbidden: &'static [&'static str],
    /// 何を書くか（落ちたときの文）
    what: &'static str,
}

/// 境界を通らない書き方（中身が秘密の書き手で禁じる）
const RAW_WRITES: &[&str] = &[
    "#[cfg(",
    "std::fs::write(",
    "OpenOptions",
    "set_permissions(",
];

/// 素の書き込みの後で絞る形は許す書き手の禁じ手（中身が秘密ではない・置き場だけを絞る）
const CFG_OR_RAW_MODE: &[&str] = &["#[cfg(", "OpenOptions", "set_permissions("];

/// #2581 で境界へ寄せた書き手
const SECRET_WRITERS: &[SecretWriter] = &[
    SecretWriter {
        rel: "crates/tako-control/src/discovery.rs",
        signature: "\npub(crate) fn write_to(",
        calls: &[
            "private_file::restrict_dir(",
            "private_file::create(",
            "private_file::restrict(",
        ],
        forbidden: RAW_WRITES,
        what: "IPC の接続情報（control.json / instances/。トークン入り）",
    },
    SecretWriter {
        rel: "crates/tako-control/src/lib.rs",
        signature: "\nfn load_or_create_token_at(",
        calls: &["private_file::create(", "private_file::restrict("],
        forbidden: RAW_WRITES,
        what: "IPC / MCP の常設トークン",
    },
    SecretWriter {
        rel: "crates/tako-control/src/usage.rs",
        signature: "\npub(crate) fn load_or_create_salt(",
        calls: &["private_file::create_new("],
        forbidden: RAW_WRITES,
        what: "usage の鍵の塩（machine_id）",
    },
    SecretWriter {
        rel: "crates/tako-core/src/remote_shortcuts.rs",
        signature: "\n    pub fn save_to(",
        calls: &["private_file::restrict_dir(", "private_file::restrict("],
        forbidden: CFG_OR_RAW_MODE,
        what: "remote/remote_shortcuts.json（管理トークンと同じ置き場）",
    },
    SecretWriter {
        rel: "crates/tako-control/src/remote.rs",
        signature: "\nfn save_received_file(",
        calls: &["private_file::create(", "private_file::restrict("],
        forbidden: RAW_WRITES,
        what: "スマホから受け取ったファイル（#287 P2-1）",
    },
    SecretWriter {
        rel: "crates/tako-control/src/remote.rs",
        signature: "\nfn handle_upload(",
        calls: &["save_received_file("],
        forbidden: CFG_OR_RAW_MODE,
        what: "スマホからの受け取り（書くのは save_received_file の 1 か所）",
    },
    SecretWriter {
        rel: "crates/tako-core/src/tmux_backend.rs",
        signature: "\nfn ensure_pane_died_dir(",
        calls: &["private_file::restrict_dir("],
        forbidden: CFG_OR_RAW_MODE,
        what: "ペインの最後の出力の置き場（#2085。ペインの中身が入る）",
    },
];

/// 境界の外で本人だけの mode（`0o600` / `0o700`）を当ててよい所（#2581）。
/// `(ファイル, 囲む関数の名前, 理由)`。どれも**秘密の中身を持たない unix だけの実体**で、
/// Windows には同じ実体が無い（境界の DACL の対象ではない）
const OWNER_ONLY_MODE_ALLOWED: &[(&str, &str, &str)] = &[
    (
        "crates/tako-control/src/ipc.rs",
        "bind",
        "IPC の受け口の Unix ドメインソケット。Windows は名前付きパイプ（カーネルオブジェクト）で、トークン認証が二段目",
    ),
    ("crates/tako-control/src/ipc.rs", "rebind", "同上（張り直し）"),
    (
        "crates/tako-core/src/ipc_socket.rs",
        "write_pointer",
        "ソケットの実体パス 1 行だけの参照ファイル（秘密を含まない）",
    ),
    (
        "crates/tako-core/src/platform/jobserver.rs",
        "open_or_create",
        "jobserver の fifo（unix だけの実体）",
    ),
    (
        "crates/tako-control/src/platform/local_endpoint.rs",
        "bind_unix",
        "remote API の Unix ドメインソケット（unix だけの腕）",
    ),
];

/// 本人だけの mode の綴り
const OWNER_ONLY_MODES: &[&str] = &["0o600", "0o700"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

/// 本番コードの「コードだけの眺め」（テストの item・コメント・文字列を空白へ潰す。
/// バイト長と行番号は保たれるので `file:line` の名指しがそのまま使える）
fn code(rel: &str) -> String {
    code_with_floor(rel, production_range::MIN_COVERAGE)
}

/// 書き手の検査用（#2581）。テストが大半を占めるファイル（`tmux_backend.rs` は本番 26%）も見るので
/// 下限を緩める（下限は「範囲取りが本番を飲み込んでいない」の安全網で、書き手の約束とは別）
fn writer_code(rel: &str) -> String {
    code_with_floor(rel, 0.2)
}

fn code_with_floor(rel: &str, floor: f64) -> String {
    let path = repo_root().join(rel);
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
    production_range::code_view_of(&production_range::production_with_floor(&src, rel, floor))
}

fn line_of(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())].matches('\n').count() + 1
}

/// `from` 以降で最初に現れる関数 1 本の本文（シグネチャから同インデントの `}` まで）。
/// 戻り値は `(シグネチャの行番号, 本文)`
fn fn_body(source: &str, rel: &str, from: usize, signature: &str) -> (usize, String) {
    let at = source[from..]
        .find(signature)
        .map(|i| from + i)
        .unwrap_or_else(|| {
            panic!(
                "{rel}:{} に `{}` が見つからない（#1632。改名したら番犬も直す）",
                line_of(source, from),
                signature.trim()
            )
        });
    let sig = signature.trim_start_matches('\n');
    let start = at + (signature.len() - sig.len());
    let indent = " ".repeat(sig.len() - sig.trim_start().len());
    let close = format!("\n{indent}}}");
    let end = source[start..]
        .find(&close)
        .map(|i| start + i)
        .unwrap_or(source.len());
    (line_of(source, start), source[start..end].to_string())
}

// --------------------------------------------------------------- 検査本体
// 注入テストからも呼ぶので、読むソースは引数で受ける

fn check_boundary_windows_arm(source: &str, rel: &str) {
    let arm = source.find(WINDOWS_ARM).unwrap_or_else(|| {
        panic!(
            "{rel}:1 Windows の腕（`#[cfg(windows)] mod imp`）が無い（#1632）。\n\
             `#[cfg(not(unix))]` の素通りへ戻すと、Windows では秘密のファイルが親の ACL を受け継ぐだけになる"
        )
    });
    for (signature, kind) in [
        ("\n    pub fn restrict_dir(", "置き場"),
        ("\n    pub fn create(", "作る"),
        ("\n    pub fn create_new(", "無いときだけ作る"),
        ("\n    pub fn restrict(", "絞る"),
    ] {
        let (line, body) = fn_body(source, rel, arm, signature);
        assert!(
            body.contains("apply("),
            "{rel}:{line} Windows の腕の「{kind}」口が DACL を当てていない（#1632）。\n\
             `apply(…)` を通らない形（`let _ = path; Ok(())` など）は修正前の no-op と同じ。本文:\n{body}"
        );
    }
    let (line, apply) = fn_body(source, rel, arm, "\n    fn apply(");
    assert!(
        apply.contains("owner_only_sddl("),
        "{rel}:{line} `apply` が本人 + SYSTEM だけの DACL（`owner_only_sddl`）を組んでいない（#1632）。本文:\n{apply}"
    );
    let (line, set) = fn_body(source, rel, arm, "\n    fn set_dacl_of(");
    for needle in [
        "SetNamedSecurityInfoW(",
        "DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION",
    ] {
        assert!(
            set.contains(needle),
            "{rel}:{line} DACL の書き込みが `{needle}` を通っていない（#1632）。\n\
             `PROTECTED_DACL_SECURITY_INFORMATION` が無いと親（%APPDATA%）の ACE が受け継がれたまま残る。本文:\n{set}"
        );
    }
    assert!(
        apply.contains("set_dacl_of("),
        "{rel}:{line} `apply` が DACL の書き込み（`set_dacl_of`）を呼んでいない（#1632）。本文:\n{apply}"
    );
}

fn check_remote_writers(source: &str, rel: &str) {
    for (signature, call) in [
        ("\nfn ensure_state_dir(", "private_file::restrict_dir("),
        ("\nfn write_secret_file(", "private_file::create("),
        ("\nfn restrict_permissions(", "private_file::restrict("),
    ] {
        let (line, body) = fn_body(source, rel, 0, signature);
        assert!(
            body.contains(call),
            "{rel}:{line} `{}` が境界の `{call}…)` を通っていない（#1632）。\n\
             秘密の書き手は `tako_core::platform::private_file` の 1 実装で絞る（Windows は継承を切った DACL）。本文:\n{body}",
            signature.trim()
        );
        if let Some(at) = body.find("#[cfg(") {
            panic!(
                "{rel}:{} `{}` に `#[cfg]` の腕が戻っている（#1632）。\n\
                 OS ごとの差は境界 `platform::private_file` の中に閉じる（remote.rs に cfg を撒かない = #1616 の作法）。本文:\n{body}",
                line + body[..at].matches('\n').count(),
                signature.trim()
            );
        }
        for raw in ["std::fs::write(", "OpenOptions", "set_permissions("] {
            if let Some(at) = body.find(raw) {
                panic!(
                    "{rel}:{} `{}` が境界を通らずに `{raw}` で書いている（#1632）。\n\
                     素の書き込みは Windows で親の ACL を受け継ぐだけになる。本文:\n{body}",
                    line + body[..at].matches('\n').count(),
                    signature.trim()
                );
            }
        }
    }
    let (line, body) = fn_body(source, rel, 0, "\npub fn generate_qr_png(");
    let saved = body.find(".save(").unwrap_or_else(|| {
        panic!("{rel}:{line} `generate_qr_png` の保存（`.save(`）が見つからない（番犬も直す）")
    });
    let restricted = body.find("restrict_permissions(").unwrap_or_else(|| {
        panic!(
            "{rel}:{line} QR（トークン入り URL）の保存の後で絞っていない（#1632 / #104）。本文:\n{body}"
        )
    });
    assert!(
        saved < restricted,
        "{rel}:{line} QR を絞るのが保存より前にある（保存が新しく作ったファイルが絞られない）"
    );
}

/// 書き手 1 本が境界の口を通り、禁じ手を持たない（#2581）
fn check_secret_writer(source: &str, w: &SecretWriter) {
    let rel = w.rel;
    let (line, body) = fn_body(source, rel, 0, w.signature);
    for call in w.calls {
        assert!(
            body.contains(call),
            "{rel}:{line} `{}`（{}）が境界の `{call}…)` を通っていない（#2581）。\n\
             秘密の書き手は `tako_core::platform::private_file` の 1 実装で絞る（Windows は継承を切った DACL）。本文:\n{body}",
            w.signature.trim(),
            w.what
        );
    }
    for raw in w.forbidden {
        if let Some(at) = body.find(raw) {
            panic!(
                "{rel}:{} `{}`（{}）が境界を通らずに `{raw}` を使っている（#2581）。\n\
                 unix でだけ絞る形は Windows で親の ACL を受け継ぐだけになる（OS ごとの差は境界の中に閉じる）。本文:\n{body}",
                line + body[..at].matches('\n').count(),
                w.signature.trim(),
                w.what
            );
        }
    }
}

/// 位置 `at` を囲む関数の名前（`fn <名前>(` / `fn <名前><`。コードだけの眺めで後ろ向きに探す）
fn enclosing_fn(source: &str, at: usize) -> Option<&str> {
    let head = &source[..at];
    head.rmatch_indices("fn ").find_map(|(i, _)| {
        let boundary = head[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        let rest = &source[i + 3..];
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        let follows = rest[end..].starts_with('(') || rest[end..].starts_with('<');
        (boundary && end > 0 && follows).then(|| &rest[..end])
    })
}

/// 本番コード 1 ファイルで、境界の外の本人だけの mode を名指す（`(行, 関数, mode)` の並び）
fn owner_only_modes_outside(source: &str, rel: &str) -> Vec<(usize, String, &'static str)> {
    let mut out = Vec::new();
    if rel == BOUNDARY {
        return out;
    }
    for mode in OWNER_ONLY_MODES {
        for (at, _) in source.match_indices(mode) {
            let func = enclosing_fn(source, at).unwrap_or("?").to_string();
            let allowed = OWNER_ONLY_MODE_ALLOWED
                .iter()
                .any(|(file, name, _)| *file == rel && *name == func);
            if !allowed {
                out.push((line_of(source, at), func, *mode));
            }
        }
    }
    out
}

fn check_owner_only_modes(files: &[(String, String)]) {
    let mut found = Vec::new();
    for (rel, source) in files {
        for (line, func, mode) in owner_only_modes_outside(source, rel) {
            found.push(format!(
                "{rel}:{line} `fn {func}` が `{mode}` を境界の外で当てている"
            ));
        }
    }
    assert!(
        found.is_empty(),
        "境界 `tako_core::platform::private_file` の外で本人だけの mode を当てている（#2581）。\n\
         unix でだけ絞る形は Windows で親の ACL を受け継ぐだけになる（#1632 / #2581 が直した形）。\n\
         秘密を書くなら境界の `create` / `create_new` / `restrict` / `restrict_dir` を通す。\n\
         秘密を含まない unix だけの実体（ソケット・fifo）なら理由を添えて `OWNER_ONLY_MODE_ALLOWED` へ:\n{}",
        found.join("\n")
    );
}

/// 全 crate の `src/` の本番コードの眺め（`(リポジトリからの相対パス, コードだけの眺め)`）
fn production_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut paths = Vec::new();
    for krate in std::fs::read_dir(root.join("crates"))
        .expect("crates/ を読める")
        .flatten()
    {
        walk(&krate.path().join("src"), &mut paths);
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let src = std::fs::read_to_string(&path).unwrap();
            // 小さなファイルも見るので下限の確認（`production`）は通さず、潰すだけにする
            let kept = production_range::scan(&src).text;
            (rel, production_range::code_view_of(&kept))
        })
        .collect()
}

// --------------------------------------------------------------- テスト

#[test]
fn 境界のwindowsの腕がdaclを当てる() {
    check_boundary_windows_arm(&code(BOUNDARY), BOUNDARY);
}

#[test]
fn remoteの秘密の書き手が境界を通る() {
    check_remote_writers(&code(REMOTE), REMOTE);
}

#[test]
fn 秘密を含む残りの書き手が境界を通る() {
    for w in SECRET_WRITERS {
        check_secret_writer(&writer_code(w.rel), w);
    }
}

#[test]
fn 境界の外で本人だけのmodeを当てない() {
    let files = production_sources();
    assert!(
        files.len() > 100,
        "走査したファイルが {} 本しか無い（crates/*/src を読めていない）",
        files.len()
    );
    check_owner_only_modes(&files);
    // 許し札が実在の関数を指している（改名・削除で空振りの札が残らない）
    for (rel, name, why) in OWNER_ONLY_MODE_ALLOWED {
        let (_, source) = files
            .iter()
            .find(|(r, _)| r == rel)
            .unwrap_or_else(|| panic!("許し札のファイル {rel} が無い（{why}）"));
        let used = OWNER_ONLY_MODES.iter().any(|mode| {
            source
                .match_indices(mode)
                .any(|(at, _)| enclosing_fn(source, at) == Some(name))
        });
        assert!(
            used,
            "{rel} の `fn {name}` に本人だけの mode が無い = 許し札が空振り（札を外す。{why}）"
        );
    }
}

/// #2581 の番犬に検出力があること: 寄せる前の形を実際に落とす（注入は `cargo test` の中で完結し、
/// リポジトリのファイルは触らない）
#[test]
fn 寄せる前の形を実際に落とす_2581() {
    let fails = |f: &dyn Fn()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err();
    let writer = |rel: &str, signature: &str| {
        SECRET_WRITERS
            .iter()
            .find(|w| w.rel == rel && w.signature == signature)
            .expect("表に在る")
    };

    // ① #2581 前の discovery::write_to（素の write + unix だけの chmod）
    let discovery = "crates/tako-control/src/discovery.rs";
    let legacy = "
pub(crate) fn write_to(path: &Path, info: &ControlInfo) -> io::Result<()> {
    let tmp = path.with_extension(\"json.tmp\");
    std::fs::write(&tmp, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}
";
    let w = writer(discovery, "\npub(crate) fn write_to(");
    assert!(
        fails(&|| check_secret_writer(legacy, w)),
        "① #2581 前の control.json の書き手を見逃した"
    );
    // 同じ形は広い走査でも名指される（許し札に無い関数の 0o700 / 0o600）
    let found = owner_only_modes_outside(&production_range::code_view_of(legacy), discovery);
    assert_eq!(found.len(), 2, "① 広い走査が 2 か所を名指さない: {found:?}");
    assert!(
        found.iter().all(|(_, func, _)| func == "write_to"),
        "{found:?}"
    );

    // ② 現行の書き手へ素の書き込みを足す（境界の呼び出しを残したままでも落ちる）
    let lib = "crates/tako-control/src/lib.rs";
    let token = writer_code(lib);
    let raw = token.replacen(
        "    tako_core::platform::private_file::restrict(path)?;\n    Ok(token)",
        "    tako_core::platform::private_file::restrict(path)?;\n    std::fs::write(path, &token)?;\n    Ok(token)",
        1,
    );
    assert_ne!(raw, token, "② の足場（トークンの restrict）が見つからない");
    assert!(
        fails(&|| check_secret_writer(&raw, writer(lib, "\nfn load_or_create_token_at("))),
        "② トークンの素の書き込みを見逃した"
    );

    // ③ 塩を unix だけの mode へ戻す
    let usage = "crates/tako-control/src/usage.rs";
    let salt = writer_code(usage);
    let back = salt.replacen(
        "match tako_core::platform::private_file::create_new(path) {",
        "let mut options = std::fs::OpenOptions::new();\n    options.write(true).create_new(true);\n    #[cfg(unix)]\n    {\n        use std::os::unix::fs::OpenOptionsExt as _;\n        options.mode(0o600);\n    }\n    match options.open(path) {",
        1,
    );
    assert_ne!(back, salt, "③ の足場（塩の create_new）が見つからない");
    assert!(
        fails(&|| check_secret_writer(
            &back,
            writer(usage, "\npub(crate) fn load_or_create_salt(")
        )),
        "③ 塩の unix だけの mode を見逃した"
    );
    let found = owner_only_modes_outside(&back, usage);
    assert_eq!(
        found.len(),
        1,
        "③ 広い走査が塩の 0o600 を名指さない: {found:?}"
    );
    assert_eq!(found[0].1, "load_or_create_salt");

    // ④ 受け取りが素の OpenOptions へ戻る
    let remote = code(REMOTE);
    let upload = remote.replacen(
        "    if let Err(e) = save_received_file(&target_path, &file_data) {",
        "    let _ = std::fs::OpenOptions::new().write(true).open(&target_path);\n    if let Err(e) = save_received_file(&target_path, &file_data) {",
        1,
    );
    assert_ne!(
        upload, remote,
        "④ の足場（handle_upload の書き込み）が見つからない"
    );
    assert!(
        fails(&|| check_secret_writer(&upload, writer(REMOTE, "\nfn handle_upload("))),
        "④ 受け取りの素の OpenOptions を見逃した"
    );

    // ⑤ 新しい書き手が unix だけの chmod で秘密を書く（表に無い関数でも広い走査が名指す）
    let fresh = "
fn write_new_secret(path: &Path) {
    std::fs::write(path, \"s\").unwrap();
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}
";
    let found = owner_only_modes_outside(fresh, "crates/tako-control/src/new_secret.rs");
    assert_eq!(found.len(), 1, "⑤ 新しい書き手を名指さない: {found:?}");
    assert_eq!((found[0].0, found[0].1.as_str()), (6, "write_new_secret"));
    assert!(
        fails(&|| check_owner_only_modes(&[(
            "crates/tako-control/src/new_secret.rs".into(),
            fresh.into()
        )])),
        "⑤ 新しい書き手を見逃した"
    );
    // 許し札の関数でも、別のファイルなら許さない
    let socket = "\nfn bind(path: &Path) {\n    let _ = set(path, 0o600);\n}\n";
    assert_eq!(
        owner_only_modes_outside(socket, "crates/tako-control/src/other.rs").len(),
        1,
        "⑤ 許し札をファイルをまたいで効かせている"
    );
    assert!(owner_only_modes_outside(socket, "crates/tako-control/src/ipc.rs").is_empty());

    // ⑥ 現行の形は通る（常に落ちる無意味な番犬になっていないこと）
    for w in SECRET_WRITERS {
        check_secret_writer(&writer_code(w.rel), w);
    }
}

/// 番犬に検出力があること: 直前の 2 本が**#1632 以前の形を実際に落とす**。
///
/// 製品ソースを書き換える代わりに、戻した形の断片を同じ検査へ通す
/// （注入は `cargo test` の中で完結し、リポジトリのファイルは触らない）
#[test]
fn 修正前の形を実際に落とす() {
    let boundary = code(BOUNDARY);
    let remote = code(REMOTE);
    let fails = |f: &dyn Fn()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err();

    // ① 境界の Windows の腕の「絞る」口を no-op へ戻す
    let noop = boundary.replacen(
        "    pub fn restrict(path: &Path) -> io::Result<()> {\n        keep_going(apply(path, Kind::File))\n    }",
        "    pub fn restrict(path: &Path) -> io::Result<()> {\n        let _ = path;\n        Ok(())\n    }",
        1,
    );
    assert_ne!(
        noop, boundary,
        "① の足場（Windows の restrict）が見つからない"
    );
    assert!(
        fails(&|| check_boundary_windows_arm(&noop, "注入")),
        "① Windows の腕の no-op を見逃した"
    );

    // ② 継承を切らずに DACL を書く（親の ACE が残る）
    let inherit = boundary.replacen(
        "DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION",
        "DACL_SECURITY_INFORMATION",
        1,
    );
    assert_ne!(inherit, boundary, "② の足場（保護の印）が見つからない");
    assert!(
        fails(&|| check_boundary_windows_arm(&inherit, "注入")),
        "② 継承を切らない形を見逃した"
    );

    // ③ Windows の腕ごと消して `#[cfg(not(unix))]` の素通りにする
    let gone = boundary.replacen(WINDOWS_ARM, "#[cfg(not(unix))]\nmod imp {", 1);
    assert!(
        fails(&|| check_boundary_windows_arm(&gone, "注入")),
        "③ Windows の腕の消失を見逃した"
    );

    // ④ #1632 以前の remote.rs そのもの（`#[cfg(not(unix))] { let _ = path; }`）
    let legacy = "
fn ensure_state_dir() -> io::Result<std::path::PathBuf> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    tako_core::platform::private_file::restrict_dir(&dir);
    Ok(dir)
}

fn write_secret_file(path: &std::path::Path, content: &str) -> io::Result<()> {
    let tmp = tako_core::platform::private_file::create(&tmp)?;
    Ok(())
}

fn restrict_permissions(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}
";
    assert!(
        fails(&|| check_remote_writers(legacy, "注入")),
        "④ #1632 以前の restrict_permissions を見逃した"
    );

    // ⑤ 書き手が素の std::fs::write へ戻る（境界の呼び出しを残したまま足しても落ちる）
    let raw = remote.replacen(
        "        f.sync_all()?;\n",
        "        f.sync_all()?;\n        std::fs::write(&tmp, content)?;\n",
        1,
    );
    assert_ne!(
        raw, remote,
        "⑤ の足場（write_secret_file の sync_all）が見つからない"
    );
    assert!(
        fails(&|| check_remote_writers(&raw, "注入")),
        "⑤ 素の std::fs::write を見逃した"
    );

    // ⑥ 現行の形は通る（常に落ちる無意味な番犬になっていないこと）
    check_boundary_windows_arm(&boundary, BOUNDARY);
    check_remote_writers(&remote, REMOTE);
}
