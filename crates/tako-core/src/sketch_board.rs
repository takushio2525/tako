//! Arduino のスケッチの基板（FQBN）とポートの選択（Issue #1952 / FR-3.18.8。エピック #1943 の S5。
//! 設計 `.agent/plans/2026-10-language-coverage.md` §5.4 の 4）
//!
//! **正本は Arduino 公式の `sketch.yaml`**（`default_fqbn` / `default_port`）。Arduino IDE / arduino-cli と
//! 同じファイルを見るので食い違わない。**書くのも `arduino-cli board attach`**（書式を tako が真似しない）。
//! ここは子プロセスを起こさない純粋な層で、起こすのは tako-control の `sketch_board`（background だけ）。
//!
//! ## 書く前に tako が検める理由
//!
//! `board attach` は**何でも書く**（実測 1.2.0: 入っているコアに無い基板 `arduino:avr:nosuch`・入っていない
//! コアの基板 `arduino:renesas_uno:unor4wifi`・形の崩れた `foo` まで rc=0 で `default_fqbn` に書く）。
//! そのまま書くと compile と言語サーバが英語の 1 行で落ちる状態をこちらから作ることになるので、
//! `board listall`（入っているコアの基板の一覧）で**コアが入っているか・その基板があるか**を確かめてから
//! 書く。コアが無ければ `arduino-cli core install <コア>` を**示すだけ**で代行しない（数百 MB の
//! ツールチェーンを落とす = 設計 §5.5）。
//!
//! ## 一覧の取り方（実測 1.2.0・この PC の 5 コア 545 基板）
//!
//! | 呼び方 | 出力 | 時間 |
//! |---|---|---|
//! | `board listall --format json` | 20.9 MB（基板ごとにコアの全基板を繰り返す） | 2.0 秒 |
//! | `board listall --format jsonmini` | 11.5 MB | 0.4 秒 |
//! | `board listall`（表） | 40 KB | 0.36 秒 |
//!
//! 表は見出しが訳されうる・列の区切りが空白なので読まず、`jsonmini` を**使う欄だけの型**で読む
//! （入れ子の基板の列は serde が読み飛ばすので、11 MB を `Value` に展開しない）。
//! `board list`（繋がっている基板とポート）は小さいので `json`（#1950 の案内と同じ 1 実装 = [`parse_board_list`]）。

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::i18n::Lang;
use crate::platform::support::Platform;

/// `arduino-cli` へ渡す引数: 繋がっている基板とポート（#1950 の案内の `Followup` と同じ）
pub const LIST_ARGS: &[&str] = &["board", "list", "--format", "json"];
/// `arduino-cli` へ渡す引数: 入っているコアの基板（上の表の理由で `jsonmini`）
pub const LISTALL_ARGS: &[&str] = &["board", "listall", "--format", "jsonmini"];

/// A/B（`TAKO_1952_LEGACY=1`）: 基板とポートの欄・書き込みの口が無い #1952 以前へ戻す
pub fn legacy() -> bool {
    std::env::var_os("TAKO_1952_LEGACY").is_some_and(|v| v == "1")
}

/// `.ino` のファイルが属するスケッチのフォルダ（`.ino` のあるディレクトリ = Code Runner の
/// プロジェクト種別 `arduino` のルートと同じ規則）。`.ino` でなければ / A/B 中なら `None`。
/// ファイルシステムは見ない（UI スレッドで呼んでよい）
pub fn sketch_dir_of(file: &Path) -> Option<PathBuf> {
    if legacy() || crate::runner_project::arduino_disabled() {
        return None;
    }
    let ext = file.extension()?.to_string_lossy().to_ascii_lowercase();
    (ext == "ino").then(|| file.parent().map(Path::to_path_buf))?
}

/// `sketch.yaml` の今の値（無い鍵は `None`）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SketchDefaults {
    pub fqbn: Option<String>,
    pub port: Option<String>,
    /// `default_profile`（あれば compile は `default_fqbn` より**こちらを使う** = 実測 1.2.0）
    pub profile: Option<String>,
}

