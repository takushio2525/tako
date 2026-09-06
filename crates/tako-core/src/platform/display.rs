//! 窓を置くディスプレイの選択（抽象境界 B26。#1141）
//!
//! ## なぜ要るか
//!
//! 隔離 GUI（`TAKO_ISOLATED=1` の tako-app・セルフテスト・visual-test）は検証のたびに
//! 何度も立つ。既定のままだと**ユーザーのメイン画面の前面に窓が出て作業を妨げる**ので、
//! 常設の仮想ディスプレイ（既定名 [`DEFAULT_VIRTUAL_DISPLAY_NAME`]）へ逃がす。
//!
//! **別 Space へ逃がすのでは代わりにならない**: GPUI は窓が完全に隠れる（他窓に覆われる /
//! 表示中でない Space にある）と描画を止める（#470 の実測。同じ絵が撮れ続け、描画依存の
//! セルフテスト項目が進まない）。退避先は **OS から実ディスプレイとして見える面**が要る。
//!
//! ## 何がプラットフォーム依存か
//!
//! ディスプレイの**列挙**は GPUI（`cx.displays()`）が両 OS ぶん面倒を見るので、ここには無い。
//! 呼び出し側が [`DisplayCandidate`] を組んで [`select`] へ渡す。
//!
//! 依存するのは **名前の引き方**だけ（[`display_names`]）。macOS は `system_profiler` の
//! `SPDisplaysDataType` から `_name` と `_spdisplays_displayID` を読む
//! （後者は**16 進文字列**で、値は `CGDirectDisplayID` = GPUI の `DisplayId` そのもの。
//! #1141 で実測: 仮想スクリーン `tako-vd` は `"d"` = 13）。Windows は手段を配線していない
//! （仮想ディスプレイの作り方自体が別 = #1141 の追跡対象）。
//!
//! ## 判定は純粋関数
//!
//! [`select`] は候補の一覧を受け取るだけなので **macOS 上から Windows 側の挙動も検証できる**
//! （`support` / `window_lifecycle` / `dpi` と同じ作法）。
//!
//! ## 見つからないときは止めない
//!
//! 指定されたディスプレイが無いのは**検証の都合**であって、tako が起動できない理由ではない。
//! [`Selection::NotFound`] を返して呼び出し側は既定動作（メイン画面）へ落ち、
//! 理由を persist.log へ 1 行だけ残す。

use std::sync::{Mutex, OnceLock};

/// 常設の仮想ディスプレイの既定名。`TAKO_DISPLAY` 未指定の隔離起動はこれを探す。
///
/// 作るのは `scripts/lib/virtual-display.sh ensure`（tako 本体は作らない = 他人の
/// ディスプレイ構成を勝手に変えない）。
pub const DEFAULT_VIRTUAL_DISPLAY_NAME: &str = "tako-vd";

/// 窓を置くディスプレイを指定する環境変数。値は「名前 | UUID | index」
pub const ENV_DISPLAY: &str = "TAKO_DISPLAY";

/// 名前引きにかける時間の上限。超えたら名前無しで進む（起動を待たせない）
const NAME_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// 選択の候補 1 枚。呼び出し側（tako-app）が GPUI の `cx.displays()` から組む
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayCandidate {
    /// `cx.displays()` の並び順（0 始まり）。`TAKO_DISPLAY=1` の 1 はこれ
    pub index: usize,
    /// プラットフォームのディスプレイ ID（macOS は `CGDirectDisplayID`）
    pub id: u64,
    /// 再起動をまたいで安定する識別子（GPUI の `PlatformDisplay::uuid`）
    pub uuid: Option<String>,
    /// OS 上の表示名（[`display_names`] 由来）
    pub name: Option<String>,
    /// 主ディスプレイ（メニューバーのある面）か
    pub primary: bool,
}

impl DisplayCandidate {
    /// 一覧・診断に出す 1 行表現（`TAKO_DISPLAY` へそのまま渡せる形を含む）
    pub fn label(&self) -> String {
        format!(
            "[{}] id={} name={} uuid={}{}",
            self.index,
            self.id,
            self.name.as_deref().unwrap_or("?"),
            self.uuid.as_deref().unwrap_or("?"),
            if self.primary { " (primary)" } else { "" },
        )
    }
}

