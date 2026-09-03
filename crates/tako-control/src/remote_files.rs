//! リモートからのファイル参照（#1079。リモート刷新 柱 3-E）。
//!
//! スマホから PC のファイルを**見る / プレビューする / 保存する**ための API。
//! 書き戻し（柱 3-F）と SSH 先（柱 3-G）は別 Issue で、ここは読み出しだけを扱う。
//!
//! # 認可の正（このモジュールの本体）
//!
//! 読めるのは **「tako のファイルツリーに現に出ているルートの配下」だけ**。
//! ルートの一覧は daemon が持たず、毎回 tako app へ問い合わせて組む
//! （`Request::TreeFolder { action: "roots" }`）ので、Mac 側でペインを閉じるか
//! ピン留めを外せば**その瞬間に読めなくなる**。daemon 側に独自の許可リストを
//! 育てない = 画面に出ていないものは配らない、を構造で守る。
//!
//! 拒否は 2 段構え（どちらか一方でも通れば拒否）:
//!
//! 1. `check_relative_shape` — **FS に触らない純粋関数**。絶対パス（POSIX / Windows /
//!    UNC の各形）と `..` を含む形を、ホスト OS に関係なく落とす
//! 2. `resolve_in_root` — `canonicalize` してからルート配下かを**コンポーネント単位**で
//!    照合する。ルート内に置かれた symlink が外を指していてもここで落ちる
//!
//! # 監査
//!
//! `audit_payload` が組む JSON には**パス・ファイル名・ルート名を一切載せない**
//! （ペイン内容と同基準。#287 P2-2）。載っていないことは
//! `audit_payloadにパスが混ざらない` と番犬テストが機械検証する。
//!
//! # このモジュールの置き場所
//!
//! HTTP の受け口までここに閉じてあるのは、並行して `remote.rs` を改修している
//! 他の作業とコンフリクトさせないため（#1077 / #1078 / #1080）。`remote.rs` 側の
//! 変更はルータのアーム 1 個・role 表の 1 分岐・IPC を 1 往復する小さなヘルパーだけ。

use crate::remote_auth::DeviceRole;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

/// ディレクトリ一覧で返すエントリ数の上限（超えたら `truncated` が true）
pub const MAX_ENTRIES: usize = 1000;

/// プレビュー（`/api/files/content`）で返す最大バイト数。
/// これを超えるファイルは本文を返さず「ダウンロードしてください」に倒す
pub const MAX_TEXT_BYTES: u64 = 512 * 1024;

/// ダウンロードの上限。ストリーミングで返すので daemon のメモリは食わないが、
/// スマホ側の事故（数十 GB を掴む）を避けるために上限は設ける
pub const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

// --- ルート ---

/// ファイルツリーに現に出ているルート 1 件。
///
/// `id` はパスから導く短い不透明値。**URL に絶対パスを載せない**ためで、
/// 秘密ではない（認可は「今のルート一覧に在るか」の照合で行う）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRoot {
    pub id: String,
    pub path: PathBuf,
    /// 表示名（末尾のフォルダ名）
    pub name: String,
    pub tab: u64,
    pub tab_title: String,
}

impl TreeRoot {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "tab": self.tab,
            "tab_title": self.tab_title,
        })
    }
}

/// パスから 12 桁の id を作る（FNV-1a 64bit）。
///
/// 暗号学的強度は要らない: id は秘密ではなく、認可は
/// 「その id が**今の**ルート一覧に在るか」の照合で行うため。
/// 万一衝突しても `roots_from_payload` が接尾辞で分離するので取り違えは起きない
pub fn root_id_of(path: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in path.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:012x}")[..12].to_string()
}

/// tako app の `TreeFolder { action: "roots" }` 応答からルート一覧を組む。
///
/// id が衝突したら（ハッシュ衝突・同一パスが複数タブに出ている）接尾辞で分離する。
/// **同一パスが別タブにも出ている場合は先に出たものだけを残す**
/// （配下のファイルは同じなので、二重に見せる意味がない）
pub fn roots_from_payload(payload: &Value) -> Vec<TreeRoot> {
    let mut out: Vec<TreeRoot> = Vec::new();
    let mut seen_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let Some(tabs) = payload["tabs"].as_array() else {
        return out;
    };
    for tab in tabs {
        let tab_id = tab["tab"].as_u64().unwrap_or(0);
        let tab_title = tab["title"].as_str().unwrap_or_default().to_string();
        let Some(roots) = tab["roots"].as_array() else {
            continue;
        };
        for root in roots {
            let Some(path) = root.as_str() else { continue };
            if path.is_empty() || !seen_paths.insert(path.to_string()) {
                continue;
            }
            let base = root_id_of(path);
            let mut id = base.clone();
            let mut n = 1;
            while out.iter().any(|r| r.id == id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            let name = Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string());
            out.push(TreeRoot {
                id,
                path: PathBuf::from(path),
                name,
                tab: tab_id,
                tab_title: tab_title.clone(),
            });
        }
    }
    out
}

// --- 拒否の種別 ---

/// 読み出しを断る理由。理由 + 次の一手を日英で返す（#435）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    /// 指定された root がツリーに出ていない（消えた / そもそも無い）
    UnknownRoot,
    /// path が絶対パス（POSIX / Windows ドライブ / UNC のいずれか）
    AbsolutePath,
    /// path に `..` が含まれる
    Traversal,
    /// 解決した実体がルートの外（ルート内の symlink が外を指している等）
    EscapesRoot,
    /// 実体が無い
    NotFound,
    /// ディレクトリを期待したのにファイルだった
    NotADirectory,
    /// ファイルを期待したのにディレクトリだった
    NotAFile,
    /// 上限を超える大きさ
    TooLarge,
    /// 読めなかった（権限など）
    Unreadable,
}