/// スケッチのフォルダの `sketch.yaml` を読む（無ければ全部 `None`。読み手は Code Runner の
/// 案内・言語サーバの前提と同じ 1 実装）
pub fn sketch_defaults(sketch: &Path) -> SketchDefaults {
    crate::runner_project::sketch_yaml_defaults(sketch)
}

// ─── arduino-cli の答えを読む ──────────────────────────────────────────

/// 基板の名前と FQBN
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardName {
    pub name: String,
    pub fqbn: String,
}

/// `board list` の 1 ポート
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedPort {
    /// `/dev/cu.usbmodem1101` / `COM3` / ネットワークのアドレス
    pub address: String,
    pub label: String,
    /// `serial` / `network` …
    pub protocol: String,
    /// `Serial Port` / `Serial Port (USB)` …（arduino-cli の文言のまま）
    pub protocol_label: String,
    /// そのポートに繋がっていると判った基板（FQBN のあるものだけ）
    pub boards: Vec<BoardName>,
}

/// `board list --format json` を読む（読めなければ `None`）。
///
/// 形（実測 1.2.0 + 公式の JSON 出力）: `{"detected_ports":[{"matching_boards":[{"name","fqbn"}],
/// "port":{"address","label","protocol","protocol_label", …}}]}`。基板を繋いでいなければ
/// `matching_boards` が無い（ポートだけが並ぶ）。何も無い機では `{}`
pub fn parse_board_list(stdout: &str) -> Option<Vec<DetectedPort>> {
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        detected_ports: Vec<Detected>,
    }
    #[derive(Deserialize)]
    struct Detected {
        #[serde(default)]
        matching_boards: Vec<Matching>,
        #[serde(default)]
        port: Port,
    }
    #[derive(Deserialize, Default)]
    struct Port {
        #[serde(default)]
        address: String,
        #[serde(default)]
        label: String,
        #[serde(default)]
        protocol: String,
        #[serde(default)]
        protocol_label: String,
    }
    #[derive(Deserialize)]
    struct Matching {
        #[serde(default)]
        name: String,
        #[serde(default)]
        fqbn: String,
    }
    let root: Root = serde_json::from_str(stdout).ok()?;
    Some(
        root.detected_ports
            .into_iter()
            .map(|d| DetectedPort {
                boards: d
                    .matching_boards
                    .into_iter()
                    .filter(|b| !b.fqbn.is_empty())
                    .map(|b| BoardName {
                        name: if b.name.is_empty() {
                            b.fqbn.clone()
                        } else {
                            b.name
                        },
                        fqbn: b.fqbn,
                    })
                    .collect(),
                address: d.port.address,
                label: d.port.label,
                protocol: d.port.protocol,
                protocol_label: d.port.protocol_label,
            })
            .collect(),
    )
}

/// 入っているコアの基板 1 つ（`board listall`）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledBoard {
    pub name: String,
    pub fqbn: String,
    /// `<ベンダー>:<アーキテクチャ>`（`platform.metadata.id`）
    pub core: String,
}

/// `board listall --format jsonmini`（`json` でも同じ）を読む（読めなければ `None`）。
///
/// 形（実測 1.2.0）: `{"boards":[{"name","fqbn","platform":{"metadata":{"id"},"release":{…全基板…}}}]}`。
/// `release` の中の基板の列はコアごとに全基板を繰り返す（出力が 11〜21 MB になる原因）ので**型に持たない**
/// （serde が読み飛ばす）。コアの ID が無い行は FQBN の先頭 2 段で補う
pub fn parse_listall(stdout: &str) -> Option<Vec<InstalledBoard>> {
    #[derive(Deserialize)]
    struct Root {
        #[serde(default)]
        boards: Vec<Board>,
    }
    #[derive(Deserialize)]
    struct Board {
        #[serde(default)]
        name: String,
        #[serde(default)]
        fqbn: String,
        #[serde(default)]
        platform: Option<PlatformInfo>,
    }
    #[derive(Deserialize)]
    struct PlatformInfo {
        #[serde(default)]
        metadata: Option<Metadata>,
    }
    #[derive(Deserialize)]
    struct Metadata {
        #[serde(default)]
        id: String,
    }
    let root: Root = serde_json::from_str(stdout).ok()?;
    Some(
        root.boards
            .into_iter()
            .filter(|b| !b.fqbn.is_empty())
            .map(|b| {
                let core = b
                    .platform
                    .and_then(|p| p.metadata)
                    .map(|m| m.id)
                    .filter(|id| !id.is_empty())
                    .or_else(|| core_of(&b.fqbn).map(str::to_string))
                    .unwrap_or_default();
                InstalledBoard {
                    name: if b.name.is_empty() {
                        b.fqbn.clone()
                    } else {
                        b.name
                    },
                    fqbn: b.fqbn,
                    core,
                }
            })
            .collect(),
    )
}

