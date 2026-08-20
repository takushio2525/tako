//! シェル統合（FR-2.4.1）— OSC 7 / 133 を発行するスクリプトの書き出しと自動注入
//!
//! `shell-integration/` のスクリプトをバイナリへ埋め込み、初回 spawn 時にデータ
//! ディレクトリへ書き出す。**そこから先の届け方が OS で違う**ので、境界（B13）は
//! このモジュール自身が持つ。
//!
//! - unix: シェルが拾う環境変数を注入するだけで済む。**シェル判定はしない**
//!   （zsh は `ZDOTDIR`、bash は `PROMPT_COMMAND`、fish は `XDG_DATA_DIRS` しか
//!   見ないため、3 点セットを常時注入しても互いに無害）。ユーザーのファイルは触らない
//! - Windows: PowerShell に `ZDOTDIR` 相当の環境変数が無いので、`$PROFILE` へ
//!   マーカーで囲んだ 1 ブロックを書く（#525）。書き込みは `tako setup` の 1 ステップで、
//!   [`install`] / [`uninstall`] / [`status`] を CLI・MCP へ 1:1 で出す
//!
//! 無効化は `TAKO_NO_SHELL_INTEGRATION=1`（FR-2.4.4 の設定 UI までの暫定）。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::paths::data_dir;

const ZSH_ZSHENV: &str = include_str!("../shell-integration/zshenv.zsh");
const BASH_SCRIPT: &str = include_str!("../shell-integration/tako.bash");
const FISH_SCRIPT: &str = include_str!("../shell-integration/tako.fish");
const POWERSHELL_SCRIPT: &str = include_str!("../shell-integration/tako.ps1");

/// `$PROFILE` へ書くブロックの開始・終了マーカー。**この 2 行が管理範囲の定義**で、
/// 再配置（冪等）も解除もこの区間だけを見る。文字列を変えると既存の配置を
/// 見失うので、変えるときは移行を用意すること
const BLOCK_BEGIN: &str = "# >>> tako shell integration >>>";
const BLOCK_END: &str = "# <<< tako shell integration <<<";

/// spawn する子シェルに注入する統合用環境変数。プロセス内で一度だけ書き出して使い回す
pub fn env() -> &'static [(String, String)] {
    static ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();
    ENV.get_or_init(|| {
        if disabled() {
            return Vec::new();
        }
        match write_scripts() {
            Ok(env) => env,
            Err(e) => {
                tracing::warn!("シェル統合スクリプトを書き出せない（統合なしで継続）: {e}");
                Vec::new()
            }
        }
    })
}

fn disabled() -> bool {
    std::env::var_os("TAKO_NO_SHELL_INTEGRATION").is_some_and(|v| !v.is_empty())
}

/// スクリプト一式をデータディレクトリへ書き出し、注入 env を返す
fn write_scripts() -> std::io::Result<Vec<(String, String)>> {
    let Some(root) = script_root() else {
        return Ok(Vec::new());
    };

    let zsh_dir = root.join("zsh");
    std::fs::create_dir_all(&zsh_dir)?;
    std::fs::write(zsh_dir.join(".zshenv"), ZSH_ZSHENV)?;

    let bash_path = root.join("tako.bash");
    std::fs::write(&bash_path, BASH_SCRIPT)?;

    let fish_conf_dir = root.join("fish-data/fish/vendor_conf.d");
    std::fs::create_dir_all(&fish_conf_dir)?;
    std::fs::write(fish_conf_dir.join("tako.fish"), FISH_SCRIPT)?;

    write_powershell_script(&root)?;

    Ok(imp::injected_env(&root, &bash_path))
}

/// PowerShell 用スクリプトの書き出し。**BOM 付き UTF-8** で置く
/// （Windows PowerShell 5.1 は BOM 無しの `.ps1` を ANSI コードページとして読む）
fn write_powershell_script(root: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let path = root.join("tako.ps1");
    let mut bytes = Vec::with_capacity(POWERSHELL_SCRIPT.len() + 3);
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(POWERSHELL_SCRIPT.as_bytes());
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn script_root() -> Option<PathBuf> {
    data_dir().map(|base| base.join("shell-integration"))
}

/// この環境でシェル統合が対象にするシェル。
///
/// **`tako setup` の環境チェックがここを引く**。対応シェルの知識はこのモジュールが持つ
/// （マトリクスのキーは MCP ツール名と 1:1 なので、ツールではないシェル統合は
/// あちらに載せられない）
pub fn shells() -> &'static str {
    imp::SHELLS
}

