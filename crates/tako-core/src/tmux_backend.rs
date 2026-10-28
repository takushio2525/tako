//! tmux_backend — 全ペインの PTY を tmux セッションとして保持する永続化層（Phase 5.5 / FR-5）
//!
//! シェルを直接 spawn する代わりに、tako 専用の tmux サーバー（`tmux -L tako`。
//! ユーザーの既定サーバーとは分離）上のセッションへ attach するクライアントを spawn する。
//! tako が終了してもセッション（実行中プロセス + 画面内容）は tmux サーバー側に残り、
//! 再起動時に同じセッション名へ attach し直すことで完全復元する。
//!
//! - `new-session -A` により「新規作成」と「再起動後の再 attach」が**同一コマンド**になる
//!   （セッションが生きていれば attach、消えていれば（再起動・kill 後）新規作成）
//! - `-D` で他クライアントを切り離す（多重起動時は最新インスタンスへ収束）
//! - tmux 不在環境では呼び出し側（tako-app）が `crate::backend::capabilities().survives_app_exit` を見て従来の直接 spawn へ
//!   無害に劣化する（ゼロコンフィグ原則）
//! - サーバーは専用 conf（`<data_dir>/tmux-backend.conf`）で起動し、ユーザーの
//!   `~/.tmux.conf` は読まない（status バー・prefix キー等が見えない裏方に徹する）

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::backend::BackendCapabilities;
use crate::paths::data_dir;
use crate::terminal::{SpawnCommand, SpawnOptions};

/// バックエンドセッション名の接頭辞。シェル統合スクリプトが「tako のバックエンド配下か」を
/// 判定する目印（ソケット名も同じ接頭辞）なので変更時はスクリプト側も揃えること
pub const SESSION_PREFIX: &str = "tako-";

/// backend ソケット名を差し替える環境変数（セルフテスト・隔離起動の一括隔離が使う）
pub const SOCKET_ENV: &str = "TAKO_TMUX_SOCKET";

/// 既定の backend ソケット名（`TAKO_TMUX_SOCKET` 未指定時）
pub const DEFAULT_SOCKET: &str = "tako";

/// 専用 tmux サーバーのソケット名（`tmux -L`）。ユーザーの既定サーバーと分離する。
/// `TAKO_TMUX_SOCKET` で差し替え可能（セルフテストの隔離に使う）。
///
/// **他プロセスのソケット名は [`crate::tmux_cleanup::resolve_socket_name`] が復元する**
/// （こちらは自分の環境だけを見る。CLI プロセスで隔離名へ化けないため）
pub fn socket_name() -> String {
    std::env::var(SOCKET_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_SOCKET.into())
}