impl Denial {
    /// HTTP ステータス。**認可の失敗は必ず 403**（存在の有無を漏らさないため
    /// ツリー外は 404 ではなく 403 に倒す）
    pub fn status(self) -> u16 {
        match self {
            Self::UnknownRoot | Self::AbsolutePath | Self::Traversal | Self::EscapesRoot => 403,
            Self::NotFound => 404,
            Self::NotADirectory | Self::NotAFile => 400,
            Self::TooLarge => 413,
            Self::Unreadable => 500,
        }
    }

    /// 機械可読な種別（PWA の出し分け用）
    pub fn kind(self) -> &'static str {
        match self {
            Self::UnknownRoot => "unknown_root",
            Self::AbsolutePath => "absolute_path",
            Self::Traversal => "traversal",
            Self::EscapesRoot => "escapes_root",
            Self::NotFound => "not_found",
            Self::NotADirectory => "not_a_directory",
            Self::NotAFile => "not_a_file",
            Self::TooLarge => "too_large",
            Self::Unreadable => "unreadable",
        }
    }

    pub fn message_ja(self) -> &'static str {
        match self {
            Self::UnknownRoot => {
                "このフォルダは tako のファイルツリーに出ていません。Mac 側でフォルダを開いてください"
            }
            Self::AbsolutePath => "絶対パスは指定できません。ツリーのルートからの相対パスで指定してください",
            Self::Traversal => "上位フォルダへの参照（..）は指定できません",
            Self::EscapesRoot => "ツリーに出ているフォルダの外は参照できません",
            Self::NotFound => "見つかりません",
            Self::NotADirectory => "フォルダではありません",
            Self::NotAFile => "ファイルではありません",
            Self::TooLarge => "大きすぎるため表示できません。ダウンロードしてください",
            Self::Unreadable => "読み取れませんでした（権限を確認してください）",
        }
    }

    pub fn message_en(self) -> &'static str {
        match self {
            Self::UnknownRoot => {
                "This folder is not shown in tako's file tree. Open it on the Mac first"
            }
            Self::AbsolutePath => {
                "Absolute paths are not allowed; use a path relative to the tree root"
            }
            Self::Traversal => "Parent directory references (..) are not allowed",
            Self::EscapesRoot => "Cannot read outside the folders shown in the tree",
            Self::NotFound => "Not found",
            Self::NotADirectory => "Not a directory",
            Self::NotAFile => "Not a file",
            Self::TooLarge => "Too large to display; download it instead",
            Self::Unreadable => "Could not read it (check permissions)",
        }
    }

    /// エラー応答の本体。`error` は既存 API と同じキー名（PWA が拾う）
    pub fn to_json(self) -> Value {
        json!({
            "error": self.message_ja(),
            "error_en": self.message_en(),
            "kind": self.kind(),
        })
    }
}

// --- 純粋関数: 相対パスの形の検査（FS に触らない） ---

/// `path` パラメータの形を検査する。**ホスト OS に依存しない**ので
/// macOS 上から Windows 形の攻撃文字列も総当たりで検査できる。
///
/// `/` と `\` の**両方**を区切りとして扱い、どちらかの区切りで `..` になる形を落とす。
/// POSIX では `a\..\b` は traversal ではない（`\` はただの文字）が、
/// 「拒否しすぎる」側は安全で、両 OS で同じ答えになる利点が大きい
/// （代償: `\` や `C:` を名前に含む POSIX のファイルはリモートから開けない）。
///
/// ドライブらしい形は**先頭だけでなく全コンポーネント**で落とす:
/// `PathBuf::push` は Windows で prefix つきの断片を渡すと**それまでのパスを捨てる**ので、
/// `a/C:/x` を素通しにするとルートの外へ出られる（最後の配下判定でも捕まるが、
/// 認可を 1 層に頼らない）
pub fn check_relative_shape(rel: &str) -> Result<(), Denial> {
    if rel.is_empty() {
        return Ok(());
    }
    // NUL・制御文字はパスとして扱わない（ログ・ヘッダ汚染も防ぐ）
    if rel.chars().any(|c| c.is_control()) {
        return Err(Denial::Traversal);
    }
    // POSIX 絶対 / Windows UNC
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(Denial::AbsolutePath);
    }
    for part in rel.split(['/', '\\']) {
        if part == ".." {
            return Err(Denial::Traversal);
        }
        // Windows ドライブ（`C:` / `c:x`）。ホスト OS に関係なく落とす
        let b = part.as_bytes();
        if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
            return Err(Denial::AbsolutePath);
        }
    }
    Ok(())
}

/// `target` が `root` の**配下**かを判定する純粋関数。
///
/// 文字列の前方一致では `/a/bc` が `/a/b` の配下に見えてしまうので、
/// `Path::starts_with`（コンポーネント単位の比較）を使う
pub fn is_within(root: &Path, target: &Path) -> bool {
    target == root || target.starts_with(root)
}