/// 統合の届け方。**ユーザーに「何をすれば効くのか」を説明するために種類が要る**
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// 環境変数の注入だけで済む（ユーザーのファイルを触らない）
    Automatic,
    /// ユーザーの `$PROFILE` へブロックを書く必要がある
    Profile,
}

/// 配置先 1 件（PowerShell のエディションごと）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileTarget {
    /// 表示名（`PowerShell 7` / `Windows PowerShell 5.1`）
    pub label: String,
    /// 解決に使った実行ファイル
    pub exe: String,
    /// `$PROFILE.CurrentUserAllHosts`
    pub path: PathBuf,
    /// 管理ブロックが入っているか
    pub installed: bool,
    /// 入っているブロックが現行の内容と一致するか（`installed` が false なら常に false）
    pub up_to_date: bool,
}

/// 現在の配置状態
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub delivery: Delivery,
    /// 統合スクリプト本体（データディレクトリ配下）
    pub script: Option<PathBuf>,
    /// 配置先。`Automatic` では常に空
    pub targets: Vec<ProfileTarget>,
    /// 配置できていても**器（永続バックエンド）が OSC を通さない**ために
    /// 統合が届かない場合の説明。届くなら `None`（#525）
    pub blocked_by_backend: Option<String>,
}

impl Status {
    /// 配置そのものが済んでいるか（器の事情は見ない）
    pub fn installed(&self) -> bool {
        match self.delivery {
            Delivery::Automatic => true,
            Delivery::Profile => {
                !self.targets.is_empty() && self.targets.iter().all(|t| t.installed && t.up_to_date)
            }
        }
    }

    /// この環境で統合が**実際にペインへ効く**か。
    /// 配置済みでも器が OSC を落とすなら効かない（psmux。#525）
    pub fn effective(&self) -> bool {
        self.installed() && self.blocked_by_backend.is_none()
    }
}

/// 器が OSC を通さないときの説明（通るなら `None`）。
///
/// **器に尋ねる**（実装名で分岐しない）。将来 psmux が素通しに対応したり、
/// 別の器を足したりしても、この 1 箇所の判定がそのまま追従する
fn backend_block() -> Option<String> {
    let caps = crate::backend::capabilities();
    if caps.osc_passthrough {
        return None;
    }
    Some(format!(
        "永続バックエンド（{}）がシェルの出す OSC を外側へ通さないため、\
         この器を使っている間はペインの cwd 追従とコマンド実行状態が働かない",
        caps.label
    ))
}

/// 1 ファイルに対して行った（行わなかった）こと
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// 新しく書いた
    Installed,
    /// 既存のブロックを現行内容へ差し替えた
    Updated,
    /// 既に最新だったので触っていない
    Unchanged,
    /// ブロックを取り除いた
    Removed,
    /// もともと入っていなかった
    Absent,
    /// 解除の結果ファイルが空になったので削除した
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub label: String,
    pub path: PathBuf,
    pub kind: ChangeKind,
}

/// 現在の配置状態を調べる
pub fn status() -> Status {
    imp::status()
}

/// 統合を配置する（冪等。2 回実行してもブロックは 1 個）
pub fn install() -> Result<Vec<Change>, String> {
    imp::install()
}

/// 統合を解除する。**加えた区切りごと取り除き、元のバイト列へ戻す**
pub fn uninstall() -> Result<Vec<Change>, String> {
    imp::uninstall()
}

// --- ここから下はプラットフォーム非依存の純粋関数（macOS 上でも全部テストできる） ---

/// `$PROFILE` へ書くブロック本文（末尾は改行 1 個）。
///
/// **本文は ASCII だけで書く**。書き込み先はユーザーのファイルで、符号は UTF-8 とは
/// 限らない（BOM 無しの `.ps1` は Windows PowerShell 5.1 では ANSI = 日本語環境なら
/// CP932）。日本語のコメントを混ぜると、そのファイルを開いたときだけ化ける
/// 「符号が混ざったファイル」を作ってしまう。日本語の説明は `tako setup` の出力と
/// ドキュメントが担当する（そちらは端末の符号が分かっているので安全に出せる）
fn profile_block(script: &Path) -> String {
    let literal = powershell_ascii_literal(&script.display().to_string());
    format!(
        "{BLOCK_BEGIN}\n\
         # Managed by `tako setup`. Remove with `tako setup --shell-integration uninstall`.\n\
         # Enables pane cwd tracking and command state (OSC 7 / 133) inside tako panes.\n\
         if ($env:TAKO_PANE_ID) {{\n\
         \x20   $__takoShellIntegrationScript = {literal}\n\
         \x20   if (Test-Path -LiteralPath $__takoShellIntegrationScript) {{ . $__takoShellIntegrationScript }}\n\
         }}\n\
         {BLOCK_END}\n"
    )
}