/// tmux バイナリが実在して動くか（`tmux -V` が成功するか）。プロセス内でキャッシュする。
/// バイナリは `tmux::tmux_bin`（ログインシェル解決込み）で引く（.app の最小 PATH 対策）。
///
/// これは**環境の事実**であって選択ではない。選択は `backend::choice()` が決める
pub fn tmux_binary_present() -> bool {
    static PRESENT: OnceLock<bool> = OnceLock::new();
    *PRESENT.get_or_init(|| {
        crate::tmux::tmux_command(None)
            .arg("-V")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// バックエンドサーバーの設定。見えない裏方として振る舞うための最小構成:
/// - `status off` / `prefix None`: tmux の UI・キー介入を消す（操作はすべて tako 側）
/// - `mouse on`: ホイールを tmux のスクロール（copy-mode）に写す。attach 構成では
///   スクロールバックを tmux が持つため、tako 側の自前スクロールバックの代替になる
/// - `allow-passthrough on`: シェル統合の OSC 7 / 133 をパススルーで外（tako）へ届かせる
/// - `extended-keys` + `terminal-features extkeys`: kitty keyboard / CSI u
///   （Shift+Enter 等の区別。FR の常用要件）を tmux 越しに維持する
/// - `update-environment`: 再 attach 時にセッション環境の TAKO_SOCKET / TAKO_TOKEN /
///   TAKO_MCP_URL を新インスタンスの値へ更新する（既存プロセスには届かないが、
///   それは CLI の control.json フォールバック = FR-2.2.9 が吸収する）。
///   tako mod（#1879）の CLAUDE_CODE_PLUGIN_DIRS / TAKO_CLI も同じく新インスタンスの値へ寄せる
///   （展開先と CLI の置き場はインスタンスごとに違いうる）。
///   TAKO_PANE_ID / TAKO_TAB_ID はペイン固有の値のため update-environment には入れず、
///   `wrap_options` で `new-session -e` により各セッションに直接注入する
/// - `copy-mode-position-format ''`: copy-mode（ホイールスクロール）右上の
///   位置インジケータを消す。tmux 3.6 の既定フォーマットは先頭行タイムスタンプ
///   （`15:13 [10/77]` のような時刻表示）を含み、通常ペインのスクロール中に
///   謎の時刻として見えてしまう（2026-06-12 実機バグ (2)）。
///   スクロール位置は tako 側のスクロールバー（FR-2.5.13）が示す
///
/// **語彙は器によって違う**（#974）。行ごとに「書くために器へ求める能力」を
/// [`Directive::needs`] で宣言し、[`backend_conf`] が能力に合わせて組む。
/// 器の実装名で分岐しないので、将来の器を足しても各行の判断は変わらない
const CONF_HEADER: &str =
    "# tako tmux バックエンド設定（自動生成。手で編集しない。tako-core::tmux_backend）\n";

/// バックエンド設定の 1 行と、それを書くために器へ求める能力（#974）。
///
/// **器は tmux 互換の CLI を名乗っても設定の語彙まで同じではない**。psmux 3.3.7 は
/// `extended-keys` / `extended-keys-format` / `terminal-features` /
/// `copy-mode-position-format` を知らず、書くと 1 行につき 1 件の
/// `psmux: N config warning(s):` が**ペインの出力へ**混ざる（#974 の実機実測）。
/// 画面解析と `tako read` の中身を汚すので、器が解さない行は最初から書かない。
///
/// 落としても器の挙動は変わらない。**警告が出る = その行は元から効いていない**からで、
/// 変わるのはノイズの有無だけである（#974 の安全性の根拠）
struct Directive {
    /// この行を書くために器へ求める能力。`None` はどの器へも書く
    needs: Option<Need>,
    line: &'static str,
}

/// [`Directive`] が求める能力。[`BackendCapabilities`] の 1 マスへ 1:1 で対応する
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Need {
    /// 拡張キー（CSI u / kitty keyboard protocol）の要求を器へ渡せること
    ExtendedKeys,
    /// copy mode の位置インジケータを出さずに済むこと
    SuppressesCopyModeIndicator,
}

impl Need {
    fn met_by(self, caps: &BackendCapabilities) -> bool {
        match self {
            Need::ExtendedKeys => caps.extended_keys,
            Need::SuppressesCopyModeIndicator => caps.suppresses_copy_mode_indicator,
        }
    }
}

const DIRECTIVES: &[Directive] = &[
    Directive {
        needs: None,
        line: "set -g status off",
    },
    Directive {
        needs: None,
        line: "set -g prefix None",
    },
    Directive {
        needs: None,
        line: "set -g mouse on",
    },
    Directive {
        needs: None,
        line: "set -g history-limit 10000",
    },
    Directive {
        needs: None,
        line: "set -g allow-passthrough on",
    },
    Directive {
        needs: None,
        line: "set -g focus-events on",
    },
    Directive {
        needs: None,
        line: "set -g set-clipboard on",
    },
    Directive {
        needs: None,
        line: "set -g default-terminal tmux-256color",
    },
    Directive {
        needs: None,
        line: "set -s escape-time 10",
    },
    Directive {
        needs: Some(Need::ExtendedKeys),
        line: "set -s extended-keys always",
    },
    Directive {
        needs: Some(Need::ExtendedKeys),
        line: "set -sq extended-keys-format csi-u",
    },
    // extkeys の申告が主目的で、RGB（truecolor）が同じ行へ相乗りしている。
    // 器が `terminal-features` を持たないなら申告手段そのものが無いので、
    // 分割しても書けるようにはならない
    Directive {
        needs: Some(Need::ExtendedKeys),
        line: "set -as terminal-features 'xterm*:extkeys:RGB'",
    },
    Directive {
        needs: None,
        line: "set -g update-environment 'TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI'",
    },
    Directive {
        needs: Some(Need::SuppressesCopyModeIndicator),
        line: "set -gq copy-mode-position-format ''",
    },
];

/// 器の能力に合わせてバックエンド設定を組む（**純関数**）。
///
/// 能力を引数で受けるので、**macOS 上から psmux 向けの出力も検査できる**
/// （実機を出さずに #974 の回帰を止められる）。`pub` なのは統合テストが
/// 「本番がその器へ実際に書く中身」を実バイナリに食わせて確かめるため
pub fn backend_conf(caps: &BackendCapabilities) -> String {
    let mut out = String::from(CONF_HEADER);
    for directive in DIRECTIVES {
        if directive.needs.is_none_or(|need| need.met_by(caps)) {
            out.push_str(directive.line);
            out.push('\n');
        }
    }
    out
}

/// いまの器へ書く設定の中身。
///
/// A/B は `TAKO_974_LEGACY=1`（能力を無視して全部書く = #974 前の挙動）と
/// `TAKO_2085_LEGACY=1`（自ら閉じたペインの最後の出力を書き出さない = #2085 前の挙動）
fn conf_body() -> String {
    let legacy = std::env::var("TAKO_974_LEGACY").is_ok_and(|v| v == "1");
    let caps = crate::backend::capabilities();
    let mut body = backend_conf(&apply_legacy(caps, legacy));
    body.push_str(&pane_died_conf(
        &caps,
        pane_died_dir().as_deref(),
        issue2085_legacy(),
    ));
    body
}

/// 自ら閉じたペインの最後の出力の置き場（#2085。`<data_dir>/pane-died/`）
pub const PANE_DIED_DIR: &str = "pane-died";

/// 書き出しの拡張子。名前は `<セッション名>.<history_size>.<history_limit>.<history_bytes>.<alternate_on>.dump`
const PANE_DIED_EXT: &str = "dump";

/// 読まれずに残った書き出しを消すまでの猶予（#2085）。
///
/// 書き出しは器が消えた直後（数 ms〜数百 ms）に tako が読んで消す。残るのは tako が居ない間に
/// 消えた器（次に同じセッション名で器を作るときにも消す）・ペインログを書かない副インスタンスの器・
/// 器の外の `tako-` セッションくらいで、どれも読む人が居ない
pub const PANE_DIED_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(600);

/// #2085 の A/B（`TAKO_2085_LEGACY=1`）。同一バイナリのまま修正前の形へ戻す:
/// 器に `remain-on-exit` / `pane-died` フックを立てず（立っていれば外す）、閉じるときに
/// 書き出しを読まない。**ゲートはこの 1 か所**（tako-app もここを読む）
pub fn issue2085_legacy() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_2085_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// 自ら閉じたペインの最後の出力の置き場（[`PANE_DIED_DIR`]）。データディレクトリが無ければ None
pub fn pane_died_dir() -> Option<PathBuf> {
    data_dir().map(|dir| dir.join(PANE_DIED_DIR))
}

/// 器が自ら閉じるペインの最後の出力を書き出させる設定（#2085。**純関数**）。
///
/// 器の履歴はセッションと一緒に消えるので、出力し続けている最中に中のコマンドが終わると
/// ペインログの最後の取り込みの後の行（#2018 で最大 10 秒ぶん）が入らなかった。
/// `remain-on-exit on` でペインを死んだまま残し、`pane-died` フックで
/// 「全履歴 + 画面」を `capture-pane` → `save-buffer` で書かせてから `kill-pane` する。
/// 書くのは tmux サーバー自身なので**子プロセスは 1 本も増えない**。tako は器が消えて
/// attach クライアントが終わったとき（`CloseOrigin::ProcessExit`）にそれを読む。
///
/// tmux 3.6b の実測で決めた形（`.agent/plans/2026-10-lightweight.md` §12）:
///
/// - **フックの中でコマンドがその場で失敗すると、同じリストの後ろを全部捨てる**
///   （実測: 存在しないバッファの `save-buffer` で `kill-pane` まで捨てられ、死んだペインが
///   残り続けた = **ペインが永久に閉じなくなる**）。失敗しうるコマンドは `if -F` の入れ子
///   （別のリスト）へ隔離し、`kill-pane` は外側に置く。置き場が無いときの書き込みの失敗は
///   後から報告されるだけで後ろを捨てない
/// - **`set-hook` が通らなかったら `remain-on-exit` も立てない**。同じ行に `;` でつなぎ、
///   `set-hook` の失敗で後ろが捨てられるようにする（逆順だと死んだペインが残り続ける）
/// - **`save-buffer` のパスの書式はフックのペインの文脈で展開されない**（複数のペインが
///   同時に死ぬと別のセッションの名前・行数になった）。ターゲットを持つ `set -gF` で
///   フックのペインの文脈のまま名前を作り、`save-buffer` はそのユーザーオプションを読むだけにする
/// - `-t '#{hook_pane}'` は展開されない（`can't find pane`）ので、対象は既定（フックのペイン）に任せる
/// - `remain-on-exit-format ''` が無いと、死んだときに画面を 1 行送って
///   `Pane is dead (…)` を書き込む（履歴が 1 行増え、画面の最後の行が化ける）
/// - 書き出すのは**器の最後のペインが死んだとき**（= セッションが消えるとき）だけ。
///   分割したペインや `tako-` 以外のセッションは書かずに `kill-pane` だけ（`remain-on-exit` off と同じ）
///
/// 能力の無い器（psmux）へは**1 行も書かない**。A/B（`legacy`）と置き場が無いときは、
/// 生きているサーバーに立っていたものを外す行を書く
pub fn pane_died_conf(caps: &BackendCapabilities, dir: Option<&Path>, legacy: bool) -> String {
    if !caps.keeps_exit_output {
        return String::new();
    }
    let Some(dir) = dir.filter(|_| !legacy) else {
        return "set -g remain-on-exit off\nset-hook -gu pane-died\n".to_string();
    };
    format!(
        "set -g @tako-pane-died-dir {dir}
set-hook -g pane-died {{
  if -F '#{{&&:#{{m:{prefix}*,#{{session_name}}}},#{{&&:#{{==:#{{window_panes}},1}},#{{==:#{{session_windows}},1}}}}}}' {{
    set -gF @tako-pane-died-name '#{{session_name}}.#{{history_size}}.#{{history_limit}}.#{{history_bytes}}.#{{alternate_on}}'
    capture-pane -b tako-pane-died -S - -E -
    save-buffer -b tako-pane-died '#{{@tako-pane-died-dir}}/#{{@tako-pane-died-name}}.{PANE_DIED_EXT}'
    delete-buffer -b tako-pane-died
  }}
  kill-pane
}} ; set -g remain-on-exit on ; set -gq remain-on-exit-format ''
",
        dir = conf_quote(&dir.display().to_string()),
        prefix = SESSION_PREFIX,
    )
}

/// tmux の設定ファイルの 1 語として引用する（#2085。データディレクトリのパス用）。
/// 単引用符の中は何も解釈されないので、単引用符を含まなければそれで括る。含むときは
/// 二重引用符で括り、その中で意味を持つ `\` `"` `$` を逃がす
fn conf_quote(word: &str) -> String {
    if !word.contains('\'') {
        return format!("'{word}'");
    }
    let mut out = String::from("\"");
    for c in word.chars() {
        if matches!(c, '\\' | '"' | '$') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// 器が自ら閉じたペインの最後の出力（#2085。`pane-died` フックが書いた全履歴 + 画面）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneDiedDump {
    /// 死んだ瞬間の probe（行数・上限・バイト数・alt screen）
    pub probe: crate::tmux::PaneLogProbe,
    /// 履歴（`probe.history` 行。古い順）
    pub history: Vec<String>,
    /// 死んだ瞬間の画面の行
    pub screen: Vec<String>,
}

/// 書き出しのファイル名から probe を読む（純関数）。`session` のものでなければ None。
/// セッション名に `.` が入っても読めるよう、数値の 4 つは右から切る
pub fn parse_pane_died_name(file_name: &str, session: &str) -> Option<crate::tmux::PaneLogProbe> {
    let rest = file_name
        .strip_prefix(session)?
        .strip_prefix('.')?
        .strip_suffix(PANE_DIED_EXT)?
        .strip_suffix('.')?;
    let mut f = rest.split('.');
    let probe = crate::tmux::PaneLogProbe {
        history: f.next()?.parse().ok()?,
        limit: f.next()?.parse().ok()?,
        bytes: f.next()?.parse().ok()?,
        alternate: match f.next()? {
            "1" => true,
            "0" => false,
            _ => return None,
        },
    };
    f.next().is_none().then_some(probe)
}

/// 書き出しの中身を履歴と画面に分ける（純関数）。`capture-pane -S - -E -` は履歴
/// （`#{history_size}` 行）の後ろに画面の行を出す。履歴に足りなければ壊れた書き出しとして None
pub fn split_pane_died_dump(probe: crate::tmux::PaneLogProbe, text: &str) -> Option<PaneDiedDump> {
    let mut history: Vec<String> = text.lines().map(|l| l.trim_end().to_string()).collect();
    if history.len() < probe.history {
        return None;
    }
    let screen = history.split_off(probe.history);
    Some(PaneDiedDump {
        probe,
        history,
        screen,
    })
}

/// `session` の書き出しを読んで消す（#2085）。無ければ None。
///
/// 1 つのセッションが消えるのは 1 回なので普通は 1 つしか無い。複数あれば
/// 新しいものを読み、残りも消す（読まれない書き出しを溜めない）。
///
/// **器が消えた後に呼ぶ**（#2357）。フックは書き終えてから `kill-pane` するので、消えた後なら
/// 書き終わっている。書きかけ（履歴に足りない）を読むと壊れた書き出しとして消し、残りの
/// 書き込みは名前の無いファイルへ行く = 書き出しが失われる。製品は attach クライアントが
/// 終わった後（`CloseOrigin::ProcessExit`）にだけ呼ぶ
pub fn take_pane_died_dump(dir: &Path, session: &str) -> Option<PaneDiedDump> {
    let mut found: Vec<(std::time::SystemTime, PathBuf, crate::tmux::PaneLogProbe)> =
        std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter_map(|entry| {
                let probe = parse_pane_died_name(entry.file_name().to_str()?, session)?;
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                Some((modified, entry.path(), probe))
            })
            .collect();
    found.sort_by_key(|(modified, _, _)| *modified);
    let (_, newest, probe) = found.pop()?;
    let text = std::fs::read(&newest).ok();
    for path in found.iter().map(|(_, path, _)| path).chain([&newest]) {
        let _ = std::fs::remove_file(path);
    }
    split_pane_died_dump(probe, &String::from_utf8_lossy(&text?))
}

/// `session` の書き出しを読まずに消す（#2085）。器を作る・繋ぎ直すときに呼ぶ:
/// tako が居ない間に消えた器の書き出しを、同じ名前で作り直した器の死と取り違えない
pub fn discard_pane_died_dumps(dir: &Path, session: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let named = entry
            .file_name()
            .to_str()
            .is_some_and(|name| parse_pane_died_name(name, session).is_some());
        if named {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 読まれずに `max_age` を過ぎた書き出しを消す（#2085。[`PANE_DIED_STALE_AFTER`]）。
/// 消した数を返す
pub fn sweep_pane_died_dumps(dir: &Path, max_age: std::time::Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let is_dump = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(&format!(".{PANE_DIED_EXT}")));
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|modified| now.duration_since(modified).unwrap_or_default() >= max_age);
        if is_dump && stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 置き場を作る（#2085）。ペインの中身が入るので本人だけが読める形にする
fn ensure_pane_died_dir(dir: &Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
}

/// A/B の適用（**純関数**）。`TAKO_974_LEGACY=1` は器の能力を無視して全部書く
/// = #974 前の挙動へ戻す。環境変数を読まないのでテストが直列化を要らない
fn apply_legacy(mut caps: BackendCapabilities, legacy: bool) -> BackendCapabilities {
    if legacy {
        caps.extended_keys = true;
        caps.suppresses_copy_mode_indicator = true;
    }
    caps
}

/// ユーザー自前 tmux サーバー（ネスト tmux）向けの推奨設定スニペット（FR-2.17.5）。
/// tako ペイン内で `tmux attach` するユーザーサーバーが既定値のままだと、
/// ホイールのスクロールバック遡り（mouse off で SGR を握り潰す）と
/// Shift+Enter（extended-keys off で kitty 要求を拒否 → 素の Enter に劣化）が
/// ネスト境界で死ぬ（2026-06-12 実機バグ (1)(4) の根因）。
/// FR-2.17 のワンタップ適用・診断はこの定義を正とする。
/// 品質はネストチェーン e2e（ホイール / CSI u）で保証する
pub const NESTED_TMUX_SNIPPET: &str = "\
# tako 連携: tako ペイン内で attach した tmux でもホイール遡りと Shift+Enter を通す
set -g mouse on
# always 必須: tmux はペインからの kitty keyboard 要求（\\e[>1u。Claude Code が使う）を
# 認識しない（modifyOtherKeys 形式のみ）ため、on では S-Enter が素の Enter に劣化する
set -s extended-keys always
set -sq extended-keys-format csi-u
# 外側端末（tako バックエンド = TERM tmux-256color / iTerm2 等 = xterm-256color）が
# 拡張キー対応であることを明示する。これが無いとネスト側が CSI u 入力を解釈せず捨てる
set -as terminal-features 'tmux*:extkeys'
set -as terminal-features 'xterm*:extkeys'
# copy-mode の右上インジケータ（時刻 + [位置/履歴] 表示）を出さない
set -gq copy-mode-position-format ''
";

/// 専用 conf をデータディレクトリへ書き出す（毎起動上書き = バージョン更新追従）。
/// 書けない環境では `/dev/null` を返し「ユーザー conf を読まない」ことだけは維持する。
///
/// **一時ファイル → rename で差し替える**（#625）。`wrap_options` はペインを spawn する
/// たびにここを通るので、複数ペインを同時に立てると「書き手が truncate している最中の
/// conf」を、別ペインが起動した tmux サーバーが `-f` で読みうる。読ませてしまうと
/// サーバーは既定設定（status on / mouse off / extended-keys off / prefix C-b）で
/// 立ち上がり、ステータスバーが出る・ホイールが素通しされない・Shift+Enter が
/// 素の Enter に劣化する（#28 / #167 と同じ症状クラス）。rename は同一ディレクトリ内で
/// 原子的なので、読み手は常に完全な conf を見る。
///
/// `pub` なのはセルフテスト（#772）が**この backend のソケット**へ自分で器を立てるため。
/// そこでサーバーが初めて起きると、後続のペインは同じサーバーを使う（`-f` は起動時にしか
/// 読まれない）ので、製品と同じ conf で起こす（#1918）
pub fn ensure_conf() -> PathBuf {
    // #2085: `pane-died` フックが書く置き場を先に作る（無いと書き出しが失敗する。
    // その場合も `kill-pane` は走るので、ペインが閉じないことにはならない）
    if crate::backend::capabilities().keeps_exit_output && !issue2085_legacy() {
        if let Some(dir) = pane_died_dir() {
            ensure_pane_died_dir(&dir);
        }
    }
    let body = conf_body();
    data_dir()
        .and_then(|dir| write_conf_in(&dir, &body).ok())
        .unwrap_or_else(|| PathBuf::from("/dev/null"))
}

/// conf を `dir` へ原子的に置き、そのパスを返す
fn write_conf_in(dir: &Path, body: &str) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    std::fs::create_dir_all(dir)?;
    let path = dir.join("tmux-backend.conf");
    // tmp 名は **書き込み 1 回ごとに** 固有にする。プロセス固有までしか分けないと、
    // 同時に走った書き手同士が同じ tmp を奪い合い（A が truncate 中に B が rename）、
    // 途中状態がそのまま原子的に差し替わってしまう（この修正を作る過程で実測）。
    // data_dir はプライマリ / セカンダリでも共有されうるので pid も併記する
    let tmp = dir.join(format!(
        "tmux-backend.conf.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, body)?;
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(path)
}

/// 稼働中のバックエンドサーバーへ最新 conf を再適用する。
/// conf は `-f` でサーバー**起動時**にしか読まれず、サーバーは tako の再起動を
/// 生き残る（FR-5 の永続化）ため、tako のバージョン更新で変えた設定が
/// 既存サーバーへ届かない（2026-06-12 実機バグ (2) の温床）。
/// アプリ起動時・persist 有効化時に呼ぶ。サーバー不在なら何もしない（起動もしない）。
///
/// 読まれずに残った自ら閉じたペインの書き出し（#2085）もここで片付ける
pub fn sync_conf(socket: &str) {
    let conf = ensure_conf();
    let _ = crate::tmux::tmux_command(Some(socket))
        .arg("source-file")
        .arg(&conf)
        .output();
    if let Some(dir) = pane_died_dir() {
        sweep_pane_died_dumps(&dir, PANE_DIED_STALE_AFTER);
    }
}

/// SpawnOptions を tmux セッション経由に書き換える。
/// `options.env`（TAKO_* 注入を含む）はクライアント経由でセッション作成時の環境になる。
/// `options.cwd` は `-c` で渡す（既存セッションへの attach では tmux が無視する）
pub fn wrap_options(options: SpawnOptions, socket: &str, session: &str) -> SpawnOptions {
    // #2085: 同じ名前の器の前の死の書き出し（tako が居ない間に消えた器）を、
    // これから作る / 繋ぐ器の死と取り違えないよう先に消す
    if let Some(dir) = pane_died_dir() {
        discard_pane_died_dumps(&dir, session);
    }
    let mut args = vec![
        // UTF-8 を強制する。Finder 起動の .app は LANG / LC_CTYPE が無く、tmux が
        // 非 UTF-8 クライアント扱いで CJK を `_` に置換してしまう（2026-06-12 P0:
        // 日本語が全滅した実機リグレッション）。ロケール非依存の -u が確実
        "-u".to_string(),
        "-L".to_string(),
        socket.to_string(),
        "-f".to_string(),
        ensure_conf().display().to_string(),
        "new-session".to_string(),
        "-A".to_string(),
        "-D".to_string(),
        "-s".to_string(),
        session.to_string(),
    ];
    // ペイン固有の環境変数を tmux new-session -e で直接注入する（tmux 3.2+）。
    // tmux サーバーのグローバル環境は最初のクライアントから継承され、後続セッションも
    // その stale な値を使う。-e はセッション作成時に値を確定させるため、
    // シェル起動後の set-environment（タイミング問題）やクライアント環境の継承に依存しない。
    // **シェル統合の置き場（ZDOTDIR 等）もここに含める**（#1105）: 含めないと、同じ
    // socket 名に別インスタンスのサーバーが残っているときに前のインスタンスの
    // 置き場を指し、OSC 7 / 133 が一切届かなくなる（cwd 追従とコマンド状態が黙って死ぬ）
    for (key, val) in crate::backend::session_pinned_pairs(&options.env, socket) {
        args.push("-e".to_string());
        args.push(format!("{key}={val}"));
    }
    if let Some(cwd) = &options.cwd {
        args.push("-c".to_string());
        args.push(cwd.display().to_string());
    }
    // 内側で動かすコマンド。**未指定時はあえて渡さない**: tmux はコマンド指定があると
    // `default-shell -c <コマンド>` で実行し、この非対話 zsh ラッパーが tako の
    // シェル統合 .zshenv を読んで ZDOTDIR を消費してしまう（内側の対話シェルに
    // 統合が届かなくなる。2026-06-12 のスパイクで判明）。未指定なら tmux が
    // default-shell（$SHELL → passwd の順で解決）をログインシェルとして直接 spawn
    // するので、直接 spawn 時と同じく統合が効く。
    // 明示コマンドは残余引数が空白連結 + sh -c されるため、各語をクォートして 1 引数で渡す。
    // **第 1 語の書き方だけは器によって違う**ので組み立ては backend 側の 1 か所へ（#881）。
    // 素のペインでも、統合を起動引数で読ませる環境（Windows の psmux。#2432）では
    // そのシェルを明示する（器の default-shell は引数なしの pwsh を起こすため）。
    // unix は `pane_shell` が None なので上の理由どおり渡さないまま
    let mut inner = None;
    if let Some(command) = options
        .command
        .clone()
        .or_else(crate::shell_integration::pane_shell)
    {
        inner = Some(crate::backend::inner_command_line(&command));
    }
    // #2367: GUI が fd 上限を引き上げたなら、器の中のペインの子は起動時の値へ戻す段
    // （GUI 自身の実行ファイル）を通す。残余引数が 2 個以上だと tmux は `execvp` で直に起こし
    // （シェルの -c を挟まない = 上の .zshenv の問題は起きない）、段は soft を下げてから tmux が
    // 自分で起こすはずだった形（ログインシェル / `$SHELL -c <inner>`）を exec し直す。
    // このクライアントには引き上げた値が渡り（`TerminalSession::spawn` が並びを見て決める）、
    // それが起こす器の tmux サーバーが受け継ぐ。段を通さないなら従来どおり（A/B は TAKO_2367_LEGACY=1）
    match crate::platform::fd_limit::container_child_argv(inner.as_deref()) {
        Some(argv) => args.extend(argv),
        None => args.extend(inner),
    }
    SpawnOptions {
        command: Some(SpawnCommand {
            program: crate::tmux::tmux_bin().to_string(),
            args,
        }),
        ..options
    }
}

/// SpawnOptions を**既存セッションへ attach するだけ**のクライアントに書き換える（Issue #1857）。
///
/// [`wrap_options`] の `new-session -A` は「無ければ作る」ので、attach クライアントだけが
/// 外から終わった後の再 attach に使うと、その間にセッションが終わっていたときに
/// **黙って新しいシェルが生える**。`attach-session` なら無ければ失敗して終わるので、
/// 呼び出し側は改めて生死を確かめて閉じられる（`crate::backend_reattach`）。
/// `attach-session` はサーバーを起こさないので `-f`（conf）も要らない。
///
/// `-d`（他クライアントの切り離し）は付けない: 再 attach は「他に誰も attach していない」
/// ことを確かめてから撃つもので、その間に誰かが attach したなら奪わない。
/// `options` の env / cwd は attach クライアント自身の環境になるだけ（セッションには効かない）
pub fn reattach_options(options: SpawnOptions, socket: &str, session: &str) -> SpawnOptions {
    let args = vec![
        // UTF-8 の強制は `wrap_options` と同じ理由（Finder 起動の .app に LANG が無い）
        "-u".to_string(),
        "-L".to_string(),
        socket.to_string(),
        "attach-session".to_string(),
        "-t".to_string(),
        crate::tmux::exact_target(session),
    ];
    SpawnOptions {
        command: Some(SpawnCommand {
            program: crate::tmux::tmux_bin().to_string(),
            args,
        }),
        ..options
    }
}

/// PTY 直下に起こすものが器（tmux / psmux）のクライアントか（#2454）。
///
/// [`wrap_options`] / [`reattach_options`] は、どちらも器の実行ファイル（[`crate::tmux::tmux_bin`]）を
/// PTY 直下に起こす。器のサーバーは `new-session -A` が無ければそのクライアントの子として起こすので、
/// ペインの子孫をまとめて止める入れ物（`platform::job`）に入れると、ペインを閉じたとき・
/// GUI を終えたときに器ごと消える（再起動をまたいで残す約束が壊れる）。判断は
/// [`is_container_client_with`] の 1 実装
pub fn is_container_client(command: Option<&SpawnCommand>) -> bool {
    command.is_some_and(|c| is_container_client_with(&c.program, crate::tmux::tmux_bin()))
}

/// [`is_container_client`] の判断部（純粋関数。macOS から Windows の形も検査できる）。
///
/// 解決済みの器の実行ファイルと同じか、名前（`/` と `\` の後ろ）が器の実行ファイル名
/// （[`crate::backend::is_plumbing_process`] = psmux が配る 3 本と tmux）なら器のクライアント。
/// 迷う形は器の側へ倒す（入れ損ねても回収が従来どおりに戻るだけだが、器を入れると利用者の器が消える）
pub fn is_container_client_with(program: &str, tmux_bin: &str) -> bool {
    if program == tmux_bin {
        return true;
    }
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    crate::backend::is_plumbing_process(name)
}

/// 器の中のペインについて、器だけが知っている材料（#1199 で pid を足した）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneFacts {
    /// ペインの tty（`/dev/ttysNNN`）。psmux は実在しない値を返すので当てにしない
    pub tty: Option<String>,
    /// ペインのシェルの pid（`#{pane_pid}`）。側路の書き手の同一性
    /// （[`crate::osc_sink`]）に使う
    pub pid: Option<u32>,
}

/// 器の 1 行から `#{pane_tty}` / `#{pane_pid}` を切り出す（純粋関数）
pub fn parse_pane_facts(line: &str) -> PaneFacts {
    let mut fields = line.split('\t');
    let tty = fields
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let pid = fields
        .next()
        .map(str::trim)
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|pid| *pid != 0);
    PaneFacts { tty, pid }
}

/// バックエンドセッション内ペインについて器へ 1 回だけ問い合わせる（#1199）。
///
/// tty はペイン配下のプロセスの制御端末なので、listen ポート検知（FR-2.4.2）と
/// tmuxview の tty 突き合わせ（FR-2.13.2）がそちらへ差し替える。pid は側路の
/// 待ち合わせ先（`<pane>@p<pid>.osc`）を決める材料で、**同じ 1 回の呼び出しで**採る
/// （器へのプロセス起動を増やさない）。
/// `list-panes` を使う（`display-message -p` はクライアント無しだと空を返す）。
/// セッション未作成・tmux 不在では既定値（呼び出し側がリトライする）
pub fn pane_facts(socket: &str, session: &str) -> PaneFacts {
    let Ok(output) = crate::tmux::tmux_command(Some(socket))
        .args([
            "list-panes",
            "-t",
            &crate::tmux::exact_target(session),
            "-F",
            "#{pane_tty}\t#{pane_pid}",
        ])
        .output()
    else {
        return PaneFacts::default();
    };
    if !output.status.success() {
        return PaneFacts::default();
    }
    parse_pane_facts(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or(""),
    )
}

/// バックエンドセッション内ペインの tty（`/dev/ttysNNN`）。
/// 材料は [`pane_facts`] と同じ 1 回の問い合わせから採る
pub fn pane_tty(socket: &str, session: &str) -> Option<String> {
    pane_facts(socket, session).tty
}

/// セッションの現在の作業ディレクトリを取得する（orphan 復帰時のタブ名推定用）。
///
/// 起動時の orphan 自動復帰（#191）が UI スレッドで呼ぶので、器への問い合わせは
/// 上限つきの [`crate::tmux::run_tmux`] を通す（#1991。修正前は上限なしの `output()`）
pub fn session_cwd(socket: &str, session: &str) -> Option<String> {
    let stdout = crate::tmux::run_tmux(
        Some(socket),
        &[
            "list-panes",
            "-t",
            &crate::tmux::exact_target(session),
            "-F",
            "#{pane_current_path}",
        ],
    )
    .ok()?;
    let path = stdout.lines().next().unwrap_or("").trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// セッション環境変数を読む（`tmux show-environment -t <session> <name>`）。
/// orphan 復元時に `TAKO_ORCHESTRATOR_ROLE` / `TAKO_PANE_ID` を取り出す用途（#210）。
/// 上限つき（#1991。[`session_cwd`] と同じ理由）
pub fn session_env(socket: &str, session: &str, name: &str) -> Option<String> {
    let stdout = crate::tmux::run_tmux(
        Some(socket),
        &[
            "show-environment",
            "-t",
            &crate::tmux::exact_target(session),
            name,
        ],
    )
    .ok()?;
    let line = stdout.trim().to_string();
    // "NAME=value" 形式。`-NAME` は unset を意味する
    line.strip_prefix(&format!("{name}=")).map(str::to_string)
}

/// セッション環境変数を設定する（`tmux set-environment -t <session> <name> <value>`）。
/// orphan 復元後に TAKO_PANE_ID を新 pane ID に更新する用途（#210）。
/// 上限つき（#1991。[`session_cwd`] と同じ理由）
pub fn set_session_env(socket: &str, session: &str, name: &str, value: &str) {
    let _ = crate::tmux::run_tmux(
        Some(socket),
        &[
            "set-environment",
            "-t",
            &crate::tmux::exact_target(session),
            name,
            value,
        ],
    );
}

/// セッションを破棄する（ペインの明示 close 時。tako 終了時は呼ばない = 永続化）。
/// セッションが既に無い（シェル exit で消えた後）のエラーは無害なので潰す
pub fn kill_session(socket: &str, session: &str) {
    let _ = crate::tmux::kill_session(Some(socket), session);
}

/// layout.json に載っていない生存中の `tako-*` セッション（orphan）を返す。
/// [`cleanup_orphans`] と**同じ材料・同じ判定**（`tmux_cleanup::is_orphan`）を通すが、
/// kill せずに名前一覧だけ返す。起動時の自動復帰（#191）で、layout 復元では拾えなかった
/// セッションを発見するのに使う（表示用ラッパー `tako-view-*` は復帰対象にしない）
pub fn find_orphans(socket: &str, protected: &std::collections::HashSet<String>) -> Vec<String> {
    find_orphans_with(socket, protected, crate::tmux_cleanup::legacy_1188())
}

/// `find_orphans` の A/B 用（`legacy` = #1188 前の `session_grouped` 判定）
pub fn find_orphans_with(
    socket: &str,
    protected: &std::collections::HashSet<String>,
    legacy: bool,
) -> Vec<String> {
    use crate::tmux_cleanup::{is_orphan, OrphanPurpose};
    list_session_rows(socket)
        .into_iter()
        .filter(|row| is_orphan(row, protected, OrphanPurpose::Recover, None, 0, legacy))
        .map(|row| row.name)
        .collect()
}

/// `list-sessions` を 1 回引いて行を解く（cleanup / find が同じ材料を見る）
fn list_session_rows(socket: &str) -> Vec<crate::tmux_cleanup::SessionRow> {
    crate::tmux::run_tmux(
        Some(socket),
        &["list-sessions", "-F", crate::tmux_cleanup::LIST_FORMAT],
    )
    .unwrap_or_default()
    .lines()
    .filter_map(crate::tmux_cleanup::parse_session_row)
    .collect()
}

/// 安全設計（誤爆防止の四重ガード）:
/// - **attached**（= いずれかのペイン/クライアントが使用中）は決して触らない
/// - **グループに生きた仲間がいる**（= 表示中ビューの元セッション or その `tako-view-*`
///   ラッパー）も触らない。生きているビューの足元を崩さないため。判定は
///   `#{session_group_size}` > 1 で、**`session_grouped` は使わない**（tmux は
///   メンバーが 1 つになってもグループを消さないので、1 度でも `tako tmux open` した
///   セッションが永久に掃除対象から外れる = #1188）
/// - `protected`（現存ペイン・バックグラウンドペインの backend 名、表示中ビューの元/ラッパー名）は二重の安全網
/// - `min_idle_secs` を指定すると、最終アクティビティ（`session_activity`）がそれより
///   新しいセッションも触らない。起動時の自動実行が「直前まで動いていた実行中セッション」を
///   巻き込まないための猶予（Issue #113: 多重起動の layout.json 汚染で protected から
///   漏れた実行中 worker を、次回起動の自動 cleanup が実プロセスごと kill した）。
///   明示操作（`tako tmux cleanup` / MCP）は None = 従来どおり全対象
///
/// これらにより、ユーザーの実セッション（既定サーバー・非 `tako-` 名）や使用中ビューは
/// 構造上 kill されない。対象は「クラッシュ等で取り残された detached な裸のバックエンド
/// セッション」だけになる
pub fn cleanup_orphans(
    socket: &str,
    protected: &std::collections::HashSet<String>,
    min_idle_secs: Option<u64>,
) -> Vec<String> {
    cleanup_orphans_with(
        socket,
        protected,
        min_idle_secs,
        crate::tmux_cleanup::legacy_1188(),
    )
}

/// `cleanup_orphans` の A/B 用（`legacy` = #1188 前の `session_grouped` 判定）
pub fn cleanup_orphans_with(
    socket: &str,
    protected: &std::collections::HashSet<String>,
    min_idle_secs: Option<u64>,
    legacy: bool,
) -> Vec<String> {
    use crate::tmux_cleanup::{is_orphan, OrphanPurpose};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut killed = Vec::new();
    for row in list_session_rows(socket) {
        if !is_orphan(
            &row,
            protected,
            OrphanPurpose::Cleanup,
            min_idle_secs,
            now,
            legacy,
        ) {
            continue;
        }
        kill_session(socket, &row.name);
        killed.push(row.name);
    }
    killed
}

/// 器の tmux サーバーの fd の上限と使用数（#2367。`check_health` の `fd_limit.container` と
/// persist.log が読む）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerNofile {
    /// サーバーが居ない（`no server running`・ソケットが無い・死んだサーバーのソケットが残っている）
    NotRunning,
    /// 問い合わせが届かない。fd の上限に届いたサーバーは接続を受けても客の fd を受け取れず、
    /// 生きたまま**すべての問い合わせ**を `server exited unexpectedly` で返す（tmux 3.6b の実測）。
    /// `last` はこのプロセスが前に読めた同じソケットのサーバー（pid が生きていれば使用数を数え直す）
    Unreachable {
        reason: String,
        last: Option<ServerLimits>,
    },
    Running(ServerLimits),
}

/// 器の tmux サーバー 1 つの上限と使用数
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerLimits {
    pub pid: u32,
    /// soft（`run-shell` の子が受け継ぐ値 = サーバーの値。tmux は自分の上限を変えない）。読めなければ None
    pub soft: Option<u64>,
    /// hard（None = 無制限か読めない。[`ServerLimits::hard_unlimited`] で分ける）
    pub hard: Option<u64>,
    pub hard_unlimited: bool,
    /// 開いている fd の本数（`proc_pidinfo`。fd を開かずに数える = 上限に届いていても読める）
    pub open: Option<u64>,
}

/// 器の tmux サーバーへの問い合わせの上限（打ち切りは [`ServerNofile::Unreachable`]）
pub const SERVER_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// 器の tmux サーバーが居ても器のペインが 0 本のときに開いている fd（tmux 3.6b の lsof の実測。
/// 器の 1 ペインごとに [`crate::platform::fd_limit::CONTAINER_FDS_PER_PANE`] 本ずつ増える）
pub const SERVER_BASE_FDS: u64 = 6;

/// soft の器の tmux サーバーが持てる器のペインのおおよその数（約 80 ペイン = soft 256 の目安）
pub fn server_pane_capacity(soft: u64) -> u64 {
    soft.saturating_sub(SERVER_BASE_FDS) / crate::platform::fd_limit::CONTAINER_FDS_PER_PANE
}

/// `display-message -p '#{pid}'` の結果を読む（純粋関数）。pid が読めれば Ok、読めなければ
/// サーバーが居ない / 届かない。エラーの字面は tmux 3.6b の実測（[`crate::backend_reattach::classify_probe`] と同じ）
pub fn parse_server_pid(success: bool, stdout: &str, stderr: &str) -> Result<u32, ServerNofile> {
    if let Some(pid) = stdout
        .lines()
        .next()
        .and_then(|l| l.trim().parse::<u32>().ok())
        .filter(|p| *p > 0)
    {
        return Ok(pid);
    }
    let err = stderr.trim();
    if err.starts_with("no server running")
        || (err.starts_with("error connecting to")
            && (err.contains("No such file or directory") || err.contains("Connection refused")))
    {
        return Err(ServerNofile::NotRunning);
    }
    let reason = err
        .lines()
        .next()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if success {
                "応答に pid が無い".to_string()
            } else {
                "エラーの字面なしで失敗した".to_string()
            }
        });
    Err(ServerNofile::Unreachable { reason, last: None })
}

/// `run-shell 'ulimit -Sn; ulimit -Hn'` の出力を読む（純粋関数）: (soft, hard, hard が無制限か)
pub fn parse_server_limits(stdout: &str) -> (Option<u64>, Option<u64>, bool) {
    let mut lines = stdout.lines().map(str::trim);
    let soft = lines.next().and_then(|l| l.parse::<u64>().ok());
    let hard_line = lines.next();
    (
        soft,
        hard_line.and_then(|l| l.parse::<u64>().ok()),
        hard_line == Some("unlimited"),
    )
}

/// このプロセスが最後に読めた器の tmux サーバー（ソケット名つき。上限の読み直しを省く・届かないときの手がかり）
static LAST_SERVER: std::sync::Mutex<Option<(String, ServerLimits)>> = std::sync::Mutex::new(None);

/// 器へ 1 回だけ問い合わせる（上限 [`SERVER_PROBE_TIMEOUT`]）。Ok は (成功したか, stdout, stderr)、
/// Err は打ち切り・起動失敗の理由
fn server_probe(socket: &str, args: &[&str]) -> Result<(bool, String, String), String> {
    use std::process::Stdio;
    let mut command = crate::tmux::tmux_command(Some(socket));
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command
        .spawn()
        .map_err(|e| format!("tmux を起こせない: {e}"))?;
    match crate::probe::wait_with_timeout(child, &format!("tmux {}", args[0]), SERVER_PROBE_TIMEOUT)
    {
        crate::probe::Outcome::Done {
            status,
            stdout,
            stderr,
        } => Ok((
            status.success(),
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
        )),
        crate::probe::Outcome::TimedOut { waited, .. } => Err(format!(
            "{} が {} 秒待っても返らない",
            args[0],
            waited.as_secs()
        )),
        crate::probe::Outcome::Failed { reason, .. } => Err(reason),
    }
}

/// 器の tmux サーバーの fd の上限と使用数を読む（#2367）。サーバーは起こさない
/// （`display-message` も `run-shell` も起動の命令ではない）。
///
/// 2 段に分ける: pid は `display-message` だけで読み、上限は同じサーバー（同じ pid）で 1 度読めたら
/// 覚えておく（tmux は自分の上限を変えない）。上限を読む `run-shell` は `/bin/sh -c` で走る
/// （利用者の rc を読まない）サーバーの子で、サーバーの上限をそのまま受け継ぐが、fork とパイプが要るので
/// **上限の間際では返らない**（soft 64 のサーバーが 62 本のとき 3 秒の打ち切りまで返らなかった実測）。
/// 分けておけば、間際でも pid と使用数（`proc_pidinfo`）と覚えた上限で枯渇を申告できる。
/// UI スレッドから呼ばない（問い合わせ 1〜2 回。それぞれ上限 [`SERVER_PROBE_TIMEOUT`]）
pub fn server_nofile(socket: &str) -> ServerNofile {
    let pid = match server_probe(socket, &["display-message", "-p", "#{pid}"])
        .map_err(|reason| ServerNofile::Unreachable { reason, last: None })
        .and_then(|(ok, out, err)| parse_server_pid(ok, &out, &err))
    {
        Ok(pid) => pid,
        Err(ServerNofile::Unreachable { reason, .. }) => {
            let last = LAST_SERVER.lock().unwrap_or_else(|e| e.into_inner());
            return ServerNofile::Unreachable {
                reason,
                last: last
                    .as_ref()
                    .filter(|(s, l)| s == socket && crate::ports::process_alive(l.pid))
                    .map(|(_, l)| ServerLimits {
                        open: crate::platform::fd_limit::open_fds_of(l.pid),
                        ..*l
                    }),
            };
        }
        Err(other) => return other,
    };
    let known = LAST_SERVER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(s, l)| s == socket && l.pid == pid && l.soft.is_some())
        .map(|(_, l)| (l.soft, l.hard, l.hard_unlimited));
    let (soft, hard, hard_unlimited) = known.unwrap_or_else(|| {
        server_probe(socket, &["run-shell", "ulimit -Sn; ulimit -Hn"])
            .map(|(_, out, _)| parse_server_limits(&out))
            .unwrap_or((None, None, false))
    });
    let limits = ServerLimits {
        pid,
        soft,
        hard,
        hard_unlimited,
        open: crate::platform::fd_limit::open_fds_of(pid),
    };
    if soft.is_some() {
        *LAST_SERVER.lock().unwrap_or_else(|e| e.into_inner()) = Some((socket.to_string(), limits));
    }
    ServerNofile::Running(limits)
}

/// `check_health` が上限を問い合わせる器のソケット（#2367）。テストのプロセスは隔離のソケット
/// （[`SOCKET_ENV`]）を明示したときだけ問い合わせる = 既定の `-L tako`（本番の器）へ run-shell を投げない
pub fn health_probe_socket() -> Option<String> {
    (!crate::paths::is_test_process() || std::env::var_os(SOCKET_ENV).is_some()).then(socket_name)
}

/// persist.log へ置く 1 行（#2367。中身に利用者のデータは無い）
pub fn server_nofile_log_line(
    socket: &str,
    limits: &ServerLimits,
    limit: Option<crate::platform::fd_limit::ContainerLimit>,
    children_soft: Option<u64>,
) -> String {
    use crate::platform::fd_limit::{ContainerLimit, CONTAINER_LEGACY_ENV};
    let head = format!(
        "器の tmux サーバー（ソケット {socket}・pid {}）",
        limits.pid
    );
    let (Some(soft), Some(limit)) = (limits.soft, limit) else {
        return format!("{head}: fd の上限を読めなかった（#2367）");
    };
    let hard = match (limits.hard, limits.hard_unlimited) {
        (_, true) => "無制限".to_string(),
        (Some(h), false) => h.to_string(),
        (None, false) => "?".to_string(),
    };
    let capacity = server_pane_capacity(soft);
    let note = match limit {
        ContainerLimit::Raised => format!(
            "GUI が引き上げた値を受け継いだ。器のペインのシェルには {} を渡す",
            children_soft.map_or_else(|| "?".to_string(), |s| s.to_string())
        ),
        ContainerLimit::Predates => format!(
            "引き上げより前に起きたサーバーなので起動時の値のまま。上がるのは次にサーバーが起きたとき。\
             器の 1 ペインで fd 約 {} 本 = 約 {capacity} ペインで詰まる",
            crate::platform::fd_limit::CONTAINER_FDS_PER_PANE
        ),
        ContainerLimit::Legacy => {
            format!("{CONTAINER_LEGACY_ENV} のため引き上げた値を渡さない。約 {capacity} ペインで詰まる")
        }
        ContainerLimit::LaunchValue => format!(
            "tako 自身が起動時に引き上げていないので起動時の値のまま。約 {capacity} ペインで詰まる"
        ),
    };
    format!(
        "{head}: fd の上限 soft {soft}・hard {hard}（{}: {note}）（#2367）",
        limit.as_str()
    )
}

/// 器の tmux サーバーが起きるのを待つ上限（attach クライアントが非同期に起こす）
const NOTE_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// 前の記録からこの間は読みに行かない（復元で器のペインを何十本も起こしても問い合わせは 1 回）
const NOTE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// 器のペインを起こした直後に呼ぶ: 器の tmux サーバーの上限を persist.log（`sink`）へ 1 行残す
/// （#2367）。書くのはサーバーが変わったとき（pid が前回の記録と違う）だけ。
///
/// 裏のスレッドで読む（UI スレッドを止めない）。読みに行くのは同時に 1 本で、前回から
/// [`NOTE_INTERVAL`] 以内なら行かない
pub fn note_server_nofile(socket: &str, sink: fn(&str)) {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    static BUSY: AtomicBool = AtomicBool::new(false);
    static NOTED_PID: AtomicU32 = AtomicU32::new(0);
    static NOTED_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    if !crate::platform::fd_limit::APPLICABLE {
        return;
    }
    {
        let at = NOTED_AT.lock().unwrap_or_else(|e| e.into_inner());
        if at.is_some_and(|t| t.elapsed() < NOTE_INTERVAL) {
            return;
        }
    }
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let socket = socket.to_string();
    let spawned = std::thread::Builder::new()
        .name("tako-tmux-nofile-note".into())
        .spawn(move || {
            let start = std::time::Instant::now();
            let mut nap = std::time::Duration::from_millis(50);
            let limits = loop {
                match server_nofile(&socket) {
                    ServerNofile::Running(limits) => break Some(limits),
                    _ if start.elapsed() >= NOTE_WAIT => break None,
                    _ => {
                        std::thread::sleep(nap);
                        nap = (nap * 2).min(std::time::Duration::from_millis(400));
                    }
                }
            };
            *NOTED_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
            if let Some(limits) = limits {
                if NOTED_PID.swap(limits.pid, Ordering::AcqRel) != limits.pid {
                    let startup = crate::platform::fd_limit::startup_report();
                    let limit = limits.soft.map(|soft| {
                        crate::platform::fd_limit::classify_container(
                            soft,
                            startup,
                            crate::platform::fd_limit::container_legacy(),
                        )
                    });
                    sink(&server_nofile_log_line(
                        &socket,
                        &limits,
                        limit,
                        startup.and_then(crate::platform::fd_limit::Report::children_soft),
                    ));
                }
            }
            BUSY.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        BUSY.store(false, Ordering::Release);
    }
}

/// バックエンドサーバーごと落とす（セルフテストの後片付け用）
pub fn kill_server(socket: &str) {
    let _ = crate::tmux::tmux_command(Some(socket))
        .arg("kill-server")
        .output();
    remove_socket_file(socket);
}

/// tmux ソケットファイルを削除する（#1192 の残骸回収も同じ 1 実装を使う）。tmux は kill-server 後もファイルを残すことがある。
/// tmux は TMUX_TMPDIR → /tmp の順でソケットディレクトリを決定する（TMPDIR は使わない）。
/// macOS では /tmp → /private/tmp のシンボリックリンク解決でソケット名末尾に `=` が付くため
/// 両方試す
pub fn remove_socket_file(socket: &str) {
    let Some(base) = socket_dir() else { return };
    let _ = std::fs::remove_file(base.join(socket));
    let _ = std::fs::remove_file(base.join(format!("{socket}=")));
}

/// tmux がソケットを置くディレクトリ（`$TMUX_TMPDIR|/tmp` の `tmux-<uid>`）。
/// Windows には tmux もこのレイアウトも存在しないため `None`
#[cfg(unix)]
pub fn socket_dir() -> Option<std::path::PathBuf> {
    let uid = unsafe { libc::getuid() };
    let tmpdir = std::env::var("TMUX_TMPDIR").unwrap_or_else(|_| "/tmp".into());
    Some(std::path::Path::new(&tmpdir).join(format!("tmux-{uid}")))
}

#[cfg(windows)]
pub fn socket_dir() -> Option<std::path::PathBuf> {
    None
}

/// 語のリストを sh -c 安全な 1 つのコマンド文字列へ組み立てる
/// （terminal::login_shell_command とも共有する）
pub(crate) fn shell_quoted(command: &SpawnCommand) -> String {
    std::iter::once(&command.program)
        .chain(command.args.iter())
        .map(|w| crate::shell::quote_for_shell(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// テストの器の名前の接頭辞。**残骸掃除（[`TmuxTestGuard`]）はこの接頭辞の器しか拾わない**
/// ので、lib テストの器はどれかで始め、pid を `-` 区切りの 1 区画で持たせる（#1874）。
///
/// - `tako-coretest-`: 既定
/// - `tk-coretest-`: **`tako` で始まらない名前**が要る検査用（#1105: シェル統合が名前の
///   接頭辞 `tako*` で器を推測しないことを確かめる）。製品の掃除（`tako tmux cleanup --servers`）も
///   この接頭辞（`tmux_cleanup::CORE_TEST_SHORT_PREFIX`）だけは名前の pid の生死で kill する
///   （#1918。`tk*` 全体は手書きの名前なので kill しない = 残骸ソケットの削除だけ）
///
/// 接頭辞から外れた名前（#1874 の時点の `tako-coretest1857-<pid>` / `ct1105-<pid>`）は、
/// テストプロセスが途中で殺されると誰にも回収されない。番犬
/// `crates/tako-control/tests/issue1866_tmux_socket_name_watchdog.rs` がこの定義を
/// ソースから読み、外れた名前を file:line で落とす
#[cfg(test)]
pub(crate) const TEST_SOCKET_PREFIXES: &[&str] = &["tako-coretest-", "tk-coretest-"];

/// テスト用: tmux 隔離ソケットの後始末ガード。
/// 生成時に前回テストの残骸ソケット（[`TEST_SOCKET_PREFIXES`]）を掃除し、Drop でサーバー kill +
/// ソケットファイル削除を行う
#[cfg(test)]
pub(crate) struct TmuxTestGuard(Vec<String>);

#[cfg(test)]
impl TmuxTestGuard {
    pub fn new(sockets: Vec<String>) -> Self {
        Self::cleanup_stale_sockets();
        Self(sockets)
    }

    /// 前回テストの残骸（[`TEST_SOCKET_PREFIXES`] のソケット + ゾンビサーバー）を一括掃除する。
    /// テスト途中の kill -9 で Drop が走らずサーバーが残る場合の回収
    fn cleanup_stale_sockets() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let Some(dir) = socket_dir() else { return };
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if is_stale_socket(name, process_alive) {
                    kill_server(name.trim_end_matches('='));
                }
            }
        });
    }
}

/// テストソケット名（`<接頭辞><用途>-<pid>`）の所有プロセス ID。
/// 抽出は製品コードと 1 実装（#1192 の `owner_pid_candidates`）
#[cfg(test)]
fn socket_owner_pid(name: &str) -> Option<u32> {
    crate::tmux_cleanup::owner_pid_candidates(name)
        .into_iter()
        .next_back()
}

/// 掃除してよい残骸ソケットか。
///
/// **「自分の pid を含まない = 残骸」で判定してはいけない**（#625 の隔離破れ）。
/// 別ブランチの worker が同時に `cargo test` を回すと、後から起動した側の掃除が
/// 先行プロセスの**生きている**サーバーを kill + ソケット削除してしまい、
/// 相手側の tmux e2e が `[server exited]` で総崩れになる（1 掃除で 5 本同時に
/// 落ちるのを実測）。所有プロセスが生きていれば残骸ではない。
///
/// 命名規約から外れて所有者を特定できない名前は**触らない**（安全側に倒す）
#[cfg(test)]
fn is_stale_socket(name: &str, is_alive: impl Fn(u32) -> bool) -> bool {
    // tmux は /tmp → /private/tmp の解決でソケット名末尾に `=` を付けることがある
    let base = name.trim_end_matches('=');
    if !TEST_SOCKET_PREFIXES.iter().any(|p| base.starts_with(p)) {
        return false;
    }
    socket_owner_pid(base).is_some_and(|pid| !is_alive(pid))
}

/// プロセスの生存判定は製品コードと 1 実装（#1192 で `ports::process_alive` へ集約）
#[cfg(test)]
use crate::ports::process_alive;

/// テスト用: 器のソケットファイルがあるか（`=` 付きの別名も見る）。
/// 失敗の診断に載せ、「器は生きているがソケットが消えた」を見分ける（#1866）
#[cfg(all(test, unix))]
fn socket_file_present(socket: &str) -> bool {
    socket_dir()
        .is_some_and(|dir| dir.join(socket).exists() || dir.join(format!("{socket}=")).exists())
}

#[cfg(test)]
impl Drop for TmuxTestGuard {
    fn drop(&mut self) {
        for socket in &self.0 {
            kill_server(socket);
        }
    }
}

/// テスト用: ペインが alt screen（`\033[?1049h`）へ切り替わり終えるのを待つ。
/// 切り替わったら `Some(true)`、10 秒待っても切り替わらなければ最後に観測した値、
/// ペインごと消えていれば `None`。
///
/// **「履歴ゼロ」を切替の証跡に使ってはいけない**（#625 のフレークの根因）。
/// 内側が非対話シェル（`sh -c '…'`）のペインはプロンプトを出さないので
/// スクロールバックが spawn 直後から 0 行であり、`history_size == 0` は
/// 切替を待たずに真になる。切替前にキーを書き込むと、その入力は**通常画面**へ
/// エコーされ、直後の `?1049h` が alt screen を消去するので画面から消える
/// （カーソル桁だけが進んだ状態が残る）。並列負荷でシェルの起動が遅れると
/// この窓が開き、テストが確率的に落ちていた。
/// tmux 自身の `#{alternate_on}` が切替の唯一の直接的な証跡になる
#[cfg(all(test, unix))]
pub(crate) fn wait_alt_screen(socket: &str, session: &str) -> Option<bool> {
    let mut last = None;
    for _ in 0..100 {
        last = alternate_on(socket, session);
        if last == Some(true) {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    last
}

/// アクティブペインが alt screen 表示中か。ペイン不在・tmux 不在では None
#[cfg(all(test, unix))]
fn alternate_on(socket: &str, session: &str) -> Option<bool> {
    let output = crate::tmux::run_tmux(
        Some(socket),
        &[
            "list-panes",
            "-t",
            &crate::tmux::session_pane_target(session),
            "-F",
            "#{pane_active}\t#{alternate_on}",
        ],
    )
    .ok()?;
    output.lines().find_map(|line| {
        let mut f = line.split('\t');
        (f.next()? == "1").then(|| f.next() == Some("1"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // ── #1252: 器（tmux）越しの状態待ちの共通部品 ────────────────────────────
    //
    // **偽のアンカーを二度と使わないための置き場**。バックエンド conf は
    // `set -g mouse on`（このファイルの `DIRECTIVES`）なので、tmux は
    // **クライアント attach の時点で**外側端末（tako の `Term`）のマウスレポートを
    // 有効にする。つまり外側の `TerminalSession::mouse_reporting()` は
    // **内側アプリの `\033[?1000h` とは無関係に**真になる（実測: spawn から 20 ms・
    // その時点で器のペインは `mouse_any_flag=0`）。
    //
    // 要求前にホイールを打つと **tmux 自身がそれを食って copy-mode へ入り**
    // （実測 `pane_in_mode=1`）、以後のホイールは copy-mode のスクロールになるので
    // **待ちをいくら伸ばしても内側アプリへは永久に届かない**。これが #1252 の
    // 「画面が改行だけのまま assert に到達する」の正体で、待ちの長さの問題ではない。
    // 唯一の直接の証跡は器のペイン側のフラグ（`mouse_sgr_flag` / `mouse_any_flag`）。

    /// 状態待ちが予算切れしたときの材料（#1252）。
    /// 規約どおり「**何を待っていたか** / **実際に何が届いたか**」を必ず持つ
    #[cfg(unix)]
    pub(super) struct WaitTimeout {
        /// 診断行の見出し（CI ログを Issue 番号で grep できるようにする）。
        /// 既定は #1252 で、呼び出し側が [`WaitTimeout::tagged`] で名乗り直す
        tag: &'static str,
        what: String,
        observed: String,
        waited: std::time::Duration,
        budget: std::time::Duration,
        attempts: u32,
        busy: Option<f64>,
    }

    #[cfg(unix)]
    impl WaitTimeout {
        /// 観測材料を後から載せる。待ちのあいだ `session` を可変で借りている
        /// 呼び出し側でも、`wait_for_state` から戻ったあとなら画面を読める
        pub(super) fn observed(mut self, observed: impl Into<String>) -> Self {
            self.observed = observed.into();
            self
        }

        /// 診断行の見出しを呼び出し側の Issue 番号にする（#1265）
        pub(super) fn tagged(mut self, tag: &'static str) -> Self {
            self.tag = tag;
            self
        }
    }

    #[cfg(unix)]
    impl std::fmt::Display for WaitTimeout {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "{}: 待っていたもの={} / 届いたもの={} \
                 waited={:.1}s budget={:.1}s attempts={} load={}",
                self.tag,
                self.what,
                self.observed,
                self.waited.as_secs_f64(),
                self.budget.as_secs_f64(),
                self.attempts,
                self.busy
                    .map(|b| format!("{b:.2}"))
                    .unwrap_or_else(|| "unknown".into()),
            )
        }
    }

    /// **状態待ちの共通ドライバ**（#1252）。
    ///
    /// `step` を `tick` 間隔で呼び、`Some` を返したら到達。上限は
    /// `wait_budget::state_wait_budget`（混み具合で**伸ばすだけ**・4 倍で打ち切り）で、
    /// 固定の回数上限は持たない。予算切れなら診断つきの [`WaitTimeout`] を返す。
    ///
    /// `step` は**毎周期の駆動**（イベントの取り込み・ホイールの打ち直し）も担う
    /// = 「1 回だけ打って待つ」形を作れないようにしてある。
    /// 器の死の書き出しのテスト（`pane_died_tests`。#2357）も同じ 1 実装で待つ
    #[cfg(unix)]
    pub(super) fn wait_for_state<T>(
        what: &str,
        base: std::time::Duration,
        tick: std::time::Duration,
        mut step: impl FnMut() -> Option<T>,
    ) -> Result<T, WaitTimeout> {
        let busy = crate::wait_budget::machine_busy();
        let budget = crate::wait_budget::state_wait_budget(base, busy);
        let started = std::time::Instant::now();
        let mut attempts = 0u32;
        loop {
            attempts += 1;
            if let Some(value) = step() {
                return Ok(value);
            }
            if started.elapsed() >= budget {
                return Err(WaitTimeout {
                    tag: "TAKO_1252_WAIT",
                    what: what.to_string(),
                    observed: String::new(),
                    waited: started.elapsed(),
                    budget,
                    attempts,
                    busy,
                });
            }
            std::thread::sleep(tick);
        }
    }

    /// 器のペイン側のマウス要求の状態（#1252）
    #[cfg(unix)]
    #[derive(Clone, Debug, Default)]
    struct PaneMouse {
        /// 内側アプリが何らかのマウスレポートを要求している（`#{mouse_any_flag}`）
        any: bool,
        /// 内側アプリが SGR 形式を要求している（`#{mouse_sgr_flag}`）
        sgr: bool,
        /// 器がペインを copy-mode にしている（`#{pane_in_mode}`。
        /// **要求前にホイールを食われた印**）
        in_mode: bool,
        /// `display-message` の生の応答（`None` = 器を呼べなかった / 失敗した）
        raw: Option<String>,
    }

    #[cfg(unix)]
    impl PaneMouse {
        fn ready(&self) -> bool {
            self.any && self.sgr
        }

        fn describe(&self) -> String {
            format!(
                "器のペイン any={} sgr={} inmode={} 応答={}",
                self.any,
                self.sgr,
                self.in_mode,
                self.raw.as_deref().unwrap_or("器を呼べない")
            )
        }
    }

    /// 器のペインのマウス要求を 1 回問い合わせる（#1252）
    #[cfg(unix)]
    fn probe_pane_mouse(socket: &str, session: &str) -> PaneMouse {
        let output = crate::tmux::tmux_command(Some(socket))
            .args([
                "display-message",
                "-p",
                "-t",
                session,
                // **区切りはカンマ**。空白区切りだと `split_whitespace` が空欄を
                // 畳んでしまい、器が知らないフォーマット（空文字で返る）が
                // 隣の値へずれて無言で誤読する
                "#{mouse_any_flag},#{mouse_sgr_flag},#{pane_in_mode}",
            ])
            .output();
        let out = match output {
            Ok(out) if out.status.success() => out,
            Ok(out) => {
                return PaneMouse {
                    raw: Some(format!(
                        "rc={:?} stderr={:?}",
                        out.status.code(),
                        String::from_utf8_lossy(&out.stderr).trim()
                    )),
                    ..PaneMouse::default()
                }
            }
            Err(e) => {
                return PaneMouse {
                    raw: Some(format!("起動できない: {e}")),
                    ..PaneMouse::default()
                }
            }
        };
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let mut fields = text.split(',');
        let any = fields.next() == Some("1");
        let sgr = fields.next() == Some("1");
        let in_mode = fields.next() == Some("1");
        PaneMouse {
            any,
            sgr,
            in_mode,
            raw: Some(text),
        }
    }

    /// 内側アプリが**器のペイン側で** SGR マウスレポートを要求し終えるのを待つ（#1252）。
    ///
    /// これが唯一の正しいアンカー（理由はこのブロック冒頭）。`drive` は毎周期
    /// 呼ばれるので、`rx` の取り込みが要る呼び出し側はそこへ渡す
    #[cfg(unix)]
    fn wait_pane_mouse_ready(
        socket: &str,
        session: &str,
        mut drive: impl FnMut(),
    ) -> Result<PaneMouse, WaitTimeout> {
        wait_for_state(
            "内側アプリが器のペインで SGR マウスを要求する（#{mouse_sgr_flag}=1）",
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(100),
            || {
                drive();
                let pane = probe_pane_mouse(socket, session);
                pane.ready().then_some(pane)
            },
        )
        .map_err(|e| {
            let pane = probe_pane_mouse(socket, session);
            e.observed(pane.describe())
        })
    }

    /// 器のペインの内容（#1252）。`text` が `None` = **器を呼べなかった**で、
    /// 「呼べたが空だった」と区別できる（旧実装は両方 `""` に潰していたので、
    /// 「1 件も届いていない」の原因が診断から分からなかった）
    #[cfg(unix)]
    #[derive(Clone, Debug, Default)]
    struct PaneCapture {
        text: Option<String>,
        note: String,
    }

    #[cfg(unix)]
    fn capture_pane_text(socket: &str, session: &str) -> PaneCapture {
        match crate::tmux::tmux_command(Some(socket))
            .args(["capture-pane", "-t", session, "-p", "-S", "-", "-J"])
            .output()
        {
            Ok(out) if out.status.success() => {
                let text = String::from_utf8_lossy(&out.stdout).into_owned();
                PaneCapture {
                    note: format!("capture ok bytes={}", text.len()),
                    text: Some(text),
                }
            }
            Ok(out) => PaneCapture {
                text: None,
                note: format!(
                    "capture 失敗 rc={:?} stderr={:?}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
            },
            Err(e) => PaneCapture {
                text: None,
                note: format!("capture を起動できない: {e}"),
            },
        }
    }

    /// A/B: `TAKO_1252_LEGACY=1` は #1252 **前**の待ちへ戻す
    /// （アンカー = 外側 `mouse_reporting()` / ホイールは 1 発 / 固定回数の窓）
    #[cfg(unix)]
    fn legacy_1252() -> bool {
        std::env::var("TAKO_1252_LEGACY").is_ok_and(|v| v == "1")
    }

    /// 注入: 混み具合は再現できないので**遅れそのもの**を入れる（#1252）。
    ///
    /// - `late` = 内側アプリのマウス要求を 3 秒遅らせる（旧経路が確定で FAILED になる）
    /// - `never` = 内側アプリがマウスを要求しない（**新経路でも FAILED になるのが正しい**
    ///   = 状態待ちが本物の回帰を隠さないことの確認）
    #[cfg(unix)]
    fn inject_1252() -> Option<String> {
        std::env::var("TAKO_1252_INJECT")
            .ok()
            .filter(|v| !v.is_empty())
    }

    /// 内側アプリの「SGR マウス要求（+ 任意で kitty 要求）」の一行を注入つきで組む（#1252）
    #[cfg(unix)]
    fn mouse_request_prelude(kitty: bool) -> String {
        let request = if kitty {
            "printf '\\033[?1000h\\033[?1006h\\033[>1u'; "
        } else {
            "printf '\\033[?1000h\\033[?1006h'; "
        };
        match inject_1252().as_deref() {
            // 要求そのものを出さない（検出力の確認）
            Some("never") => String::new(),
            // 要求を 3 秒遅らせる（混んだ機で起きている順序の再現）
            Some("late") => format!("sleep 3; {request}"),
            _ => request.to_string(),
        }
    }

    // ── #1265: シェル統合の OSC 7 を待つ共通部品 ──────────────────────────────
    //
    // **なぜ固定回数の窓をやめるのか**。この下の 3 本は元々どれも
    // `for _ in 0..100 { … sleep 100ms }` = 固定 10 秒窓で、尽きたら
    // tako 側の画面だけを出して panic していた。そのため「打った行のエコーしか
    // 無い」画面から先が読めず、**窓が足りないのか / 器が動いていないのか**を
    // 区別できなかった（#1265 の最初の見立てが「窓不足」で外れたのはこのため）。
    //
    // 測り直しの実測（macOS / tmux 3.6b / 18 コア）:
    //
    // - CPU だけの人工負荷（load 11〜46）で修正前のまま 150 回 → **0 FAILED**。
    //   OSC 7 の到達は 744〜1,171 ms（p50 858 ms・90 サンプル）で、
    //   10 秒窓に対して **8 倍以上の余裕**がある = 窓の長さは効いていない
    // - 元の 8/150 を採った回は `/dev/ttys*` が 404 / `kern.tty.ptmx_max` 511・
    //   機上の tmux が 135 本という状態で、tmux **サーバー**が
    //   `spawn_pane → forkpty → openpty` で止まっている `sample` が採れていた。
    //   つまり真因は**機の PTY 枯渇**（テスト / 製品の欠陥ではない）
    //
    // 予算を伸ばしても PTY 枯渇そのものは救えない。それでも状態待ちへ寄せるのは、
    // **次に落ちたときに原因が診断から分かる**ようにするため（規約
    // `.agent/conventions.md`「セルフテストの待ち条件の書き方」）。予算切れでは
    // 「待っていたもの / 届いたもの」に加えて、**器が見ているペインの状態**
    // （`#{pane_dead}` / `#{pane_current_command}` / `#{pane_current_path}`）と
    // シェル統合の置き場（`ZDOTDIR` と `.zshenv` のバイト数）を必ず出す。
    //
    // この 3 材料で候補が 1 発で割れる:
    //
    // | 診断 | 読み |
    // |---|---|
    // | `pane_current_path` が目的地 | `cd` は実行済み = **パススルー側**の不着 |
    // | `pane_current_command` が `zsh` でない / `capture 失敗` | 器 / シェルがまだ立っていない（PTY 枯渇はここ） |
    // | `zshenv=None` | シェル統合スクリプトが置けていない |

    /// 予算切れのときに OSC 7 の不着の原因を切り分ける材料（#1265）
    #[cfg(unix)]
    #[derive(Default)]
    struct Osc7Probe {
        /// `#{pane_dead},#{pane_current_command},#{pane_current_path}` の生の応答
        /// （`None` = 器を呼べなかった）
        raw: Option<String>,
        /// 器が見ているペインの cwd。**OSC 7 とは独立に** tmux が知っている値なので、
        /// ここが目的地なら `cd` は実行済み = 不着はパススルー側の問題
        pane_path: String,
        /// 器が見ている実行中コマンド（`zsh` でなければシェルがまだ立っていない）
        pane_command: String,
        /// ペインが死んでいるか（`#{pane_dead}`）
        pane_dead: bool,
        /// **セッションへ固定された** `ZDOTDIR`（`None` = 固定されていない。#1105）
        zdotdir: Option<String>,
        /// 器のサーバーが持つグローバルの `ZDOTDIR`（前のインスタンスから継承した値）
        zdotdir_server: Option<String>,
        /// 実際に効く `ZDOTDIR`（セッション優先）の `.zshenv` のバイト数。
        /// `None` = 読めない = シェル統合が置けていない
        zshenv: Option<u64>,
        /// 器のペインの内容（`capture-pane`）。呼べなかったのか空なのかを区別する
        capture: PaneCapture,
    }

    #[cfg(unix)]
    impl Osc7Probe {
        fn describe(&self) -> String {
            let zshenv = self
                .zshenv
                .map(|n| format!("{n}B"))
                .unwrap_or_else(|| "読めない".into());
            // **器の画面の中身は出さない**: 内容は tako 側の画面と同じものになる一方で、
            // ここが割れる場面（器が固まる / 死ぬ）は `capture.note` の
            // `capture 失敗` / `bytes=0` で足りる。画面はプロンプト行に
            // ユーザー名・ホスト名が乗るので、出す回数は 1 つに絞る（#927）
            format!(
                "器のペイン dead={} command={:?} path={:?} 応答={} / \
                 ZDOTDIR セッション={:?} サーバー={:?} .zshenv={} / {}",
                self.pane_dead,
                self.pane_command,
                self.pane_path,
                self.raw.as_deref().unwrap_or("器を呼べない"),
                self.zdotdir,
                self.zdotdir_server,
                zshenv,
                self.capture.note,
            )
        }
    }

    /// 文字列の末尾から空でない行を最大 `n` 本（診断を 1 行に収めるため）
    #[cfg(unix)]
    fn tail_of(text: &str, n: usize) -> String {
        let mut lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        if lines.len() > n {
            lines = lines.split_off(lines.len() - n);
        }
        lines.join(" ⏎ ")
    }

    /// OSC 7 の不着の材料を 1 回採る（#1265）
    #[cfg(unix)]
    fn probe_osc7(socket: &str, session: &str) -> Osc7Probe {
        let mut probe = Osc7Probe {
            capture: capture_pane_text(socket, session),
            // #1105 の核心は「セッションへ固定できたか」なので、セッションと
            // サーバーのグローバルを**並べて**出す（片方だけでは読めない）
            zdotdir: crate::tmux::show_environment(Some(socket), Some(session), "ZDOTDIR"),
            zdotdir_server: crate::tmux::show_environment(Some(socket), None, "ZDOTDIR"),
            ..Osc7Probe::default()
        };
        probe.zshenv = probe
            .zdotdir
            .as_ref()
            .or(probe.zdotdir_server.as_ref())
            .and_then(|d| std::fs::metadata(std::path::Path::new(d).join(".zshenv")).ok())
            .map(|m| m.len());
        // **区切りはカンマ**（理由は `probe_pane_mouse` と同じ。空白区切りは空欄を畳む）
        if let Ok(out) = crate::tmux::tmux_command(Some(socket))
            .args([
                "display-message",
                "-p",
                "-t",
                session,
                "#{pane_dead},#{pane_current_command},#{pane_current_path}",
            ])
            .output()
        {
            if out.status.success() {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let mut fields = text.split(',');
                probe.pane_dead = fields.next() == Some("1");
                probe.pane_command = fields.next().unwrap_or_default().to_string();
                probe.pane_path = fields.next().unwrap_or_default().to_string();
                probe.raw = Some(text);
            } else {
                probe.raw = Some(format!(
                    "rc={:?} stderr={:?}",
                    out.status.code(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
        }
        probe
    }

    /// 診断の採取に**期限**をつける（#1265）。
    ///
    /// この診断がいちばん要るのは「器が応答しない」場面（PTY 枯渇で tmux サーバーが
    /// `openpty` で止まる）だが、`tmux` を叩く採取はどれも素の `output()` = **期限なし**
    /// なので、そのまま呼ぶと**診断ごと固まって FAILED すら出ない**（#1271 と同じ罠）。
    /// 期限切れそのものが「器が応答しない」という最も強い証跡なので、そう出す。
    ///
    /// 採取スレッドは置き去りにする（テストはこの直後に panic する = プロセスが畳まれる）
    #[cfg(unix)]
    fn probe_osc7_bounded(socket: &str, session: &str) -> String {
        const BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
        let (tx, rx) = std::sync::mpsc::channel();
        let (socket, session) = (socket.to_string(), session.to_string());
        std::thread::spawn(move || {
            let _ = tx.send(probe_osc7(&socket, &session).describe());
        });
        rx.recv_timeout(BUDGET).unwrap_or_else(|_| {
            format!(
                "診断の採取が {} 秒で期限切れ（器が応答しない = PTY 枯渇 / \
                 tmux サーバーの停止が濃厚。`ls /dev/ttys* | wc -l` と \
                 `sysctl kern.tty.ptmx_max` を比べること）",
                BUDGET.as_secs()
            )
        })
    }

    /// **シェル統合の OSC 7 で cwd が `want` になるのを待つ**（#1265）。
    ///
    /// 待ちは状態待ち + `state_wait_budget`（混み具合で**伸ばすだけ**・4 倍で打ち切り）で、
    /// 固定の回数上限は持たない。予算切れなら [`probe_osc7_bounded`] の材料つきで返す
    #[cfg(unix)]
    fn wait_osc7_cwd(
        session: &mut crate::TerminalSession,
        rx: &mut futures::channel::mpsc::UnboundedReceiver<crate::SessionEvent>,
        want: &std::path::Path,
        socket: &str,
        session_name: &str,
    ) -> Result<(), WaitTimeout> {
        let outcome = wait_for_state(
            &format!("シェル統合の OSC 7 で cwd が {} になる", want.display()),
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(100),
            || {
                while let Ok(event) = rx.try_recv() {
                    session.process_event(event);
                }
                (session.cwd() == Some(want)).then_some(())
            },
        );
        // ここまで来れば待ちのクロージャは落ちている = `session` をもう一度読める
        match outcome {
            Ok(()) => Ok(()),
            Err(timeout) => {
                let probe = probe_osc7_bounded(socket, session_name);
                let screen = session.visible_lines().join("\n");
                Err(timeout
                    .tagged("TAKO_1265_WAIT")
                    .observed(format!("{probe} / tako 側の画面={:?}", tail_of(&screen, 3))))
            }
        }
    }

    /// A/B: `TAKO_1265_LEGACY=1` は #1265 **前**の待ちへ戻す
    /// （固定 100 回 × 100 ms の窓・予算なし・診断は tako 側の画面だけ）
    #[cfg(unix)]
    fn legacy_1265() -> bool {
        std::env::var("TAKO_1265_LEGACY").is_ok_and(|v| v == "1")
    }

    /// 注入: 混み具合そのものは再現できないので**遅れ / 不着**を入れる（#1265）。
    ///
    /// - `late` = ログインシェルが最初のプロンプトへ着くのを [`INJECT_LATE_SECS`] 秒
    ///   遅らせる（PTY 枯渇で器の起動が伸びたときと同じ形）。**旧アームは固定
    ///   10 秒窓なので確定で FAILED**・新アームは混んだ機なら予算が伸びて通る
    ///   （`state_wait_budget` は伸ばすだけなので、**無負荷では両アームとも
    ///   FAILED になるのが正しい**）
    /// - `nointegration` = シェル統合の置き場を空ディレクトリへ向けて zsh を起こす。
    ///   OSC 7 は**永久に来ない**ので**両アームとも FAILED になるのが正しい**
    ///   = 状態待ちが本物の回帰を隠さないことの確認。
    ///   **`/bin/sh` へ替えるのでは無効化にならない**（実測: 統合は
    ///   `PROMPT_COMMAND` でも届くので macOS の `/bin/sh` = bash が読んでしまう）
    #[cfg(unix)]
    fn inject_1265() -> Option<String> {
        std::env::var("TAKO_1265_INJECT")
            .ok()
            .filter(|v| !v.is_empty())
    }

    /// `late` 注入の遅れ（旧アームの固定 10 秒窓より確実に長い）
    #[cfg(unix)]
    const INJECT_LATE_SECS: u32 = 12;

    /// 注入を反映したログインシェルの置き場（#1265）。
    /// 注入があるときだけ「前処理してから zsh を exec する」包みを一時ディレクトリへ置く
    /// （包みは実行中のシェルが握っているので消せない。**注入したときだけ**
    /// `<temp>/tako-1265-*-<pid>` が残る = 通常の実行では 1 つも作らない）
    #[cfg(unix)]
    fn injected_shell() -> String {
        let prelude = match inject_1265().as_deref() {
            Some("late") => format!("sleep {INJECT_LATE_SECS}\n"),
            Some("nointegration") => {
                let empty =
                    std::env::temp_dir().join(format!("tako-1265-nozdot-{}", std::process::id()));
                let _ = std::fs::create_dir_all(&empty);
                format!("ZDOTDIR='{}'\nexport ZDOTDIR\n", empty.display())
            }
            _ => return "/bin/zsh".into(),
        };
        let path = std::env::temp_dir().join(format!("tako-1265-shell-{}.sh", std::process::id()));
        let _ = std::fs::write(
            &path,
            format!("#!/bin/sh\n{prelude}exec /bin/zsh -i \"$@\"\n"),
        );
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        path.display().to_string()
    }

    /// #974 の判定に使う器の能力は、**器の実装そのものから採る**
    /// （テスト側でハードコードすると本体と無言でずれ、検出力が消える）
    fn tmux_caps() -> BackendCapabilities {
        use crate::backend::SessionBackend;
        crate::backend::TmuxBackend::new().capabilities()
    }

    fn psmux_caps() -> BackendCapabilities {
        use crate::backend::SessionBackend;
        crate::backend::PsmuxBackend::new("psmux".into(), "3.3.7".into()).capabilities()
    }

    /// #974 **前**に tmux の器へ書いていた conf のバイト列（凍結）。
    ///
    /// 能力で出し分ける形へ変えても **macOS の器（tmux）向けの出力は 1 バイトも
    /// 変わらない**ことを固定する。ここが動いたら、tmux サーバーの設定が変わって
    /// ステータスバー・ホイール・Shift+Enter（#28）の前提が崩れているということ
    const TMUX_CONF_BEFORE_974: &str = "\
# tako tmux バックエンド設定（自動生成。手で編集しない。tako-core::tmux_backend）
set -g status off
set -g prefix None
set -g mouse on
set -g history-limit 10000
set -g allow-passthrough on
set -g focus-events on
set -g set-clipboard on
set -g default-terminal tmux-256color
set -s escape-time 10
set -s extended-keys always
set -sq extended-keys-format csi-u
set -as terminal-features 'xterm*:extkeys:RGB'
set -g update-environment 'TAKO_SOCKET TAKO_TOKEN TAKO_MCP_URL CLAUDE_CODE_PLUGIN_DIRS TAKO_CLI'
set -gq copy-mode-position-format ''
";

    /// psmux が知らない設定（#974 の実機実測。1 行につき 1 件の警告がペインへ出る）。
    /// 正本は `backend::psmux::UNKNOWN_OPTIONS` で、一致は下のテストが固定する
    const PSMUX_UNKNOWN_OPTIONS: &[&str] = &[
        "extended-keys",
        "extended-keys-format",
        "terminal-features",
        "copy-mode-position-format",
    ];

    /// **受け入れ 2**: tmux の器向けの conf は #974 の前後でバイト等価
    #[test]
    fn tmuxの器へ書くconfは974前とバイト等価() {
        assert_eq!(backend_conf(&tmux_caps()), TMUX_CONF_BEFORE_974);
    }

    /// **受け入れ 1**: psmux の器向けの conf に psmux が知らない設定が 1 つも無い。
    /// 4 行だけが落ち、それ以外は tmux 向けと同じであることも同時に固定する
    #[test]
    fn psmuxの器へ書くconfは知らないオプションを含まない() {
        let conf = backend_conf(&psmux_caps());
        for unknown in PSMUX_UNKNOWN_OPTIONS {
            assert!(
                !conf.contains(unknown),
                "psmux が知らない設定 {unknown} が残っている（ペインへ警告が出る）:\n{conf}"
            );
        }
        let dropped: Vec<&str> = TMUX_CONF_BEFORE_974
            .lines()
            .filter(|line| !conf.lines().any(|kept| kept == *line))
            .collect();
        assert_eq!(
            dropped,
            vec![
                "set -s extended-keys always",
                "set -sq extended-keys-format csi-u",
                "set -as terminal-features 'xterm*:extkeys:RGB'",
                "set -gq copy-mode-position-format ''",
            ],
            "落ちる行は #974 が実測した 4 行だけのはず"
        );
    }

    /// 拒否される設定の列挙は **器の実装側が 1 つだけ持つ**
    /// （`backend::psmux::UNKNOWN_OPTIONS`）。生成側とテスト側が別々に列挙を抱えると、
    /// 器の実装が変わったときに片方だけ直って無言でずれる。
    ///
    /// 2 つの conf が別々に育つのが #974 の温床だった（psmux 版 conf は正しかったのに、
    /// 本番の spawn 経路は tmux 版の conf を書いていた = #885）
    #[test]
    fn 拒否される設定の列挙は器の実装側が正本() {
        assert_eq!(
            PSMUX_UNKNOWN_OPTIONS,
            crate::backend::psmux::UNKNOWN_OPTIONS,
            "psmux が拒否する設定の列挙が生成側とずれている"
        );
        // 実バイナリで受理を確かめてある conf にも当然入っていない
        for unknown in crate::backend::psmux::UNKNOWN_OPTIONS {
            assert!(!crate::backend::psmux::verified_conf().contains(unknown));
        }
    }

    /// A/B（`TAKO_974_LEGACY=1`）は器の能力を無視して #974 前の全部書きへ戻る。
    /// **環境変数を読まない純関数**として検査するのでテストの直列化が要らない
    #[test]
    fn legacyフラグは能力を無視して全部書く() {
        assert_eq!(
            backend_conf(&apply_legacy(psmux_caps(), true)),
            TMUX_CONF_BEFORE_974
        );
        // 既定（legacy でない）は能力どおり削る
        assert_ne!(
            backend_conf(&apply_legacy(psmux_caps(), false)),
            TMUX_CONF_BEFORE_974
        );
    }

    /// 器が無いとき（`NullBackend`）は conf を書かないが、能力の申告は
    /// 「器で止まるものが無い」= 全部書ける側でなければならない
    /// （器の縮退と混同すると `tako persist` の読み手が誤る）
    #[test]
    fn 器が無いときは拡張キーが器で止まらない() {
        use crate::backend::SessionBackend;
        let none = crate::backend::NullBackend.capabilities();
        assert!(
            none.extended_keys,
            "器が無い = CSI u は tako がそのまま扱う"
        );
        assert!(
            none.suppresses_copy_mode_indicator,
            "器が無い = copy mode が無い"
        );
        assert_eq!(backend_conf(&none), TMUX_CONF_BEFORE_974);
    }

    #[test]
    fn 単語のクォートはシェル安全() {
        use crate::shell::quote_for_shell;
        assert_eq!(quote_for_shell("/bin/zsh"), "/bin/zsh");
        assert_eq!(quote_for_shell("-l"), "-l");
        assert_eq!(quote_for_shell("a b"), "'a b'");
        assert_eq!(quote_for_shell("it's"), r#"'it'\''s'"#);
        assert_eq!(quote_for_shell(""), "''");
        // 先頭 = は zsh の equals 展開を踏むため必ず包む（途中の = は安全）
        assert_eq!(quote_for_shell("=dnd-src"), "'=dnd-src'");
        assert_eq!(quote_for_shell("TMUX="), "TMUX=");
        assert_eq!(
            shell_quoted(&SpawnCommand {
                program: "npm".into(),
                args: vec!["run".into(), "dev server".into()],
            }),
            "npm run 'dev server'"
        );
    }

    /// conf の差し替え中でも、読み手は「完全な conf」しか観測しない（#625）。
    /// `wrap_options` はペイン spawn のたびに conf を書き直すので、複数ペインを同時に
    /// 立てると別ペインが起動する tmux サーバーの `-f` 読み取りと重なる。途中状態を
    /// 読ませると既定設定のサーバーが立ち、ステータスバー表示・ホイール素通し不可・
    /// Shift+Enter 劣化を起こす。
    ///
    /// 検出力は**回数**で区切る（#2098）。以前は 700 ms の実時間の窓で読み取り回数を
    /// 数えて `n > 100` を求めていたので、置き換え中の読み取りが共有違反で `Err` に
    /// なりやすい Windows の負荷の下では 24 回しか読めずに落ちた。いまは成功した
    /// 読み取りと書き換えがそれぞれ規定回数に達するまで回し、`LIMIT` は打ち切りの
    /// 上限としてだけ使う（届かなければ「検出力が無い」で落とす = 意味は以前と同じ）
    #[test]
    fn conf差し替え中も読み手は完全な内容しか見ない() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        // 書き換えも数えるのは、速い機で書き手が 1 度も回らないうちに読み終え、
        // 重なりの無い読み取りだけで通ってしまわないため
        const READS: usize = 200;
        const WRITES: usize = 200;
        const WRITERS: usize = 4;
        const READERS: usize = 2;
        const LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

        let dir = std::env::temp_dir().join(format!("tako-conf-625-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // 器に依らず決定的にするため、中身は 1 回だけ組んで全スレッドで共有する（#974）
        let body = Arc::new(backend_conf(&tmux_caps()));
        let path = write_conf_in(&dir, &body).expect("conf を置ける");

        let stop = Arc::new(AtomicBool::new(false));
        let partial = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let failed_reads = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        // 全員が揃ってから回し始める = 数える読み取りはすべて書き手が回っている間のもの
        let start = Arc::new(Barrier::new(WRITERS + READERS));
        let mut writers = Vec::new();
        for _ in 0..WRITERS {
            let (d, s, b, w, g) = (
                dir.clone(),
                stop.clone(),
                body.clone(),
                writes.clone(),
                start.clone(),
            );
            writers.push(std::thread::spawn(move || {
                g.wait();
                while !s.load(Ordering::Relaxed) {
                    if write_conf_in(&d, &b).is_ok() {
                        w.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }));
        }
        let mut readers = Vec::new();
        for _ in 0..READERS {
            let (p, bad, n, e, w, b, g) = (
                path.clone(),
                partial.clone(),
                reads.clone(),
                failed_reads.clone(),
                writes.clone(),
                body.clone(),
                start.clone(),
            );
            readers.push(std::thread::spawn(move || {
                g.wait();
                let deadline = std::time::Instant::now() + LIMIT;
                while (n.load(Ordering::Relaxed) < READS || w.load(Ordering::Relaxed) < WRITES)
                    && std::time::Instant::now() < deadline
                {
                    match std::fs::read_to_string(&p) {
                        Ok(read) => {
                            n.fetch_add(1, Ordering::Relaxed);
                            if read != *b {
                                bad.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        // Windows では置き換え中の open が共有違反で失敗する。
                        // 途中状態を読んだわけではないので数から外す
                        Err(_) => {
                            e.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }));
        }
        for t in readers {
            t.join().expect("読み手が panic しない");
        }
        // 書き手は読み手が抜けてから止める = 読み取りの全区間で書き換えが走っている
        stop.store(true, Ordering::Relaxed);
        for t in writers {
            t.join().expect("書き手が panic しない");
        }
        let (n, failed, w, bad) = (
            reads.load(Ordering::Relaxed),
            failed_reads.load(Ordering::Relaxed),
            writes.load(Ordering::Relaxed),
            partial.load(Ordering::Relaxed),
        );
        let _ = std::fs::remove_dir_all(&dir);
        // 途中状態を 1 回でも見たら、回数が足りていなくても確定で落ちる
        assert_eq!(bad, 0, "{n} 回中 {bad} 回、不完全な conf を観測した");
        assert!(
            n >= READS && w >= WRITES,
            "読み取り回数が少なすぎて検出力が無い: 読み取り {n} / {READS} 回\
             （失敗 {failed} 回）・書き換え {w} / {WRITES} 回（上限 {LIMIT:?} で打ち切り）"
        );
    }

    /// 残骸ソケットの判定（#625）。
    /// 「自分の pid を含まない = 残骸」だと、別 worker の `cargo test` が同時に走ったとき
    /// 相手の**生きている** tmux サーバーを kill してしまい、tmux e2e が総崩れになる
    #[test]
    fn 生きている別プロセスのテストソケットは残骸ではない() {
        // pid 4242 だけが生きている世界
        let alive = |pid: u32| pid == 4242;
        // 別プロセス（生存）のソケット = 触らない ← #625 の回帰
        assert!(!is_stale_socket("tako-coretest-scr0-4242", alive));
        // macOS の /private/tmp 解決で末尾に `=` が付いた形も同じ判定
        assert!(!is_stale_socket("tako-coretest-scr0-4242=", alive));
        // 所有プロセスが died → 残骸なので掃除してよい
        assert!(is_stale_socket("tako-coretest-scr0-999999", alive));
        assert!(is_stale_socket("tako-coretest-nestw-in-999999", alive));
        // テスト用でないソケット（本番の tako バックエンド等）は対象外
        assert!(!is_stale_socket("tako", |_| false));
        assert!(!is_stale_socket("tako-999999", |_| false));
        // 命名規約から外れて所有者を特定できないものは安全側に倒して触らない
        assert!(!is_stale_socket("tako-coretest-nopid", |_| false));
    }

    /// #1874: `tako` で始まらない名前が要る検査（#1105）の接頭辞も拾い、接頭辞から外れた
    /// 名前は拾わない（= 外れた名前の器はテストプロセスが殺されると誰にも回収されない。
    /// テスト側の名前の揃えは番犬 `issue1866_tmux_socket_name_watchdog.rs` が見張る）
    #[test]
    fn 残骸掃除はテストの器の接頭辞をすべて拾う() {
        let dead = |_: u32| false;
        for prefix in TEST_SOCKET_PREFIXES {
            assert!(
                is_stale_socket(&format!("{prefix}1105-999999="), dead),
                "{prefix}"
            );
        }
        assert!(is_stale_socket("tk-coretest-1105-999999", dead));
        // #1874 の時点で外れていた 2 つ（接頭辞の `-` 抜け / `tako` で始まらない名前）
        assert!(!is_stale_socket("tako-coretest1857-999999", dead));
        assert!(!is_stale_socket("ct1105-999999", dead));
        // 生きている所有者の器は接頭辞が何であれ触らない（#625）
        assert!(!is_stale_socket("tk-coretest-1105-4242", |pid| pid == 4242));
    }

    #[test]
    fn テストソケット名から所有pidを取れる() {
        assert_eq!(socket_owner_pid("tako-coretest-scr0-1234"), Some(1234));
        assert_eq!(socket_owner_pid("tako-coretest-nestw-in-77"), Some(77));
        assert_eq!(socket_owner_pid("tako-coretest-nopid"), None);
    }

    /// 自プロセスは当然「生きている」= 自分のソケットを掃除しない
    #[test]
    #[cfg(unix)]
    fn 自プロセスは生存判定される() {
        assert!(process_alive(std::process::id()));
        assert!(!is_stale_socket(
            &format!("tako-coretest-scr0-{}", std::process::id()),
            process_alive
        ));
    }

    #[test]
    fn wrapはtmux_attach同一コマンドを組み立てる() {
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "echo hi".into()],
            }),
            cwd: Some("/tmp".into()),
            env: vec![("TAKO_PANE_ID".into(), "3".into())],
            scrollback_lines: None,
        };
        let wrapped = wrap_options(options, "tako-test", "tako-abc123");
        let command = wrapped.command.expect("tmux コマンドに置き換わる");
        // バイナリはログインシェル解決で絶対パスになることがある（.app の最小 PATH 対策）
        assert!(command.program.ends_with("tmux"));
        let args = command.args;
        // -L <socket> と new-session -A -D -s <session> を含む
        let l = args.iter().position(|a| a == "-L").unwrap();
        assert_eq!(args[l + 1], "tako-test");
        assert!(args.contains(&"new-session".to_string()));
        assert!(args.contains(&"-A".to_string()));
        assert!(args.contains(&"-D".to_string()));
        let s = args.iter().position(|a| a == "-s").unwrap();
        assert_eq!(args[s + 1], "tako-abc123");
        let c = args.iter().position(|a| a == "-c").unwrap();
        assert_eq!(args[c + 1], "/tmp");
        // 内側コマンドはクォート済みの 1 引数
        assert_eq!(args.last().unwrap(), "/bin/sh -c 'echo hi'");
        // TAKO_PANE_ID は -e フラグでセッション環境に直接注入される
        let e_pos = args.iter().position(|a| a == "-e").unwrap();
        assert_eq!(args[e_pos + 1], "TAKO_PANE_ID=3");
        // env / cwd は維持される（env はクライアントプロセスの環境にもなる）
        assert_eq!(wrapped.env.len(), 1);
        assert_eq!(wrapped.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
    }

    #[test]
    fn コマンド未指定はtmuxの既定シェルに任せる() {
        let wrapped = wrap_options(SpawnOptions::default(), "tako-test", "tako-x");
        let command = wrapped.command.unwrap();
        // コマンドを渡さない（zsh -c ラッパーがシェル統合の ZDOTDIR を消費するのを
        // 避け、tmux がログインシェルを直接 spawn する経路に乗せる）。
        // #1105 でセッション名の後ろに `-e` 対が並ぶようになったので、
        // 「最後の語」ではなく「セッション名の後は `-e` 対だけ」を見る
        let s_at = command
            .args
            .iter()
            .position(|a| a == "-s")
            .expect("-s が要る");
        assert_eq!(command.args[s_at + 1], "tako-x");
        let mut tail = &command.args[s_at + 2..];
        // #2432: 統合を起動引数で読ませる環境（Windows）だけは、統合つきの既定シェルを
        // 内側コマンドとして明示する。unix は None なので従来どおり何も足さない
        if let Some(shell) = crate::shell_integration::pane_shell() {
            let (last, rest) = tail.split_last().expect("内側コマンドがある");
            assert_eq!(last, &crate::backend::inner_command_line(&shell));
            tail = rest;
        }
        assert!(
            tail.chunks(2).all(|c| c.len() == 2 && c[0] == "-e"),
            "セッション名の後に内側コマンドが足されている: {tail:?}"
        );
    }

    /// Issue #113 回帰: 「detached だが直近までアクティブ」なセッション（多重起動事故で
    /// layout.json から漏れた実行中 worker 相当）は、起動時経路（min_idle_secs 付き）では
    /// kill されず、明示操作（None = 従来挙動）では従来どおり kill される。
    /// 修正前の cleanup（猶予なし相当）ならこのセッションは消えていた
    /// #1188: `tako tmux open` で 1 度取り込んだセッションが、ビューを閉じた後に
    /// 掃除対象へ戻ること。実 tmux で「グループは残るがメンバー数は戻る」を踏む
    #[test]
    fn issue1188_ビューを閉じた元セッションは掃除対象へ戻る() {
        // この検査は**本物の tmux のグループ意味論**（`new-session -t` で
        // `#{session_group_size}` が増減する）に依っている。tmux 互換を名乗る別実装
        // （Windows の psmux）は同じ値を返さないので、「器があるか」
        // （`survives_app_exit`）ではなく「本物の tmux を引けるか」で入る（#1278）。
        // 判定は `tmux::announces_only_tmux` の 1 実装（#873 と同じ規則）
        if !crate::tmux_test_gate::real_tmux() {
            return;
        }
        let socket = format!("tako-coretest-{}-grouped", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let orig = "tako-grouped-orig";
        let view = "tako-view-tako-grouped-orig-7";
        let run = |args: &[&str]| {
            crate::tmux::tmux_command(Some(&socket))
                .args(args)
                .output()
                .expect("tmux を実行できる")
        };
        assert!(
            run(&[
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                orig,
                "sleep",
                "300"
            ])
            .status
            .success(),
            "元セッションを作れる"
        );
        let unprotected: std::collections::HashSet<String> = std::collections::HashSet::new();
        let row_of = |name: &str| {
            list_session_rows(&socket)
                .into_iter()
                .find(|r| r.name == name)
                .unwrap_or_else(|| panic!("{name} が list-sessions に無い"))
        };

        // ① 取り込む前は素の orphan
        let row = row_of(orig);
        assert!(!row.grouped_flag && row.group_size.is_none(), "{row:?}");

        // ② ビュー（grouped session）を作ると、元もラッパーも守られる
        assert!(
            run(&["new-session", "-d", "-t", orig, "-s", view])
                .status
                .success(),
            "ビューを作れる"
        );
        for name in [orig, view] {
            let row = row_of(name);
            assert_eq!(row.group_size, Some(2), "{name} のメンバー数: {row:?}");
        }
        assert!(
            cleanup_orphans(&socket, &unprotected, None).is_empty(),
            "表示中ビューがあるあいだは元もラッパーも kill しない"
        );

        // ③ ビューを kill すると `grouped` は 1 のままメンバー数だけ 1 に戻る
        //    （ここが #1188 の本体。旧実装は grouped を見ていたので永久に見送っていた）
        assert!(
            run(&["kill-session", "-t", &crate::tmux::exact_target(view)])
                .status
                .success()
        );
        let row = row_of(orig);
        assert!(
            row.grouped_flag,
            "tmux はメンバーが 1 つでもグループを消さない（前提が崩れたら判定を見直す）: {row:?}"
        );
        assert_eq!(row.group_size, Some(1), "メンバー数は戻る: {row:?}");

        // ④ 修正前（legacy）は見送り、修正後は掃除する
        assert!(
            cleanup_orphans_with(&socket, &unprotected, None, true).is_empty(),
            "#1188 の修正前は永久に見送る（A/B の片側）"
        );
        assert!(
            crate::tmux::session_alive(Some(&socket), orig),
            "legacy アームでは生き残っている"
        );
        assert_eq!(
            cleanup_orphans_with(&socket, &unprotected, None, false),
            vec![orig.to_string()],
            "修正後は掃除対象へ戻る"
        );
        assert!(!crate::tmux::session_alive(Some(&socket), orig));
    }

    #[test]
    #[cfg(unix)]
    fn cleanup_orphansは直近アクティブなdetachedセッションを猶予する() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-grace", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // detached セッションを直接作る（クライアント無し = attached 0。
        // 作成直後なので session_activity は「今」= 実行中 worker の状態を再現）
        let session = "tako-grace-victim";
        let created = crate::tmux::tmux_command(Some(&socket))
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                session,
                "sleep",
                "300",
            ])
            .output()
            .expect("tmux new-session を実行できる");
        assert!(
            created.status.success(),
            "detached セッションを作成できる: {}",
            String::from_utf8_lossy(&created.stderr)
        );
        let unprotected: std::collections::HashSet<String> = std::collections::HashSet::new();
        // 起動時経路（猶予 1 時間）: protected から漏れていても直近アクティブなら生き残る
        let killed = cleanup_orphans(&socket, &unprotected, Some(3600));
        assert!(
            killed.is_empty(),
            "猶予内のセッションは kill されない: {killed:?}"
        );
        assert!(
            crate::tmux::session_alive(Some(&socket), session),
            "セッションが生き残る"
        );
        // 明示操作（猶予なし）: 従来どおり kill される（= 修正前の起動時挙動でもある）
        let killed = cleanup_orphans(&socket, &unprotected, None);
        assert_eq!(
            killed,
            vec![session.to_string()],
            "明示 cleanup は従来どおり kill する"
        );
        assert!(
            !crate::tmux::session_alive(Some(&socket), session),
            "kill 後はセッションが消える"
        );
    }

    /// find_orphans は protected 外の tako-* セッションを返し、
    /// tako-view-* やユーザーセッションは除外する（Issue #191）
    #[test]
    #[cfg(unix)]
    fn find_orphansはprotected外のtakoセッションだけ返す() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-orphan", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // orphan 候補（tako-*）、ビュー（tako-view-*）、ユーザーセッション（user-*）を作成
        for name in &["tako-orphan1", "tako-orphan2", "tako-view-x", "user-sess"] {
            let r = crate::tmux::tmux_command(Some(&socket))
                .args([
                    "-f",
                    "/dev/null",
                    "new-session",
                    "-d",
                    "-s",
                    name,
                    "sleep",
                    "300",
                ])
                .output()
                .expect("tmux new-session");
            assert!(r.status.success(), "セッション {name} の作成に失敗");
        }
        // orphan1 を protected に入れる
        let mut protected = std::collections::HashSet::new();
        protected.insert("tako-orphan1".to_string());
        let orphans = find_orphans(&socket, &protected);
        // orphan2 だけが返る（orphan1 は protected、tako-view-x は除外、user-sess は非 tako-*）
        assert_eq!(orphans, vec!["tako-orphan2".to_string()]);
    }

    /// session_cwd はセッションの現在の作業ディレクトリを返す（Issue #191）
    #[test]
    #[cfg(unix)]
    fn session_cwdはセッションのcwdを返す() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-cwd", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let session = "tako-cwd-test";
        let r = crate::tmux::tmux_command(Some(&socket))
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                session,
                "-c",
                "/tmp",
            ])
            .output()
            .expect("tmux new-session");
        assert!(r.status.success());
        let cwd = session_cwd(&socket, session);
        assert!(cwd.is_some(), "cwd が取得できる");
        let cwd = cwd.unwrap();
        // /tmp は /private/tmp のシンボリックリンクの場合がある
        assert!(cwd == "/tmp" || cwd == "/private/tmp", "cwd = {cwd}");
    }

    /// 永続化の根幹 e2e: クライアント（tako 側）を破棄してもセッションが生き、
    /// 同一コマンドで attach し直すと画面内容ごと戻る。tmux 不在環境ではスキップ
    #[test]
    #[cfg(unix)]
    fn セッションはクライアント切断後もattachで内容ごと戻る() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let session = "tako-e2e-persist";
        // rc ファイルを読まない /bin/sh で決定的に
        let base = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };

        fn wait_for(session: &crate::TerminalSession, needle: &str) -> bool {
            for _ in 0..100 {
                if session
                    .visible_lines()
                    .iter()
                    .any(|line| line.contains(needle))
                {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        }

        // 1 回目: セッション作成 + マーカー出力
        let (first, _rx1) =
            crate::TerminalSession::spawn(80, 24, wrap_options(base.clone(), &socket, session))
                .expect("tmux クライアントを spawn できる");
        // 入力エコーと区別するためクォートを挟む（出力にだけ素の文字列が現れる）
        first.write(b"echo TAKO-PERSIST-'OK'\r".to_vec());
        assert!(
            wait_for(&first, "TAKO-PERSIST-OK"),
            "1 回目のセッションでマーカーが出力される"
        );
        // クライアント破棄（tako 終了相当）。セッションはサーバー側に残る
        drop(first);

        // 2 回目: 同一コマンドで attach → 画面内容が戻っている
        let (second, _rx2) =
            crate::TerminalSession::spawn(80, 24, wrap_options(base, &socket, session))
                .expect("再 attach の tmux クライアントを spawn できる");
        // 画面を落とす: 失敗が「復元できなかった」のか「サーバーごと消えた
        // （= 外から kill された）」のかをログだけで切り分けられるようにする
        assert!(
            wait_for(&second, "TAKO-PERSIST-OK"),
            "再 attach で画面内容が復元される。画面: {:?}",
            second.visible_lines().join("\n")
        );
    }

    #[test]
    fn reattachはattach専用で新しいセッションを作らない() {
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![],
            }),
            cwd: Some("/tmp".into()),
            env: vec![("TAKO_PANE_ID".into(), "3".into())],
            scrollback_lines: None,
        };
        let wrapped = reattach_options(options, "tako-test", "tako-abc123");
        let command = wrapped.command.expect("tmux コマンドに置き換わる");
        assert!(command.program.ends_with("tmux"), "{}", command.program);
        let args = command.args;
        let l = args.iter().position(|a| a == "-L").expect("-L が要る");
        assert_eq!(args[l + 1], "tako-test");
        let t = args
            .iter()
            .position(|a| a == "attach-session")
            .expect("attach-session");
        assert_eq!(args[t + 1], "-t");
        assert_eq!(args[t + 2], crate::tmux::exact_target("tako-abc123"));
        // 無ければ作る語・他のクライアントを切り離す語を持たない
        for forbidden in ["new-session", "-A", "-D", "-d", "-e", "-s"] {
            assert!(
                !args.iter().any(|a| a == forbidden),
                "{forbidden} を含む: {args:?}"
            );
        }
        // 内側コマンドは渡さない（attach 先のセッションが持っている）
        assert_eq!(args.last(), Some(&crate::tmux::exact_target("tako-abc123")));
        assert_eq!(wrapped.cwd.as_deref(), Some(std::path::Path::new("/tmp")));
    }

    /// **#2454 の器の約束**: 器を起こす / 繋ぐ組み立て（新規・復元の `new-session -A` と
    /// 再 attach の `attach-session`）は、どちらも器のクライアントと判定される =
    /// `TerminalSession::spawn_in` がペインの子孫を入れる Job に入れない。
    /// 入れると器のサーバー（クライアントの子として起きる）が Job を受け継ぎ、ペインを閉じたとき・
    /// GUI を終えたときに器ごと消える。素のシェル・明示コマンドのペインは入れる側
    #[test]
    fn issue2454_器を起こす組み立てはjobに入れない側と判定される() {
        let base = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "claude".into()],
            }),
            cwd: Some("/tmp".into()),
            env: vec![("TAKO_PANE_ID".into(), "3".into())],
            scrollback_lines: None,
        };
        assert!(
            !is_container_client(base.command.as_ref()),
            "器でない明示コマンドを器と読んだ（子孫が回収されない）"
        );
        assert!(
            !is_container_client(None),
            "既定シェルのペインを器と読んだ（子孫が回収されない）"
        );
        let wrapped = wrap_options(base.clone(), "tako-test", "tako-2454a");
        assert!(
            is_container_client(wrapped.command.as_ref()),
            "器の新規・復元のクライアントを Job に入れる側と読んだ（器ごと消える）: {:?}",
            wrapped.command
        );
        let reattach = reattach_options(base, "tako-test", "tako-2454a");
        assert!(
            is_container_client(reattach.command.as_ref()),
            "器の再 attach のクライアントを Job に入れる側と読んだ（器ごと消える）: {:?}",
            reattach.command
        );
    }

    /// [`is_container_client_with`] の形（Windows の psmux の置き場も macOS から検査する）
    #[test]
    fn issue2454_器の実行ファイルは名前と解決済みのパスで見分ける() {
        const PSMUX: &str = r"C:\Users\winuser\AppData\Local\psmux\tmux.exe";
        // 解決済みの器の実行ファイルそのもの
        assert!(is_container_client_with(PSMUX, PSMUX));
        assert!(is_container_client_with("tmux", "tmux"));
        // 名前で見分ける（大小・拡張子・区切りの違い）
        for program in [
            "/opt/homebrew/bin/tmux",
            r"C:\Tools\PSMUX.EXE",
            r"C:\Tools\psmux.exe",
            r"C:\Tools\pmux.exe",
            "psmux",
            "TMUX.exe",
        ] {
            assert!(
                is_container_client_with(program, PSMUX),
                "{program} を器でないと読んだ"
            );
        }
        // 器でないもの（名前に tmux を含むだけのもの・シェル・agent）
        for program in [
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
            "pwsh",
            r"C:\Windows\System32\cmd.exe",
            "/bin/zsh",
            "claude",
            r"C:\Tools\tmux-helper.exe",
            "/usr/local/bin/tmuxinator",
        ] {
            assert!(
                !is_container_client_with(program, PSMUX),
                "{program} を器と読んだ（子孫が回収されない）"
            );
        }
    }

    /// #1857: attach クライアント**だけ**を外から SIGTERM しても、セッションと中のシェルは
    /// 生きていて、attach 専用のクライアントで画面ごと戻る。セッションが終わった後の
    /// attach 専用クライアントは**新しいセッションを作らずに**失敗する。
    ///
    /// kill するのは隔離ソケットの `list-clients` で引いた**自分のクライアントの pid だけ**
    /// （名前・パターン指定の kill は使わない = 事故の原因そのもの）
    #[test]
    #[cfg(unix)]
    fn issue1857_クライアントだけ殺されてもattach専用で戻りセッションが無ければ作らない() {
        use crate::backend_reattach::{probe_session, ClientExit, ReattachState, SessionProbe};
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-1857-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let session = "tako-e2e-1857";
        let base = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        fn wait_for(session: &crate::TerminalSession, needle: &str) -> bool {
            for _ in 0..100 {
                if session.visible_lines().iter().any(|l| l.contains(needle)) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        }
        /// 端末のイベントを流し、子の終わり（ChildExit）が届いたら true。
        /// 上限は 600 回 × 100 ms（#2098: 10 秒では並列の負荷の下で届かない回があった）。
        /// 状態で待つので、届く回は上限に依らずすぐ抜ける
        fn wait_child_exit(
            session: &mut crate::TerminalSession,
            rx: &mut futures::channel::mpsc::UnboundedReceiver<crate::SessionEvent>,
        ) -> bool {
            for _ in 0..600 {
                while let Ok(event) = rx.try_recv() {
                    let child_exit = matches!(
                        event,
                        crate::SessionEvent::Term(crate::TermEvent::ChildExit(_))
                    );
                    session.process_event(event);
                    if child_exit {
                        return true;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        }
        /// 子の終わりが届かなかったときの切り分け（#2098）。`ps` の状態が実行中なら
        /// tmux がクライアントをまだ終わらせていない（器の側の遅れ）、`Z` なら終わったのに
        /// ChildExit が届いていない（イベントの側の遅れ）、載らなければ既に回収済み
        fn client_state(pid: Option<u32>) -> String {
            let Some(pid) = pid else {
                return "pid 不明".into();
            };
            match std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
            {
                Ok(out) if out.status.success() => format!(
                    "pid {pid} の状態={:?}",
                    String::from_utf8_lossy(&out.stdout).trim()
                ),
                Ok(_) => format!("pid {pid} は居ない"),
                Err(e) => format!("ps を起動できない: {e}"),
            }
        }
        let client_of = |name: &str| {
            for _ in 0..50 {
                if let Some((pid, _)) = crate::tmux::list_client_pids(Some(&socket))
                    .into_iter()
                    .find(|(_, s)| s == name)
                {
                    return Some(pid);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            None
        };
        // サーバーが最後のセッションと一緒に消えない（exit-empty）よう、別のセッションを置く。
        // 器を起こすのはこの行なので `-f /dev/null` で利用者の `~/.tmux.conf` を読ませない（#1874）
        let _ = crate::tmux::tmux_command(Some(&socket))
            .args([
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "tako-e2e-1857-keep",
                "sleep 600",
            ])
            .status();

        let (mut first, mut rx1) =
            crate::TerminalSession::spawn(80, 24, wrap_options(base.clone(), &socket, session))
                .expect("tmux クライアントを spawn できる");
        first.write(b"echo TAKO-1857-'A'\r".to_vec());
        assert!(wait_for(&first, "TAKO-1857-A"), "1 回目のマーカー");
        let pid = client_of(session).expect("自分のクライアントが list-clients に載る");
        assert_eq!(
            first.child_pid(),
            Some(pid),
            "PTY の子 = attach クライアント（自分の pid を『別のクライアント』と数えない根拠）"
        );
        // SAFETY: 隔離ソケットの list-clients で引いた、このテストが立てたクライアントの pid
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        assert!(
            wait_child_exit(&mut first, &mut rx1),
            "attach クライアントの終わりが届く。クライアント: {}",
            client_state(first.child_pid())
        );
        // tmux のクライアントは SIGTERM を受けると後始末をして exit 1 で終わる
        assert_eq!(
            ClientExit::from_status(first.exit_status()),
            ClientExit::Code(1)
        );
        let probe = probe_session(&socket, session);
        let SessionProbe::Alive { clients } = &probe else {
            panic!("クライアントだけ死んだのでセッションは生きている: {probe:?}");
        };
        assert!(
            clients.iter().all(|c| *c == pid),
            "他のクライアントは居ない: {clients:?}"
        );
        let mut state = ReattachState::new();
        let verdict = state.decide(&probe, Some(pid), std::time::Instant::now());
        assert!(
            matches!(
                verdict,
                crate::backend_reattach::Verdict::Reattach { attempt: 1, .. }
            ),
            "{verdict:?}"
        );

        // attach 専用のクライアントで戻る: 1 回目の出力が残り、同じシェルが続きを受け付ける
        let (mut second, mut rx2) =
            crate::TerminalSession::spawn(80, 24, reattach_options(base.clone(), &socket, session))
                .expect("attach 専用のクライアントを spawn できる");
        assert!(
            wait_for(&second, "TAKO-1857-A"),
            "再 attach で画面内容が戻る。画面: {:?}",
            second.visible_lines().join("\n")
        );
        second.write(b"echo TAKO-1857-'B'\r".to_vec());
        assert!(
            wait_for(&second, "TAKO-1857-B"),
            "同じシェルが続きを受け付ける"
        );

        // セッションを終わらせると、attach 中のクライアントも終わり、生死は「無い」になる。
        // **kill の結果を捨てない**（#1866: ここで落ちた回は理由が 1 ビットも残らず、
        // 「kill が器へ届いていない」のか「クライアントの終わりが届かない」のかを
        // 切り分けられなかった）
        let killed = crate::tmux::kill_session(Some(&socket), session);
        let exited = wait_child_exit(&mut second, &mut rx2);
        assert!(
            exited,
            "セッションの終わりでクライアントも終わる。kill-session={killed:?} 生死={:?} \
             ソケットファイル={} クライアント: {}",
            probe_session(&socket, session),
            socket_file_present(&socket),
            client_state(second.child_pid()),
        );
        assert_eq!(probe_session(&socket, session), SessionProbe::SessionGone);

        // 無いセッションへの attach 専用クライアントは、作らずに失敗して終わる
        let (mut third, mut rx3) =
            crate::TerminalSession::spawn(80, 24, reattach_options(base, &socket, session))
                .expect("spawn 自体はできる");
        assert!(
            wait_child_exit(&mut third, &mut rx3),
            "無いセッションへの attach は作らずに終わる。クライアント: {}",
            client_state(third.child_pid())
        );
        assert!(
            !crate::tmux::has_session(Some(&socket), session),
            "attach 専用のクライアントが新しいセッションを作っていない"
        );
    }

    /// シェル統合の OSC 7 が tmux パススルー（allow-passthrough + スクリプトの包み直し）で
    /// tako 側の TapPty まで届くことの e2e（FR-2.4.1 × Phase 5.5 の共存検証）。
    /// zsh / tmux が無い環境ではスキップ
    #[test]
    #[cfg(unix)]
    fn osc7はtmuxパススルーで外へ届く() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        if !std::path::Path::new("/bin/zsh").exists() {
            crate::tmux_test_gate::skip("zsh が無い環境");
            return;
        }
        let socket = format!("tako-coretest-osc-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // シェル統合（ZDOTDIR 等）+ TAKO_PANE_ID（統合スクリプトの発動条件）。
        // コマンドは指定しない = tmux の default-shell（SHELL 環境変数）経由で
        // ログインシェルが直接 spawn され、シェル統合が本番と同じ経路で効く
        let mut env: Vec<(String, String)> = crate::shell_integration::env().to_vec();
        env.push(("TAKO_PANE_ID".into(), "1".into()));
        env.push(("SHELL".into(), injected_shell()));
        let options = SpawnOptions {
            command: None,
            cwd: Some("/".into()),
            env,
            scrollback_lines: None,
        };
        let session_name = "tako-e2e-osc";
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session_name))
                .expect("tmux クライアントを spawn できる");
        session.write(b"cd /private/tmp\r".to_vec());
        // TAKO_1265_LEGACY_ARM 開始（#1265 前の固定 10 秒窓を再現する A/B のアーム）
        if legacy_1265() {
            for _ in 0..100 {
                while let Ok(event) = rx.try_recv() {
                    session.process_event(event);
                }
                if session.cwd() == Some(std::path::Path::new("/private/tmp")) {
                    return; // OSC 7 がパススルーで届いた
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            panic!(
                "OSC 7 が届かない。画面: {:?}",
                session.visible_lines().join("\n")
            );
        }
        // TAKO_1265_LEGACY_ARM 終了
        if let Err(e) = wait_osc7_cwd(
            &mut session,
            &mut rx,
            std::path::Path::new("/private/tmp"),
            &socket,
            session_name,
        ) {
            panic!("OSC 7 が届かない。{e}");
        }
    }

    /// #1105 回帰: **器のサーバーが別インスタンスのシェル統合を指している**状態でも
    /// OSC 7 が届く。
    ///
    /// tmux サーバーのグローバル環境は最初のクライアントから継承されるので、同じ
    /// socket 名に前のインスタンスのサーバーが残っていると `ZDOTDIR` が前の（消えて
    /// いるかもしれない）置き場を指す。`wrap_options` が統合の置き場を `-e` で
    /// セッションへ固定しないと、統合が読み込まれず cwd 追従が黙って死ぬ。
    ///
    /// **`options.env` には統合を入れない**のが要点: production では
    /// [`crate::TerminalSession::spawn`] が**外側 PTY** の env へ足すので
    /// `wrap_options` からは見えない（既存の `osc7はtmuxパススルーで外へ届く` は
    /// テストが自分で `options.env` へ入れているため、この穴を踏まない）
    #[test]
    #[cfg(unix)]
    fn 器のサーバーが別インスタンスを指していてもosc7が届く() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        if !std::path::Path::new("/bin/zsh").exists() {
            crate::tmux_test_gate::skip("zsh が無い環境");
            return;
        }
        let socket = format!("tako-coretest-stale-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // 前のインスタンス相当のサーバーを先に立てる。**tako の conf を読ませる**
        // （素通し設定は on = 原因を ZDOTDIR の継承だけに絞る）。
        // 置き場は空ディレクトリ = 統合が読み込めない値
        let stale = std::env::temp_dir().join(format!("tako-stale-zdotdir-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&stale);
        let conf = ensure_conf();
        let started = crate::tmux::tmux_command(Some(&socket))
            .args([
                "-f",
                &conf.display().to_string(),
                "new-session",
                "-d",
                "-s",
                "stale-pre",
                "sleep",
                "120",
            ])
            .env("ZDOTDIR", &stale)
            // `default-shell` は**サーバー起動時の環境**から決まるので、ここで
            // 明示しないと器の中のシェルが「テストランナーの SHELL」になる。
            // 注入（#1265 の `late` / `nointegration`）もここを通さないと
            // このテストにだけ効かない
            .env("SHELL", injected_shell())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !started {
            crate::tmux_test_gate::skip("先行サーバーを立てられない");
            let _ = std::fs::remove_dir_all(&stale);
            return;
        }
        // production と同じ形: 統合は options.env に入れない
        let options = SpawnOptions {
            command: None,
            cwd: Some("/".into()),
            env: vec![
                ("TAKO_PANE_ID".into(), "1".into()),
                ("SHELL".into(), injected_shell()),
            ],
            scrollback_lines: None,
        };
        let session_name = "tako-e2e-stale";
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session_name))
                .expect("tmux クライアントを spawn できる");
        session.write(b"cd /private/tmp\r".to_vec());
        // TAKO_1265_LEGACY_ARM 開始（#1265 前の固定 10 秒窓を再現する A/B のアーム）
        if legacy_1265() {
            for _ in 0..100 {
                while let Ok(event) = rx.try_recv() {
                    session.process_event(event);
                }
                if session.cwd() == Some(std::path::Path::new("/private/tmp")) {
                    let _ = std::fs::remove_dir_all(&stale);
                    return; // OSC 7 が届いた
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let screen = session.visible_lines().join("\n");
            let server_zdotdir = crate::tmux::show_environment(Some(&socket), None, "ZDOTDIR");
            let session_zdotdir =
                crate::tmux::show_environment(Some(&socket), Some(session_name), "ZDOTDIR");
            let _ = std::fs::remove_dir_all(&stale);
            panic!(
                "OSC 7 が届かない（#1105）。server_zdotdir={server_zdotdir:?} \
                 session_zdotdir={session_zdotdir:?} 画面: {screen:?}"
            );
        }
        // TAKO_1265_LEGACY_ARM 終了
        let outcome = wait_osc7_cwd(
            &mut session,
            &mut rx,
            std::path::Path::new("/private/tmp"),
            &socket,
            session_name,
        );
        // 先行サーバー用の空 ZDOTDIR は成否によらず片付ける
        let _ = std::fs::remove_dir_all(&stale);
        if let Err(e) = outcome {
            // #1105 の核心（サーバーの stale な値をセッションが上書きできたか）は
            // 診断の `ZDOTDIR セッション= / サーバー=` に出る。**ここで別途
            // 器を叩き足さない**（器が応答しない場面で診断ごと固まるため）
            panic!("OSC 7 が届かない（#1105）。{e}");
        }
    }

    /// #1105 回帰: **ソケット名が `tako` で始まらなくても** OSC 7 が届く。
    ///
    /// シェル統合は「自分が tako の器の中か」で OSC を DCS パススルーで包むかを
    /// 決める。判定材料がソケット名の接頭辞 `tako*` だったので、`TAKO_TMUX_SOCKET` に
    /// 別の名前を与えると包まずに素の OSC を出し、tmux がそれを飲んで
    /// **cwd 追従（OSC 7）とコマンド状態（OSC 133）が両方黙って死んでいた**
    /// （検証用のソケット名で踏んだ。#1105）。
    /// 器が名前を明示（`BACKEND_SOCKET_ENV`）すれば推測が要らない
    #[test]
    #[cfg(unix)]
    fn ソケット名がtakoで始まらなくてもosc7が届く() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        if !std::path::Path::new("/bin/zsh").exists() {
            crate::tmux_test_gate::skip("zsh が無い環境");
            return;
        }
        // **接頭辞をわざと外す**（`tako` で始まらない名前）。それでも残骸掃除が拾う
        // `tk-coretest-` にする（#1874: 旧名 `ct1105-<pid>` は途中で殺されると残り続けた）
        let socket = format!("tk-coretest-1105-{}", std::process::id());
        assert!(
            !socket.starts_with("tako"),
            "この検査は名前が tako で始まらないことが前提"
        );
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let options = SpawnOptions {
            command: None,
            cwd: Some("/".into()),
            env: vec![
                ("TAKO_PANE_ID".into(), "1".into()),
                ("SHELL".into(), injected_shell()),
            ],
            scrollback_lines: None,
        };
        let session_name = "tako-e2e-name";
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session_name))
                .expect("tmux クライアントを spawn できる");
        session.write(b"cd /private/tmp\r".to_vec());
        // TAKO_1265_LEGACY_ARM 開始（#1265 前の固定 10 秒窓を再現する A/B のアーム）
        if legacy_1265() {
            for _ in 0..100 {
                while let Ok(event) = rx.try_recv() {
                    session.process_event(event);
                }
                if session.cwd() == Some(std::path::Path::new("/private/tmp")) {
                    return; // OSC 7 が届いた
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            panic!(
                "OSC 7 が届かない（#1105。socket={socket} = tako で始まらない名前）。画面: {:?}",
                session.visible_lines().join("\n")
            );
        }
        // TAKO_1265_LEGACY_ARM 終了
        if let Err(e) = wait_osc7_cwd(
            &mut session,
            &mut rx,
            std::path::Path::new("/private/tmp"),
            &socket,
            session_name,
        ) {
            panic!("OSC 7 が届かない（#1105。socket={socket} = tako で始まらない名前）。{e}");
        }
    }

    /// マウスレポートと拡張キー（CSI u）が tmux 越しでも**生のまま**内側アプリへ届く e2e。
    /// 「アプリがマウスレポートを要求したら必ず生のマウスイベントが届く」は tako の
    /// 存在意義に関わる保証（2026-06-12 実機リグレッションの再発防止）。
    /// 内側は受信バイトを可視化する `cat -v`（^[ = ESC）
    #[test]
    #[cfg(unix)]
    fn マウスレポートと拡張キーがtmux越しに生で届く() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-mouse-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let session_name = "tako-e2e-mouse";
        // 内側アプリ: SGR マウス + kitty keyboard を要求してから受信バイトを表示
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    format!("{}exec cat -v", mouse_request_prelude(true)),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session_name))
                .expect("tmux クライアントを spawn できる");

        // **アンカーは器のペイン側の要求**（#1252。理由は `wait_pane_mouse_ready` の上）。
        // 外側 `mouse_reporting()` は conf の `set -g mouse on` で attach の時点で
        // 真になるので、内側アプリが立ち上がった証跡にはならない
        if legacy_1252() {
            // TAKO_1252_LEGACY_ARM 開始（#1252 **前**の待ち。番犬はここを対象外にする）
            // 旧アンカー: 外側 `mouse_reporting()` を固定 100 回窓で待つ
            let mut mouse_on = false;
            for _ in 0..100 {
                if session.mouse_reporting() {
                    mouse_on = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert!(
                mouse_on,
                "内側アプリのマウス要求が外側端末モードへ伝わる。画面: {:?}",
                session.visible_lines().join("\n")
            );
            // TAKO_1252_LEGACY_ARM 終了
        } else {
            let pane = wait_pane_mouse_ready(&socket, session_name, || {}).unwrap_or_else(|e| {
                panic!(
                    "内側アプリのマウス要求が器のペインへ届かない（#1252）。{e} 画面: {:?}",
                    session.visible_lines().join("\n")
                )
            });
            eprintln!("TAKO_1252: {} legacy=false", pane.describe());
        }

        // 器の `set -g mouse on` が外側端末（tako の Term）へ伝わっている。
        // **内側の要求が伝わった証跡ではない**（#1252 で取り違えていた点）ので、
        // ここは「器のマウス設定が効いている」ことだけを見る
        wait_for_state(
            "器の mouse on が外側端末（tako の Term）へ伝わる",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(100),
            || session.mouse_reporting().then_some(()),
        )
        .unwrap_or_else(|e| {
            panic!(
                "器の mouse on が外側端末モードへ伝わらない。{} 画面: {:?}",
                e.observed(format!("mouse_reporting={}", session.mouse_reporting())),
                session.visible_lines().join("\n")
            )
        });

        // ホイール → 生の SGR マウスイベントが内側アプリへ届く（矢印キー変換は禁止）。
        // **毎周期打ち直す**（#1180 の項目 73 と同じ形）: ホイールは器が食いうる
        // 相対イベントなので、1 発だけ打って待つ形は「食われたら永久に届かない」
        enum Wheel {
            Delivered,
            /// 矢印キーに化けた（リグレッション）。画面をそのまま持たせる
            Arrow(String),
        }
        let wheel = if legacy_1252() {
            // TAKO_1252_LEGACY_ARM 開始（#1252 **前**の待ち。番犬はここを対象外にする）
            // 旧経路: ホイールを 1 発だけ打って固定 50 回窓で待つ
            session.scroll_wheel(1, 5, 5);
            let mut out = None;
            for _ in 0..50 {
                let lines = session.visible_lines().join("\n");
                if lines.contains("^[[A") || lines.contains("^[OA") {
                    out = Some(Wheel::Arrow(lines));
                    break;
                }
                if lines.contains("[<64;6;6M") {
                    out = Some(Wheel::Delivered);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            // TAKO_1252_LEGACY_ARM 終了
            out.ok_or_else(|| "固定窓（#1252 前）を使い切った".to_string())
        } else {
            wait_for_state(
                "内側アプリへ生の SGR ホイール `[<64;6;6M` が届く",
                std::time::Duration::from_secs(5),
                std::time::Duration::from_millis(100),
                || {
                    let lines = session.visible_lines().join("\n");
                    if lines.contains("^[[A") || lines.contains("^[OA") {
                        return Some(Wheel::Arrow(lines));
                    }
                    if lines.contains("[<64;6;6M") {
                        return Some(Wheel::Delivered);
                    }
                    session.scroll_wheel(1, 5, 5);
                    None
                },
            )
            .map_err(|e| {
                e.observed(format!(
                    "画面={:?} {}",
                    session.visible_lines().join("\n"),
                    probe_pane_mouse(&socket, session_name).describe()
                ))
                .to_string()
            })
        };
        match wheel {
            Ok(Wheel::Delivered) => {}
            Ok(Wheel::Arrow(lines)) => {
                panic!("ホイールが矢印キーに化けている（リグレッション）。画面: {lines:?}")
            }
            Err(diag) => panic!("生の SGR ホイールイベントが届かない（#1252）。{diag}"),
        }

        // Shift+Enter（CSI u）も tmux 越しで**kitty 形式のまま**内側へ届く
        // （extended-keys always + extended-keys-format csi-u。FR の常用要件）
        session.write(b"\x1b[13;2u".to_vec());
        wait_for_state(
            "Shift+Enter（CSI u）のエコー `[13;2u`",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(100),
            || {
                session
                    .visible_lines()
                    .join("\n")
                    .contains("[13;2u")
                    .then_some(())
            },
        )
        .unwrap_or_else(|e| {
            panic!(
                "Shift+Enter（CSI u）が tmux 越しに kitty 形式で届かない。{}",
                e.observed(format!("画面={:?}", session.visible_lines().join("\n")))
            )
        });
        // 外側（tako の Term）には拡張キーモードが伝わらない（tmux の仕様）。
        // そのため UI 層はバックエンドペインで disambiguate を強制する（main.rs の
        // handle_key）。ここでは前提（伝わらない）が変わったら気づけるよう記録する
        eprintln!(
            "外側 disambiguate = {}（false 想定。true になったら main.rs の強制は不要）",
            session.disambiguate_keys()
        );

        // Esc 単押し（素の \e。UI 層 handle_key はバックエンドペインで Esc を
        // CSI 27u にしない = CsiUMode::ModifiedOnly）も内側ペインへ素のまま届く。
        // tmux は CSI 27u を内側の kitty 要求に関係なく素通しするため、CSI u に
        // すると非対応アプリで「27u」が文字化けする（2026-06-12 実機バグ）。
        // 素の \e は escape-time で正しく解釈され素のまま届く（その固定）
        session.write(b"\x1b".to_vec());
        session.write(b"ESC-RAW\r".to_vec());
        wait_for_state(
            "Esc（素の \\e）のエコー `^[ESC-RAW`",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(100),
            || {
                session
                    .visible_lines()
                    .join("\n")
                    .contains("^[ESC-RAW")
                    .then_some(())
            },
        )
        .unwrap_or_else(|e| {
            panic!(
                "Esc（素の \\e）が tmux 越しに素のまま届かない。{}",
                e.observed(format!("画面={:?}", session.visible_lines().join("\n")))
            )
        });
        assert!(
            !session.visible_lines().join("\n").contains("27u"),
            "Esc が CSI 27u 断片として漏れている（2026-06-12 実機バグの回帰）。画面: {:?}",
            session.visible_lines().join("\n")
        );
    }

    /// マウスレポート洪水（トラックパッド慣性スクロール級）でも ESC 欠落断片が
    /// 内側アプリへテキストとして漏れない e2e（#167）。
    /// 転送レートが下流（tmux クライアント PTY / 内側 PTY）の処理能力を超えると
    /// macOS の tty 入力キューがバイトを黙って捨て、ESC を失った断片
    /// （例: `4;45;18M`）が平文として内側の入力欄に入る（実 claude で再現済み）。
    /// terminal.rs のホイール転送レート制限がこれを防ぐことを検証する。
    /// 内側は claude と同じ raw mode + 受信バイトの即時可視化（perl）
    #[test]
    #[cfg(unix)]
    fn マウスレポート洪水でも断片がテキスト化しない() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-flood-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // 内側アプリ: SGR マウス要求 + raw mode + ESC を「^[」に可視化して即時 echo
        // （claude 等の raw mode TUI が受け取るバイト列の観測装置）
        let inner = format!(
            r#"stty raw -echo; {}exec perl -e '$|=1; while (sysread(STDIN,$b,4096)) {{ $b =~ s/\x1b/^[/g; syswrite(STDOUT,$b) }}'"#,
            mouse_request_prelude(false)
        );
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), inner],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let session_name = "tako-e2e-flood";
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session_name))
                .expect("tmux クライアントを spawn できる");

        // **アンカーは器のペイン側の要求**（#1252）。要求前に洪水を打つと器が
        // 1 発目を食って copy-mode へ入り、2,100 イベント全部がスクロールに消える
        // （= `intact=0` で「転送が死んでいる」に見える）。
        // rx（PtyWrite = tmux の端末クエリへの応答）は実運用の UI 層と同様に毎周期
        // 処理する（捨てると tmux クライアントが応答待ちのままになり、経路の再現にならない）
        if legacy_1252() {
            // TAKO_1252_LEGACY_ARM 開始（#1252 **前**の待ち。番犬はここを対象外にする）
            // 旧アンカー: 外側 `mouse_reporting()` を固定 100 回窓で待つ
            let mut mouse_on = false;
            for _ in 0..100 {
                while let Ok(event) = rx.try_recv() {
                    session.process_event(event);
                }
                if session.mouse_reporting() {
                    mouse_on = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert!(mouse_on, "内側アプリのマウス要求が外側端末モードへ伝わる");
            // TAKO_1252_LEGACY_ARM 終了
        } else {
            let pane = wait_pane_mouse_ready(&socket, session_name, || {
                while let Ok(event) = rx.try_recv() {
                    session.process_event(event);
                }
            })
            .unwrap_or_else(|e| {
                panic!(
                    "内側アプリのマウス要求が器のペインへ届かない（#1252）。{e} 画面: {:?}",
                    session.visible_lines().join("\n")
                )
            });
            eprintln!("TAKO_1252: {} legacy=false", pane.describe());
        }

        // 洪水: 慣性スクロール相当（2,100 イベント要求）を全速で連打
        for _ in 0..700 {
            session.scroll_wheel(3, 5, 5);
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
        }

        // 配送が安定するまで待つ（intact 数が 1 件以上 かつ 前周期と同数になるまで）。
        // 予算切れでも **最後に観測した値をそのまま使う**（届いてはいるが増え続けている
        // = 洪水がまだ飛行中、という状態は #167 の主題とは別物なので落とさない）
        const INTACT: &str = "^[[<64;6;6M";
        let mut last_count: Option<usize> = None;
        let mut last_capture = PaneCapture::default();
        let settled = wait_for_state(
            "洪水の配送が安定する（intact >= 1 かつ前周期と同数）",
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(200),
            || {
                while let Ok(event) = rx.try_recv() {
                    session.process_event(event);
                }
                let capture = capture_pane_text(&socket, session_name);
                let count = capture.text.as_deref().map(|t| t.matches(INTACT).count());
                let stable = matches!((count, last_count), (Some(c), Some(l)) if c > 0 && c == l);
                last_count = count;
                last_capture = capture;
                stable.then_some(())
            },
        );
        let all = last_capture.text.clone().unwrap_or_default();
        let intact_count = all.matches(INTACT).count();

        // intact な SGR レポートを全部取り除いた残りに座標断片が残っていたら、
        // ESC 欠落断片がテキストとして内側へ届いている（= #167 の症状）
        let stripped = all.replace(INTACT, "");
        assert!(
            !stripped.contains("6;6M"),
            "ESC 欠落断片が内側へテキストとして漏れている（#167 再発）。\
             intact={intact_count} 残骸例: {:?}",
            stripped
                .lines()
                .filter(|l| l.contains("6;6M"))
                .take(3)
                .collect::<Vec<_>>()
        );
        // レート制限が全イベントを殺していないこと（正常なレポートは届く）。
        // ここが #1252 の落ちどころだったので、**何を待って何が届いたか**を出す
        // （`capture` の成否も含める = 「器を呼べなかった」と「呼べたが空」を混ぜない）
        assert!(
            intact_count > 0,
            "SGR レポートが 1 件も届いていない（転送が死んでいる）。{} {} 画面: {:?}",
            settled
                .as_ref()
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "配送は安定した".into()),
            format_args!(
                "{} / {}",
                last_capture.note,
                probe_pane_mouse(&socket, session_name).describe()
            ),
            session.visible_lines().join("\n")
        );
        // レート制限が生きていること（2,100 イベントの洪水がそのまま流れていない。
        // 制限が消えると飛行中バイト量が増え、書き込み停滞時の断片化リスクが戻る）
        assert!(
            intact_count < 200,
            "洪水がレート制限されずそのまま転送されている（#167 の防御が消失）: {intact_count}"
        );
        eprintln!(
            "洪水 2100 イベント要求 → intact 配送 {intact_count} 件・断片ゼロ（{}・{}）",
            last_capture.note,
            settled
                .map(|()| "配送は安定".to_string())
                .unwrap_or_else(|e| format!("安定待ちは予算切れ: {e}"))
        );
    }

    /// 器のペインへのクリックの e2e（#1961 = #167 の拡張）。押す・ドラッグ・離すの洪水を
    /// GUI と同じ 1 実装（`mouse_forward::send_tmux` = `send-keys -H`）で注入しても、
    /// ESC の欠けた断片が内側の入力へ平文で混ざらず、**押す・離すは 1 つも欠けない**。
    /// 内側は claude と同じ raw mode + ボタンを押したままの移動の要求（1002 + SGR）+
    /// kitty keyboard の要求（claude が出す `CSI > 1 u` = 拡張キーの器の経路を通す）
    #[test]
    #[cfg(unix)]
    fn クリックのtmux直接注入は断片を混ぜず押す離すを欠かさない() {
        use crate::mouse_forward::{self as mf, ClickPhase, Encoding};
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-click-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let inner = r#"stty raw -echo; printf '\033[?1002h\033[?1006h\033[>1u'; exec perl -e '$|=1; while (sysread(STDIN,$b,4096)) { $b =~ s/\x1b/^[/g; syswrite(STDOUT,$b) }'"#;
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), inner.into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let session_name = "tako-e2e-click";
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session_name))
                .expect("tmux クライアントを spawn できる");
        // rx（PtyWrite = tmux の端末クエリへの応答）は実運用の UI 層と同様に毎周期処理する
        let mut drain = || {
            while let Ok(event) = rx.try_recv() {
                session.process_event(event);
            }
        };
        // アンカーは器のペイン側の要求（#1252。外側は器の mouse on で常に真）
        wait_pane_mouse_ready(&socket, session_name, &mut drain).unwrap_or_else(|e| {
            panic!("内側アプリのマウス要求が器のペインへ届かない（#1252）。{e}")
        });
        let target = crate::scroll::ScrollTarget::Backend {
            socket: socket.clone(),
            session: session_name.into(),
        };
        // GUI と同じ問い合わせで「ドラッグも要求する SGR」と読める
        let flags = mf::tmux_mouse_flags(&target).expect("器へ問い合わせられる");
        let report = mf::decide(flags.report(), flags.in_mode)
            .unwrap_or_else(|r| panic!("渡す判断にならない: {} {flags:?}", r.as_str()));
        assert_eq!(report.encoding, Encoding::Sgr, "{flags:?}");
        assert!(
            report.drag,
            "1002 を要求したのにドラッグを渡さない: {flags:?}"
        );

        // 洪水: 押す → 12 セルのドラッグ → 離すを 150 回。GUI の送信（`pump_click`）と同じく
        // 送信中に溜まったぶんを 1 回の send-keys へまとめる（ここでは 1 往復ぶんずつ）
        const ROUNDS: usize = 150;
        let cell = |phase, col| mf::click_report_bytes(report, phase, col, 5, false).unwrap();
        for _ in 0..ROUNDS {
            let mut bytes = cell(ClickPhase::Press, 5);
            for col in 6..18 {
                bytes.extend(cell(ClickPhase::Drag, col));
            }
            bytes.extend(cell(ClickPhase::Release, 17));
            mf::send_tmux(&target, &bytes).expect("send-keys -H が通る");
            drain();
        }

        const PRESS: &str = "^[[<0;6;6M";
        const RELEASE: &str = "^[[<0;18;6m";
        let mut last_capture = PaneCapture::default();
        let settled = wait_for_state(
            "押す・離すが全部届く",
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(200),
            || {
                drain();
                last_capture = capture_pane_text(&socket, session_name);
                let text = last_capture.text.as_deref().unwrap_or("");
                (text.matches(RELEASE).count() >= ROUNDS).then_some(())
            },
        );
        let all = last_capture.text.clone().unwrap_or_default();
        let (press, release) = (all.matches(PRESS).count(), all.matches(RELEASE).count());
        // 無傷のレポートを全部取り除いた残りに座標の断片が残っていたら、ESC 欠落断片が
        // 平文として内側へ届いている（= #167 の症状）
        let mut stripped = all.replace(PRESS, "").replace(RELEASE, "");
        for col in 7..=18 {
            stripped = stripped.replace(&format!("^[[<32;{col};6M"), "");
        }
        assert!(
            !stripped.contains(";6M") && !stripped.contains(";6m"),
            "ESC 欠落断片が内側へテキストとして漏れている（#167 型）。残骸例: {:?}",
            stripped
                .lines()
                .filter(|l| l.contains(";6M") || l.contains(";6m"))
                .take(3)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            (press, release),
            (ROUNDS, ROUNDS),
            "押す・離すが欠けた（離すが欠けると内側アプリが押しっぱなしだと思い込む）。{} {}",
            settled
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "揃った".into()),
            last_capture.note
        );
        eprintln!("クリック {ROUNDS} 回（ドラッグ各 12）→ 押す {press} / 離す {release}・断片ゼロ");
    }

    /// マウスを要求しないアプリ（シェル相当）の器のペインへは AI のクリックを渡さず、
    /// 入力に制御列を 1 バイトも打たない（#1961 の受け入れ 3）。理由の語は `not_requested`
    #[test]
    #[cfg(unix)]
    fn マウスを要求しない器のペインへはクリックを渡さない() {
        use crate::mouse_forward as mf;
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-noclick-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "printf 'READY\\n'; exec cat -v".into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let session_name = "tako-e2e-noclick";
        let (mut session, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session_name))
                .expect("tmux クライアントを spawn できる");
        let mut capture_until = |what: &str, needle: &str| {
            wait_for_state(
                what,
                std::time::Duration::from_secs(10),
                std::time::Duration::from_millis(100),
                || {
                    while let Ok(event) = rx.try_recv() {
                        session.process_event(event);
                    }
                    capture_pane_text(&socket, session_name)
                        .text
                        .filter(|t| t.contains(needle))
                },
            )
            .unwrap_or_else(|e| panic!("{what} が見えない: {e}"))
        };
        capture_until("内側アプリの起動（READY）", "READY");
        let target = crate::scroll::ScrollTarget::Backend {
            socket: socket.clone(),
            session: session_name.into(),
        };
        let err = mf::click_tmux(&target, 4, 4).expect_err("要求していないアプリへ渡した");
        assert!(err.contains("not_requested"), "理由の語が無い: {err}");
        // 打っていないことを、打った場合に必ず見える印（cat -v の `^[[<`）が無いことで見る。
        // 後から打った END が見えた時点で、それより前の入力は届き切っている
        crate::tmux::query(
            Some(&socket),
            &["send-keys", "-t", session_name, "-l", "END"],
        )
        .expect("END を打てる");
        let text = capture_until("後から打った END のエコー", "END");
        assert!(
            !text.contains("^[[<") && !text.contains("^[[M"),
            "マウスを要求しないアプリへ制御列が届いた: {text:?}"
        );
    }

    /// Esc 単押しが「kitty を要求していない」内側アプリ（素の zsh 相当）にも
    /// 素の \e のまま届き、「27u」が文字として漏れない e2e
    /// （2026-06-12 実機バグの再発防止）。
    /// 後半は前提のカナリア: tmux が受信 CSI 27u を非要求ペインへ素通しすること
    /// （= UI 層が Esc を CSI u で送ってはいけない理由）を観測ログに残す。
    /// tmux 側が将来「非要求ペインへはレガシー再エンコード」に変われば
    /// CsiUMode::ModifiedOnly の Esc 例外は不要にできる
    #[test]
    #[cfg(unix)]
    fn esc単押しは非kittyアプリにも素のescで届き27uが漏れない() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-esc-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // 内側アプリ: kitty を**要求しない** cat -v（素の zsh で Esc を押した状況の再現）
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), "echo TAKO-ESC-READY; exec cat -v".into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-esc"))
                .expect("tmux クライアントを spawn できる");
        let wait_for = |needle: &str| -> bool {
            for _ in 0..100 {
                if session
                    .visible_lines()
                    .iter()
                    .any(|line| line.contains(needle))
                {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        };
        assert!(
            wait_for("TAKO-ESC-READY"),
            "内側アプリが立ち上がらない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // UI 層（handle_key の CsiUMode::ModifiedOnly）と同じバイト列: Esc は素の \e
        session.write(b"\x1b".to_vec());
        session.write(b"ESC-RAW\r".to_vec());
        assert!(
            wait_for("^[ESC-RAW"),
            "Esc（素の \\e）が内側へ素のまま届かない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        assert!(
            !session.visible_lines().join("\n").contains("27u"),
            "Esc 単押しで「27u」が文字として漏れた（2026-06-12 実機バグの回帰）。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // カナリア: CSI 27u は非要求ペインにも素通しされる（tmux 3.6 の実測挙動。
        // これが変わったら main.rs の Esc 例外を見直せる）
        session.write(b"\x1b[27u".to_vec());
        session.write(b"\r".to_vec());
        let passthrough = wait_for("^[[27u");
        eprintln!(
            "CSI 27u の非要求ペインへの素通し = {passthrough}（true 想定。false になったら \
             tmux が再エンコードするようになった = CsiUMode::ModifiedOnly の Esc 例外を再検討）"
        );
    }

    /// ネスト tmux（バックエンド → ユーザー自前 tmux → アプリ）のチェーン e2e 用ヘルパ。
    /// ユーザーサーバー側は NESTED_TMUX_SNIPPET（FR-2.17 の推奨設定）で起動する。
    ///
    /// 返り値の [`ScratchDir`](crate::test_residue::ScratchDir) は**ネスト conf の置き場**で、
    /// 呼び出し側がセッションと同じスコープで持つ（落とすと conf が消える = #1312）
    #[cfg(unix)]
    fn spawn_nested(
        backend_socket: &str,
        nested_socket: &str,
        inner_cmd: &str,
    ) -> (crate::TerminalSession, crate::test_residue::ScratchDir) {
        spawn_nested_with_conf(
            backend_socket,
            nested_socket,
            inner_cmd,
            NESTED_TMUX_SNIPPET,
        )
    }

    /// 器の側のセッション名（`spawn_nested_with_conf` が `wrap_options` へ渡す）
    #[cfg(unix)]
    const NEST_BACKEND_SESSION: &str = "tako-e2e-nest";

    /// ネスト tmux 側のセッション名
    #[cfg(unix)]
    const NEST_SESSION: &str = "nest";

    /// [`spawn_nested`] のネスト conf を差し替えられる版（#2406 の注入 `never` が
    /// `mouse off` = バグ (1) の設定で起こす）
    #[cfg(unix)]
    fn spawn_nested_with_conf(
        backend_socket: &str,
        nested_socket: &str,
        inner_cmd: &str,
        nested_conf: &str,
    ) -> (crate::TerminalSession, crate::test_residue::ScratchDir) {
        let conf_dir = crate::test_residue::ScratchDir::new("nest-conf");
        let conf_path = conf_dir.join(format!("{nested_socket}.conf"));
        std::fs::write(&conf_path, nested_conf).expect("ネスト conf を書ける");
        // バックエンドペインの中でユーザー tmux サーバーへ new-session する
        // （実機の「自前 tmux セッションを tako 内で attach」構成の再現）
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: crate::tmux::tmux_bin().to_string(),
                args: vec![
                    "-u".into(),
                    "-L".into(),
                    nested_socket.into(),
                    "-f".into(),
                    conf_path.display().to_string(),
                    "new-session".into(),
                    "-A".into(),
                    "-s".into(),
                    NEST_SESSION.into(),
                    inner_cmd.into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let (session, _rx) = crate::TerminalSession::spawn(
            80,
            24,
            wrap_options(options, backend_socket, NEST_BACKEND_SESSION),
        )
        .expect("ネスト構成を spawn できる");
        (session, conf_dir)
    }

    // ── #2406: ネスト tmux 越しのホイールの待ち ──────────────────────────────
    //
    // **`LINE-99` が見えても、ネスト tmux のマウス要求が器のペインに在るとは限らない**。
    // ネスト tmux は起動中、端末の応答（DA / XTVERSION）を受けるたびに `tty_invalidate` で
    // マウス要求を外し（`\e[?1000l` …）、次の周回で付け直す。外しと付け直しは**別の書き込み**に
    // なりうる（実測: 既存セッションへの attach で、「外し + LINE-99 の描画」の書き込みの
    // 40 µs 後に付け直しの書き込み）。応答が描画より後に着く順序（混んだ機）では、器は
    // 「LINE-99 が見えるのにペインの `mouse_any_flag=0`」の窓を持つ。
    //
    // その窓に届いたホイールは**捨てられる**。器の既定の `WheelUpPane` は
    // `#{||:#{alternate_on},#{pane_in_mode},#{mouse_any_flag}}` で `send-keys -M` へ回し
    // （器のペインはネスト tmux の alt screen なので常に真 = copy-mode には入らない。#1252 とは
    // ここが違う）、マウスを要求していないペインへのマウスイベントは tmux が落とす。旧テストは
    // ホイールを 1 発だけ打っていたので、最下部（`LINE-78`〜`LINE-99`）のまま窓が尽きていた
    // （CI の形）。外側の `mouse_reporting()` は器の `mouse on` で attach 時点から真なので、
    // これも証跡にならない（#1252 と同じ）

    /// A/B: `TAKO_2406_LEGACY=1` は #2406 **前**の待ちへ戻す
    /// （アンカー = 外側 `mouse_reporting()` + `LINE-99` / ホイールは 1 発 / 固定回数の窓）
    #[cfg(unix)]
    fn legacy_2406() -> bool {
        std::env::var("TAKO_2406_LEGACY").is_ok_and(|v| v == "1")
    }

    /// 注入 `gap` で、外したマウス要求を付け直すまでの遅れ
    #[cfg(unix)]
    const GAP_2406: std::time::Duration = std::time::Duration::from_secs(1);

    /// 注入（#2406）。混み具合は再現できないので**ネストのマウス要求が外れている窓**そのものを作る
    #[cfg(unix)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Inject2406 {
        /// 最初のホイールの直前にネストのマウス要求を外し（器のペインから消えたのを確かめてから
        /// 打つ）、付け直しを [`GAP_2406`] 遅らせる = `tty_invalidate` の窓が混んだ機で伸びた形。
        /// 旧経路は確定で FAILED
        Gap,
        /// ネスト conf を `mouse off` にする = バグ (1) の設定（**新経路でも FAILED が正しい**）
        Never,
    }

    #[cfg(unix)]
    fn inject_2406() -> Option<Inject2406> {
        match std::env::var("TAKO_2406_INJECT").ok().as_deref() {
            None | Some("") => None,
            Some("gap") => Some(Inject2406::Gap),
            Some("never") => Some(Inject2406::Never),
            // 綴り違いを「注入なし」として黙って通さない
            Some(other) => panic!("TAKO_2406_INJECT={other:?} は知らない値（gap / never）"),
        }
    }

    /// ネスト conf（注入 `never` は `mouse on` を `mouse off` へ）
    #[cfg(unix)]
    fn nested_conf_2406(inject: Option<Inject2406>) -> String {
        if inject != Some(Inject2406::Never) {
            return NESTED_TMUX_SNIPPET.to_string();
        }
        let conf = NESTED_TMUX_SNIPPET.replace("set -g mouse on", "set -g mouse off");
        assert!(
            conf.contains("set -g mouse off"),
            "注入 never: ネスト conf の `set -g mouse on` を差し替えられない"
        );
        conf
    }

    /// 注入 `gap` の前半: ネストのマウス要求を外し、器のペインから消えたのを確かめる
    #[cfg(unix)]
    fn inject_2406_before_wheel(backend: &str, nested: &str) {
        let out = crate::tmux::tmux_command(Some(nested))
            .args(["set", "-g", "mouse", "off"])
            .output()
            .expect("注入: ネスト tmux を呼べる");
        assert!(
            out.status.success(),
            "注入 gap: ネストの mouse を外せない。stderr={:?}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        wait_for_state(
            "注入: 器のペインからネストのマウス要求が消える（#{mouse_any_flag}=0）",
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(20),
            || (!probe_pane_mouse(backend, NEST_BACKEND_SESSION).any).then_some(()),
        )
        .unwrap_or_else(|e| {
            panic!(
                "注入 gap が効かない（#2406）。{}",
                e.tagged("TAKO_2406_INJECT")
                    .observed(probe_pane_mouse(backend, NEST_BACKEND_SESSION).describe())
            )
        });
    }

    /// 注入 `gap` の後半: 付け直しを [`GAP_2406`] 遅らせる（打ったホイールはその間に器へ届いて
    /// 捨てられる）。テストが先に終わって器が消えていても落とさない
    #[cfg(unix)]
    fn inject_2406_restore_later(nested: &str) {
        let nested = nested.to_string();
        std::thread::spawn(move || {
            std::thread::sleep(GAP_2406);
            let _ = crate::tmux::tmux_command(Some(&nested))
                .args(["set", "-g", "mouse", "on"])
                .output();
        });
    }

    /// 画面の先頭行の `LINE-n` の n（遡れたかの判定）
    #[cfg(unix)]
    fn nested_top_line(session: &crate::TerminalSession) -> Option<usize> {
        session
            .visible_lines()
            .first()
            .and_then(|l| l.trim_end().strip_prefix("LINE-")?.parse().ok())
    }

    /// ネスト側のスクロールの状態（失敗の診断用。器が食ったのか / ネストが受けて
    /// 動かなかったのかを分ける。#2406）
    #[cfg(unix)]
    fn describe_nested_scroll(nested: &str) -> String {
        match crate::tmux::tmux_command(Some(nested))
            .args([
                "display-message",
                "-p",
                "-t",
                NEST_SESSION,
                "#{pane_in_mode},#{scroll_position},#{history_size},#{mouse}",
            ])
            .output()
        {
            Ok(out) if out.status.success() => format!(
                "ネスト側 inmode,scroll,history,mouse={}",
                String::from_utf8_lossy(&out.stdout).trim()
            ),
            Ok(out) => format!(
                "ネスト側 rc={:?} stderr={:?}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => format!("ネスト側を呼べない: {e}"),
        }
    }

    /// ネスト tmux 越しのホイールがユーザーサーバーの copy-mode スクロールに乗る e2e
    /// （2026-06-12 実機バグ (1) の再発防止。NESTED_TMUX_SNIPPET の mouse on が前提）。
    /// 経路: tako の SGR → バックエンド tmux（mouse_any=1 で send -M 生転送）→
    /// ネスト tmux（mouse on）→ copy-mode でネスト側スクロールバックを遡る。
    ///
    /// アンカーは**器のペイン側のマウス要求**で、ホイールは**遡るまで毎周期打ち直す**。
    /// 要求が外れた窓に落ちたホイールは器が捨てるので、1 発だけでは届かない回がある
    /// （#2406。理由はこの上のブロック）
    #[test]
    #[cfg(unix)]
    fn ネストtmux越しのホイールで内側スクロールバックを遡れる() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let backend = format!("tako-coretest-nestw-{}", std::process::id());
        let nested = format!("tako-coretest-nestw-in-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![backend.clone(), nested.clone()]);
        let inject = inject_2406();
        let (session, _nest_conf) = spawn_nested_with_conf(
            &backend,
            &nested,
            "i=0; while [ $i -lt 100 ]; do echo LINE-$i; i=$((i+1)); done; exec sleep 60",
            &nested_conf_2406(inject),
        );
        if legacy_2406() {
            // TAKO_2406_LEGACY_ARM 開始（#2406 **前**の待ち。番犬はここを対象外にする）
            // ネスト内の出力完了 + 外側のマウスモード（バックエンド mouse on）を待つ
            let mut ready = false;
            for _ in 0..100 {
                if session.mouse_reporting()
                    && session
                        .visible_lines()
                        .iter()
                        .any(|l| l.trim_end() == "LINE-99")
                {
                    ready = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert!(
                ready,
                "ネスト構成が立ち上がらない。画面: {:?}",
                session.visible_lines().join("\n")
            );
            // ホイール上 → ネスト tmux の copy-mode で遡る（過去の LINE-n が見える）
            if inject == Some(Inject2406::Gap) {
                inject_2406_before_wheel(&backend, &nested);
            }
            session.scroll_wheel(3, 10, 10);
            if inject == Some(Inject2406::Gap) {
                inject_2406_restore_later(&nested);
            }
            let mut scrolled = false;
            for _ in 0..50 {
                if nested_top_line(&session).is_some_and(|n| n < 77) {
                    scrolled = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            assert!(
                scrolled,
                "ネスト越しのホイールでスクロールバックを遡れない（バグ (1) の回帰）。画面: {:?}",
                session.visible_lines().join("\n")
            );
            // TAKO_2406_LEGACY_ARM 終了
            return;
        }

        // 遡る先（ネストの出力の最後まで）が揃うのを待つ
        wait_for_state(
            "ネストの出力の最後 `LINE-99` が画面に出る",
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(100),
            || {
                session
                    .visible_lines()
                    .iter()
                    .any(|l| l.trim_end() == "LINE-99")
                    .then_some(())
            },
        )
        .unwrap_or_else(|e| {
            panic!(
                "ネスト構成が立ち上がらない（#2406）。{} 画面: {:?}",
                e.tagged("TAKO_2406_WAIT")
                    .observed(describe_nested_scroll(&nested)),
                session.visible_lines().join("\n")
            )
        });
        // ネスト tmux のマウス要求が器のペインに届いている
        let pane = wait_pane_mouse_ready(&backend, NEST_BACKEND_SESSION, || {}).unwrap_or_else(|e| {
            panic!(
                "ネスト tmux のマウス要求が器のペインへ届かない（バグ (1) の回帰。#2406）。{} {} 画面: {:?}",
                e.tagged("TAKO_2406_WAIT"),
                describe_nested_scroll(&nested),
                session.visible_lines().join("\n")
            )
        });
        eprintln!(
            "TAKO_2406: {} legacy=false inject={inject:?}",
            pane.describe()
        );

        // ホイール上 → ネスト tmux の copy-mode で遡る（過去の LINE-n が見える）。
        // 要求は付け直しの途中で一瞬外れうる（外れた窓のホイールは器が捨てる）ので、
        // 毎周期 器のペインを見てから打ち、遡るまで打ち直す
        let mut sent = 0u32;
        let mut unready = 0u32;
        let scrolled = wait_for_state(
            "ホイールでネストのスクロールバックを遡る（先頭が LINE-77 未満）",
            std::time::Duration::from_secs(10),
            std::time::Duration::from_millis(100),
            || {
                if nested_top_line(&session).is_some_and(|n| n < 77) {
                    return Some(());
                }
                if !probe_pane_mouse(&backend, NEST_BACKEND_SESSION).ready() {
                    unready += 1;
                    return None;
                }
                let inject_gap = sent == 0 && inject == Some(Inject2406::Gap);
                if inject_gap {
                    inject_2406_before_wheel(&backend, &nested);
                }
                session.scroll_wheel(3, 10, 10);
                sent += 1;
                if inject_gap {
                    inject_2406_restore_later(&nested);
                }
                None
            },
        );
        eprintln!("TAKO_2406: wheel sent={sent} unready={unready}");
        scrolled.unwrap_or_else(|e| {
            panic!(
                "ネスト越しのホイールでスクロールバックを遡れない（バグ (1) の回帰。#2406）。{} 画面: {:?}",
                e.tagged("TAKO_2406_WAIT").observed(format!(
                    "ホイール sent={sent} unready={unready} / {} / {}",
                    probe_pane_mouse(&backend, NEST_BACKEND_SESSION).describe(),
                    describe_nested_scroll(&nested)
                )),
                session.visible_lines().join("\n")
            )
        });
    }

    /// ネスト tmux 越しの CSI u（Shift+Enter）が最内のアプリへ kitty 形式のまま届く e2e
    /// （2026-06-12 実機バグ (4) の再発防止。NESTED_TMUX_SNIPPET の extended-keys on +
    /// バックエンド conf の extended-keys always が両輪）。
    /// 最内は kitty を要求して受信バイトを可視化する cat -v
    #[test]
    #[cfg(unix)]
    fn ネストtmux越しのcsi_uが最内アプリへ届く() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let backend = format!("tako-coretest-nestk-{}", std::process::id());
        let nested = format!("tako-coretest-nestk-in-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![backend.clone(), nested.clone()]);
        let (session, _nest_conf) = spawn_nested(
            &backend,
            &nested,
            r"printf '\033[>1u'; echo TAKO-NEST-'READY'; exec cat -v",
        );
        let wait_for = |needle: &str| -> bool {
            for _ in 0..100 {
                if session
                    .visible_lines()
                    .iter()
                    .any(|line| line.contains(needle))
                {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            false
        };
        assert!(
            wait_for("TAKO-NEST-READY"),
            "ネスト構成が立ち上がらない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // Shift+Enter（CSI u）。バックエンドペインは UI 層が CSI u 送出を常時有効化
        // するため、ここでも生の CSI u を書く（handle_key と同じバイト列）
        session.write(b"\x1b[13;2u".to_vec());
        assert!(
            wait_for("[13;2u"),
            "CSI u がネスト tmux 越しに素の Enter へ劣化した（バグ (4) の回帰）。画面: {:?}",
            session.visible_lines().join("\n")
        );
    }

    /// CJK が tmux 越しでも描画される e2e（2026-06-12 P0 リグレッションの再発防止）。
    /// Finder 起動の .app はロケール環境変数が無い（= POSIX ロケール）。それを LC_ALL=C の
    /// 強制で再現し、`-u`（UTF-8 強制）が効いて日本語が `_` に置換されないことを検証する
    #[test]
    #[cfg(unix)]
    fn cjkはロケール無し環境でもtmux越しに描画される() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-cjk-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let options = SpawnOptions {
            // 出力経路を直接検証する（タイプ入力を経由しない）: 日本語を printf して待機
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "printf '日本語テストOK\\n'; exec sleep 30".into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            // .app（Finder 起動）のロケール無し環境を再現する（テスト実行シェルの
            // LANG を C で上書き。子プロセスへは合成 env が優先で渡る）
            env: vec![("LC_ALL".into(), "C".into()), ("LANG".into(), "C".into())],
            scrollback_lines: None,
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-cjk"))
                .expect("tmux クライアントを spawn できる");
        for _ in 0..100 {
            let lines = session.visible_lines().join("\n");
            if lines.contains("日本語テストOK") {
                return; // CJK がそのまま描画された
            }
            // tmux が非 UTF-8 扱いすると _ に置換される（P0 の症状）
            assert!(
                !lines.contains("____"),
                "CJK が _ に置換されている（tmux のロケール退行）。画面: {lines:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!(
            "CJK 出力が現れない。画面: {:?}",
            session.visible_lines().join("\n")
        );
    }

    /// 通常画面・非マウスのペイン（素のシェルや Claude Code）へのホイールは
    /// バックエンド tmux の copy-mode でスクロールバックを遡り、かつ右上に
    /// 位置インジケータ（tmux 3.6 既定は先頭行タイムスタンプ = 時刻を含む）を
    /// **描かない**ことの e2e（2026-06-12 実機バグ (2) の再発防止。
    /// conf の `copy-mode-position-format ''` が回帰検知の対象）
    #[test]
    #[cfg(unix)]
    fn 通常ペインのホイールはcopy_modeで遡りインジケータを出さない() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-ind-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // 100 行出力して待機する sh（通常画面・非マウス。Claude Code と同型）
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "i=0; while [ $i -lt 100 ]; do echo LINE-$i; i=$((i+1)); done; exec sleep 60"
                        .into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-ind"))
                .expect("tmux クライアントを spawn できる");
        let wait_top = |pred: &dyn Fn(&str) -> bool| -> Option<String> {
            for _ in 0..100 {
                let lines = session.visible_lines();
                if let Some(top) = lines.first() {
                    if pred(top) {
                        return Some(top.clone());
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            session.visible_lines().first().cloned()
        };
        // 出力完了（最終行が見えている）を待つ
        for _ in 0..100 {
            if session
                .visible_lines()
                .iter()
                .any(|l| l.trim_end() == "LINE-99")
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // ホイール上 → copy-mode で遡る（1 イベント目が copy-mode 入り、以後スクロール）
        session.scroll_wheel(3, 10, 10);
        let top = wait_top(&|top| {
            let t = top.trim_end();
            t.starts_with("LINE-") && t != "LINE-77"
        })
        .expect("先頭行が取れる");
        let t = top.trim_end();
        let n: usize = t
            .strip_prefix("LINE-")
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                panic!("スクロール後の先頭行が LINE-n でない（インジケータ等の混入）: {top:?}")
            });
        assert!(n < 77, "ホイールで遡れていない。先頭行: {top:?}");
        // 行全体が LINE-n のみ = 右上に時刻 / [位置/履歴] インジケータが無い
        assert_eq!(
            t,
            format!("LINE-{n}"),
            "右上に位置インジケータが描かれている（バグ (2) の回帰）: {top:?}"
        );
        // ホイール下で最下部へ戻ると copy-mode が解けて元の画面（LINE-99）に戻る
        session.scroll_wheel(-30, 10, 10);
        let mut back = false;
        for _ in 0..50 {
            if session
                .visible_lines()
                .iter()
                .any(|l| l.trim_end() == "LINE-99")
            {
                back = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(back, "ホイール下で最下部へ戻らない");
    }

    /// sync_conf が**稼働中**のサーバーへ最新 conf を再適用することの e2e。
    /// サーバーは tako 再起動を生き残るため、これが無いと conf 更新が永久に届かない
    #[test]
    #[cfg(unix)]
    fn sync_confは稼働中サーバーへ設定を再適用する() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-sync-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let tmux = crate::tmux::tmux_bin();
        // 旧バージョン相当: conf 無し（/dev/null）でサーバーを起動しておく
        let status = Command::new(tmux)
            .args([
                "-L",
                &socket,
                "-f",
                "/dev/null",
                "new-session",
                "-d",
                "-s",
                "x",
            ])
            .arg("sleep 30")
            .status()
            .expect("tmux サーバーを起動できる");
        assert!(status.success());
        let show = |opt: &str| -> Option<String> {
            let out = Command::new(tmux)
                .args(["-L", &socket, "show-options", "-g", "-v", opt])
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        // 既定では copy-mode-position-format が空でない（tmux 3.6+。
        // オプション自体が無い古い tmux では検証をスキップ）
        let Some(before) = Command::new(tmux)
            .args([
                "-L",
                &socket,
                "show-options",
                "-g",
                "-w",
                "-v",
                "copy-mode-position-format",
            ])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        else {
            crate::tmux_test_gate::skip("copy-mode-position-format 非対応の tmux");
            return;
        };
        assert!(
            !before.is_empty(),
            "前提が変わった: 既定でインジケータが空（テストの意味が無い）"
        );
        sync_conf(&socket);
        let after = Command::new(tmux)
            .args([
                "-L",
                &socket,
                "show-options",
                "-g",
                "-w",
                "-v",
                "copy-mode-position-format",
            ])
            .output()
            .expect("show-options が動く");
        assert_eq!(
            String::from_utf8_lossy(&after.stdout).trim(),
            "",
            "sync_conf 後もインジケータ書式が既定のまま（再適用されていない）"
        );
        // 他の主要設定も同期されている（mouse on は wheel 配送の前提）
        assert_eq!(show("mouse").as_deref(), Some("on"));
    }

    /// マウス**非要求**の alt-screen アプリ（ペイン内 `tmux attach` のネストや全画面 TUI）への
    /// ホイールが矢印キーに化けない e2e（2026-06-12 実機リグレッション (1) の再発防止）。
    /// tmux の既定はこの構成でホイール → ↑↓ 変換（入力履歴が回る事故の元）なので、
    /// バックエンド conf がこれを抑止していることを検証する
    #[test]
    #[cfg(unix)]
    fn alt_screenの非マウスペインでホイールが矢印に化けない() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-alt-{}", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        // 内側: alt screen に入るだけでマウスは要求しない（claude を内包する
        // ネスト tmux クライアントや less / vim 既定がこの形）
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), r"printf '\033[?1049h'; exec cat -v".into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-alt"))
                .expect("tmux クライアントを spawn できる");
        // 前提（alt screen ペインであること）の成立を待つ。これが無いと通常画面のまま
        // ホイールを送って「矢印に化けない」を見てしまい、検出力が落ちる（#625）
        assert_eq!(
            wait_alt_screen(&socket, "tako-e2e-alt"),
            Some(true),
            "alt-screen 切替が完了しない。画面: {:?}",
            session.visible_lines().join("\n")
        );
        // 外側のマウスモード（バックエンドの mouse on）を待つ
        for _ in 0..100 {
            if session.mouse_reporting() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(session.mouse_reporting(), "バックエンドの mouse on が効く");
        // 上下ホイール → 矢印キーが内側へ送られないこと
        session.scroll_wheel(1, 5, 5);
        session.scroll_wheel(-1, 5, 5);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let lines = session.visible_lines().join("\n");
        assert!(
            !lines.contains("^[[A") && !lines.contains("^[OA") && !lines.contains("^[[B"),
            "ホイールが矢印キーに化けている（リグレッション (1)）。画面: {lines:?}"
        );
    }
    #[test]
    fn pane_factsはttyとpidを1行から切り出す() {
        let f = super::parse_pane_facts("/dev/ttys012\t8768");
        assert_eq!(f.tty.as_deref(), Some("/dev/ttys012"));
        assert_eq!(f.pid, Some(8768));

        // psmux は tty を持たないことがある（pid だけ採れれば側路は張れる）
        let f = super::parse_pane_facts("\t8768");
        assert_eq!(f.tty, None);
        assert_eq!(f.pid, Some(8768));

        // セッション未作成 / 器不在
        assert_eq!(super::parse_pane_facts(""), super::PaneFacts::default());
        // pid 0 は「取れなかった」と同じ扱い（張ると誰も書かないファイルを読む）
        assert_eq!(super::parse_pane_facts("/dev/ttys012\t0").pid, None);
    }
}

/// #2085: 器が自ら閉じたペインの最後の出力の書き出し（`pane-died` フック）
#[cfg(test)]
mod pane_died_tests {
    use super::*;
    use crate::backend::SessionBackend;

    fn tmux_caps() -> BackendCapabilities {
        crate::backend::TmuxBackend::new().capabilities()
    }

    fn psmux_caps() -> BackendCapabilities {
        crate::backend::PsmuxBackend::new("psmux".into(), "3.3.7".into()).capabilities()
    }

    /// テストの一時ディレクトリ（Drop で消す）
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("tako-coretest-2085-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn 能力の無い器へは1行も書かない() {
        // psmux は `pane-died` の形を実測していない。1 行でも書くと未知の設定の警告が
        // ペインへ出る形（#974）を作りうるので、何も書かない
        assert_eq!(
            pane_died_conf(&psmux_caps(), Some(Path::new("/tmp/d")), false),
            ""
        );
        assert_eq!(pane_died_conf(&psmux_caps(), None, true), "");
    }

    #[test]
    fn ab_と置き場が無いときは立っていたものを外す() {
        let off = "set -g remain-on-exit off\nset-hook -gu pane-died\n";
        assert_eq!(
            pane_died_conf(&tmux_caps(), Some(Path::new("/tmp/d")), true),
            off
        );
        assert_eq!(pane_died_conf(&tmux_caps(), None, false), off);
    }

    /// 実測で決めた形を崩さない（崩れると死んだペインが残り続けて**閉じなくなる**）
    #[test]
    fn フックはkill_paneを入れ子の外に置きremain_on_exitはset_hookと同じ行() {
        let conf = pane_died_conf(&tmux_caps(), Some(Path::new("/tmp/x")), false);
        let lines: Vec<&str> = conf.lines().collect();
        let at = |needle: &str| {
            lines
                .iter()
                .position(|l| *l == needle)
                .unwrap_or_else(|| panic!("{needle:?} が無い:\n{conf}"))
        };
        let inner_end = at("  }");
        let kill = at("  kill-pane");
        let hook_end = lines
            .iter()
            .position(|l| l.starts_with("} ;"))
            .expect("set-hook の閉じ");
        assert!(
            inner_end < kill && kill < hook_end,
            "kill-pane は入れ子の外・フックの中:\n{conf}"
        );
        // 失敗しうるコマンドは入れ子の中だけ
        for risky in ["capture-pane", "save-buffer", "delete-buffer", "set -gF"] {
            let line = lines
                .iter()
                .position(|l| l.contains(risky))
                .unwrap_or_else(|| panic!("{risky} が無い"));
            assert!(
                line < inner_end && lines[line].starts_with("    "),
                "{risky} が入れ子の外にある:\n{conf}"
            );
        }
        // `remain-on-exit on` は set-hook と同じ行（set-hook が通らなければ立たない）
        let on: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains("remain-on-exit on"))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(on, vec![hook_end], "remain-on-exit on の置き場:\n{conf}");
        assert!(lines[hook_end].contains("remain-on-exit-format ''"));
        // save-buffer のパスはフックのペインの書式を直接読まない（同時死で別のペインになる）
        let save = lines.iter().find(|l| l.contains("save-buffer")).unwrap();
        assert!(
            !save.contains("#{session_name}") && save.contains("#{@tako-pane-died-name}"),
            "{save}"
        );
    }

    #[test]
    fn 置き場のパスは設定の1語として引用される() {
        assert_eq!(conf_quote("/a b/c"), "'/a b/c'");
        assert_eq!(conf_quote("/it's/$x"), "\"/it's/\\$x\"");
        assert_eq!(conf_quote("/q\"\\"), "'/q\"\\'");
        let conf = pane_died_conf(&tmux_caps(), Some(Path::new("/d d")), false);
        assert!(
            conf.starts_with("set -g @tako-pane-died-dir '/d d'\n"),
            "{conf}"
        );
    }

    #[test]
    fn 書き出しの名前を読む() {
        let p = parse_pane_died_name("tako-ab.281.10000.42040.0.dump", "tako-ab").unwrap();
        assert_eq!(
            (p.history, p.limit, p.bytes, p.alternate),
            (281, 10000, 42040, false)
        );
        assert!(
            parse_pane_died_name("tako-ab.0.10000.80.1.dump", "tako-ab")
                .unwrap()
                .alternate
        );
        // 前方一致の別のセッション・形の崩れたものは読まない
        assert_eq!(
            parse_pane_died_name("tako-abc.1.2.3.0.dump", "tako-ab"),
            None
        );
        assert_eq!(parse_pane_died_name("tako-ab.1.2.3.0.txt", "tako-ab"), None);
        assert_eq!(parse_pane_died_name("tako-ab.1.2.0.dump", "tako-ab"), None);
        assert_eq!(
            parse_pane_died_name("tako-ab.1.2.3.4.0.dump", "tako-ab"),
            None
        );
        assert_eq!(
            parse_pane_died_name("tako-ab.x.2.3.0.dump", "tako-ab"),
            None
        );
        assert_eq!(
            parse_pane_died_name("tako-ab.1.2.3.2.dump", "tako-ab"),
            None
        );
        // セッション名に `.` が入っても数値は右から読む
        assert_eq!(
            parse_pane_died_name("a.b.5.6.7.0.dump", "a.b").map(|p| p.history),
            Some(5)
        );
    }

    #[test]
    fn 書き出しを履歴と画面に分ける() {
        let probe = parse_pane_died_name("s.2.10.0.0.dump", "s").unwrap();
        let dump = split_pane_died_dump(probe, "h1\nh2  \ns1\n\n").unwrap();
        assert_eq!(dump.history, vec!["h1", "h2"]);
        assert_eq!(dump.screen, vec!["s1", ""]);
        // 履歴に足りない書き出しは壊れている
        assert_eq!(split_pane_died_dump(probe, "h1\n"), None);
    }

    #[test]
    fn 書き出しを読んで消し古いものを掃除する() {
        let tmp = TempDir::new("files");
        let dir = &tmp.0;
        std::fs::write(dir.join("tako-a.1.10.0.0.dump"), "old\nscr\n").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("tako-a.2.10.0.0.dump"), "h1\nh2\nscr\n").unwrap();
        std::fs::write(dir.join("tako-b.0.10.0.0.dump"), "b\n").unwrap();
        std::fs::write(dir.join("note.txt"), "x").unwrap();
        let dump = take_pane_died_dump(dir, "tako-a").expect("新しい方を読む");
        assert_eq!(dump.history, vec!["h1", "h2"]);
        assert_eq!(dump.screen, vec!["scr"]);
        assert!(
            take_pane_died_dump(dir, "tako-a").is_none(),
            "読んだら同じセッションの書き出しは全部消える"
        );
        discard_pane_died_dumps(dir, "tako-b");
        assert!(!dir.join("tako-b.0.10.0.0.dump").exists());
        std::fs::write(dir.join("tako-c.0.10.0.0.dump"), "c\n").unwrap();
        assert_eq!(
            sweep_pane_died_dumps(dir, std::time::Duration::from_secs(3600)),
            0,
            "新しい書き出しは掃除しない"
        );
        assert_eq!(sweep_pane_died_dumps(dir, std::time::Duration::ZERO), 1);
        assert!(dir.join("note.txt").exists(), "書き出し以外は触らない");
        assert!(take_pane_died_dump(&dir.join("none"), "tako-a").is_none());
    }

    /// tmux の器をテストの conf（製品と同じ 2 つの生成物）で起こす
    #[cfg(unix)]
    fn start_server(socket: &str, conf: &Path) {
        let r = crate::tmux::tmux_command(Some(socket))
            .args(["-f"])
            .arg(conf)
            .args(["new-session", "-d", "-s", "keep", "sleep", "300"])
            .output()
            .expect("tmux new-session");
        assert!(r.status.success(), "{r:?}");
    }

    #[cfg(unix)]
    fn write_conf(tmp: &TempDir, dump_dir: &Path) -> PathBuf {
        let conf = tmp.0.join("tmux.conf");
        let caps = tmux_caps();
        std::fs::write(
            &conf,
            backend_conf(&caps) + &pane_died_conf(&caps, Some(dump_dir), false),
        )
        .unwrap();
        conf
    }

    /// 器を 1 つ起こす（サーバーは [`start_server`] が起こし済み。`-f` は起動時にしか
    /// 読まれないが、器を起こす文には必ず付ける = #1874 / #1918 の番犬の規則）
    #[cfg(unix)]
    fn new_vessel(socket: &str, conf: &Path, name: &str, script: &str) {
        let r = crate::tmux::tmux_command(Some(socket))
            .args(["-f"])
            .arg(conf)
            .args([
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                "80",
                "-y",
                "20",
                "/bin/sh",
                "-c",
                script,
            ])
            .output()
            .expect("tmux new-session");
        assert!(r.status.success(), "{r:?}");
    }

    /// 器の死・消滅を待つ上限の基準（#2357）。止まったときの保険で、合図は状態
    /// （混み具合で `wait_budget::state_wait_budget` が伸ばす）
    #[cfg(unix)]
    const VESSEL_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

    /// 器が消えたと**確かめられる**まで待つ（#2357）。上限で偽 + 診断を stderr へ。
    ///
    /// 消えたの判定は製品の `backend_reattach::probe_session`（セッションが無い・
    /// サーバーごと無い）。`has_session` の偽は打ち切り・起動失敗でも偽なので使わない
    #[cfg(unix)]
    fn wait_gone(socket: &str, name: &str) -> bool {
        use crate::backend_reattach::{probe_session, SessionProbe};
        let mut last = SessionProbe::Unknown;
        super::tests::wait_for_state(
            &format!("器 {name} が消える"),
            VESSEL_WAIT,
            std::time::Duration::from_millis(100),
            || {
                last = probe_session(socket, name);
                matches!(last, SessionProbe::SessionGone | SessionProbe::ServerGone).then_some(())
            },
        )
        .map_err(|e| {
            eprintln!(
                "{}",
                e.observed(format!("{last:?}")).tagged("TAKO_2357_WAIT")
            )
        })
        .is_ok()
    }

    /// 器が消えたと確かめてから、その死の書き出しを読む（#2357）。無ければ名指しで落とす。
    ///
    /// tmux は `pane-died` フックの中で `save-buffer`（fopen → fwrite → fclose。書き終わるまで
    /// 後ろのコマンドを待たせる）を終えてから `kill-pane` するので、**器が消えた時点で
    /// 書き出しは書き終わっている**。製品も同じ順で読む（attach クライアントが終わった =
    /// 器が消えた後。tako-app の `finish_backend_pane_log_exit`）。
    ///
    /// 修正前は「書き出しの出現」を合図に [`take_pane_died_dump`] を 100 ms ごとに呼んでいた。
    /// 書きかけ（履歴に足りない）を読むと壊れた書き出しとして**消す**ので、残りの書き込みは
    /// 名前の無いファイルへ行き、書き出しは二度と現れない（CI の「tako-m5 の書き出しが無い」。
    /// 書き出しを 2 回に分けて間を空ける注入で同じ文言を再現した）。
    ///
    /// [`new_vessel`] の器専用（器を同期に起こすので、消えた = 起きた後に死んだ）。
    /// `TerminalSession::spawn` の器は非同期に起きるので、先に attach クライアントの終わりを待つ
    #[cfg(unix)]
    fn wait_dump(socket: &str, dir: &Path, session: &str) -> PaneDiedDump {
        assert!(
            wait_gone(socket, session),
            "{session} の書き出しが無い: 器が消えない（死なない・死んだペインが残っている）"
        );
        take_dump_after_gone(dir, session)
    }

    /// 消えた器の書き出しを 1 回だけ読む（#2357）。無い・壊れているなら名指しで落とす
    #[cfg(unix)]
    fn take_dump_after_gone(dir: &Path, session: &str) -> PaneDiedDump {
        let named: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_string))
            .filter(|n| parse_pane_died_name(n, session).is_some())
            .collect();
        take_pane_died_dump(dir, session).unwrap_or_else(|| {
            if named.is_empty() {
                panic!("{session} の書き出しが無い: 器は消えたのにフックが書いていない")
            }
            panic!("{session} の書き出しが壊れている（履歴の行数に足りない）: {named:?}")
        })
    }

    /// 番号つきの行の並び（`<接頭辞><番号>` の行だけを拾う）
    #[cfg(unix)]
    fn numbers(lines: &[String], prefix: &str) -> Vec<usize> {
        lines
            .iter()
            .filter_map(|l| l.strip_prefix(prefix)?.parse().ok())
            .collect()
    }

    /// 製品の経路（`wrap_options` = 製品の conf で起きる attach クライアント）で、
    /// 出力し続けている最中に中のコマンドが終わっても最後の行まで書き出される
    #[test]
    #[cfg(unix)]
    fn 実tmux_出力中に器が自ら消えても最後の行まで書き出される() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-2085wrap", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let session = "tako-2085-wrap";
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "i=0; while [ $i -lt 3000 ]; do echo W$i; i=$((i+1)); done".into(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
            scrollback_lines: None,
        };
        let (mut client, mut rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, session))
                .expect("tmux クライアントを spawn できる");
        // 製品が書き出しを読む合図（attach クライアントの終わり = `CloseOrigin::ProcessExit`）を
        // 待つ。器は非同期に起きるので「器が無い」は合図にならない（起きる前も無い。#2357）。
        // 端末のイベントは UI 層と同じく流す（tmux の端末クエリへの応答）
        super::tests::wait_for_state(
            "attach クライアントが終わる",
            VESSEL_WAIT,
            std::time::Duration::from_millis(100),
            || {
                let mut exited = false;
                while let Ok(event) = rx.try_recv() {
                    exited |= matches!(
                        client.process_event(event),
                        Some(crate::SessionNotice::Exited)
                    );
                }
                exited.then_some(())
            },
        )
        .unwrap_or_else(|e| {
            let screen = client.visible_lines().join("\n");
            panic!("{}", e.observed(screen).tagged("TAKO_2357_WAIT"))
        });
        assert!(wait_gone(&socket, session), "書き出した後に器が消えない");
        drop(client);
        let dir = pane_died_dir().expect("テストのデータディレクトリ");
        let dump = take_dump_after_gone(&dir, session);
        let mut all = dump.history.clone();
        all.extend(dump.screen.iter().cloned());
        assert_eq!(
            numbers(&all, "W"),
            (0..3000).collect::<Vec<_>>(),
            "0〜2999 が欠けず・重ならずに入る"
        );
        assert_eq!(dump.history.len(), dump.probe.history);
        assert_eq!(
            dump.screen.len(),
            24,
            "画面の行（attach クライアントの寸法）"
        );
        assert!(
            dump.screen.iter().all(|l| !l.contains("Pane is dead")),
            "remain-on-exit の既定の案内は書き込まない: {:?}",
            dump.screen
        );
    }

    /// tako が居ない間に消えた器の書き出しは、同じ名前の器を作り直す / 繋ぎ直すとき
    /// （`wrap_options`）に消える = 作り直した器の死と取り違えない
    #[test]
    fn 同じ名前の器を作るときは前の書き出しを消す() {
        let dir = pane_died_dir().expect("テストのデータディレクトリ");
        std::fs::create_dir_all(&dir).unwrap();
        let session = format!("tako-2085-stale-{}", std::process::id());
        let stale = dir.join(format!("{session}.3.10000.0.0.dump"));
        std::fs::write(&stale, "a\nb\nc\nscreen\n").unwrap();
        let other = dir.join(format!("{session}x.3.10000.0.0.dump"));
        std::fs::write(&other, "x\n").unwrap();
        let _ = wrap_options(SpawnOptions::default(), "tako-test", &session);
        assert!(!stale.exists(), "前の死の書き出しが残っている");
        assert!(other.exists(), "別の器の書き出しまで消した");
        let _ = std::fs::remove_file(other);
    }

    /// alt screen（TUI）のまま死んでも書き出され、ペインは閉じる（名前の alternate_on が 1）
    #[test]
    #[cfg(unix)]
    fn 実tmux_altscreenのまま死んでも書き出して閉じる() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-2085alt", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let tmp = TempDir::new("alt");
        let conf = write_conf(&tmp, &tmp.0);
        start_server(&socket, &conf);
        new_vessel(
            &socket,
            &conf,
            "tako-alt",
            "printf '\\033[?1049h'; seq 1 30",
        );
        assert!(
            wait_gone(&socket, "tako-alt"),
            "alt screen のまま死んだペインが残る"
        );
        let dump = take_dump_after_gone(&tmp.0, "tako-alt");
        assert!(dump.probe.alternate, "alt screen の印: {:?}", dump.probe);
        // alt screen の画面（履歴の無い面）の最後の行まで、番号が続いて入る
        let shown = numbers(&dump.screen, "");
        assert_eq!(
            shown.last(),
            Some(&30),
            "alt screen の画面: {:?}",
            dump.screen
        );
        assert!(shown.windows(2).all(|w| w[1] == w[0] + 1), "{shown:?}");
    }

    /// 明示の kill（tako がペインを閉じる経路）はフックを起こさない = 書き出さない
    #[test]
    #[cfg(unix)]
    fn 実tmux_明示のkillでは書き出さない() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-2085kill", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let tmp = TempDir::new("kill");
        let conf = write_conf(&tmp, &tmp.0);
        start_server(&socket, &conf);
        new_vessel(&socket, &conf, "tako-live", "seq 1 50; sleep 300");
        std::thread::sleep(std::time::Duration::from_millis(300));
        kill_session(&socket, "tako-live");
        assert!(wait_gone(&socket, "tako-live"));
        assert!(take_pane_died_dump(&tmp.0, "tako-live").is_none());
    }

    /// 書き出しが失敗しても `kill-pane` は走る = ペインは閉じる。
    ///
    /// その場で失敗するコマンド（存在しないバッファの `save-buffer`）は同じリストの後ろを
    /// 全部捨てるので、入れ子に隔離していないと死んだペインが残り続ける（修正の過程で実測した形）。
    /// 置き場が無いときの書き込みの失敗は後から報告されるだけ（こちらも閉じることを見る）
    #[test]
    #[cfg(unix)]
    fn 実tmux_書き出しに失敗してもペインは閉じる() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-2085fail", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let tmp = TempDir::new("fail");
        let caps = tmux_caps();
        // その場で失敗する: 製品の conf の `save-buffer` だけを無いバッファへ向ける
        let conf = tmp.0.join("tmux.conf");
        let failing = pane_died_conf(&caps, Some(&tmp.0), false).replace(
            "save-buffer -b tako-pane-died ",
            "save-buffer -b tako-pane-died-missing ",
        );
        assert!(failing.contains("tako-pane-died-missing"));
        std::fs::write(&conf, backend_conf(&caps) + &failing).unwrap();
        start_server(&socket, &conf);
        new_vessel(&socket, &conf, "tako-fail", "seq 1 50");
        assert!(
            wait_gone(&socket, "tako-fail"),
            "書き出しの失敗で kill-pane まで捨てられた（死んだペインが残っている）"
        );
        // 置き場が無い（書き込みが後から失敗する）
        let missing_dir = write_conf(&tmp, &tmp.0.join("no-such-dir"));
        crate::tmux::tmux_command(Some(&socket))
            .arg("source-file")
            .arg(&missing_dir)
            .output()
            .expect("tmux source-file");
        new_vessel(&socket, &missing_dir, "tako-fail2", "seq 1 50");
        assert!(
            wait_gone(&socket, "tako-fail2"),
            "置き場が無いと死んだペインが残る"
        );
    }

    /// 同時に死んだ器でも、書き出しの名前（セッション名・行数）と中身が食い違わない。
    /// `save-buffer` のパスへ書式を直接書くと、別のペインの文脈で展開された（実測）
    #[test]
    #[cfg(unix)]
    fn 実tmux_同時に死んでも名前と中身が食い違わない() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-2085many", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let tmp = TempDir::new("many");
        let conf = write_conf(&tmp, &tmp.0);
        start_server(&socket, &conf);
        for i in 1..=5 {
            new_vessel(
                &socket,
                &conf,
                &format!("tako-m{i}"),
                &format!("seq -f 'M{i}-%g' 1 {}", i * 100),
            );
        }
        // 器の外のセッション（`tako-` で始まらない）は書き出さない（kill-pane だけ）
        new_vessel(&socket, &conf, "other", "seq 1 5");
        let dumps: Vec<PaneDiedDump> = (1..=5)
            .map(|i| wait_dump(&socket, &tmp.0, &format!("tako-m{i}")))
            .collect();
        assert!(wait_gone(&socket, "other"));
        for (i, dump) in (1..=5).zip(dumps) {
            let mut all = dump.history.clone();
            all.extend(dump.screen.iter().cloned());
            assert_eq!(
                numbers(&all, &format!("M{i}-")),
                (1..=i * 100).collect::<Vec<_>>(),
                "tako-m{i} の書き出しに別の器の中身が入った"
            );
            assert_eq!(dump.history.len(), i * 100 + 1 - 20, "tako-m{i} の行数");
        }
        let left: Vec<_> = std::fs::read_dir(&tmp.0)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".dump"))
            .collect();
        assert!(
            left.is_empty(),
            "器の外のセッションまで書き出した: {left:?}"
        );
    }

    /// 書き出しが分割されて間が空いても・器が即座に死んでも、書き終わった書き出しを読む（#2357）。
    ///
    /// tmux の `save-buffer` は fopen → fwrite → fclose の 1 回だが、書きかけ（作られた直後・
    /// 途中まで）は外から見える。ここでは `tako-split` の書き出しを「履歴の最後の 1 行の
    /// 手前まで → 1 秒あけて → 残りを追記」に分けて、その窓を広げる。書き出しの出現を
    /// 合図に読む修正前の待ち方は書きかけを壊れた書き出しとして消し、追記は末尾だけの
    /// 新しいファイルになってそれも消し、「書き出しが無い」で落ちた
    #[test]
    #[cfg(unix)]
    fn 実tmux_書き出しが分割されて遅れても器が即座に死んでも書き終わってから読む() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-2357split", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let tmp = TempDir::new("split");
        let caps = tmux_caps();
        let original = pane_died_conf(&caps, Some(&tmp.0), false);
        let save = original
            .lines()
            .find(|l| l.contains("save-buffer"))
            .expect("製品の conf の save-buffer の行");
        let path = save
            .split_once("-b tako-pane-died ")
            .expect("save-buffer の書き先")
            .1;
        let split = original.replacen(
            save,
            &format!(
                "    if -F '#{{==:#{{session_name}},tako-split}}' {{
      capture-pane -b tako-2357-head -S - -E -2
      save-buffer -b tako-2357-head {path}
      run-shell 'sleep 1'
      capture-pane -b tako-2357-tail -S -1 -E -
      save-buffer -a -b tako-2357-tail {path}
      delete-buffer -b tako-2357-head
      delete-buffer -b tako-2357-tail
    }} {{
  {save}
    }}"
            ),
            1,
        );
        assert_ne!(split, original);
        let conf = tmp.0.join("tmux.conf");
        std::fs::write(&conf, backend_conf(&caps) + &split).unwrap();
        start_server(&socket, &conf);
        new_vessel(&socket, &conf, "tako-split", "seq -f 'S-%g' 1 300");
        new_vessel(&socket, &conf, "tako-instant", "echo I-1");
        let dump = wait_dump(&socket, &tmp.0, "tako-split");
        let mut all = dump.history.clone();
        all.extend(dump.screen.iter().cloned());
        assert_eq!(
            numbers(&all, "S-"),
            (1..=300).collect::<Vec<_>>(),
            "分割された書き出しの全体"
        );
        assert_eq!(dump.history.len(), 300 + 1 - 20, "tako-split の行数");
        let instant = wait_dump(&socket, &tmp.0, "tako-instant");
        assert_eq!(instant.probe.history, 0, "{:?}", instant.probe);
        assert_eq!(
            numbers(&instant.screen, "I-"),
            vec![1],
            "{:?}",
            instant.screen
        );
    }

    /// `set-hook` が通らない tmux では `remain-on-exit` も立たない（同じ行 = 同じグループ）。
    /// 逆だと、死んだペインを片付ける人が居ないまま残し続ける
    #[test]
    #[cfg(unix)]
    fn 実tmux_set_hookが通らなければremain_on_exitも立たない() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-2085hook", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        let tmp = TempDir::new("hook");
        let conf = tmp.0.join("tmux.conf");
        let caps = tmux_caps();
        // 知らないフック名 = `set-hook` だけが失敗する tmux を模す
        let broken = pane_died_conf(&caps, Some(&tmp.0), false)
            .replace("set-hook -g pane-died", "set-hook -g pane-died-unknown");
        std::fs::write(&conf, backend_conf(&caps) + &broken).unwrap();
        start_server(&socket, &conf);
        let out = crate::tmux::tmux_command(Some(&socket))
            .args(["show-options", "-g", "remain-on-exit"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "remain-on-exit off"
        );
        new_vessel(&socket, &conf, "tako-nohook", "seq 1 5");
        assert!(wait_gone(&socket, "tako-nohook"));
    }
}