// ─── FQBN とポートの形 ─────────────────────────────────────────────────

/// FQBN の段（`<ベンダー>:<アーキテクチャ>:<基板>[:<オプション>]`）。3 段に満たなければ `None`
fn segments(fqbn: &str) -> Option<[&str; 3]> {
    let mut it = fqbn.splitn(4, ':');
    let (v, a, b) = (it.next()?, it.next()?, it.next()?);
    Some([v, a, b])
}

/// コア（`<ベンダー>:<アーキテクチャ>`）。`arduino:avr:nano:cpu=atmega328old` → `arduino:avr`
pub fn core_of(fqbn: &str) -> Option<&str> {
    let [v, a, _] = segments(fqbn)?;
    Some(&fqbn[..v.len() + 1 + a.len()])
}

/// オプションを除いた基板（`<ベンダー>:<アーキテクチャ>:<基板>`）
pub fn board_of(fqbn: &str) -> Option<&str> {
    let [v, a, b] = segments(fqbn)?;
    Some(&fqbn[..v.len() + a.len() + b.len() + 2])
}

/// FQBN の形として受けるか。段は英数字と `_` `.` `-`（arduino-cli の `ParseFQBN` と同じ集合）、
/// 4 段目（オプション）は `key=value` の `,` 区切り
pub fn valid_fqbn(fqbn: &str) -> bool {
    let word = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    };
    let mut parts = fqbn.splitn(4, ':');
    let head: Vec<&str> = parts.by_ref().take(3).collect();
    if head.len() != 3 || !head.iter().all(|s| word(s)) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(opts) => opts.split(',').all(|kv| {
            kv.split_once('=')
                .is_some_and(|(k, v)| word(k) && !v.is_empty() && !v.contains(char::is_control))
        }),
    }
}

/// ポートとして受けるか（空・前後の空白だけ・改行や制御文字を含むものは受けない。
/// 実在は問わない = 基板を繋いでいないときにも選べる。`COM3` / `/dev/cu.usbmodem1101` / IP）
pub fn valid_port(port: &str) -> bool {
    !port.trim().is_empty() && !port.contains(char::is_control)
}

// ─── 候補 ──────────────────────────────────────────────────────────────

/// 選べる基板 1 つ
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub fqbn: String,
    pub name: String,
    /// 繋がっているポート（繋がっていなければ `None`）
    pub port: Option<String>,
    pub connected: bool,
    /// コアが入っているか（`board listall` を聞けなかったら `None` = 分からない）
    pub installed: Option<bool>,
}

/// 候補 = **繋がっている基板**（`board list` の FQBN 付きのもの。ポート順）→ **今の基板**
/// （まだ並んでいなければ）。入っているコアの基板の全部は [`matches`] で引く（数百ある）。
/// `installed` は `board listall` の答えのコアで決める（答えが無ければ `None`）
pub fn candidates(
    ports: Option<&[DetectedPort]>,
    installed: Option<&[InstalledBoard]>,
    current: Option<&str>,
) -> Vec<Candidate> {
    let installed_core = |fqbn: &str| -> Option<bool> {
        let list = installed?;
        let core = core_of(fqbn)?;
        Some(list.iter().any(|b| b.core == core))
    };
    let mut out: Vec<Candidate> = Vec::new();
    for p in ports.into_iter().flatten() {
        for b in &p.boards {
            if out
                .iter()
                .any(|c| c.fqbn == b.fqbn && c.port.as_deref().unwrap_or_default() == p.address)
            {
                continue;
            }
            out.push(Candidate {
                fqbn: b.fqbn.clone(),
                name: b.name.clone(),
                port: Some(p.address.clone()).filter(|a| !a.is_empty()),
                connected: true,
                installed: installed_core(&b.fqbn),
            });
        }
    }
    if let Some(cur) = current.filter(|c| !c.is_empty()) {
        if !out.iter().any(|c| c.fqbn == cur) {
            let name = installed
                .and_then(|l| l.iter().find(|b| Some(b.fqbn.as_str()) == board_of(cur)))
                .map(|b| b.name.clone())
                .unwrap_or_else(|| cur.to_string());
            out.push(Candidate {
                fqbn: cur.to_string(),
                name,
                port: None,
                connected: false,
                installed: installed_core(cur),
            });
        }
    }
    out
}