/// 指定をどう解釈したか。`Selected` 以外は呼び出し側が既定動作へ落ちる
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// 指定が無い（`TAKO_DISPLAY` 未設定 かつ 隔離既定にも当たらない）
    NotRequested,
    /// 解決できた
    Selected {
        /// 当たった候補
        display: DisplayCandidate,
        /// 何で当たったか（uuid / name / index）
        matched_by: MatchKind,
        /// 指定文字列
        spec: String,
    },
    /// 指定はあったが当たらなかった。`available` は候補の一覧（診断に出す）
    NotFound {
        /// 指定文字列
        spec: String,
        /// 実在した候補の一覧表現
        available: Vec<String>,
    },
}

/// 何を根拠に当てたか
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// UUID の完全一致（大文字小文字は無視）
    Uuid,
    /// 表示名の完全一致（大文字小文字は無視）
    Name,
    /// `cx.displays()` の並び順（0 始まり）
    Index,
}

impl MatchKind {
    /// 診断・JSON に出す語
    pub fn as_str(self) -> &'static str {
        match self {
            MatchKind::Uuid => "uuid",
            MatchKind::Name => "name",
            MatchKind::Index => "index",
        }
    }
}

/// どのディスプレイを狙うかを決める。
///
/// - `TAKO_DISPLAY` が空でなければそれ
/// - **検証用の起動**なら [`DEFAULT_VIRTUAL_DISPLAY_NAME`]
/// - どちらでもなければ `None`（= 既定動作。**通常起動の挙動は 1 ビットも変えない**）
pub fn requested_spec(env_display: Option<&str>, verification_gui: bool) -> Option<String> {
    match env_display.map(str::trim) {
        Some(s) if !s.is_empty() => Some(s.to_string()),
        _ if verification_gui => Some(DEFAULT_VIRTUAL_DISPLAY_NAME.to_string()),
        _ => None,
    }
}

/// 「ユーザーに見せる窓ではない = 検証のために立てた窓」か。
///
/// 隔離起動（`TAKO_ISOLATED`）だけでなく**セルフテストと visual-test も含む**のが要点。
/// セルフテストは `TAKO_ISOLATED` を立てずに走ることがあり、そこを外すと
/// 「隔離だけ仮想ディスプレイ・セルフテストはユーザーの画面」という半端な状態になる
/// （#1141 の実測で 1 度踏んだ）。**窓を出す検証はどれもユーザーの画面に出さない**。
pub fn is_verification_gui(isolated: Option<&str>, self_test: bool, visual_test: bool) -> bool {
    matches!(isolated, Some("1" | "true" | "on")) || self_test || visual_test
}

/// 指定文字列を候補へ当てる（純粋関数）。
///
/// 当てる順は **UUID → 名前 → index**。UUID は 36 文字ハイフン区切りで構造的に紛れないので
/// 先に見る。名前を index より先に見るのは、「2」という名前のディスプレイがあったときに
/// 名前を優先したいから（index はいつでも UUID / 名前で言い換えられる）。
pub fn select(spec: &str, candidates: &[DisplayCandidate]) -> Selection {
    let spec = spec.trim();
    if spec.is_empty() {
        return Selection::NotRequested;
    }
    let found = candidates
        .iter()
        .find(|c| {
            c.uuid
                .as_deref()
                .is_some_and(|u| u.eq_ignore_ascii_case(spec))
        })
        .map(|c| (c, MatchKind::Uuid))
        .or_else(|| {
            candidates
                .iter()
                .find(|c| {
                    c.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(spec))
                })
                .map(|c| (c, MatchKind::Name))
        })
        .or_else(|| {
            let raw = spec.strip_prefix("index:").unwrap_or(spec);
            raw.parse::<usize>()
                .ok()
                .and_then(|i| candidates.iter().find(|c| c.index == i))
                .map(|c| (c, MatchKind::Index))
        });
    match found {
        Some((c, matched_by)) => Selection::Selected {
            display: c.clone(),
            matched_by,
            spec: spec.to_string(),
        },
        None => Selection::NotFound {
            spec: spec.to_string(),
            available: candidates.iter().map(DisplayCandidate::label).collect(),
        },
    }
}