/// #2367: 器の tmux サーバーの fd の上限と使用数の読み・persist.log の 1 行
#[cfg(test)]
mod nofile_tests {
    use super::*;
    use crate::platform::fd_limit::ContainerLimit;

    fn limits(soft: Option<u64>) -> ServerLimits {
        ServerLimits {
            pid: 4242,
            soft,
            hard: None,
            hard_unlimited: true,
            open: Some(90),
        }
    }

    #[test]
    fn 応答からpidと上限を読む() {
        assert_eq!(parse_server_pid(true, "4242\n", ""), Ok(4242));
        assert_eq!(
            parse_server_limits("256\nunlimited\n"),
            (Some(256), None, true)
        );
        assert_eq!(
            parse_server_limits("10240\n10240\n"),
            (Some(10240), Some(10240), false),
            "hard が数（launchctl limit maxfiles で絞った機）"
        );
        assert_eq!(
            parse_server_limits(""),
            (None, None, false),
            "run-shell が返らなかった（上限の間際）なら上限は不明"
        );
    }

    #[test]
    fn 居ないサーバーと届かないサーバーを分ける() {
        for err in [
            "no server running on /private/tmp/tmux-501/tako\n",
            "error connecting to /private/tmp/tmux-501/tako (No such file or directory)\n",
            "error connecting to /private/tmp/tmux-501/tako (Connection refused)\n",
        ] {
            assert_eq!(
                parse_server_pid(false, "", err),
                Err(ServerNofile::NotRunning),
                "{err}"
            );
        }
        assert_eq!(
            parse_server_pid(false, "", "server exited unexpectedly\n"),
            Err(ServerNofile::Unreachable {
                reason: "server exited unexpectedly".to_string(),
                last: None,
            }),
            "上限に届いたサーバーの字面（tmux 3.6b の実測）"
        );
        assert!(matches!(
            parse_server_pid(false, "", ""),
            Err(ServerNofile::Unreachable { .. })
        ));
        assert!(matches!(
            parse_server_pid(true, "0\n", ""),
            Err(ServerNofile::Unreachable { .. })
        ));
    }