/// 解決済みのパス（ルート配下であることが確認済み）
#[derive(Debug, Clone)]
pub struct Resolved {
    /// 実体の絶対パス（canonicalize 済み）
    pub path: PathBuf,
    /// ルートからの相対パス（`/` 区切りに正規化。応答と PWA の表示に使う）
    pub rel: String,
    /// 由来のルート
    pub root_id: String,
    /// ルートの表示名（パンくずに出す。**絶対パスは応答に載せない**）
    pub root_name: String,
    /// ルートの canonical パス。配下判定の基準をここに持っておく
    /// （相対パスの深さから逆算すると、深さの数え方を間違えた瞬間に
    /// 「ルートの外を配下と見なす」壊れ方をする）
    pub root_canon: PathBuf,
}

/// ルート一覧から `root_id` を引き、`rel` を配下のパスとして解決する。
///
/// **認可はここが正**。呼び出し側は必ずこれを通す
pub fn resolve_in_root(roots: &[TreeRoot], root_id: &str, rel: &str) -> Result<Resolved, Denial> {
    let root = roots
        .iter()
        .find(|r| r.id == root_id)
        .ok_or(Denial::UnknownRoot)?;
    check_relative_shape(rel)?;

    // ルート自身も canonicalize する（ルートが symlink でも両辺の基準を揃える）。
    // ルートが消えていれば「ツリーに出ていない」と同じ扱い
    let root_canon = root.path.canonicalize().map_err(|_| Denial::UnknownRoot)?;

    let mut joined = root_canon.clone();
    for part in rel.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        joined.push(part);
    }

    // canonicalize が symlink を解決する。ここで外を指していれば落ちる
    let target = joined.canonicalize().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Denial::NotFound,
        std::io::ErrorKind::PermissionDenied => Denial::Unreadable,
        _ => Denial::NotFound,
    })?;
    if !is_within(&root_canon, &target) {
        return Err(Denial::EscapesRoot);
    }

    let rel_norm = target
        .strip_prefix(&root_canon)
        .map(|p| {
            p.components()
                .filter_map(|c| match c {
                    Component::Normal(s) => Some(s.to_string_lossy().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_default();

    Ok(Resolved {
        path: target,
        rel: rel_norm,
        root_id: root.id.clone(),
        root_name: root.name.clone(),
        root_canon,
    })
}

// --- ディレクトリ一覧 ---

/// 1 エントリの JSON を組む。`symlink` は symlink_metadata で判定する
/// （リンク自身の種別を出す。開いたときの認可は `resolve_in_root` が別途行う）
fn entry_json(name: &str, dir_path: &Path, root_canon: &Path) -> Value {
    let full = dir_path.join(name);
    let link_meta = full.symlink_metadata().ok();
    let is_symlink = link_meta
        .as_ref()
        .is_some_and(|m| m.file_type().is_symlink());
    // ルートの外を指す symlink は開けない（`resolve_in_root` が 403 にする）。
    // 一覧の時点で印を付けておくと、PWA が「押しても 403」の行を避けられる
    let escapes = is_symlink
        && !full
            .canonicalize()
            .is_ok_and(|target| is_within(root_canon, &target));
    // symlink はリンク先の種別で「開けるか」が決まるので metadata（follow）で見る
    let meta = full.metadata().ok();
    let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    json!({
        "name": name,
        "dir": is_dir,
        "size": if is_dir { Value::Null } else { json!(size) },
        "modified": modified,
        "symlink": is_symlink,
        "escapes_root": escapes,
        "hidden": name.starts_with('.'),
    })
}

/// ディレクトリの中身を返す。並びは**フォルダ先 → 名前順**（サイドバーと同じ感覚）
pub fn list_directory(resolved: &Resolved) -> Result<Value, Denial> {
    let meta = resolved.path.metadata().map_err(|_| Denial::Unreadable)?;
    if !meta.is_dir() {
        return Err(Denial::NotADirectory);
    }
    let read = std::fs::read_dir(&resolved.path).map_err(|_| Denial::Unreadable)?;
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    // キーの生成は 1 要素 1 回にする（比較ごとに String を作らない）
    names.sort_by_cached_key(|a| a.to_lowercase());

    let mut entries: Vec<Value> = names
        .iter()
        .map(|n| entry_json(n, &resolved.path, &resolved.root_canon))
        .collect();
    entries.sort_by_key(|e| !e["dir"].as_bool().unwrap_or(false));

    let truncated = entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);

    Ok(json!({
        "root": resolved.root_id,
        "root_name": resolved.root_name,
        "path": resolved.rel,
        "entries": entries,
        "truncated": truncated,
    }))
}

// --- ファイル本文 ---

/// テキストとして読めたか / バイナリか
pub struct FileContent {
    pub text: Option<String>,
    pub size: u64,
    pub binary: bool,
}

/// プレビュー用の本文を読む。UTF-8 として解釈できなければ `binary` に倒す
/// （画像・PDF は PWA が download 経由で開く）
pub fn read_content(resolved: &Resolved) -> Result<FileContent, Denial> {
    let meta = resolved.path.metadata().map_err(|_| Denial::Unreadable)?;
    if meta.is_dir() {
        return Err(Denial::NotAFile);
    }
    let size = meta.len();
    if size > MAX_TEXT_BYTES {
        return Ok(FileContent {
            text: None,
            size,
            binary: false,
        });
    }
    let bytes = std::fs::read(&resolved.path).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => Denial::Unreadable,
        std::io::ErrorKind::NotFound => Denial::NotFound,
        _ => Denial::Unreadable,
    })?;
    // NUL を含む = テキストとして出さない（端末・DOM の汚染を避ける）
    if bytes.contains(&0) {
        return Ok(FileContent {
            text: None,
            size,
            binary: true,
        });
    }
    // UTF-8 として解釈できたときだけ本文にする（複製せずそのまま String へ移す）
    match String::from_utf8(bytes) {
        Ok(text) => Ok(FileContent {
            text: Some(text),
            size,
            binary: false,
        }),
        Err(_) => Ok(FileContent {
            text: None,
            size,
            binary: true,
        }),
    }
}

/// content API の応答を組む
pub fn content_payload(resolved: &Resolved, content: &FileContent) -> Value {
    json!({
        "root": resolved.root_id,
        "root_name": resolved.root_name,
        "path": resolved.rel,
        "size": content.size,
        "binary": content.binary,
        "truncated": content.text.is_none() && !content.binary,
        "text": content.text,
    })
}

/// ダウンロード対象として開く。上限を超えていれば断る
pub fn open_for_download(resolved: &Resolved) -> Result<(std::fs::File, u64), Denial> {
    let meta = resolved.path.metadata().map_err(|_| Denial::Unreadable)?;
    if meta.is_dir() {
        return Err(Denial::NotAFile);
    }
    if meta.len() > MAX_DOWNLOAD_BYTES {
        return Err(Denial::TooLarge);
    }
    let file = std::fs::File::open(&resolved.path).map_err(|e| match e.kind() {
        std::io::ErrorKind::PermissionDenied => Denial::Unreadable,
        std::io::ErrorKind::NotFound => Denial::NotFound,
        _ => Denial::Unreadable,
    })?;
    Ok((file, meta.len()))
}

/// `Content-Disposition` の値を組む。
///
/// ファイル名は**ヘッダに載せる前に必ずここを通す**: 生の名前には改行・引用符・
/// 非 ASCII が入りうるので、ASCII 側は安全な文字だけに落とし、
/// 本来の名前は RFC 5987 の `filename*`（UTF-8 パーセント符号化）で渡す
pub fn content_disposition(file_name: &str) -> String {
    let ascii: String = file_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let ascii = if ascii.trim_matches('_').is_empty() {
        "download".to_string()
    } else {
        ascii
    };
    let encoded: String = file_name
        .as_bytes()
        .iter()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_') {
                (*b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

// --- role ---

/// このモジュールが受け持つパスの必要 role を返す（担当外なら None）。
///
/// **読み出しでも Interact 以上**にしてある（#1079）: 画面の閲覧（Observe）と、
/// ファイルの中身を丸ごと持ち出せることは危険度が別物なので、
/// Mac 側で明示的に昇格した端末にだけ許す。
///
/// メソッドは見ない（= 下限だけを言う）。`remote.rs` の `required_role` は
/// **POST の分岐より後**でこれを引くので、未知の POST は従来どおり Manage のまま
pub fn required_role_for(path: &str) -> Option<DeviceRole> {
    if path == "/api/files" || path.starts_with("/api/files/") {
        return Some(DeviceRole::Interact);
    }
    None
}

// --- 監査 ---

/// 監査ログに載せる JSON。**パス・ファイル名・ルート名は 1 つも載せない**
/// （ペイン内容と同基準。#287 P2-2）。何をどれだけ持ち出したかだけを残す
pub fn audit_payload(kind: &str, bytes: u64, entries: usize) -> Value {
    json!({
        "kind": kind,
        "bytes": bytes,
        "entries": entries,
    })
}

/// 監査 JSON に載ってよいキー。番犬テストと `audit_payload` の唯一の正
pub const AUDIT_KEYS: &[&str] = &["kind", "bytes", "entries"];

// --- HTTP の受け口 ---
//
// `remote.rs` 側の変更をルータ登録だけに閉じるため、必要な外部作用は
// 引数のクロージャで受け取る（IPC の送信と監査の追記）。おかげでこの層も
// 実 daemon 無しでテストできる。

/// `remote.rs` から渡される外部作用
pub struct FilesDeps<'a> {
    /// tako app へ IPC で問い合わせる
    pub send: &'a dyn Fn(crate::protocol::Request) -> Result<Value, String>,
    /// 監査ログへ 1 行足す（event, extra）
    pub audit: &'a dyn Fn(&str, Value),
    /// 応答に付ける CORS ヘッダ（`remote.rs` の 1 実装をそのまま使う）
    pub cors: Vec<tiny_http::Header>,
}

fn header(name: &[u8], value: &[u8]) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name, value).expect("固定ヘッダ")
}