/// 起動時に決めた配置。`tako_check_health` が読む（設計原則 5: AI から状態が読める）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// 指定文字列（無指定なら `None`）
    pub requested: Option<String>,
    /// 当たったディスプレイ
    pub resolved: Option<DisplayCandidate>,
    /// 何で当たったか
    pub matched_by: Option<MatchKind>,
    /// 当たらなかった / 名前を引けなかった理由（診断用の 1 行）
    pub reason: Option<String>,
    /// 起動時に見えていた候補の一覧表現
    pub available: Vec<String>,
}

impl Placement {
    /// 指定が無かったときの記録
    pub fn not_requested() -> Self {
        Self {
            requested: None,
            resolved: None,
            matched_by: None,
            reason: None,
            available: Vec::new(),
        }
    }

    /// persist.log へ残す 1 行。**当たっても外れても必ず出す**（次に同じことを
    /// 調べる人が「そもそも狙ったのか」から確かめられるように）
    pub fn log_line(&self) -> String {
        match (&self.requested, &self.resolved) {
            (None, _) => "ディスプレイ指定なし: 既定の面へ開く".to_string(),
            (Some(spec), Some(d)) => format!(
                "ディスプレイ指定 {spec}: {} で解決 → {}",
                self.matched_by.map(MatchKind::as_str).unwrap_or("?"),
                d.label(),
            ),
            (Some(spec), None) => format!(
                "ディスプレイ指定 {spec}: 見つからないので既定の面へ開く（理由={} / 候補=[{}]）",
                self.reason.as_deref().unwrap_or("不明"),
                self.available.join(", "),
            ),
        }
    }
}

static PLACEMENT: OnceLock<Mutex<Option<Placement>>> = OnceLock::new();

fn placement_cell() -> &'static Mutex<Option<Placement>> {
    PLACEMENT.get_or_init(|| Mutex::new(None))
}

/// 起動時に決めた配置を記録する（tako-app が窓を開く直前に 1 回）
pub fn record_placement(p: Placement) {
    if let Ok(mut slot) = placement_cell().lock() {
        *slot = Some(p);
    }
}

/// 記録された配置（未記録なら `None`）。`check_health` が読む
pub fn placement() -> Option<Placement> {
    placement_cell().lock().ok().and_then(|s| s.clone())
}

/// このプラットフォームでディスプレイの名前を引けるか。
/// 引けない環境では `TAKO_DISPLAY` に UUID か index を渡す
pub const fn name_lookup_supported() -> bool {
    cfg!(target_os = "macos")
}