/// 入っているコアの基板を名前か FQBN で探す（大文字小文字を区別しない部分一致。空の語 = 全部。
/// `listall` の順 = arduino-cli の並びのまま）
pub fn matches<'a>(installed: &'a [InstalledBoard], query: &str) -> Vec<&'a InstalledBoard> {
    let q = query.trim().to_lowercase();
    installed
        .iter()
        .filter(|b| {
            q.is_empty() || b.name.to_lowercase().contains(&q) || b.fqbn.to_lowercase().contains(&q)
        })
        .collect()
}

/// 名前（FQBN のオプションは見ない）。一覧に無ければ `None`
pub fn name_of(installed: &[InstalledBoard], fqbn: &str) -> Option<String> {
    let base = board_of(fqbn)?;
    installed
        .iter()
        .find(|b| b.fqbn == base)
        .map(|b| b.name.clone())
}

// ─── 書く前の検め ──────────────────────────────────────────────────────

/// 選んだ基板・ポートを書かない理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// FQBN の形でない
    BadFqbn(String),
    /// ポートが空・制御文字を含む
    BadPort(String),
    /// コアが入っていない（`core install` を示す）
    CoreMissing { fqbn: String, core: String },
    /// コアは入っているが、その基板が無い（近い候補を並べる）
    UnknownBoard {
        fqbn: String,
        core: String,
        near: Vec<String>,
    },
}

/// 書いてよいか（`installed` = `board listall` の答え）。ポートは形だけを見る
pub fn check(
    board: Option<&str>,
    port: Option<&str>,
    installed: &[InstalledBoard],
) -> Result<(), Rejection> {
    if let Some(p) = port {
        if !valid_port(p) {
            return Err(Rejection::BadPort(p.to_string()));
        }
    }
    let Some(fqbn) = board else {
        return Ok(());
    };
    if !valid_fqbn(fqbn) {
        return Err(Rejection::BadFqbn(fqbn.to_string()));
    }
    let (Some(core), Some(base)) = (core_of(fqbn), board_of(fqbn)) else {
        return Err(Rejection::BadFqbn(fqbn.to_string()));
    };
    if !installed.iter().any(|b| b.core == core) {
        return Err(Rejection::CoreMissing {
            fqbn: fqbn.to_string(),
            core: core.to_string(),
        });
    }
    if !installed.iter().any(|b| b.fqbn == base) {
        // 近い候補: 同じコアで、基板の段か名前が打った基板の段を含む / 含まれる
        let id = base.rsplit(':').next().unwrap_or_default().to_lowercase();
        let near = installed
            .iter()
            .filter(|b| b.core == core)
            .filter(|b| {
                let bid = b.fqbn.rsplit(':').next().unwrap_or_default().to_lowercase();
                !id.is_empty()
                    && (bid.contains(&id)
                        || id.contains(&bid)
                        || b.name.to_lowercase().contains(&id))
            })
            .take(5)
            .map(|b| b.fqbn.clone())
            .collect();
        return Err(Rejection::UnknownBoard {
            fqbn: fqbn.to_string(),
            core: core.to_string(),
            near,
        });
    }
    Ok(())
}