    #[test]
    fn 器のペインの数の目安() {
        assert_eq!(
            server_pane_capacity(256),
            83,
            "Finder 起動の soft 256 = 約 80 ペイン"
        );
        assert_eq!(server_pane_capacity(10240), 3411);
        assert_eq!(
            server_pane_capacity(4),
            0,
            "基礎の本数に届かなくても負にならない"
        );
    }

    #[test]
    fn persist_logの1行は上限の決まり方ごとに組める() {
        let raised = server_nofile_log_line(
            "tako",
            &limits(Some(10240)),
            Some(ContainerLimit::Raised),
            Some(256),
        );
        assert_eq!(
            raised,
            "器の tmux サーバー（ソケット tako・pid 4242）: fd の上限 soft 10240・hard 無制限\
             （raised: GUI が引き上げた値を受け継いだ。器のペインのシェルには 256 を渡す）（#2367）"
        );
        let predates = server_nofile_log_line(
            "tako",
            &limits(Some(256)),
            Some(ContainerLimit::Predates),
            Some(256),
        );
        assert!(
            predates.contains("引き上げより前に起きたサーバーなので起動時の値のまま")
                && predates.contains("約 83 ペインで詰まる"),
            "{predates}"
        );
        let legacy = server_nofile_log_line(
            "tako",
            &limits(Some(256)),
            Some(ContainerLimit::Legacy),
            Some(256),
        );
        assert!(legacy.contains("TAKO_2367_LEGACY のため"), "{legacy}");
        let launch = server_nofile_log_line(
            "tako",
            &limits(Some(256)),
            Some(ContainerLimit::LaunchValue),
            None,
        );
        assert!(
            launch.contains("tako 自身が起動時に引き上げていない"),
            "{launch}"
        );
        assert_eq!(
            server_nofile_log_line("tako", &limits(None), None, None),
            "器の tmux サーバー（ソケット tako・pid 4242）: fd の上限を読めなかった（#2367）"
        );
    }