/// バイト列中の部分列を探す（マーカーはすべて ASCII なので符号を問わない）
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// ブロックの位置（開始バイト, 終了バイト = 末尾改行を含む）。
///
/// **文字列ではなくバイト列で扱う**。ユーザーの `$PROFILE` は UTF-8 とは限らず
/// （BOM 無しの `.ps1` は Windows PowerShell 5.1 では ANSI = 日本語環境なら CP932）、
/// 一度でも `String` へ lossy 変換して書き戻すと**中身を壊す**。マーカーもブロック本文も
/// ASCII なので、バイトのまま切った貼ったすれば元の符号のまま扱える
fn find_block(text: &[u8]) -> Option<(usize, usize)> {
    let begin = find_bytes(text, BLOCK_BEGIN.as_bytes())?;
    let end_marker = find_bytes(&text[begin..], BLOCK_END.as_bytes())? + begin;
    let after = end_marker + BLOCK_END.len();
    // 終端マーカー行の改行まで飲む（無ければ EOF）
    let end = match find_bytes(&text[after..], b"\n") {
        Some(nl) => after + nl + 1,
        None => text.len(),
    };
    Some((begin, end))
}

/// ブロックを配置した結果のファイル内容（あれば置換、無ければ追記）。
///
/// **追記時に足す区切りは常に改行 1 個**（空ファイルなら 0 個）。これを守ると
/// [`remove_block`] が「ブロック + 直前の改行 1 個」を消すだけで
/// **元のバイト列へ完全に戻せる**（元ファイルが改行で終わっていてもいなくても）
fn apply_block(original: &[u8], block: &str) -> Vec<u8> {
    let block = block.as_bytes();
    if let Some((begin, end)) = find_block(original) {
        let mut out = Vec::with_capacity(original.len() + block.len());
        out.extend_from_slice(&original[..begin]);
        out.extend_from_slice(block);
        out.extend_from_slice(&original[end..]);
        return out;
    }
    if original.is_empty() {
        return block.to_vec();
    }
    let mut out = Vec::with_capacity(original.len() + block.len() + 1);
    out.extend_from_slice(original);
    out.push(b'\n');
    out.extend_from_slice(block);
    out
}

/// ブロックを取り除いた結果のファイル内容。[`apply_block`] が足した改行も戻す
fn remove_block(current: &[u8]) -> Vec<u8> {
    let Some((begin, end)) = find_block(current) else {
        return current.to_vec();
    };
    let mut head = &current[..begin];
    // apply_block が追記したときの区切り改行 1 個ぶんを戻す
    if let Some((b'\n', rest)) = head.split_last() {
        head = rest;
    }
    let mut out = Vec::with_capacity(head.len() + current.len() - end);
    out.extend_from_slice(head);
    out.extend_from_slice(&current[end..]);
    out
}

/// 文字列を **ASCII だけで書かれた** PowerShell の文字列式にする。
///
/// 5.1 は BOM 無しの `.ps1` を ANSI コードページとして読むため、既存プロファイルへ
/// 非 ASCII を追記すると化ける（この機の `$PROFILE` は実際に
/// `…\OneDrive\ドキュメント\PowerShell\` にある）。非 ASCII は `[char]0xNNNN` へ逃がす
fn powershell_ascii_literal(value: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut literal = String::new();
    for unit in value.encode_utf16() {
        // 制御文字も literal に入れない（行が壊れる）
        if (0x20..0x7f).contains(&unit) {
            let ch = char::from(unit as u8);
            if ch == '\'' {
                literal.push_str("''");
            } else {
                literal.push(ch);
            }
        } else {
            if !literal.is_empty() {
                parts.push(format!("'{literal}'"));
                literal.clear();
            }
            parts.push(format!("[char]0x{unit:04X}"));
        }
    }
    if !literal.is_empty() || parts.is_empty() {
        parts.push(format!("'{literal}'"));
    }
    parts.join(" + ")
}