/// `arduino-cli` へ渡す `board attach` の引数（**シェルを通さない** argv。値は `--fqbn=` / `--port=` の
/// 1 語にする = `-` で始まるポートを別のフラグと読ませない。実測 1.2.0 で `--port=-weird` がそのまま
/// `default_port` に入る）。どちらも無ければ空
pub fn attach_args(board: Option<&str>, port: Option<&str>, sketch: &Path) -> Vec<String> {
    if board.is_none() && port.is_none() {
        return Vec::new();
    }
    let mut args = vec!["board".to_string(), "attach".to_string()];
    if let Some(b) = board {
        args.push(format!("--fqbn={b}"));
    }
    if let Some(p) = port {
        args.push(format!("--port={p}"));
    }
    args.push(sketch.to_string_lossy().into_owned());
    args
}

/// コアを入れるコマンド（案内だけ。**tako は打たない**）。`cli` は見つけた arduino-cli（フルパスか名前）。
/// 引用と PowerShell の呼び出し演算子は実行ペインの方言（Code Runner の `${arduino_cli}` と同じ 1 実装）
pub fn core_install_command(cli: &str, core: &str, platform: Platform) -> String {
    let dialect = crate::platform::shell::run_pane_dialect_for(platform);
    format!(
        "{} core install {core}",
        dialect.command_words(&[cli.to_string()])
    )
}

/// 書かない理由の文（日英）。`cli` / `platform` はコアの導入コマンドに使う
pub fn rejection_message(r: &Rejection, cli: &str, platform: Platform) -> String {
    let lang = crate::i18n::lang();
    match r {
        Rejection::BadFqbn(s) => match lang {
            Lang::Ja => format!(
                "基板の名前（FQBN）は <ベンダー>:<アーキテクチャ>:<基板> の形で指定する（例: arduino:avr:uno。指定: {s:?}）"
            ),
            Lang::En => format!(
                "A board (FQBN) is <vendor>:<architecture>:<board> (e.g. arduino:avr:uno; got {s:?})"
            ),
        },
        Rejection::BadPort(p) => match lang {
            Lang::Ja => format!("ポートが空か、改行・制御文字を含む（指定: {p:?}）"),
            Lang::En => format!("The port is empty or contains a newline / control character (got {p:?})"),
        },
        Rejection::CoreMissing { fqbn, core } => {
            let cmd = core_install_command(cli, core, platform);
            match lang {
                Lang::Ja => format!(
                    "基板 {fqbn} のコア {core} が入っていないので選べません（sketch.yaml は変えていません）。\
                     tako はコアを代わりに入れません（数百 MB のツールチェーンを落とすため）。\
                     次のコマンドで入れてから選び直してください:\n  {cmd}"
                ),
                Lang::En => format!(
                    "The core {core} for board {fqbn} is not installed, so it was not selected \
                     (sketch.yaml is unchanged). tako does not install cores for you (they download \
                     hundreds of MB of toolchain). Install it with the command below, then select again:\n  {cmd}"
                ),
            }
        }
        Rejection::UnknownBoard { fqbn, core, near } => {
            let near = if near.is_empty() {
                String::new()
            } else {
                near.join(" / ")
            };
            match (lang, near.is_empty()) {
                (Lang::Ja, true) => format!(
                    "コア {core} に基板 {fqbn} がありません（sketch.yaml は変えていません）。\
                     入っている基板は arduino-cli board listall で見られます"
                ),
                (Lang::Ja, false) => format!(
                    "コア {core} に基板 {fqbn} がありません（sketch.yaml は変えていません）。近い候補: {near}"
                ),
                (Lang::En, true) => format!(
                    "The core {core} has no board {fqbn} (sketch.yaml is unchanged). \
                     List the installed boards with arduino-cli board listall"
                ),
                (Lang::En, false) => format!(
                    "The core {core} has no board {fqbn} (sketch.yaml is unchanged). Close matches: {near}"
                ),
            }
        }
    }
}

