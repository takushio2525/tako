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
            // SSH 先ルート（#1085）と 1 本の一覧に並ぶので、どちら側かを明示する
            "ssh": false,
        })
    }
}

/// パスから 12 桁の id を作る（FNV-1a 64bit）。
///
/// 暗号学的強度は要らない: id は秘密ではなく、認可は
/// 「その id が**今の**ルート一覧に在るか」の照合で行うため。
/// 万一衝突しても `roots_from_payload` が接尾辞で分離するので取り違えは起きない
pub fn root_id_of(path: &str) -> String {
    format!("{:012x}", fnv1a64(path.as_bytes()))[..12].to_string()
}

/// FNV-1a 64bit。id（`root_id_of`）と検証子（`content_etag`）が同じ 1 実装を通る。
///
/// 暗号学的強度は**どちらの用途でも要らない**（理由はそれぞれの doc に書いた）
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
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

/// content API の応答を組む。
///
/// `etag` は**書き込みで返してもらう検証子**（#1084）。本文を返せなかったとき
/// （バイナリ・大きすぎる）は付けない = 検証子が無いものは書き込めない
pub fn content_payload(resolved: &Resolved, content: &FileContent) -> Value {
    json!({
        "root": resolved.root_id,
        "root_name": resolved.root_name,
        "path": resolved.rel,
        "size": content.size,
        "binary": content.binary,
        "truncated": content.text.is_none() && !content.binary,
        "text": content.text,
        "etag": content.text.as_ref().map(|t| content_etag(t.as_bytes())),
        // ローカルのファイルは SSH 先と違って「相手が落ちている」状態が無い
        "ssh": false,
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

// ============================================================================
// 競合検知の検証子（#1084。リモート刷新 柱 3-F）
// ============================================================================

/// 内容から作る検証子（HTTP の ETag 相当）。読み出しの応答に載せ、
/// 書き込みでは**それをそのまま返してもらう**ことで「スマホが読んだ時点から
/// 中身が変わっていないか」を判定する。
///
/// # なぜスマホに計算させないか
///
/// 指紋をスマホ側で作る形にすると、同じ算法を JavaScript と Rust の両方へ
/// 実装することになり、**片方だけ壊れたときに「競合を見落とす」側へ倒れる**。
/// daemon が作った値を預けて返させるだけなら算法は Rust の中で完結し、
/// 変えたいときも 1 箇所で済む。
///
/// # なぜ暗号学的ハッシュでないか
///
/// これが守るのは「他の誰かの変更を**うっかり**踏み潰さないこと」で、偽造への
/// 耐性は要らない: 検証子を偽れる端末は Interact 以上なので、そもそも本文に
/// 何を書いても通る（偽造で新たに得られる権限が無い）。必要なのは
/// **偶然の一致が起きないこと**だけなので、長さと 64bit の内容ハッシュを併記する
pub fn content_etag(bytes: &[u8]) -> String {
    format!("{}-{:016x}", bytes.len(), fnv1a64(bytes))
}

// ============================================================================
// SSH 先のルート（#1085。リモート刷新 柱 3-G）
// ============================================================================

/// SSH 先ルートの id 接頭辞。
///
/// ローカルのルート id は 16 進数（+ 衝突時の `-N`）なので `s` で始まることは無く、
/// **id を見ただけでどちら側かが決まる**。取り違えて別の側の認可を使う形にしない
pub const SSH_ID_PREFIX: &str = "s-";

/// `remote-folder open` 済みの SSH 先フォルダ 1 件（ツリーに出ているリモートルート）。
///
/// ローカルの [`TreeRoot`] と対で、**どちらも「Mac の画面に現に出ているもの」**が
/// 認可の正。SSH 側の一覧は `RemoteFolder { action: "list" }` を毎リクエスト引くので、
/// Mac 側でフォルダを閉じればその瞬間に読めなくなる
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshRoot {
    pub id: String,
    pub host: String,
    /// リモート側の絶対パス（Windows の相手は `/C:/Users/...` の形で来る）
    pub path: String,
    /// 表示名（末尾のフォルダ名）
    pub name: String,
    pub tab: u64,
    pub tab_title: String,
    /// ControlMaster が生きているか（#919 の `connected`。false = 切断中）
    pub connected: bool,
    /// ツリー側の読み込み状態（loaded / loading / pending / error: … / なし）
    pub state: Option<String>,
    /// ローカルルートの前か後ろか（#1041。`leading` / `trailing`）。
    /// **並び規則は app の答えをそのまま使う**（daemon 側で二重に決めない）
    pub placement: Option<String>,
}

impl SshRoot {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "tab": self.tab,
            "tab_title": self.tab_title,
            // ここから下が SSH 先ルートだけに付く（PWA はこれでバッジを出す）
            "ssh": true,
            "host": self.host,
            "connected": self.connected,
            "state": self.state,
        })
    }
}

/// ホストとリモートパスから id を作る
pub fn ssh_root_id_of(host: &str, path: &str) -> String {
    let key = format!("{host}\u{0}{path}");
    format!(
        "{SSH_ID_PREFIX}{}",
        &format!("{:012x}", fnv1a64(key.as_bytes()))[..12]
    )
}

/// `RemoteFolder { action: "list" }` の応答から SSH 先ルート一覧を組む。
///
/// 同じ (host, path) が複数タブに出ていたら**先に出たものだけ**を残す
/// （配下は同じなので二重に見せる意味がない。[`roots_from_payload`] と同じ方針）
pub fn ssh_roots_from_payload(payload: &Value) -> Vec<SshRoot> {
    let mut out: Vec<SshRoot> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let Some(tabs) = payload["tabs"].as_array() else {
        return out;
    };
    for tab in tabs {
        let tab_id = tab["tab"].as_u64().unwrap_or(0);
        let tab_title = tab["title"].as_str().unwrap_or_default().to_string();
        let Some(folders) = tab["remote_folders"].as_array() else {
            continue;
        };
        for folder in folders {
            let (Some(host), Some(path)) = (folder["host"].as_str(), folder["path"].as_str())
            else {
                continue;
            };
            if host.is_empty() || path.is_empty() {
                continue;
            }
            let key = format!("{host}\u{0}{path}");
            if !seen.insert(key) {
                continue;
            }
            let base = ssh_root_id_of(host, path);
            let mut id = base.clone();
            let mut n = 1;
            while out.iter().any(|r| r.id == id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            out.push(SshRoot {
                id,
                host: host.to_string(),
                path: path.to_string(),
                name: tako_core::remote_fs::base_name(path),
                tab: tab_id,
                tab_title: tab_title.clone(),
                connected: folder["connected"].as_bool().unwrap_or(false),
                state: folder["state"].as_str().map(str::to_string),
                placement: folder["placement"].as_str().map(str::to_string),
            });
        }
    }
    out
}

/// SSH 先の解決済みパス（開いているリモートルートの配下であることが確認済み）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshResolved {
    pub root_id: String,
    pub root_name: String,
    pub host: String,
    /// リモート側の絶対パス（ルート + 相対パス）
    pub path: String,
    /// ルートからの相対パス（`/` 区切り）
    pub rel: String,
    pub connected: bool,
}