/// ディスプレイ ID → 表示名。引けないプラットフォームでは空。
///
/// macOS は `system_profiler SPDisplaysDataType -json` を読む（#1141 で実測 0.17 秒）。
/// **名前の指定が要るときにしか呼ばない**ので、通常起動はこの費用を払わない。
/// 応答が遅い機で起動を待たせないよう [`NAME_LOOKUP_TIMEOUT`] で見切る。
pub fn display_names() -> Vec<(u64, String)> {
    #[cfg(target_os = "macos")]
    {
        imp::display_names_with_timeout(NAME_LOOKUP_TIMEOUT)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::sync::mpsc;
    use std::time::Duration;

    /// `system_profiler` を別スレッドで走らせ、`timeout` まで待つ。
    ///
    /// 時間切れでも**子を殺さない**（`system_profiler` は放っておいても自分で終わる。
    /// 殺すには `Child` を共有する必要があり、その unsafe を持ち込む価値が無い）。
    /// 待つのをやめるだけなので、起動が遅れるのは最大 `timeout`。
    pub(super) fn display_names_with_timeout(timeout: Duration) -> Vec<(u64, String)> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let out = std::process::Command::new("/usr/sbin/system_profiler")
                .args(["SPDisplaysDataType", "-json"])
                .output();
            let json = out
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .unwrap_or_default();
            let _ = tx.send(json);
        });
        match rx.recv_timeout(timeout) {
            Ok(json) => parse_names(&json),
            Err(_) => Vec::new(),
        }
    }

    /// `system_profiler SPDisplaysDataType -json` から (displayID, 名前) を拾う。
    ///
    /// `_spdisplays_displayID` は **16 進文字列**（`"d"` = 13。#1141 で実測）で、
    /// 値は `CGDirectDisplayID` = GPUI の `DisplayId`。
    pub(super) fn parse_names(json: &str) -> Vec<(u64, String)> {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for group in v["SPDisplaysDataType"].as_array().into_iter().flatten() {
            for screen in group["spdisplays_ndrvs"].as_array().into_iter().flatten() {
                let (Some(name), Some(id)) = (
                    screen["_name"].as_str(),
                    screen["_spdisplays_displayID"].as_str(),
                ) else {
                    continue;
                };
                let hex = id.trim().trim_start_matches("0x");
                if let Ok(id) = u64::from_str_radix(hex, 16) {
                    out.push((id, name.to_string()));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(index: usize, id: u64, name: Option<&str>, uuid: Option<&str>) -> DisplayCandidate {
        DisplayCandidate {
            index,
            id,
            uuid: uuid.map(String::from),
            name: name.map(String::from),
            primary: index == 0,
        }
    }

    fn fixture() -> Vec<DisplayCandidate> {
        vec![
            cand(
                0,
                1,
                Some("Color LCD"),
                Some("37D8832A-2D66-02CA-B9F7-8F30A301B230"),
            ),
            cand(
                1,
                13,
                Some("tako-vd"),
                Some("E88E3509-9E5F-4CD8-B5E8-54D1E118394B"),
            ),
        ]
    }

    #[test]
    fn 名前で当たる() {
        let s = select("tako-vd", &fixture());
        match s {
            Selection::Selected {
                display,
                matched_by,
                ..
            } => {
                assert_eq!(display.id, 13);
                assert_eq!(matched_by, MatchKind::Name);
            }
            other => panic!("名前で当たらない: {other:?}"),
        }
    }

    #[test]
    fn 名前の大文字小文字は無視する() {
        assert!(matches!(
            select("TAKO-VD", &fixture()),
            Selection::Selected { .. }
        ));
    }

    #[test]
    fn uuidで当たる() {
        match select("e88e3509-9e5f-4cd8-b5e8-54d1e118394b", &fixture()) {
            Selection::Selected {
                display,
                matched_by,
                ..
            } => {
                assert_eq!(display.id, 13);
                assert_eq!(matched_by, MatchKind::Uuid);
            }
            other => panic!("UUID で当たらない: {other:?}"),
        }
    }

    #[test]
    fn indexで当たる() {
        for spec in ["1", "index:1"] {
            match select(spec, &fixture()) {
                Selection::Selected {
                    display,
                    matched_by,
                    ..
                } => {
                    assert_eq!(display.id, 13, "spec={spec}");
                    assert_eq!(matched_by, MatchKind::Index, "spec={spec}");
                }
                other => panic!("index で当たらない spec={spec}: {other:?}"),
            }
        }
    }

    /// index より名前を先に見る（数字の名前を付けられても言い換えが効くのは index の側）
    #[test]
    fn 数字の名前はindexより優先する() {
        let cands = vec![
            cand(0, 1, Some("Color LCD"), None),
            cand(1, 13, Some("0"), None),
        ];
        match select("0", &cands) {
            Selection::Selected {
                display,
                matched_by,
                ..
            } => {
                assert_eq!(display.id, 13);
                assert_eq!(matched_by, MatchKind::Name);
            }
            other => panic!("名前が優先されない: {other:?}"),
        }
    }

    #[test]
    fn 無い名前は候補つきで見つからないになる() {
        match select("no-such-display", &fixture()) {
            Selection::NotFound { spec, available } => {
                assert_eq!(spec, "no-such-display");
                assert_eq!(available.len(), 2);
                assert!(
                    available[1].contains("tako-vd"),
                    "候補に名前が出る: {available:?}"
                );
            }
            other => panic!("見つからない扱いにならない: {other:?}"),
        }
    }

    #[test]
    fn 名前が引けない環境ではuuidとindexだけで当たる() {
        let cands = vec![
            cand(0, 1, None, Some("37D8832A-2D66-02CA-B9F7-8F30A301B230")),
            cand(1, 13, None, Some("E88E3509-9E5F-4CD8-B5E8-54D1E118394B")),
        ];
        assert!(matches!(
            select("tako-vd", &cands),
            Selection::NotFound { .. }
        ));
        assert!(matches!(select("1", &cands), Selection::Selected { .. }));
    }

    #[test]
    fn 検証用の起動は指定が無くても仮想ディスプレイを狙う() {
        assert_eq!(
            requested_spec(None, true).as_deref(),
            Some(DEFAULT_VIRTUAL_DISPLAY_NAME)
        );
        // 通常起動は狙わない = 既定動作のまま
        assert_eq!(requested_spec(None, false), None);
        // 空文字は無指定と同じ
        assert_eq!(requested_spec(Some("  "), false), None);
        // 明示指定は検証用でも優先
        assert_eq!(
            requested_spec(Some("other"), true).as_deref(),
            Some("other")
        );
    }

    /// **セルフテスト / visual-test も検証用**（隔離だけにすると半端に漏れる）
    #[test]
    fn セルフテストとvisualtestも検証用の起動として扱う() {
        assert!(is_verification_gui(None, true, false), "セルフテスト");
        assert!(is_verification_gui(None, false, true), "visual-test");
        assert!(
            is_verification_gui(Some("0"), true, false),
            "隔離でなくても"
        );
        // 隔離の語彙は remote::is_isolated と同じ
        for v in ["1", "true", "on"] {
            assert!(is_verification_gui(Some(v), false, false), "v={v}");
        }
        for v in ["0", "false", "off", ""] {
            assert!(!is_verification_gui(Some(v), false, false), "v={v}");
        }
        // どれでもない = 通常起動
        assert!(!is_verification_gui(None, false, false));
    }

    /// 当たっても外れても 1 行残る（診断の入口）
    #[test]
    fn 記録は当たっても外れても1行残る() {
        let hit = Placement {
            requested: Some("tako-vd".into()),
            resolved: Some(fixture()[1].clone()),
            matched_by: Some(MatchKind::Name),
            reason: None,
            available: Vec::new(),
        };
        assert!(hit.log_line().contains("tako-vd"));
        assert!(hit.log_line().contains("name"));

        let miss = Placement {
            requested: Some("nope".into()),
            resolved: None,
            matched_by: None,
            reason: Some("該当なし".into()),
            available: vec!["[0] id=1 name=Color LCD uuid=? (primary)".into()],
        };
        let line = miss.log_line();
        assert!(line.contains("既定の面へ開く"), "{line}");
        assert!(line.contains("Color LCD"), "候補も残す: {line}");

        assert!(Placement::not_requested().log_line().contains("指定なし"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn system_profilerのdisplayidは16進として読む() {
        // #1141 の実採取（値だけ抜粋）。`"d"` は 13
        let json = r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[
            {"_name":"Color LCD","_spdisplays_displayID":"1"},
            {"_name":"tako-vd","_spdisplays_displayID":"d"},
            {"_name":"壊れた行"},
            {"_name":"16進でない","_spdisplays_displayID":"zz"}
        ]}]}"#;
        let names = super::imp::parse_names(json);
        assert_eq!(
            names,
            vec![(1, "Color LCD".to_string()), (13, "tako-vd".to_string())],
            "16 進で読めた行だけを返す"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn 壊れた出力でも落ちない() {
        assert!(super::imp::parse_names("").is_empty());
        assert!(super::imp::parse_names("not json").is_empty());
        assert!(super::imp::parse_names("{}").is_empty());
    }
}