/// `default_profile` があると基板の選択が compile に効かない（実測 1.2.0: `default_profile: uno` +
/// `default_fqbn: arduino:avr:mega` の compile は uno の容量 32256 バイトで組む）。そのときの注意
pub fn profile_overrides_note(profile: &str) -> String {
    match crate::i18n::lang() {
        Lang::Ja => format!(
            "sketch.yaml の default_profile（{profile}）が default_fqbn より優先されます。\
             compile と書き込みはプロファイルの基板を使います"
        ),
        Lang::En => format!(
            "default_profile ({profile}) in sketch.yaml takes precedence over default_fqbn; \
             compile and upload use the profile's board"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 実測 1.2.0（基板を繋いでいない機）の `board list --format json` の形
    const LIST_NONE: &str = r#"{"detected_ports":[
        {"port":{"address":"/dev/cu.debug-console","label":"/dev/cu.debug-console","protocol":"serial","protocol_label":"Serial Port","properties":{}}},
        {"port":{"address":"/dev/cu.Bluetooth-Incoming-Port","label":"/dev/cu.Bluetooth-Incoming-Port","protocol":"serial","protocol_label":"Serial Port","properties":{}}}]}"#;

    /// Windows の機の形（公式の JSON 出力の形。**実機は未確認** = macOS の単体で固定する）
    const LIST_WINDOWS: &str = r#"{"detected_ports":[
        {"matching_boards":[{"name":"Arduino Uno","fqbn":"arduino:avr:uno"}],
         "port":{"address":"COM3","label":"COM3","protocol":"serial","protocol_label":"Serial Port (USB)",
                 "properties":{"pid":"0x0043","vid":"0x2341"}}},
        {"matching_boards":[{"name":"Arduino UNO R4 WiFi","fqbn":"arduino:renesas_uno:unor4wifi"}],
         "port":{"address":"COM7","label":"COM7","protocol":"serial","protocol_label":"Serial Port (USB)"}},
        {"port":{"address":"COM1","label":"COM1","protocol":"serial","protocol_label":"Serial Port"}}]}"#;

    /// 実測 1.2.0 の `board listall --format jsonmini` を 1 コア 3 基板へ縮めたもの（入れ子の列も残す）
    const LISTALL: &str = r#"{"boards":[
        {"name":"Arduino Yún","fqbn":"arduino:avr:yun","platform":{"metadata":{"id":"arduino:avr","maintainer":"Arduino"},
         "release":{"name":"Arduino AVR Boards","version":"1.8.8","installed":true,"boards":[{"name":"Arduino UNO","fqbn":"arduino:avr:uno"}]}}},
        {"name":"Arduino UNO","fqbn":"arduino:avr:uno","platform":{"metadata":{"id":"arduino:avr"}}},
        {"name":"Arduino Nano","fqbn":"arduino:avr:nano","platform":{"metadata":{"id":"arduino:avr"}}}]}"#;

    fn installed() -> Vec<InstalledBoard> {
        parse_listall(LISTALL).unwrap()
    }

    #[test]
    fn board_listの答えを読む() {
        let none = parse_board_list(LIST_NONE).unwrap();
        assert_eq!(none.len(), 2);
        assert!(none.iter().all(|p| p.boards.is_empty()));
        assert_eq!(none[0].protocol_label, "Serial Port");
        // 何も無い機・読めない出力
        assert_eq!(parse_board_list("{}"), Some(vec![]));
        assert_eq!(parse_board_list("not json"), None);
    }

    #[test]
    fn windowsのポートを読んで候補にする() {
        let ports = parse_board_list(LIST_WINDOWS).unwrap();
        assert_eq!(
            ports.iter().map(|p| p.address.as_str()).collect::<Vec<_>>(),
            vec!["COM3", "COM7", "COM1"]
        );
        let list = installed();
        let c = candidates(Some(&ports), Some(&list), None);
        assert_eq!(c.len(), 2, "FQBN の付いたポートだけ: {c:?}");
        assert_eq!(
            (c[0].fqbn.as_str(), c[0].port.as_deref(), c[0].installed),
            ("arduino:avr:uno", Some("COM3"), Some(true))
        );
        // 繋がっているがコアが入っていない基板（board list は既知の VID/PID で名前を返す）
        assert_eq!(
            (c[1].fqbn.as_str(), c[1].port.as_deref(), c[1].installed),
            ("arduino:renesas_uno:unor4wifi", Some("COM7"), Some(false))
        );
        // 書く引数: COM3 は 1 語のまま（シェルを通さない）
        assert_eq!(
            attach_args(
                Some("arduino:avr:uno"),
                Some("COM3"),
                Path::new(r"C:\s\Blink")
            ),
            vec![
                "board",
                "attach",
                "--fqbn=arduino:avr:uno",
                "--port=COM3",
                r"C:\s\Blink"
            ]
        );
        assert!(check(Some("arduino:avr:uno"), Some("COM3"), &list).is_ok());
    }

    #[test]
    fn listallは入れ子の列を持たずに読む() {
        let list = installed();
        assert_eq!(list.len(), 3);
        assert_eq!(
            (
                list[1].name.as_str(),
                list[1].fqbn.as_str(),
                list[1].core.as_str()
            ),
            ("Arduino UNO", "arduino:avr:uno", "arduino:avr")
        );
        // コアの ID が無い行は FQBN の先頭 2 段で補う
        let bare = parse_listall(r#"{"boards":[{"name":"X","fqbn":"v:a:x"}]}"#).unwrap();
        assert_eq!(bare[0].core, "v:a");
        // 何も入っていない機（boards が無い）・読めない出力
        assert_eq!(parse_listall("{}"), Some(vec![]));
        assert_eq!(parse_listall("Board Name  FQBN"), None);
    }

    #[test]
    fn fqbnの形() {
        for ok in [
            "arduino:avr:uno",
            "arduino:avr:nano:cpu=atmega328old",
            "esp32:esp32:esp32s3:CDCOnBoot=cdc,PSRAM=opi",
            "rp2040:rp2040:0xcb_helios",
        ] {
            assert!(valid_fqbn(ok), "{ok}");
        }
        for bad in [
            "",
            "foo",
            "arduino:avr",
            "arduino::uno",
            "arduino:avr:uno:",
            "arduino:avr:uno:cpu",
            "arduino:avr:u no",
            "arduino:avr:uno\n",
        ] {
            assert!(!valid_fqbn(bad), "{bad:?}");
        }
        assert_eq!(core_of("arduino:avr:nano:cpu=x"), Some("arduino:avr"));
        assert_eq!(board_of("arduino:avr:nano:cpu=x"), Some("arduino:avr:nano"));
        assert_eq!(core_of("arduino:avr"), None);
    }

    #[test]
    fn 入っていないコアの基板は書かずにcore_installを示す() {
        let list = installed();
        let r = check(Some("arduino:renesas_uno:unor4wifi"), None, &list).unwrap_err();
        assert_eq!(
            r,
            Rejection::CoreMissing {
                fqbn: "arduino:renesas_uno:unor4wifi".into(),
                core: "arduino:renesas_uno".into()
            }
        );
        let ide = "/Applications/Arduino IDE.app/Contents/Resources/app/lib/backend/resources/arduino-cli";
        let msg = rejection_message(&r, ide, Platform::MacOs);
        assert!(
            msg.contains(&format!("'{ide}' core install arduino:renesas_uno")),
            "{msg}"
        );
        // Windows の PowerShell は呼び出し演算子つき
        let win =
            r"C:\Program Files\Arduino IDE\resources\app\lib\backend\resources\arduino-cli.exe";
        assert_eq!(
            core_install_command(win, "arduino:avr", Platform::Windows),
            format!("& '{win}' core install arduino:avr")
        );
        // 何も入っていない機でも同じ（listall が空）
        assert!(matches!(
            check(Some("arduino:avr:uno"), None, &[]),
            Err(Rejection::CoreMissing { .. })
        ));
    }

    #[test]
    fn 入っているコアに無い基板は近い候補を出す() {
        let list = installed();
        let Err(Rejection::UnknownBoard { near, .. }) = check(Some("arduino:avr:un"), None, &list)
        else {
            panic!("UnknownBoard にならない");
        };
        // 基板の段の部分一致（`un` は yun と uno の両方。並びは listall の順）
        assert_eq!(
            near,
            vec!["arduino:avr:yun".to_string(), "arduino:avr:uno".to_string()]
        );
        let Err(Rejection::UnknownBoard { near, .. }) = check(Some("arduino:avr:nan"), None, &list)
        else {
            panic!("UnknownBoard にならない");
        };
        assert_eq!(near, vec!["arduino:avr:nano".to_string()]);
        // 似たものが無ければ空（文は listall を案内する）
        let Err(r @ Rejection::UnknownBoard { .. }) = check(Some("arduino:avr:zzz"), None, &list)
        else {
            panic!("UnknownBoard にならない");
        };
        assert!(rejection_message(&r, "arduino-cli", Platform::MacOs).contains("board listall"));
        // オプション付きでも基板の段で照らす
        assert!(check(Some("arduino:avr:nano:cpu=atmega328old"), None, &list).is_ok());
        assert!(matches!(
            check(Some("foo"), None, &list),
            Err(Rejection::BadFqbn(_))
        ));
    }

    #[test]
    fn ポートの形() {
        for ok in [
            "COM3",
            "/dev/cu.usbmodem1101",
            "192.168.1.5",
            "-weird",
            "/dev/cu.with space",
        ] {
            assert!(valid_port(ok), "{ok}");
        }
        for bad in ["", "   ", "COM3\n", "a\tb"] {
            assert!(!valid_port(bad), "{bad:?}");
            assert!(matches!(
                check(None, Some(bad), &[]),
                Err(Rejection::BadPort(_))
            ));
        }
        // ポートだけなら基板の一覧は要らない（繋いでいない基板にも選べる）
        assert!(check(None, Some("/dev/cu.x"), &[]).is_ok());
    }

    #[test]
    fn 書く引数は値を1語にする() {
        let s = Path::new("/s/Blink");
        assert_eq!(
            attach_args(None, Some("-weird"), s),
            vec!["board", "attach", "--port=-weird", "/s/Blink"]
        );
        assert_eq!(
            attach_args(Some("arduino:avr:uno"), None, s),
            vec!["board", "attach", "--fqbn=arduino:avr:uno", "/s/Blink"]
        );
        assert!(attach_args(None, None, s).is_empty());
    }

    #[test]
    fn 候補は繋がっている基板と今の基板() {
        let list = installed();
        let ports = parse_board_list(LIST_NONE).unwrap();
        // 繋いでいない + 未選択 = 候補 0（入っている基板は matches で引く）
        assert!(candidates(Some(&ports), Some(&list), None).is_empty());
        // 今の基板は名前を一覧から引く（オプション付きでも）
        let c = candidates(Some(&ports), Some(&list), Some("arduino:avr:nano:cpu=x"));
        assert_eq!(
            (c[0].name.as_str(), c[0].connected, c[0].installed),
            ("Arduino Nano", false, Some(true))
        );
        // 一覧を聞けなかった = 入っているかは分からない
        let c = candidates(None, None, Some("arduino:avr:uno"));
        assert_eq!(
            (c[0].name.as_str(), c[0].installed),
            ("arduino:avr:uno", None)
        );
        // 繋がっている基板が今の基板なら重ねない
        let win = parse_board_list(LIST_WINDOWS).unwrap();
        assert_eq!(
            candidates(Some(&win), Some(&list), Some("arduino:avr:uno")).len(),
            2
        );
    }

    #[test]
    fn 探す語で絞る() {
        let list = installed();
        assert_eq!(matches(&list, "").len(), 3);
        assert_eq!(
            matches(&list, "NANO")
                .iter()
                .map(|b| b.fqbn.as_str())
                .collect::<Vec<_>>(),
            vec!["arduino:avr:nano"]
        );
        assert_eq!(matches(&list, "avr:uno").len(), 1);
        assert!(matches(&list, "esp32").is_empty());
        assert_eq!(
            name_of(&list, "arduino:avr:uno").as_deref(),
            Some("Arduino UNO")
        );
    }

    #[test]
    fn inoでなければスケッチではない() {
        assert_eq!(
            sketch_dir_of(Path::new("/s/Blink/Blink.ino")),
            Some(PathBuf::from("/s/Blink"))
        );
        assert_eq!(
            sketch_dir_of(Path::new("/s/Blink/Blink.INO")),
            Some(PathBuf::from("/s/Blink"))
        );
        assert_eq!(sketch_dir_of(Path::new("/s/Blink/helper.h")), None);
        assert_eq!(sketch_dir_of(Path::new("/s/a.py")), None);
    }
}