#[cfg(unix)]
mod imp {
    use std::path::{Path, PathBuf};

    use super::{Change, Delivery, Status};

    pub(super) const SHELLS: &str = "zsh / bash / fish";

    pub(super) fn injected_env(root: &Path, bash_path: &Path) -> Vec<(String, String)> {
        let mut env = Vec::new();
        let zsh_dir = root.join("zsh");
        // zsh: ZDOTDIR を統合ディレクトリへ向け、元の値は .zshenv が復元する
        if let Some(orig) = std::env::var_os("ZDOTDIR") {
            env.push((
                "TAKO_ORIG_ZDOTDIR".into(),
                orig.to_string_lossy().into_owned(),
            ));
        }
        env.push(("ZDOTDIR".into(), zsh_dir.display().to_string()));
        // bash: 最初のプロンプトで統合スクリプトを source させる（スクリプト側で置換）
        env.push((
            "PROMPT_COMMAND".into(),
            format!("source '{}'", bash_path.display()),
        ));
        // fish: vendor_conf.d の自動読み込みに乗せる
        let fish_data = root.join("fish-data").display().to_string();
        let xdg = match std::env::var("XDG_DATA_DIRS") {
            Ok(dirs) if !dirs.is_empty() => format!("{fish_data}:{dirs}"),
            // fish の既定検索パスを保つ（XDG_DATA_DIRS を上書きすると既定が消えるため明示）
            _ => format!("{fish_data}:/usr/local/share:/usr/share"),
        };
        env.push(("XDG_DATA_DIRS".into(), xdg));
        env
    }

    /// unix は env 注入だけで完結する（ユーザーのファイルを触らない）
    pub(super) fn status() -> Status {
        Status {
            delivery: Delivery::Automatic,
            script: super::script_root().map(|r| r.join("tako.bash")),
            targets: Vec::new(),
            blocked_by_backend: super::backend_block(),
        }
    }

    pub(super) fn install() -> Result<Vec<Change>, String> {
        ensure_scripts()?;
        Ok(Vec::new())
    }

    pub(super) fn uninstall() -> Result<Vec<Change>, String> {
        Err(
            "この環境のシェル統合は環境変数の注入だけで完結するため、解除する配置がありません"
                .to_string(),
        )
    }

    fn ensure_scripts() -> Result<PathBuf, String> {
        super::write_scripts().map_err(|e| format!("統合スクリプトを書き出せません: {e}"))?;
        super::script_root().ok_or_else(|| "データディレクトリを解決できません".to_string())
    }
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{Change, ChangeKind, Delivery, ProfileTarget, Status};

    pub(super) const SHELLS: &str = "PowerShell 7 / Windows PowerShell 5.1";

    /// Windows は `$PROFILE` 経由なので、spawn 時に注入する環境変数は無い。
    /// POSIX 用の 3 点セットを撒いても PowerShell は 1 つも見ないため、ペインの
    /// 環境を汚さないよう出さない
    pub(super) fn injected_env(_root: &Path, _bash_path: &Path) -> Vec<(String, String)> {
        Vec::new()
    }

    pub(super) fn status() -> Status {
        let script = super::script_root().map(|r| r.join("tako.ps1"));
        let block = script.as_deref().map(super::profile_block);
        let targets = editions()
            .into_iter()
            .filter_map(|(label, exe)| {
                let path = profile_path_cached(&exe)?;
                let content = std::fs::read(&path).unwrap_or_default();
                let found = super::find_block(&content);
                let up_to_date = match (&found, &block) {
                    (Some((b, e)), Some(block)) => &content[*b..*e] == block.as_bytes(),
                    _ => false,
                };
                Some(ProfileTarget {
                    label,
                    exe,
                    path,
                    installed: found.is_some(),
                    up_to_date,
                })
            })
            .collect();
        Status {
            delivery: Delivery::Profile,
            script,
            targets,
            blocked_by_backend: super::backend_block(),
        }
    }