/// **ファイル名から組んだ値**のように、コード側で固定でないヘッダ。
/// `content_disposition` は安全文字だけへ落とすので実際には失敗しないが、
/// そこが将来壊れたときに daemon のリクエストスレッドを panic させない
/// （落とし込みが効いていることは
/// `content_dispositionはどんな名前でもヘッダとして組める` が別途固定する）
fn header_or(name: &[u8], value: &str, fallback: &[u8]) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name, value.as_bytes()).unwrap_or_else(|_| header(name, fallback))
}

/// JSON 応答。ファイルの中身はすべて機密扱いなので `no-store, private` を必ず付ける
fn respond_json(request: tiny_http::Request, deps: &FilesDeps, status: u16, body: &Value) {
    let mut resp = tiny_http::Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(header(b"Content-Type", b"application/json"));
    for h in deps.cors.clone() {
        resp = resp.with_header(h);
    }
    resp = resp.with_header(header(b"Cache-Control", b"no-store, private"));
    let _ = request.respond(resp);
}

fn respond_denial(request: tiny_http::Request, deps: &FilesDeps, denial: Denial) {
    respond_json(request, deps, denial.status(), &denial.to_json());
}

/// 現在ツリーに出ているルートを app から取り直す。**毎リクエスト取り直す**のが要点:
/// daemon 側に許可リストを溜めると、Mac 側で閉じたフォルダが読めたままになる
fn current_roots(deps: &FilesDeps) -> Result<Vec<TreeRoot>, String> {
    let payload = (deps.send)(crate::protocol::Request::TreeFolder {
        action: "roots".to_string(),
        path: None,
        tab: None,
        pane: None,
        limit: None,
    })?;
    Ok(roots_from_payload(&payload))
}