    /// 実 tmux: 居なければ起こさない・soft を絞って起こしたサーバーの soft / hard / 使用数を読む
    #[test]
    #[cfg(target_os = "macos")]
    fn issue2367_実tmuxのサーバーの上限と使用数を読みサーバーは起こさない() {
        if !crate::tmux_test_gate::container() {
            return;
        }
        let socket = format!("tako-coretest-{}-nofile2367", std::process::id());
        let _cleanup = TmuxTestGuard::new(vec![socket.clone()]);
        assert_eq!(server_nofile(&socket), ServerNofile::NotRunning);
        assert!(
            !socket_file_present(&socket),
            "探針（display-message ; run-shell）はサーバーを起こさない"
        );
        // soft だけ 200 に絞ったシェルからサーバーを起こす（hard は触らない）
        let mut sh = std::process::Command::new("/bin/sh");
        crate::platform::process::no_console_window(&mut sh);
        let start = sh
            .arg("-c")
            .arg(format!(
                "ulimit -Sn 200 && exec {} -L {} -f /dev/null new-session -d -s n1 'sleep 600'",
                crate::shell::quote_for_shell(crate::tmux::tmux_bin()),
                crate::shell::quote_for_shell(&socket),
            ))
            .status()
            .expect("sh を起こせる");
        assert!(start.success(), "絞った soft でサーバーを起こせる");
        let ServerNofile::Running(first) = server_nofile(&socket) else {
            panic!("起きたサーバーを読める: {:?}", server_nofile(&socket));
        };
        assert_eq!(
            first.soft,
            Some(200),
            "run-shell の子はサーバーの soft を受け継ぐ"
        );
        assert!(
            first.hard_unlimited || first.hard.is_some_and(|h| h >= 200),
            "{first:?}"
        );
        let open = first.open.expect("proc_pidinfo で数えられる");
        assert!(open >= 3, "{first:?}");
        // 器のペイン（detached = クライアント無し）を 5 本足すと 5 本増える
        for _ in 0..5 {
            let out = crate::tmux::tmux_command(Some(&socket))
                .args(["new-window", "-d", "sleep 600"])
                .output()
                .expect("new-window");
            assert!(out.status.success());
        }
        let ServerNofile::Running(later) = server_nofile(&socket) else {
            panic!("読める");
        };
        assert_eq!(later.pid, first.pid);
        assert_eq!(
            later.soft,
            Some(200),
            "同じサーバーの上限は覚えた値（run-shell を投げ直さない）"
        );
        // 問い合わせた客の接続はサーバーが EOF を読むまで 1 本残ることがある（どちらの読みにも）
        let grew = later.open.expect("数えられる").saturating_sub(open);
        assert!((4..=6).contains(&grew), "{first:?} → {later:?}");
    }
}