    pub(super) fn install() -> Result<Vec<Change>, String> {
        let root =
            super::script_root().ok_or_else(|| "データディレクトリを解決できません".to_string())?;
        let script = super::write_powershell_script(&root)
            .map_err(|e| format!("統合スクリプトを書き出せません: {e}"))?;
        let block = super::profile_block(&script);

        let targets = status().targets;
        if targets.is_empty() {
            return Err("PowerShell が見つからないため配置先がありません".to_string());
        }
        let mut changes = Vec::new();
        for target in targets {
            if target.installed && target.up_to_date {
                changes.push(Change {
                    label: target.label,
                    path: target.path,
                    kind: ChangeKind::Unchanged,
                });
                continue;
            }
            let kind = if target.installed {
                ChangeKind::Updated
            } else {
                ChangeKind::Installed
            };
            write_profile(&target.path, &block)?;
            changes.push(Change {
                label: target.label,
                path: target.path,
                kind,
            });
        }
        Ok(changes)
    }

    pub(super) fn uninstall() -> Result<Vec<Change>, String> {
        let mut changes = Vec::new();
        for target in status().targets {
            if !target.installed {
                changes.push(Change {
                    label: target.label,
                    path: target.path,
                    kind: ChangeKind::Absent,
                });
                continue;
            }
            let current = std::fs::read(&target.path)
                .map_err(|e| format!("{} を読めません: {e}", target.path.display()))?;
            // BOM もユーザーの記述もバイトのまま持ち回る（符号を勝手に変えない）
            let next = super::remove_block(&current);
            let kind = if split_bom(&next).1.iter().all(u8::is_ascii_whitespace) {
                // 配置のために作ったファイルを残さない
                std::fs::remove_file(&target.path)
                    .map_err(|e| format!("{} を削除できません: {e}", target.path.display()))?;
                ChangeKind::Deleted
            } else {
                std::fs::write(&target.path, &next)
                    .map_err(|e| format!("{} を書き換えられません: {e}", target.path.display()))?;
                ChangeKind::Removed
            };
            changes.push(Change {
                label: target.label,
                path: target.path,
                kind,
            });
        }
        if changes.is_empty() {
            return Err("PowerShell が見つからないため配置先がありません".to_string());
        }
        Ok(changes)
    }