/// `/api/files*` の受け口。role の検査は呼び出し側（`remote.rs` の共通経路）が済ませている
pub fn handle_files_request(
    request: tiny_http::Request,
    path: &str,
    url_full: &str,
    deps: &FilesDeps,
) {
    let roots = match current_roots(deps) {
        Ok(r) => r,
        Err(e) => {
            return respond_json(
                request,
                deps,
                503,
                &json!({
                    "error": format!("tako app に問い合わせできません: {e}"),
                    "error_en": format!("Could not reach the tako app: {e}"),
                    "kind": "app_unreachable",
                }),
            );
        }
    };

    let root_param = query_value(url_full, "root");
    let path_param = query_value(url_full, "path").unwrap_or_default();

    match path {
        // ルート一覧（root 省略）またはディレクトリ一覧
        "/api/files" => match root_param {
            None => {
                let body = json!({
                    "roots": roots.iter().map(TreeRoot::to_json).collect::<Vec<_>>(),
                });
                (deps.audit)("files", audit_payload("roots", 0, roots.len()));
                respond_json(request, deps, 200, &body)
            }
            Some(root) => match resolve_in_root(&roots, &root, &path_param)
                .and_then(|r| list_directory(&r).map(|v| (r, v)))
            {
                Ok((_, body)) => {
                    let entries = body["entries"].as_array().map(Vec::len).unwrap_or(0);
                    (deps.audit)("files", audit_payload("list", 0, entries));
                    respond_json(request, deps, 200, &body)
                }
                Err(d) => respond_denial(request, deps, d),
            },
        },

        // プレビュー用の本文
        "/api/files/content" => {
            let Some(root) = root_param else {
                return respond_denial(request, deps, Denial::UnknownRoot);
            };
            match resolve_in_root(&roots, &root, &path_param)
                .and_then(|r| read_content(&r).map(|c| (r, c)))
            {
                Ok((resolved, content)) => {
                    let sent = content.text.as_ref().map(|t| t.len() as u64).unwrap_or(0);
                    (deps.audit)("files", audit_payload("content", sent, 0));
                    respond_json(request, deps, 200, &content_payload(&resolved, &content))
                }
                Err(d) => respond_denial(request, deps, d),
            }
        }

        // ダウンロード（ストリーミング）
        "/api/files/download" => {
            let Some(root) = root_param else {
                return respond_denial(request, deps, Denial::UnknownRoot);
            };
            match resolve_in_root(&roots, &root, &path_param)
                .and_then(|r| open_for_download(&r).map(|f| (r, f)))
            {
                Ok((resolved, (file, size))) => {
                    let name = resolved
                        .path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_else(|| "download".to_string());
                    (deps.audit)("files", audit_payload("download", size, 0));
                    let mut resp = tiny_http::Response::from_file(file)
                        .with_header(header(b"Content-Type", b"application/octet-stream"))
                        .with_header(header_or(
                            b"Content-Disposition",
                            &content_disposition(&name),
                            b"attachment",
                        ));
                    for h in deps.cors.clone() {
                        resp = resp.with_header(h);
                    }
                    resp = resp.with_header(header(b"Cache-Control", b"no-store, private"));
                    let _ = request.respond(resp);
                }
                Err(d) => respond_denial(request, deps, d),
            }
        }

        _ => respond_json(
            request,
            deps,
            404,
            &json!({ "error": "API エンドポイントが見つからない" }),
        ),
    }
}

/// クエリ 1 個を取り出す（`remote.rs` の `query_param` と同じ意味論。
/// このモジュールを単体でテストできるよう写しを持つ）
fn query_value(url: &str, key: &str) -> Option<String> {
    let qs = url.split_once('?')?.1;
    for pair in qs.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == key {
            return Some(percent_decode(v));
        }
    }
    None
}