/// SSH 先ルートの配下として `rel` を解決する。
///
/// # 何で閉じているか（ローカルとの違い）
///
/// ローカル（[`resolve_in_root`]）は `canonicalize` で**実体**を見て配下判定できるが、
/// リモートには安く実体を問う手段が無い（sftp の 1 往復が増える）。ここでは
/// ①[`check_relative_shape`] が絶対パス・`..`・制御文字を落とし
/// ②残った要素を 1 つずつ継ぐ、の 2 段で**文字列として**ルート配下に閉じる。
///
/// **相手側の symlink がルートの外を指している場合は追える**（追わない手段が無い）。
/// これは Mac のツリー自身が同じホストに対して持っている権限と同じで、
/// 受容するリスクとして `.agent/threat-model-remote.md` に明記してある。
/// 書き込み側はさらに #966 が「開いた記録が無ければ書かない」で閉じている
pub fn resolve_in_ssh_root(
    roots: &[SshRoot],
    root_id: &str,
    rel: &str,
) -> Result<SshResolved, Denial> {
    let root = roots
        .iter()
        .find(|r| r.id == root_id)
        .ok_or(Denial::UnknownRoot)?;
    check_relative_shape(rel)?;
    let parts: Vec<&str> = rel
        .split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    let mut path = root.path.clone();
    for part in &parts {
        path = tako_core::remote_fs::join_remote(&path, part);
    }
    Ok(SshResolved {
        root_id: root.id.clone(),
        root_name: root.name.clone(),
        host: root.host.clone(),
        path,
        rel: parts.join("/"),
        connected: root.connected,
    })
}