    /// プロファイルへブロックを反映する。
    ///
    /// 新規作成は **BOM 付き UTF-8**（5.1 が BOM 無しを ANSI として読むため）。
    /// 既存ファイルは **エンコーディングを変えない**（ブロック本文は ASCII のみなので、
    /// UTF-8 でも ANSI でも同じバイト列になる）
    fn write_profile(path: &Path, block: &str) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("{} を作成できません: {e}", dir.display()))?;
        }
        let raw = match std::fs::read(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{} を読めません: {e}", path.display())),
        };
        // 中身が無い（新規 / 空）ときだけ BOM を付ける。既存ファイルには足さない
        // （ANSI のファイルへ BOM を付けると別の符号として読まれる）
        let out = if split_bom(&raw).1.is_empty() {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(block.as_bytes());
            out
        } else {
            super::apply_block(&raw, block)
        };
        std::fs::write(path, out)
            .map_err(|e| format!("{} を書き換えられません: {e}", path.display()))
    }

    fn split_bom(raw: &[u8]) -> (&[u8], &[u8]) {
        if raw.starts_with(&[0xEF, 0xBB, 0xBF]) {
            raw.split_at(3)
        } else {
            (&[], raw)
        }
    }

    /// この環境に居る PowerShell のエディション。**両方に配置する**
    /// （片方だけだと、ペインで `powershell` と打った瞬間に統合が消える）
    fn editions() -> Vec<(String, String)> {
        let mut found = Vec::new();
        if let Some(pwsh) = crate::platform::exe::find("pwsh") {
            found.push(("PowerShell 7".to_string(), pwsh));
        }
        if let Some(root) = std::env::var_os("SystemRoot").and_then(|v| v.into_string().ok()) {
            let ps = format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe");
            if Path::new(&ps).exists() {
                found.push(("Windows PowerShell 5.1".to_string(), ps));
            }
        }
        found
    }

    /// 解決した `$PROFILE` のプロセス内キャッシュ。
    /// `status()` は setup の 1 回の実行中に何度も呼ばれるが、1 回あたり PowerShell の
    /// 起動（実測 200〜400ms）なので、素直に呼ぶと体感できるだけ遅くなる
    fn profile_path_cached(exe: &str) -> Option<PathBuf> {
        static CACHE: std::sync::OnceLock<std::sync::Mutex<HashMap<String, Option<PathBuf>>>> =
            std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some(hit) = cache.lock().ok().and_then(|c| c.get(exe).cloned()) {
            return hit;
        }
        let resolved = profile_path(exe);
        if let Ok(mut c) = cache.lock() {
            c.insert(exe.to_string(), resolved.clone());
        }
        resolved
    }

    /// `$PROFILE.CurrentUserAllHosts` を **PowerShell 自身に尋ねる**。
    ///
    /// `%USERPROFILE%\Documents` の決め打ちは OneDrive のフォルダーリダイレクトで外れる
    /// （この機の実測: `…\OneDrive\ドキュメント\PowerShell\profile.ps1`）。
    /// 受け取りは **UTF-8 バイトの 16 進**にする — 5.1 のリダイレクト出力は OEM
    /// コードページなので、生の文字列で受けると日本語のパスが壊れる
    fn profile_path(exe: &str) -> Option<PathBuf> {
        const SCRIPT: &str = "-join ([System.Text.Encoding]::UTF8.GetBytes($PROFILE.CurrentUserAllHosts) | ForEach-Object { '{0:x2}' -f $_ })";
        let mut cmd = Command::new(exe);
        cmd.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            SCRIPT,
        ]);
        crate::platform::process::no_console_window(&mut cmd);
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return fallback_profile_path(exe);
        }
        let hex: String = String::from_utf8_lossy(&out.stdout)
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        decode_hex(&hex)
            .and_then(|b| String::from_utf8(b).ok())
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| fallback_profile_path(exe))
    }

    /// PowerShell に尋ねられなかったときの保険。リダイレクトされていない既定の場所
    fn fallback_profile_path(exe: &str) -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE").filter(|v| !v.is_empty())?;
        let dir = if exe.to_ascii_lowercase().contains("windowspowershell") {
            "WindowsPowerShell"
        } else {
            "PowerShell"
        };
        Some(
            PathBuf::from(home)
                .join("Documents")
                .join(dir)
                .join("profile.ps1"),
        )
    }

    fn decode_hex(hex: &str) -> Option<Vec<u8>> {
        if !hex.len().is_multiple_of(2) {
            return None;
        }
        let bytes = hex.as_bytes();
        let mut out = Vec::with_capacity(hex.len() / 2);
        for pair in bytes.chunks(2) {
            let s = std::str::from_utf8(pair).ok()?;
            out.push(u8::from_str_radix(s, 16).ok()?);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block() -> String {
        profile_block(Path::new("C:\\tako\\tako.ps1"))
    }

    #[test]
    fn ブロックはマーカーで囲まれスクリプトを読み込む() {
        let b = block();
        assert!(b.starts_with(BLOCK_BEGIN), "{b}");
        assert!(b.ends_with(&format!("{BLOCK_END}\n")), "{b}");
        assert!(b.contains("'C:\\tako\\tako.ps1'"), "{b}");
        assert!(b.contains("$env:TAKO_PANE_ID"), "{b}");
    }

    /// 読みやすさのために結果を文字列で見る（テストの入力はすべて UTF-8）
    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn 空プロファイルへの配置は区切りを足さない() {
        assert_eq!(apply_block(b"", &block()), block().into_bytes());
    }

    #[test]
    fn 二回配置してもブロックは一個() {
        let once = apply_block(b"Set-Alias ll Get-ChildItem\n", &block());
        let twice = apply_block(&once, &block());
        assert_eq!(once, twice);
        assert_eq!(
            text(&twice).matches(BLOCK_BEGIN).count(),
            1,
            "{}",
            text(&twice)
        );
    }

    #[test]
    fn 内容が変わったブロックは置換されユーザーの記述は残る() {
        let user = "Set-Alias ll Get-ChildItem\n";
        let old = apply_block(
            user.as_bytes(),
            &profile_block(Path::new("C:\\old\\tako.ps1")),
        );
        let new = text(&apply_block(&old, &block()));
        assert_eq!(new.matches(BLOCK_BEGIN).count(), 1, "{new}");
        assert!(new.contains("C:\\tako\\tako.ps1"), "{new}");
        assert!(!new.contains("C:\\old\\tako.ps1"), "{new}");
        assert!(new.starts_with(user), "{new}");
    }

    /// **受け入れ条件そのもの**: 解除で元のバイト列へ完全に戻る。
    /// 末尾の改行の有無・空ファイル・ブロックの後ろにユーザーが書き足した場合まで見る
    #[test]
    fn 解除は元のバイト列へ完全に戻す() {
        for original in [
            &b""[..],
            b"Set-Alias ll Get-ChildItem\n",
            b"Set-Alias ll Get-ChildItem",          // 改行で終わらない
            b"a\n\n\nb\n",                          // 連続改行
            b"function prompt { 'x> ' }\r\nls\r\n", // CRLF
            // BOM 無しの CP932（`# 日本語コメント` の ANSI 表現）。UTF-8 として妥当でない
            // バイト列でも壊さずに戻せること = ユーザーのプロファイルを破壊しない保証
            b"# \x93\xfa\x96\x7b\x8c\xea\r\nSet-Alias ll Get-ChildItem\r\n",
        ] {
            let installed = apply_block(original, &block());
            assert_eq!(
                remove_block(&installed),
                original,
                "元へ戻らない: {original:?}"
            );
        }
    }

    #[test]
    fn ブロックの後ろに書き足されていても解除できる() {
        let installed = apply_block(b"head\n", &block());
        let mut edited = installed;
        edited.extend_from_slice(b"tail\n");
        assert_eq!(remove_block(&edited), b"head\ntail\n");
    }

    #[test]
    fn 未配置のファイルは解除で変化しない() {
        let text = b"Set-Alias ll Get-ChildItem\n";
        assert_eq!(remove_block(text), text);
    }

    #[test]
    fn asciiのパスはそのまま読める形になる() {
        // ASCII のパスはそのまま読める形
        assert_eq!(
            powershell_ascii_literal("C:\\tako\\tako.ps1"),
            "'C:\\tako\\tako.ps1'"
        );
        // 単引用符は PowerShell 流に二重化
        assert_eq!(powershell_ascii_literal("it's"), "'it''s'");
        assert_eq!(powershell_ascii_literal(""), "''");
    }

    #[test]
    fn 非asciiのパスは_charエスケープへ逃がす() {
        // 実機の $PROFILE が OneDrive の「ドキュメント」配下にある構成を想定
        let got = powershell_ascii_literal("C:\\日\\a.ps1");
        assert!(got.is_ascii(), "非 ASCII が残っている: {got}");
        assert_eq!(got, "'C:\\' + [char]0x65E5 + '\\a.ps1'");
    }

    /// **ブロック全体が ASCII**であること。ユーザーのプロファイルは ANSI（CP932）のことが
    /// あり、そこへ UTF-8 の日本語を混ぜると符号が混在したファイルになる
    #[test]
    fn ブロック全体がasciiになる() {
        // パスに日本語が入る構成（この機の $PROFILE は OneDrive の「ドキュメント」配下）
        for path in ["C:\\tako\\tako.ps1", "C:\\ドキュメント\\tako.ps1"] {
            let b = profile_block(Path::new(path));
            assert!(
                b.is_ascii(),
                "ブロックに非 ASCII が混ざっている（{path}）:\n{b}"
            );
        }
    }

    #[test]
    fn サロゲートペアもエスケープされる() {
        let got = powershell_ascii_literal("a\u{1F419}b");
        assert!(got.is_ascii(), "{got}");
        // UTF-16 の 2 単位に分かれる
        assert_eq!(got, "'a' + [char]0xD83D + [char]0xDC19 + 'b'");
    }

    #[test]
    fn powershellスクリプトはasciiのみ() {
        // 5.1 は BOM 無しの .ps1 を ANSI として読む。非 ASCII を入れると
        // プロファイルへ並べたときに化けるので、同梱スクリプト自体を ASCII に保つ
        assert!(
            POWERSHELL_SCRIPT.is_ascii(),
            "tako.ps1 に非 ASCII が混ざっている"
        );
    }

    #[cfg(unix)]
    #[test]
    fn 統合envはシェル3種ぶんのキーを含む() {
        let env = write_scripts().expect("書き出しに成功する");
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"ZDOTDIR"));
        assert!(keys.contains(&"PROMPT_COMMAND"));
        assert!(keys.contains(&"XDG_DATA_DIRS"));
        // 書き出されたファイルが実在する
        let zdotdir = env
            .iter()
            .find(|(k, _)| k == "ZDOTDIR")
            .map(|(_, v)| PathBuf::from(v))
            .unwrap();
        assert!(zdotdir.join(".zshenv").is_file());
    }
}