/// `%XX` と `+` を戻す。**認可の検査より前に**必ず通す
/// （符号化したまま検査すると `%2e%2e` が `..` を素通りさせる）
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- テスト用の実ファイル木 ---
    //
    // 実パスは環境ごとに違うので、期待値には**一時ディレクトリからの相対位置**しか書かない
    // （実ユーザー名・実ホームパスをリポジトリへ入れない。#927）

    struct Fixture {
        dir: PathBuf,
        roots: Vec<TreeRoot>,
    }

    impl Fixture {
        /// `root/`（公開）と `outside/`（非公開）を作り、`root/escape` を
        /// `outside` への symlink にする
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "tako-1079-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let root = dir.join("root");
            let outside = dir.join("outside");
            std::fs::create_dir_all(root.join("sub")).expect("root/sub");
            std::fs::create_dir_all(&outside).expect("outside");
            std::fs::write(root.join("hello.txt"), "こんにちは\n").expect("hello");
            std::fs::write(root.join("sub").join("nested.txt"), "nested\n").expect("nested");
            std::fs::write(outside.join("secret.txt"), "SECRET\n").expect("secret");
            #[cfg(unix)]
            std::os::unix::fs::symlink(&outside, root.join("escape")).expect("symlink");

            let payload = json!({
                "tabs": [{
                    "tab": 1,
                    "title": "テスト",
                    "roots": [root.display().to_string()],
                }]
            });
            let roots = roots_from_payload(&payload);
            assert_eq!(roots.len(), 1, "ルートが 1 件組めていること");
            Self { dir, roots }
        }

        fn root_id(&self) -> &str {
            &self.roots[0].id
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            // 一時ディレクトリ配下であることを確かめてから消す（#511 の事故の教訓）
            assert!(
                self.dir.starts_with(std::env::temp_dir()),
                "一時ディレクトリ配下以外は消さない"
            );
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    // --- 受け入れ条件 1: ツリー外を 403 で拒否（総当たり） ---

    /// 「ツリーの外を指す」あらゆる形。**1 つでも通れば認可が破れている**。
    /// FS に依らない形（絶対 / traversal）はホスト OS に関係なく落ちる必要がある
    const ESCAPE_ATTEMPTS: &[(&str, Denial)] = &[
        // 素の traversal
        ("..", Denial::Traversal),
        ("../", Denial::Traversal),
        ("../outside/secret.txt", Denial::Traversal),
        ("sub/../../outside/secret.txt", Denial::Traversal),
        ("./../../outside/secret.txt", Denial::Traversal),
        ("a/b/c/../../../../outside/secret.txt", Denial::Traversal),
        // Windows 区切りでの traversal（POSIX 上でも落とす）
        ("..\\outside\\secret.txt", Denial::Traversal),
        ("sub\\..\\..\\outside", Denial::Traversal),
        // POSIX 絶対パス
        ("/etc/passwd", Denial::AbsolutePath),
        ("/", Denial::AbsolutePath),
        ("//etc/passwd", Denial::AbsolutePath),
        // Windows ドライブ / UNC
        ("C:\\Windows\\System32\\config\\SAM", Denial::AbsolutePath),
        ("c:/Windows", Denial::AbsolutePath),
        ("C:relative", Denial::AbsolutePath),
        ("\\\\server\\share\\x", Denial::AbsolutePath),
        ("\\etc\\passwd", Denial::AbsolutePath),
        // 先頭でない位置のドライブ形（`PathBuf::push` が Windows で全部捨てる）
        ("sub/C:/Windows", Denial::AbsolutePath),
        ("sub\\D:x", Denial::AbsolutePath),
        ("a/b/Z:", Denial::AbsolutePath),
        // 制御文字（ヘッダ・ログ汚染）
        ("a\nb", Denial::Traversal),
        ("a\rb", Denial::Traversal),
        ("a\0b", Denial::Traversal),
    ];

    #[test]
    fn ツリー外を指すあらゆる形が拒否される() {
        let fx = Fixture::new("escape");
        for (attempt, expected) in ESCAPE_ATTEMPTS {
            let got = resolve_in_root(&fx.roots, fx.root_id(), attempt);
            let err = got
                .as_ref()
                .err()
                .copied()
                .unwrap_or_else(|| panic!("素通りした: {attempt:?}"));
            assert_eq!(err, *expected, "拒否の理由が違う: {attempt:?}");
            assert_eq!(err.status(), 403, "認可の失敗は 403: {attempt:?}");
        }
    }

    #[test]
    fn 符号化された形も復号してから検査される() {
        // `%2e%2e` を復号せずに検査すると traversal が素通りする
        for raw in [
            "/api/files?root=r&path=%2e%2e%2fsecret.txt",
            "/api/files?root=r&path=..%2Fsecret.txt",
            "/api/files?root=r&path=%2Fetc%2Fpasswd",
        ] {
            let decoded = query_value(raw, "path").expect("path が取れる");
            assert!(
                check_relative_shape(&decoded).is_err(),
                "復号後に拒否されるべき: {raw}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn 一覧はルート外を指すsymlinkに印を付ける() {
        let fx = Fixture::new("escapemark");
        let listed =
            list_directory(&resolve_in_root(&fx.roots, fx.root_id(), "").unwrap()).unwrap();
        let entries = listed["entries"].as_array().unwrap();
        let escape = entries
            .iter()
            .find(|e| e["name"] == "escape")
            .expect("symlink が一覧に出る");
        assert_eq!(escape["symlink"], true);
        assert_eq!(
            escape["escapes_root"], true,
            "押しても 403 になる行だと分かる印が要る"
        );
        // ルート内の普通のフォルダには印を付けない
        let sub = entries.iter().find(|e| e["name"] == "sub").unwrap();
        assert_eq!(sub["escapes_root"], false);
        assert_eq!(sub["symlink"], false);
    }

    #[cfg(unix)]
    #[test]
    fn ルート内のsymlinkが外を指していたら拒否される() {
        let fx = Fixture::new("symlink");
        // symlink 自身も、その先のファイルも読めてはいけない
        for attempt in ["escape", "escape/secret.txt"] {
            let err = resolve_in_root(&fx.roots, fx.root_id(), attempt)
                .err()
                .unwrap_or_else(|| panic!("symlink 越えが素通りした: {attempt}"));
            assert_eq!(err, Denial::EscapesRoot, "attempt={attempt}");
            assert_eq!(err.status(), 403);
        }
    }

    #[test]
    fn ツリーに出ていないルートは拒否される() {
        let fx = Fixture::new("unknown");
        for bogus in [
            "",
            "deadbeefdead",
            "../",
            fx.root_id().to_uppercase().as_str(),
        ] {
            let err = resolve_in_root(&fx.roots, bogus, "hello.txt")
                .err()
                .unwrap_or_else(|| panic!("未知のルートが素通りした: {bogus:?}"));
            assert_eq!(err, Denial::UnknownRoot, "bogus={bogus:?}");
            assert_eq!(err.status(), 403);
        }
    }

    #[test]
    fn ルートが消えていれば読めなくなる() {
        let fx = Fixture::new("gone");
        // 「ツリーに出ている」ことが認可の正なので、実体が消えれば即座に読めない
        assert!(resolve_in_root(&fx.roots, fx.root_id(), "hello.txt").is_ok());
        std::fs::remove_dir_all(fx.dir.join("root")).expect("root を消す");
        assert_eq!(
            resolve_in_root(&fx.roots, fx.root_id(), "hello.txt").err(),
            Some(Denial::UnknownRoot)
        );
    }

    #[test]
    fn 配下のファイルとフォルダは読める() {
        let fx = Fixture::new("allow");
        for ok in [
            "",
            ".",
            "hello.txt",
            "sub",
            "sub/nested.txt",
            "./sub/nested.txt",
        ] {
            let r = resolve_in_root(&fx.roots, fx.root_id(), ok)
                .unwrap_or_else(|e| panic!("読めるはず: {ok:?} -> {e:?}"));
            assert!(r
                .path
                .starts_with(fx.dir.join("root").canonicalize().unwrap()));
        }
        let listed =
            list_directory(&resolve_in_root(&fx.roots, fx.root_id(), "").unwrap()).unwrap();
        let names: Vec<&str> = listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"hello.txt"), "names={names:?}");
        assert!(names.contains(&"sub"), "names={names:?}");
        // フォルダはすべてファイルより前に並ぶ
        let dirs: Vec<bool> = listed["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["dir"].as_bool().unwrap())
            .collect();
        let first_file = dirs.iter().position(|d| !d).unwrap_or(dirs.len());
        assert!(
            dirs[first_file..].iter().all(|d| !d),
            "フォルダとファイルが混ざっている: {names:?}"
        );
    }

    #[test]
    fn 本文とダウンロードが読める() {
        let fx = Fixture::new("read");
        let resolved = resolve_in_root(&fx.roots, fx.root_id(), "hello.txt").unwrap();
        let content = read_content(&resolved).unwrap();
        assert_eq!(content.text.as_deref(), Some("こんにちは\n"));
        assert!(!content.binary);
        let (_, size) = open_for_download(&resolved).unwrap();
        assert_eq!(size, "こんにちは\n".len() as u64);
        // ディレクトリは本文にもダウンロードにもならない
        let dir = resolve_in_root(&fx.roots, fx.root_id(), "sub").unwrap();
        assert_eq!(read_content(&dir).err(), Some(Denial::NotAFile));
        assert_eq!(open_for_download(&dir).err(), Some(Denial::NotAFile));
        // ファイルは一覧にならない
        assert_eq!(list_directory(&resolved).err(), Some(Denial::NotADirectory));
    }

    #[test]
    fn バイナリは本文を返さない() {
        let fx = Fixture::new("binary");
        std::fs::write(fx.dir.join("root").join("bin.dat"), [0u8, 1, 2, 255]).unwrap();
        let resolved = resolve_in_root(&fx.roots, fx.root_id(), "bin.dat").unwrap();
        let content = read_content(&resolved).unwrap();
        assert!(content.binary);
        assert!(content.text.is_none(), "バイナリの本文は返さない");
    }

    // --- 受け入れ条件 2: role ---

    #[test]
    fn ファイルapiはinteract以上を要求する() {
        for p in [
            "/api/files",
            "/api/files/content",
            "/api/files/download",
            "/api/files/anything",
        ] {
            assert_eq!(required_role_for(p), Some(DeviceRole::Interact), "path={p}");
        }
        // Observe では足りない = 403 になる
        assert!(
            DeviceRole::Observe < DeviceRole::Interact,
            "Observe では足りない"
        );
        assert!(DeviceRole::Interact >= DeviceRole::Interact);
        assert!(DeviceRole::Manage >= DeviceRole::Interact);
        assert!(DeviceRole::Admin >= DeviceRole::Interact);
    }

    #[test]
    fn 未知のメソッドの下限は言わない() {
        // このモジュールが言うのは「下限は Interact」だけ。
        // POST / PUT を安全側（Manage）に倒すかは `remote.rs` の順序が決める（#1079）
        assert_eq!(required_role_for("/api/files"), Some(DeviceRole::Interact));
        assert!(
            DeviceRole::Manage > DeviceRole::Interact,
            "Manage の方が強い"
        );
    }

    #[test]
    fn 担当外のパスには口を出さない() {
        for p in [
            "/api/panes",
            "/api/health",
            "/api/filesystem",
            "/api/file",
            "/api/upload",
        ] {
            assert_eq!(required_role_for(p), None, "path={p}");
        }
    }

    // --- 受け入れ条件 4: 監査にパスが出ない ---

    #[test]
    fn audit_payloadにパスが混ざらない() {
        // 実パスに現れうる特徴的な断片を入れても、監査 JSON には 1 バイトも出ない
        let payload = audit_payload("download", 4096, 0);
        let serialized = payload.to_string();
        for marker in ["/", "\\", ".txt", "root", "path", "name", "file"] {
            assert!(
                !serialized.contains(marker),
                "監査 JSON にパスらしき断片が出た: {marker:?} in {serialized}"
            );
        }
        let keys: std::collections::BTreeSet<&str> = payload
            .as_object()
            .expect("オブジェクト")
            .keys()
            .map(String::as_str)
            .collect();
        let allowed: std::collections::BTreeSet<&str> = AUDIT_KEYS.iter().copied().collect();
        assert_eq!(keys, allowed, "監査キーは許可リストと一致すること");
    }

    #[test]
    fn audit_payloadは持ち出し量だけを残す() {
        let p = audit_payload("content", 1234, 0);
        assert_eq!(p["kind"], "content");
        assert_eq!(p["bytes"], 1234);
        assert_eq!(p["entries"], 0);
    }

    // --- 純粋関数の性質 ---

    #[test]
    fn is_withinは兄弟を配下と誤認しない() {
        assert!(is_within(Path::new("/a/b"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/b"), Path::new("/a/b/c")));
        // 文字列の前方一致だと通ってしまう形
        assert!(!is_within(Path::new("/a/b"), Path::new("/a/bc")));
        assert!(!is_within(Path::new("/a/b"), Path::new("/a/b-x/y")));
        assert!(!is_within(Path::new("/a/b"), Path::new("/a")));
    }

    #[test]
    fn 空と単なるドットは配下として許す() {
        assert!(check_relative_shape("").is_ok());
        assert!(check_relative_shape(".").is_ok());
        assert!(check_relative_shape("./a/b").is_ok());
        assert!(
            check_relative_shape("a..b").is_ok(),
            "名前の中の .. は traversal ではない"
        );
        assert!(check_relative_shape("..a").is_ok());
        assert!(check_relative_shape("a/..b/c").is_ok());
    }

    #[test]
    fn content_dispositionはヘッダを壊せない() {
        // 改行・引用符・セミコロンを入れてもヘッダの構造が壊れない
        let evil = "a\"; drop\r\nX-Evil: 1\r\n\r\n.txt";
        let got = content_disposition(evil);
        assert!(!got.contains('\r') && !got.contains('\n'), "got={got}");
        assert!(!got.contains("X-Evil: 1;"), "got={got}");
        // ASCII 側は安全文字だけ
        let ascii = got
            .split('"')
            .nth(1)
            .expect("filename=\"...\" がある")
            .to_string();
        assert!(
            ascii
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
            "ascii={ascii}"
        );
        // 非 ASCII は filename* に UTF-8 で載る
        let jp = content_disposition("報告書.pdf");
        assert!(jp.contains("filename*=UTF-8''"), "jp={jp}");
        assert!(jp.contains("%E5%A0%B1"), "jp={jp}");
        // 安全文字が 1 つも無い名前でも空にならない
        assert!(content_disposition("///").contains("\"download\""));
    }

    #[test]
    fn content_dispositionはどんな名前でもヘッダとして組める() {
        // 落とし込みが効いていれば tiny_http が必ず受理する = fallback へ落ちない
        let long = "x".repeat(300);
        for name in [
            "ok.txt",
            "報告書.pdf",
            "a\"; X-Evil: 1\r\n\r\n.txt",
            "\u{0}\u{1}\u{7f}",
            "///",
            "スペース あり.txt",
            "emoji-\u{1F600}.png",
            long.as_str(),
        ] {
            let value = content_disposition(name);
            assert!(
                tiny_http::Header::from_bytes(&b"Content-Disposition"[..], value.as_bytes())
                    .is_ok(),
                "ヘッダとして組めない: name={name:?} value={value}"
            );
        }
    }

    #[test]
    fn percent_decodeは壊れた入力でも落ちない() {
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("a+b"), "a b");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("%2"), "%2");
        assert_eq!(percent_decode(""), "");
        assert_eq!(percent_decode("%E6%97%A5"), "日");
    }

    #[test]
    fn ルート一覧はidを重複させず同じパスを二重に出さない() {
        let payload = json!({
            "tabs": [
                { "tab": 1, "title": "A", "roots": ["/w/one", "/w/two"] },
                // 別タブに同じパスが出ていても 1 件だけ
                { "tab": 2, "title": "B", "roots": ["/w/one", "/w/three"] },
            ]
        });
        let roots = roots_from_payload(&payload);
        assert_eq!(roots.len(), 3, "{roots:?}");
        let ids: std::collections::HashSet<&str> = roots.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "id が重複している: {roots:?}");
        assert_eq!(roots[0].name, "one");
        assert_eq!(roots[0].tab, 1);
        assert_eq!(roots[2].tab, 2);
        // 空・壊れた payload でも落ちない
        assert!(roots_from_payload(&json!({})).is_empty());
        assert!(roots_from_payload(&json!({ "tabs": [{ "tab": 1 }] })).is_empty());
    }

    #[test]
    fn root_idは決定的で12桁() {
        let a = root_id_of("/w/project");
        assert_eq!(a.len(), 12);
        assert_eq!(a, root_id_of("/w/project"));
        assert_ne!(a, root_id_of("/w/project2"));
        // パスの断片が id から復元できない（URL に絶対パスを載せない目的）
        assert!(!a.contains("project"));
    }
}