/// 要求されたルートがローカルか SSH 先か（id の接頭辞で決まる）
pub fn is_ssh_root_id(root_id: &str) -> bool {
    root_id.starts_with(SSH_ID_PREFIX)
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

/// このモジュールが受け持つ**書き込み系**のパス（`PUT` / `POST`）。
///
/// `remote.rs` の `required_role` は「未知の POST は Manage」で安全側に倒しており、
/// その規則を崩さないために**パスを明示列挙**する（`required_role_for` のような
/// 接頭辞一致にすると、将来足した `/api/files/*` の POST が意図せず
/// Interact へ緩む）。役割は保存も再送も同じ Interact = `/api/upload` と同じ基準:
/// 「ファイルを書き換えられる端末」は Mac 側で明示的に昇格したものだけ
pub const WRITE_PATHS: &[&str] = &["/api/files/content", "/api/files/push"];

/// [`WRITE_PATHS`] に載っているか
pub fn is_write_path(path: &str) -> bool {
    WRITE_PATHS.contains(&path)
}

// ============================================================================
// 書き込みが通らなかった理由（#1084 / #1085）
// ============================================================================

/// 書き込みの失敗。[`Denial`] と違って**動的な詳細**（リモートのエラー本文・
/// 退避の有無）を持てる。
///
/// 応答のキーは読み出し側（[`Denial::to_json`]）と揃えてあるので、PWA は
/// `kind` の分岐 1 本でどちらの失敗も扱える
#[derive(Debug, Clone)]
pub struct WriteFailure {
    pub status: u16,
    pub kind: String,
    pub ja: String,
    pub en: String,
    /// 応答へ足す追加フィールド（`pending` など）
    pub extra: Value,
}

impl WriteFailure {
    fn new(status: u16, kind: &str, ja: impl Into<String>, en: impl Into<String>) -> Self {
        Self {
            status,
            kind: kind.to_string(),
            ja: ja.into(),
            en: en.into(),
            extra: Value::Null,
        }
    }

    /// 読んだ時点から中身が変わっている。**上書きしない**
    pub fn conflict(detail: &str) -> Self {
        let mut ja = "このファイルは他で変更されています。読み直してから編集をやり直してください"
            .to_string();
        let mut en = "This file changed elsewhere; reload it and redo your edit".to_string();
        if !detail.is_empty() {
            ja.push_str(&format!("（{detail}）"));
            en.push_str(&format!(" ({detail})"));
        }
        Self::new(409, "conflict", ja, en)
    }

    /// Mac 側に未保存の編集がある。**踏み潰さない**
    pub fn busy_editing() -> Self {
        Self::new(
            409,
            "busy_editing",
            "Mac 側にこのファイルの未保存の編集があります。先に Mac で保存するか編集を取り消してください",
            "The Mac has unsaved edits to this file; save or discard them there first",
        )
    }

    /// テキストとして編集できない（バイナリ・画像・PDF・動画）
    pub fn not_text() -> Self {
        Self::new(
            400,
            "not_text",
            "この形式はスマホから編集できません（テキストファイルだけが編集できます）",
            "This format cannot be edited from the phone (text files only)",
        )
    }

    /// リモート側が読み取り専用（#966 の `read_only`）
    pub fn read_only() -> Self {
        Self::new(
            403,
            "read_only",
            "書き込みが許可されていないファイルです",
            "This file is not writable",
        )
    }

    /// 検証子が付いていない = 競合を判定できないので書かない
    pub fn missing_etag() -> Self {
        Self::new(
            400,
            "missing_etag",
            "検証子（etag）が要ります。ファイルを読み直してから保存してください",
            "An etag is required; reload the file before saving",
        )
    }

    pub fn bad_body(detail: &str) -> Self {
        Self::new(
            400,
            "bad_body",
            format!("リクエストの本文が読めません（{detail}）"),
            format!("Could not read the request body ({detail})"),
        )
    }

    pub fn too_large(limit: u64) -> Self {
        Self::new(
            413,
            "too_large",
            format!("{limit} バイトを超える内容は保存できません"),
            format!("Cannot save more than {limit} bytes"),
        )
    }

    /// tako app 側が断った（プレビューにできない・編集を開始できない等）。
    /// **理由をそのまま渡す**（daemon で言い換えると原因が消える）
    pub fn app(detail: &str) -> Self {
        Self::new(
            409,
            "app_rejected",
            format!("Mac 側で保存できませんでした（{detail}）"),
            format!("The Mac could not save it ({detail})"),
        )
    }

    pub fn app_unreachable(detail: &str) -> Self {
        Self::new(
            503,
            "app_unreachable",
            format!("tako app に問い合わせできません: {detail}"),
            format!("Could not reach the tako app: {detail}"),
        )
    }

    /// リモートへ押し出せず**退避された**（#966）。切断中の保存が無言で消えない
    pub fn remote_pending(kind: &str, detail: &str) -> Self {
        let mut f = Self::new(
            502,
            "remote_pending",
            format!(
                "リモートへ送れませんでした。内容は退避したので、つながったら送り直せます（{detail}）"
            ),
            format!("Could not reach the host; the content is stashed and can be pushed again once connected ({detail})"),
        );
        f.extra = json!({ "pending": true, "remote_kind": kind });
        f
    }

    pub fn to_json(&self) -> Value {
        let mut out = json!({
            "error": self.ja,
            "error_en": self.en,
            "kind": self.kind,
        });
        if let Some(extra) = self.extra.as_object() {
            for (k, v) in extra {
                out[k] = v.clone();
            }
        }
        out
    }
}

impl From<Denial> for WriteFailure {
    fn from(d: Denial) -> Self {
        Self {
            status: d.status(),
            kind: d.kind().to_string(),
            ja: d.message_ja().to_string(),
            en: d.message_en().to_string(),
            extra: Value::Null,
        }
    }
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

/// 現在ツリーに出ている SSH 先フォルダを app から取り直す（#1085）。
///
/// ローカルと同じ理由で**毎リクエスト**引く: Mac 側でフォルダを閉じれば
/// その瞬間に読めなくなる（daemon 側に許可リストを溜めない）
fn current_ssh_roots(deps: &FilesDeps) -> Result<Vec<SshRoot>, String> {
    let payload = (deps.send)(crate::protocol::Request::RemoteFolder {
        action: "list".to_string(),
        host: None,
        path: None,
        tab: None,
        focus: None,
        all: false,
        force: false,
        enabled: None,
        terminal: None,
    })?;
    Ok(ssh_roots_from_payload(&payload))
}

// ============================================================================
// 書き込み（#1084 / #1085）
// ============================================================================
//
// # なぜ daemon が自分でファイルへ書かないか
//
// 書き戻しの難しいところ（アトミックな置き換え・**内容そのもの**での競合検知・
// mode の復元・押し出せなかった保存の退避）は #966 が PC 側に実装してある。
// daemon が自分で書くとその保証を 2 つ持つことになり、**片方だけ直る**形になる。
// ここでは PC 側の編集経路（`PreviewEdit` → `PreviewApply` → `PreviewSave`）を
// そのまま通し、daemon は「認可」と「スマホが読んだ時点との突き合わせ」だけを持つ。
//
// # 競合検知が 2 段あるのはなぜか（窓が違う）
//
// - **スマホが読んだ → 保存を送った**の窓: 検証子（[`content_etag`]）で見る。
//   PC 側はこの窓を見られない（編集セッションの基準は「編集を始めた時点」なので、
//   保存の直前に開くと必ず「変わっていない」に見える）
// - **適用した → 書いた**の窓: PC 側の `TextBuffer::save`（ローカル）と
//   `remote_fs::save_file`（リモート）が見る
//
// どちらか一方だと素通りする組み合わせが残るので、両方通す。

/// 書き込み対象として用意できたプレビューペイン
struct PreviewTarget {
    pane: u64,
    /// プレビューの種別（code / markdown / image / pdf / video）
    mode: String,
    /// 呼ぶ前から編集モードだったか（**後で戻すため**に覚えておく）
    was_editing: bool,
    dirty: bool,
}

/// そのパスを既にプレビューしているペインを探す。
///
/// 見つかれば `OpenFile` を呼ばずに済む = **Mac のレイアウトを触らない**
/// （スマホからの保存でユーザーのプレビューペインが差し替わるのを避ける）
fn find_preview_pane(deps: &FilesDeps, abs: &str) -> Result<Option<(u64, String)>, String> {
    let list = (deps.send)(crate::protocol::Request::List)?;
    let Some(tabs) = list["tabs"].as_array() else {
        return Ok(None);
    };
    for tab in tabs {
        let Some(panes) = tab["panes"].as_array() else {
            continue;
        };
        for pane in panes {
            if same_file(pane["preview"]["path"].as_str(), abs) {
                if let Some(id) = pane["id"].as_u64() {
                    let mode = pane["preview"]["mode"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    return Ok(Some((id, mode)));
                }
            }
        }
    }
    Ok(None)
}

/// app が言うプレビューのパスと、認可で解決した実体が**同じファイルか**。
///
/// 文字列の一致では取りこぼす: `OpenFile` 経由のペインは canonicalize 済みのパスを
/// 持つが、他の経路（GUI のツリーから開いた等）は素のパスのことがあり、macOS では
/// `/var/...` と `/private/var/...` のように**同じファイルが別の文字列**になる。
/// 取りこぼすと既存のペインを見つけられず `OpenFile` を呼ぶので、
/// **ユーザーが見ているプレビューを差し替えてしまう**
fn same_file(reported: Option<&str>, abs: &str) -> bool {
    let Some(reported) = reported else {
        return false;
    };
    if reported == abs {
        return true;
    }
    match (
        std::path::Path::new(reported).canonicalize(),
        std::path::Path::new(abs).canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// 編集モードの現在状態を読む（`enabled` 省略 = 状態取得だけ。
/// **編集セッションを作らない**ので、これ自体は Mac 側の状態を変えない）
fn preview_edit_state(deps: &FilesDeps, pane: u64) -> Result<(bool, bool), String> {
    let v = (deps.send)(crate::protocol::Request::PreviewEdit {
        pane: Some(pane),
        enabled: None,
    })?;
    Ok((
        v["editing"].as_bool().unwrap_or(false),
        v["dirty"].as_bool().unwrap_or(false),
    ))
}

/// ローカルのファイルを載せたプレビューペインを用意する
fn ensure_local_preview(deps: &FilesDeps, abs: &str) -> Result<PreviewTarget, WriteFailure> {
    let existing = find_preview_pane(deps, abs).map_err(|e| WriteFailure::app_unreachable(&e))?;
    let (pane, mode) = match existing {
        // 既にそのファイルを出しているペインがあれば**そのまま使う**
        // （`OpenFile` を通すと、同じタブの他のプレビューを差し替えてしまう）
        Some(found) => found,
        None => {
            let opened = (deps.send)(crate::protocol::Request::OpenFile {
                pane: None,
                path: abs.to_string(),
                mode: None,
                direction: None,
                // **フォーカスは奪わない**（Mac で作業中のユーザーの入力先を変えない）
                focus: Some(false),
                new_tab: false,
            })
            .map_err(|e| WriteFailure::app(&e))?;
            let pane = opened["pane"]
                .as_u64()
                .ok_or_else(|| WriteFailure::app("プレビューペインを用意できなかった"))?;
            (
                pane,
                opened["mode"].as_str().unwrap_or_default().to_string(),
            )
        }
    };
    let (was_editing, dirty) = preview_edit_state(deps, pane).map_err(|e| WriteFailure::app(&e))?;
    Ok(PreviewTarget {
        pane,
        mode,
        was_editing,
        dirty,
    })
}

/// テキストとして編集していい種別か。
///
/// 拡張子の表を daemon 側に持たない（**app の分類をそのまま使う**）ので、
/// 対応形式が増えても食い違わない
fn mode_is_text(mode: &str) -> bool {
    matches!(mode, "code" | "markdown")
}

/// PC 側の編集経路を通して保存する。
///
/// 呼ぶ前に編集モードでなかったときは**終わったら戻す**（スマホからの保存で
/// Mac のプレビューが編集モードのまま残らない）。戻す失敗は無視する
/// （保存自体は済んでいるので、そこで失敗と言うと再送を促してしまう）
fn apply_and_save(
    deps: &FilesDeps,
    target: &PreviewTarget,
    text: &str,
) -> Result<Value, WriteFailure> {
    (deps.send)(crate::protocol::Request::PreviewEdit {
        pane: Some(target.pane),
        enabled: Some(true),
    })
    .map_err(|e| WriteFailure::app(&e))?;
    (deps.send)(crate::protocol::Request::PreviewApply {
        pane: Some(target.pane),
        text: text.to_string(),
    })
    .map_err(|e| WriteFailure::app(&e))?;
    let saved = (deps.send)(crate::protocol::Request::PreviewSave {
        pane: Some(target.pane),
    });
    if !target.was_editing {
        let _ = (deps.send)(crate::protocol::Request::PreviewEdit {
            pane: Some(target.pane),
            enabled: Some(false),
        });
    }
    saved.map_err(|e| WriteFailure::app(&e))
}

/// ローカルのファイルへ書く（#1084）
fn write_local(
    deps: &FilesDeps,
    resolved: &Resolved,
    text: &str,
    etag: &str,
) -> Result<Value, WriteFailure> {
    if text.len() as u64 > MAX_TEXT_BYTES {
        return Err(WriteFailure::too_large(MAX_TEXT_BYTES));
    }
    // (1) スマホが読んだ時点と同じか（**上書きの前に**見る）
    let current = read_content(resolved)?;
    let Some(current_text) = current.text.as_deref() else {
        return Err(if current.binary {
            WriteFailure::not_text()
        } else {
            WriteFailure::too_large(MAX_TEXT_BYTES)
        });
    };
    if content_etag(current_text.as_bytes()) != etag {
        return Err(WriteFailure::conflict(""));
    }

    // (2) 書き先のプレビューペインを用意する
    let abs = resolved.path.display().to_string();
    let target = ensure_local_preview(deps, &abs)?;
    if !mode_is_text(&target.mode) {
        return Err(WriteFailure::not_text());
    }
    if target.dirty {
        return Err(WriteFailure::busy_editing());
    }

    // (3) PC 側の編集経路で保存する
    let result = apply_and_save(deps, &target, text);
    match result {
        Ok(out) => Ok(json!({
            "saved": true,
            "pane": target.pane,
            "path": resolved.rel,
            "root": resolved.root_id,
            "size": text.len(),
            "etag": content_etag(text.as_bytes()),
            "dirty": out["dirty"].as_bool().unwrap_or(false),
        })),
        // 適用してから書くまでの窓で変わっていた場合。**事実で分類する**
        // （app のエラー文言に依存しない: 文言は i18n で変わりうる）
        Err(failure) => Err(match read_content(resolved) {
            Ok(after)
                if after
                    .text
                    .as_deref()
                    .map(|t| content_etag(t.as_bytes()) != etag)
                    .unwrap_or(false) =>
            {
                WriteFailure::conflict("保存の直前に変わりました")
            }
            _ => failure,
        }),
    }
}

/// SSH 先のファイルを取得してプレビューへ載せる（#1085）。
///
/// **取得のたびに #966 の競合検知の基準が進む**（`fetch_file` が
/// `write_baseline` する）ので、読み出しでも書き込みでもここを通す
fn open_ssh_preview(deps: &FilesDeps, target: &SshResolved) -> Result<Value, WriteFailure> {
    (deps.send)(crate::protocol::Request::RemoteFolder {
        action: "open-file".to_string(),
        host: Some(target.host.clone()),
        path: Some(target.path.clone()),
        tab: None,
        // フォーカスは奪わない（ローカルの `OpenFile` と同じ）
        focus: Some(false),
        all: false,
        force: false,
        enabled: None,
        terminal: None,
    })
    .map_err(|e| WriteFailure::app(&e))
}

/// 退避されている押し出しを引く（`host` / `path` で絞る）
fn ssh_pending(deps: &FilesDeps, host: Option<&str>, path: Option<&str>) -> Result<Value, String> {
    (deps.send)(crate::protocol::Request::RemoteFolder {
        action: "pending".to_string(),
        host: host.map(str::to_string),
        path: path.map(str::to_string),
        tab: None,
        focus: None,
        all: false,
        force: false,
        enabled: None,
        terminal: None,
    })
}

/// SSH 先のファイルへ書く（#1085）。
///
/// #966 の保証（アトミックな置き換え・内容での競合検知・mode 復元・退避）は
/// `PreviewSave` の中で走る。ここが足すのは「スマホが読んだ時点との突き合わせ」だけで、
/// **`force` は受け取らない**（スマホから競合を踏み潰す操作は出さない）
fn write_ssh(
    deps: &FilesDeps,
    target: &SshResolved,
    text: &str,
    etag: &str,
) -> Result<Value, WriteFailure> {
    if text.len() as u64 > MAX_TEXT_BYTES {
        return Err(WriteFailure::too_large(MAX_TEXT_BYTES));
    }
    // (1) いまのリモートの内容を取り直す（= キャッシュと競合検知の基準が進む）
    let opened = open_ssh_preview(deps, target)?;
    if opened["read_only"].as_bool().unwrap_or(false) {
        return Err(WriteFailure::read_only());
    }
    let pane = opened["pane"]
        .as_u64()
        .ok_or_else(|| WriteFailure::app("プレビューペインを用意できなかった"))?;
    let cached = opened["cached_path"]
        .as_str()
        .ok_or_else(|| WriteFailure::app("取得したファイルの置き場が分からない"))?;
    let current = read_cached_text(std::path::Path::new(cached))?;
    if content_etag(current.as_bytes()) != etag {
        return Err(WriteFailure::conflict(""));
    }
    let mode = opened["mode"].as_str().unwrap_or_default();
    if !mode_is_text(mode) {
        return Err(WriteFailure::not_text());
    }
    let (was_editing, dirty) = preview_edit_state(deps, pane).map_err(|e| WriteFailure::app(&e))?;
    if dirty {
        return Err(WriteFailure::busy_editing());
    }

    // (2) PC 側の編集経路で保存する（SFTP の書き戻しはこの中）
    let target_pane = PreviewTarget {
        pane,
        mode: mode.to_string(),
        was_editing,
        dirty,
    };
    match apply_and_save(deps, &target_pane, text) {
        Ok(out) => Ok(json!({
            "saved": true,
            "pane": pane,
            "path": target.rel,
            "root": target.root_id,
            "size": text.len(),
            "etag": content_etag(text.as_bytes()),
            "dirty": out["dirty"].as_bool().unwrap_or(false),
            // #966 の書き戻し状態（idle / uploading / saved / failed / pending）
            "remote": out["remote"].clone(),
        })),
        // 押し出せなかったときは **内容が退避されている**（#966）。
        // 種別は退避の記録から取る = app のエラー文言に依存しない
        Err(failure) => Err(classify_ssh_write_failure(deps, target, failure)),
    }
}

/// 押し出しの失敗を退避の記録から分類する。
///
/// `conflict` なら「読み直してやり直す」、それ以外は「つながったら送り直せる」。
/// 記録が無ければ app の理由をそのまま返す（保存自体が始まっていない場合）
fn classify_ssh_write_failure(
    deps: &FilesDeps,
    target: &SshResolved,
    failure: WriteFailure,
) -> WriteFailure {
    let Ok(pending) = ssh_pending(deps, Some(&target.host), Some(&target.path)) else {
        return failure;
    };
    let Some(entry) = pending["pending"].as_array().and_then(|a| a.first()) else {
        return failure;
    };
    let kind = entry["kind"].as_str().unwrap_or_default();
    let detail = entry["error"].as_str().unwrap_or_default();
    if kind == "conflict" {
        WriteFailure::conflict(detail)
    } else {
        WriteFailure::remote_pending(kind, detail)
    }
}

/// SFTP で落ちてきた写しをテキストとして読む。
///
/// パスは**app が返したもの**（`cached_path`）なので、ここは認可の対象ではない
/// （ユーザー入力から組んだパスは 1 バイトも混ざらない）。判定は読み出し側
/// （[`read_content`]）と同じ規則にしてある
fn read_cached_text(path: &std::path::Path) -> Result<String, WriteFailure> {
    let bytes = std::fs::read(path).map_err(|e| WriteFailure::app(&format!("{e}")))?;
    if bytes.contains(&0) {
        return Err(WriteFailure::not_text());
    }
    String::from_utf8(bytes).map_err(|_| WriteFailure::not_text())
}

/// `/api/files*` の受け口。role の検査は呼び出し側（`remote.rs` の共通経路）が済ませている
pub fn handle_files_request(
    mut request: tiny_http::Request,
    path: &str,
    url_full: &str,
    deps: &FilesDeps,
) {
    let method = request.method().clone();
    let root_param = query_value(url_full, "root");
    let rel = query_value(url_full, "path").unwrap_or_default();

    match (&method, path) {
        // --- 読み出し（#1079 / SSH 先は #1085） ---
        (tiny_http::Method::Get, "/api/files") => {
            let (status, body) = read_listing(deps, root_param.as_deref(), &rel);
            respond_json(request, deps, status, &body)
        }
        (tiny_http::Method::Get, "/api/files/content") => {
            let (status, body) = read_file_content(deps, root_param.as_deref(), &rel);
            respond_json(request, deps, status, &body)
        }
        (tiny_http::Method::Get, "/api/files/download") => {
            respond_download(request, deps, root_param.as_deref(), &rel)
        }
        // --- 押し出せていない保存（#1085 / #966） ---
        (tiny_http::Method::Get, "/api/files/pending") => {
            let (status, body) = read_pending(deps, root_param.as_deref());
            respond_json(request, deps, status, &body)
        }

        // --- 書き込み（#1084 / SSH 先は #1085） ---
        (tiny_http::Method::Put, "/api/files/content") => {
            let body = match read_json_body(&mut request) {
                Ok(v) => v,
                Err(e) => {
                    let f = WriteFailure::bad_body(&e);
                    return respond_json(request, deps, f.status, &f.to_json());
                }
            };
            let (status, out) = write_file_content(deps, root_param.as_deref(), &rel, &body);
            respond_json(request, deps, status, &out)
        }
        (tiny_http::Method::Post, "/api/files/push") => {
            let body = match read_json_body(&mut request) {
                Ok(v) => v,
                Err(e) => {
                    let f = WriteFailure::bad_body(&e);
                    return respond_json(request, deps, f.status, &f.to_json());
                }
            };
            let (status, out) = push_pending(deps, root_param.as_deref(), &rel, &body);
            respond_json(request, deps, status, &out)
        }

        // 受け持つパスだがメソッドが違う（405）と、そもそも無いパス（404）を分ける
        (_, p) if is_write_path(p) || required_role_for(p).is_some() => respond_json(
            request,
            deps,
            405,
            &json!({
                "error": "このメソッドには対応していない",
                "error_en": "Method not allowed",
                "kind": "method_not_allowed",
            }),
        ),
        _ => respond_json(
            request,
            deps,
            404,
            &json!({ "error": "API エンドポイントが見つからない" }),
        ),
    }
}

/// ルート一覧（`root` 省略）またはディレクトリ一覧
fn read_listing(deps: &FilesDeps, root: Option<&str>, rel: &str) -> (u16, Value) {
    match root {
        // ローカルと SSH 先を**ツリーに出ている並び**で 1 本の一覧にする（#1041）
        None => {
            let local = match current_roots(deps) {
                Ok(r) => r,
                Err(e) => return app_unreachable(&e),
            };
            // SSH 先が引けない（app が古い等）ときはローカルだけ返す
            // = ファイルビューごと開けなくならない
            let ssh = current_ssh_roots(deps).unwrap_or_default();
            let mut roots: Vec<Value> = Vec::with_capacity(local.len() + ssh.len());
            roots.extend(
                ssh.iter()
                    .filter(|r| r.placement.as_deref() != Some("trailing"))
                    .map(SshRoot::to_json),
            );
            roots.extend(local.iter().map(TreeRoot::to_json));
            roots.extend(
                ssh.iter()
                    .filter(|r| r.placement.as_deref() == Some("trailing"))
                    .map(SshRoot::to_json),
            );
            (deps.audit)("files", audit_payload("roots", 0, roots.len()));
            (200, json!({ "roots": roots }))
        }
        Some(root) if is_ssh_root_id(root) => {
            let roots = match current_ssh_roots(deps) {
                Ok(r) => r,
                Err(e) => return app_unreachable(&e),
            };
            let target = match resolve_in_ssh_root(&roots, root, rel) {
                Ok(t) => t,
                Err(d) => return (d.status(), d.to_json()),
            };
            match ssh_list_directory(deps, &target) {
                Ok(body) => {
                    let entries = body["entries"].as_array().map(Vec::len).unwrap_or(0);
                    (deps.audit)("files", audit_payload("ssh_list", 0, entries));
                    (200, body)
                }
                Err(f) => (f.status, f.to_json()),
            }
        }
        Some(root) => {
            let roots = match current_roots(deps) {
                Ok(r) => r,
                Err(e) => return app_unreachable(&e),
            };
            match resolve_in_root(&roots, root, rel).and_then(|r| list_directory(&r)) {
                Ok(body) => {
                    let entries = body["entries"].as_array().map(Vec::len).unwrap_or(0);
                    (deps.audit)("files", audit_payload("list", 0, entries));
                    (200, body)
                }
                Err(d) => (d.status(), d.to_json()),
            }
        }
    }
}

/// プレビュー用の本文
fn read_file_content(deps: &FilesDeps, root: Option<&str>, rel: &str) -> (u16, Value) {
    let Some(root) = root else {
        return (Denial::UnknownRoot.status(), Denial::UnknownRoot.to_json());
    };
    if is_ssh_root_id(root) {
        let roots = match current_ssh_roots(deps) {
            Ok(r) => r,
            Err(e) => return app_unreachable(&e),
        };
        let target = match resolve_in_ssh_root(&roots, root, rel) {
            Ok(t) => t,
            Err(d) => return (d.status(), d.to_json()),
        };
        return match ssh_content_payload(deps, &target) {
            Ok(body) => {
                let sent = body["size"].as_u64().unwrap_or(0);
                (deps.audit)("files", audit_payload("ssh_content", sent, 0));
                (200, body)
            }
            Err(f) => (f.status, f.to_json()),
        };
    }
    let roots = match current_roots(deps) {
        Ok(r) => r,
        Err(e) => return app_unreachable(&e),
    };
    match resolve_in_root(&roots, root, rel).and_then(|r| read_content(&r).map(|c| (r, c))) {
        Ok((resolved, content)) => {
            let sent = content.text.as_ref().map(|t| t.len() as u64).unwrap_or(0);
            (deps.audit)("files", audit_payload("content", sent, 0));
            (200, content_payload(&resolved, &content))
        }
        Err(d) => (d.status(), d.to_json()),
    }
}

/// 押し出せていない保存の一覧（#966 の `pending` をそのまま見せる）
fn read_pending(deps: &FilesDeps, root: Option<&str>) -> (u16, Value) {
    // `root` 指定があればそのホストだけに絞る（省略で全件）
    let host = match root {
        Some(root) if is_ssh_root_id(root) => match current_ssh_roots(deps) {
            Ok(roots) => match roots.iter().find(|r| r.id == root) {
                Some(r) => Some(r.host.clone()),
                None => return (Denial::UnknownRoot.status(), Denial::UnknownRoot.to_json()),
            },
            Err(e) => return app_unreachable(&e),
        },
        _ => None,
    };
    match ssh_pending(deps, host.as_deref(), None) {
        Ok(body) => {
            let n = body["pending"].as_array().map(Vec::len).unwrap_or(0);
            (deps.audit)("files", audit_payload("pending", 0, n));
            (200, body)
        }
        Err(e) => app_unreachable(&e),
    }
}

/// 書き込み（ローカル / SSH 先の両方）
fn write_file_content(
    deps: &FilesDeps,
    root: Option<&str>,
    rel: &str,
    body: &Value,
) -> (u16, Value) {
    let Some(root) = root else {
        return (Denial::UnknownRoot.status(), Denial::UnknownRoot.to_json());
    };
    let Some(text) = body["text"].as_str() else {
        let f = WriteFailure::bad_body("text が要ります");
        return (f.status, f.to_json());
    };
    // 検証子が無ければ**書かない**（競合を判定できないまま上書きしない）
    let Some(etag) = body["etag"].as_str().filter(|e| !e.is_empty()) else {
        let f = WriteFailure::missing_etag();
        return (f.status, f.to_json());
    };

    let result = if is_ssh_root_id(root) {
        match current_ssh_roots(deps) {
            Ok(roots) => match resolve_in_ssh_root(&roots, root, rel) {
                Ok(target) => write_ssh(deps, &target, text, etag),
                Err(d) => Err(d.into()),
            },
            Err(e) => Err(WriteFailure::app_unreachable(&e)),
        }
    } else {
        match current_roots(deps) {
            Ok(roots) => match resolve_in_root(&roots, root, rel) {
                Ok(resolved) => write_local(deps, &resolved, text, etag),
                Err(d) => Err(d.into()),
            },
            Err(e) => Err(WriteFailure::app_unreachable(&e)),
        }
    };
    match result {
        Ok(out) => {
            (deps.audit)("files", audit_payload("write", text.len() as u64, 0));
            (200, out)
        }
        Err(f) => {
            // 書けなかったことも残す（何をどれだけ**書こうとしたか**だけ。パスは載せない）
            (deps.audit)("files", audit_payload("write_denied", text.len() as u64, 0));
            (f.status, f.to_json())
        }
    }
}

/// 退避されている保存を送り直す（#966 の `push`）。
///
/// **`force` は受け取らない**: 競合を承知で踏み潰す操作をスマホから出さない
/// （読み直して編集をやり直す導線だけを残す）
fn push_pending(deps: &FilesDeps, root: Option<&str>, rel: &str, body: &Value) -> (u16, Value) {
    // 対象は root + path（省略で全件）。root は SSH 先ルートだけ
    let root = root.or_else(|| body["root"].as_str());
    let rel = if rel.is_empty() {
        body["path"].as_str().unwrap_or_default()
    } else {
        rel
    };
    let target = match root {
        Some(root) if is_ssh_root_id(root) => {
            let roots = match current_ssh_roots(deps) {
                Ok(r) => r,
                Err(e) => return app_unreachable(&e),
            };
            match resolve_in_ssh_root(&roots, root, rel) {
                Ok(t) => Some(t),
                Err(d) => return (d.status(), d.to_json()),
            }
        }
        Some(_) => {
            let f = WriteFailure::bad_body("SSH 先のフォルダを指定してください");
            return (f.status, f.to_json());
        }
        None => None,
    };
    // path 省略のときはホストだけで絞る（そのフォルダの分をまとめて送り直す）
    let (host, path) = match &target {
        Some(t) if t.rel.is_empty() => (Some(t.host.as_str()), None),
        Some(t) => (Some(t.host.as_str()), Some(t.path.as_str())),
        None => (None, None),
    };
    let sent = (deps.send)(crate::protocol::Request::RemoteFolder {
        action: "push".to_string(),
        host: host.map(str::to_string),
        path: path.map(str::to_string),
        tab: None,
        focus: None,
        all: false,
        force: false,
        enabled: None,
        terminal: None,
    });
    match sent {
        Ok(out) => {
            (deps.audit)("files", audit_payload("push", 0, 0));
            (200, out)
        }
        Err(e) => {
            (deps.audit)("files", audit_payload("push_failed", 0, 0));
            // 送り直しの失敗は「なぜ送れないか」がそのまま次の一手になる
            let f = WriteFailure::remote_pending("", &e);
            (f.status, f.to_json())
        }
    }
}

/// SSH 先のディレクトリ一覧（`RemoteFolder { action: "ls" }` を proxy して、
/// ローカルの一覧と**同じ形**へ揃える）
fn ssh_list_directory(deps: &FilesDeps, target: &SshResolved) -> Result<Value, WriteFailure> {
    let listed = (deps.send)(crate::protocol::Request::RemoteFolder {
        action: "ls".to_string(),
        host: Some(target.host.clone()),
        path: Some(target.path.clone()),
        tab: None,
        focus: None,
        all: false,
        force: false,
        enabled: None,
        terminal: None,
    })
    .map_err(|e| WriteFailure::app(&e))?;
    let mut entries: Vec<Value> = listed["entries"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|e| {
            let name = e["name"].as_str()?;
            let kind = e["kind"].as_str().unwrap_or("unknown");
            let is_dir = kind == "dir";
            Some(json!({
                "name": name,
                "dir": is_dir,
                "size": if is_dir { Value::Null } else { e["size"].clone() },
                // `ls -la` の日時は分の分解能しかないので出さない（#966 の教訓）
                "modified": Value::Null,
                "symlink": kind == "symlink",
                // リモート側の実体は安く辿れないので印は付けない（脅威モデル参照）
                "escapes_root": false,
                "hidden": name.starts_with('.'),
            }))
        })
        .collect();
    let truncated = entries.len() > MAX_ENTRIES;
    entries.truncate(MAX_ENTRIES);
    Ok(json!({
        "root": target.root_id,
        "root_name": target.root_name,
        "path": target.rel,
        "ssh": true,
        "host": target.host,
        "connected": target.connected,
        "entries": entries,
        "truncated": truncated,
    }))
}

/// SSH 先のファイル本文（`open-file` で取得 → 写しを読む）。
///
/// 読み出しでも `open-file` を通すのは #966 の設計に合わせるため:
/// 取得のたびに競合検知の基準が進むので、**読み直せば競合が解ける**
/// （「読み直してやり直す」の導線がそのまま効く）
fn ssh_content_payload(deps: &FilesDeps, target: &SshResolved) -> Result<Value, WriteFailure> {
    let opened = open_ssh_preview(deps, target)?;
    let cached = opened["cached_path"]
        .as_str()
        .ok_or_else(|| WriteFailure::app("取得したファイルの置き場が分からない"))?;
    let size = opened["size"].as_u64();
    let mode = opened["mode"].as_str().unwrap_or_default();
    let read_only = opened["read_only"].as_bool().unwrap_or(false);
    let pending = opened["pending_write"].as_bool().unwrap_or(false);
    let text = read_cached_text(std::path::Path::new(cached)).ok();
    let size = size
        .or_else(|| text.as_ref().map(|t| t.len() as u64))
        .unwrap_or(0);
    Ok(json!({
        "root": target.root_id,
        "root_name": target.root_name,
        "path": target.rel,
        "ssh": true,
        "host": target.host,
        "size": size,
        "binary": text.is_none(),
        "truncated": false,
        "text": text,
        "etag": text.as_ref().map(|t| content_etag(t.as_bytes())),
        // 書けないものは編集させない（#966。mode のどこにも `w` が無いとき）
        "read_only": read_only || !mode_is_text(mode),
        // 前のセッションで押し出せていない保存が残っている（#966）
        "pending_write": pending,
        "pane": opened["pane"].clone(),
    }))
}

/// ダウンロード（ローカル / SSH 先の両方。ストリーミング）
fn respond_download(request: tiny_http::Request, deps: &FilesDeps, root: Option<&str>, rel: &str) {
    let Some(root) = root else {
        return respond_denial(request, deps, Denial::UnknownRoot);
    };
    if is_ssh_root_id(root) {
        let roots = match current_ssh_roots(deps) {
            Ok(r) => r,
            Err(e) => {
                let (status, body) = app_unreachable(&e);
                return respond_json(request, deps, status, &body);
            }
        };
        let target = match resolve_in_ssh_root(&roots, root, rel) {
            Ok(t) => t,
            Err(d) => return respond_denial(request, deps, d),
        };
        match prepare_ssh_download(deps, &target) {
            Ok((file, name, size)) => {
                (deps.audit)("files", audit_payload("ssh_download", size, 0));
                respond_file(request, deps, file, &name)
            }
            Err(f) => respond_json(request, deps, f.status, &f.to_json()),
        }
        return;
    }
    let roots = match current_roots(deps) {
        Ok(r) => r,
        Err(e) => {
            let (status, body) = app_unreachable(&e);
            return respond_json(request, deps, status, &body);
        }
    };
    match resolve_in_root(&roots, root, rel).and_then(|r| open_for_download(&r).map(|f| (r, f))) {
        Ok((resolved, (file, size))) => {
            let name = resolved
                .path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "download".to_string());
            (deps.audit)("files", audit_payload("download", size, 0));
            respond_file(request, deps, file, &name)
        }
        Err(d) => respond_denial(request, deps, d),
    }
}

/// SSH 先のファイルをダウンロード用に開く（**応答は組まない**）。
///
/// `tiny_http::Request` は応答で消費されるので、失敗しうる処理はここで
/// 先に終わらせてから 1 回だけ応答する（request を Result に載せて持ち回すと
/// エラー型が大きくなり、呼び出し側の分岐も追いにくい）
fn prepare_ssh_download(
    deps: &FilesDeps,
    target: &SshResolved,
) -> Result<(std::fs::File, String, u64), WriteFailure> {
    let opened = open_ssh_preview(deps, target)?;
    let cached = opened["cached_path"]
        .as_str()
        .ok_or_else(|| WriteFailure::app("取得したファイルの置き場が分からない"))?;
    let file = std::fs::File::open(cached).map_err(|e| WriteFailure::app(&format!("{e}")))?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    Ok((file, tako_core::remote_fs::base_name(&target.path), size))
}

/// ファイルを添付としてストリーミングする（ローカルと SSH 先で 1 実装）
fn respond_file(request: tiny_http::Request, deps: &FilesDeps, file: std::fs::File, name: &str) {
    let mut resp = tiny_http::Response::from_file(file)
        .with_header(header(b"Content-Type", b"application/octet-stream"))
        .with_header(header_or(
            b"Content-Disposition",
            &content_disposition(name),
            b"attachment",
        ));
    for h in deps.cors.clone() {
        resp = resp.with_header(h);
    }
    resp = resp.with_header(header(b"Cache-Control", b"no-store, private"));
    let _ = request.respond(resp);
}

/// tako app へ届かなかった（両方の読み出し経路で共有する応答）
fn app_unreachable(detail: &str) -> (u16, Value) {
    let f = WriteFailure::app_unreachable(detail);
    (f.status, f.to_json())
}

/// リクエストボディを JSON として読む（上限つき）。
///
/// `remote.rs` にも同名の関数があるが、このモジュールを**単体でテストできる**形に
/// 保つため写しを持つ（`query_value` / `percent_decode` と同じ方針）
fn read_json_body(request: &mut tiny_http::Request) -> Result<Value, String> {
    use std::io::Read as _;
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES)
        .read_to_string(&mut body)
        .map_err(|_| "リクエストボディの読み取りに失敗".to_string())?;
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&body).map_err(|e| format!("JSON パースエラー: {e}"))
}

/// 書き込みのボディ上限。
///
/// 保存 1 回で運ぶのは本文だけ（読んだ時点の内容は**検証子**で預けるので
/// 往復で 2 倍にならない）。本文の上限 `MAX_TEXT_BYTES` に JSON の
/// 符号化ぶんの余裕を足した値にしてある
pub const MAX_BODY_BYTES: u64 = 4 * MAX_TEXT_BYTES;

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

    // --- 検証子（#1084） ---

    #[test]
    fn 検証子は内容が変われば変わる() {
        let a = content_etag(b"hello\n");
        assert_eq!(a, content_etag(b"hello\n"), "同じ内容なら同じ");
        assert_ne!(a, content_etag(b"hello!\n"), "1 文字違えば変わる");
        // 長さが同じでも内容が違えば変わる（長さだけの比較へ退化していない）
        assert_ne!(content_etag(b"abcdef"), content_etag(b"abcdeg"));
        // 内容が同じでも長さが違えば変わる（ハッシュ衝突の保険として長さを併記している）
        assert!(a.starts_with("6-"), "長さが先頭に出る: {a}");
        assert_eq!(content_etag(b""), format!("0-{:016x}", fnv1a64(b"")));
    }

    #[test]
    fn 検証子は日本語でも決定的() {
        let text = "日本語の本文\nと 2 行目\n";
        assert_eq!(content_etag(text.as_bytes()), content_etag(text.as_bytes()));
        assert_ne!(
            content_etag(text.as_bytes()),
            content_etag("日本語の本文\nと 3 行目\n".as_bytes())
        );
    }

    // --- SSH 先のルート（#1085） ---

    fn ssh_payload() -> Value {
        json!({
            "tabs": [
                {
                    "tab": 1,
                    "title": "作業",
                    "remote_folders": [
                        {
                            "host": "linuxbox",
                            "path": "/home/testuser/proj",
                            "label": "linuxbox:/home/testuser/proj",
                            "state": "loaded",
                            "connected": true,
                            "placement": "leading",
                        },
                        {
                            "host": "winbox",
                            "path": "/C:/Users/winuser/dev",
                            "label": "winbox:/C:/Users/winuser/dev",
                            "state": "loaded",
                            "connected": false,
                            "placement": "trailing",
                        }
                    ],
                },
                {
                    // 同じ (host, path) が別タブにも出ている = 1 件に畳む
                    "tab": 2,
                    "title": "別タブ",
                    "remote_folders": [{
                        "host": "linuxbox",
                        "path": "/home/testuser/proj",
                        "connected": true,
                    }],
                }
            ]
        })
    }

    #[test]
    fn ssh先ルートは重複を畳んで並ぶ() {
        let roots = ssh_roots_from_payload(&ssh_payload());
        assert_eq!(roots.len(), 2, "同じ (host, path) は 1 件: {roots:?}");
        assert_eq!(roots[0].host, "linuxbox");
        assert_eq!(roots[0].name, "proj");
        assert!(roots[0].connected);
        assert_eq!(roots[0].placement.as_deref(), Some("leading"));
        // Windows の相手は `/C:/...` で来る（表示名は末尾だけ）
        assert_eq!(roots[1].name, "dev");
        assert!(!roots[1].connected, "切断中も一覧には出る");
    }

    #[test]
    fn ssh先ルートのidはローカルと取り違えない() {
        let roots = ssh_roots_from_payload(&ssh_payload());
        for r in &roots {
            assert!(is_ssh_root_id(&r.id), "SSH 先の id: {}", r.id);
        }
        // ローカルの id は 16 進数なので `s` で始まることが構造的に無い
        for path in ["/w/a", "/w/b", "/Users/testuser/dev/tako", "/"] {
            let id = root_id_of(path);
            assert!(!is_ssh_root_id(&id), "ローカルの id が SSH に見える: {id}");
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit()),
                "ローカルの id は 16 進数: {id}"
            );
        }
    }

    #[test]
    fn ssh先の相対パスは配下に閉じる() {
        let roots = ssh_roots_from_payload(&ssh_payload());
        let id = roots[0].id.clone();

        // 配下は解決できる
        let ok = resolve_in_ssh_root(&roots, &id, "src/main.rs").expect("配下");
        assert_eq!(ok.path, "/home/testuser/proj/src/main.rs");
        assert_eq!(ok.rel, "src/main.rs");
        assert_eq!(ok.host, "linuxbox");
        // ルート自身
        let root_self = resolve_in_ssh_root(&roots, &id, "").expect("ルート自身");
        assert_eq!(root_self.path, "/home/testuser/proj");
        assert_eq!(root_self.rel, "");

        // 外へ出る形はすべて落ちる（ローカルと同じ純粋関数を通っている）
        for bad in [
            "..",
            "../secret",
            "src/../../secret",
            "/etc/passwd",
            "\\\\server\\share",
            "C:/Windows",
            "src/C:/Windows",
            "src\\..\\..\\secret",
            "src/\u{0}etc",
        ] {
            let denied = resolve_in_ssh_root(&roots, &id, bad).expect_err(bad);
            assert_eq!(denied.status(), 403, "{bad} は 403: {denied:?}");
        }

        // ツリーに出ていないルートは拒否
        let unknown = resolve_in_ssh_root(&roots, "s-000000000000", "x").expect_err("未知");
        assert_eq!(unknown, Denial::UnknownRoot);
    }

    #[test]
    fn ssh先のルートが閉じられれば読めなくなる() {
        let roots = ssh_roots_from_payload(&ssh_payload());
        let id = roots[0].id.clone();
        assert!(resolve_in_ssh_root(&roots, &id, "src").is_ok());
        // Mac 側でフォルダを閉じた = 一覧から消えた状態
        let closed = ssh_roots_from_payload(&json!({ "tabs": [] }));
        assert_eq!(
            resolve_in_ssh_root(&closed, &id, "src"),
            Err(Denial::UnknownRoot),
            "一覧から消えたら同じ id でも読めない"
        );
    }

    // --- 書き込みの role と失敗の形（#1084 / #1085） ---

    #[test]
    fn 書き込みパスは明示列挙されている() {
        assert!(is_write_path("/api/files/content"));
        assert!(is_write_path("/api/files/push"));
        // 列挙外は書き込み扱いにしない（`remote.rs` 側で Manage のまま残る）
        for p in [
            "/api/files",
            "/api/files/download",
            "/api/files/pending",
            "/api/files/wipe",
            "/api/files/content/x",
        ] {
            assert!(!is_write_path(p), "{p} が書き込み扱いになっている");
        }
        // 受け持ちは読み出しと同じ Interact 以上
        for p in WRITE_PATHS {
            assert_eq!(required_role_for(p), Some(DeviceRole::Interact), "{p}");
        }
    }

    #[test]
    fn 書き込みの失敗は理由と次の一手を日英で返す() {
        let cases = vec![
            (WriteFailure::conflict(""), 409, "conflict"),
            (WriteFailure::busy_editing(), 409, "busy_editing"),
            (WriteFailure::not_text(), 400, "not_text"),
            (WriteFailure::read_only(), 403, "read_only"),
            (WriteFailure::missing_etag(), 400, "missing_etag"),
            (WriteFailure::too_large(MAX_TEXT_BYTES), 413, "too_large"),
            // 詳細（app / remote 側の理由）は**そのまま通す**ので、
            // 日英の検査には ASCII の詳細を使う（下で通し方を別に固定する）
            (
                WriteFailure::remote_pending("unreachable", "no route to host"),
                502,
                "remote_pending",
            ),
        ];
        for (f, status, kind) in cases {
            assert_eq!(f.status, status, "{kind}");
            let body = f.to_json();
            assert_eq!(body["kind"].as_str(), Some(kind));
            let ja = body["error"].as_str().unwrap_or_default();
            let en = body["error_en"].as_str().unwrap_or_default();
            assert!(!ja.is_empty() && !en.is_empty(), "{kind} は日英とも要る");
            assert!(
                ja.chars().any(|c| c as u32 > 0x3000),
                "{kind} の日本語が英語のまま: {ja}"
            );
            assert!(en.is_ascii(), "{kind} の英語に日本語が混ざる: {en}");
        }
        // 退避されたことは機械可読に載る（PWA が「送り直す」を出す材料）
        let pending = WriteFailure::remote_pending("unreachable", "x").to_json();
        assert_eq!(pending["pending"].as_bool(), Some(true));
        assert_eq!(pending["remote_kind"].as_str(), Some("unreachable"));

        // remote / app 側の理由は**言い換えず**日英どちらにもそのまま入る
        // （daemon で書き直すと「なぜ送れないか」が消える）
        let detail = "ssh: connect to host … port 22: Operation timed out";
        for body in [
            WriteFailure::remote_pending("unreachable", detail).to_json(),
            WriteFailure::conflict(detail).to_json(),
            WriteFailure::app(detail).to_json(),
        ] {
            for key in ["error", "error_en"] {
                assert!(
                    body[key].as_str().unwrap_or_default().contains(detail),
                    "{key} に理由が入っていない: {body}"
                );
            }
        }
    }

    #[test]
    fn パスの拒否はそのまま書き込みの失敗になる() {
        // 読み出しの拒否種別が書き込み側で消えない（403 が 500 に化けない等）
        for d in [
            Denial::UnknownRoot,
            Denial::Traversal,
            Denial::AbsolutePath,
            Denial::EscapesRoot,
            Denial::NotFound,
        ] {
            let f: WriteFailure = d.into();
            assert_eq!(f.status, d.status(), "{d:?}");
            assert_eq!(f.kind, d.kind(), "{d:?}");
        }
    }

    #[test]
    fn テキストとして編集できる種別だけを通す() {
        assert!(mode_is_text("code"));
        assert!(mode_is_text("markdown"));
        for mode in ["image", "pdf", "video", "changelog", ""] {
            assert!(!mode_is_text(mode), "{mode} を編集可にしている");
        }
    }

    #[test]
    fn 本文の応答に検証子が載る() {
        let fx = Fixture::new("etag");
        let resolved = resolve_in_root(&fx.roots, fx.root_id(), "hello.txt").expect("解決");
        let content = read_content(&resolved).expect("読める");
        let body = content_payload(&resolved, &content);
        let etag = body["etag"].as_str().expect("検証子が載る");
        assert_eq!(etag, content_etag("こんにちは\n".as_bytes()));
        assert_eq!(body["ssh"].as_bool(), Some(false));

        // バイナリ・大きすぎるものには検証子を付けない = 書き込めない
        let bin = fx.dir.join("root").join("bin.dat");
        std::fs::write(&bin, [0u8, 1, 2]).expect("bin");
        let resolved = resolve_in_root(&fx.roots, fx.root_id(), "bin.dat").expect("解決");
        let content = read_content(&resolved).expect("読める");
        let body = content_payload(&resolved, &content);
        assert!(body["etag"].is_null(), "バイナリに検証子を付けない");
    }
}
