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
//! ## 見つからないときの構え（#1160 / #1697）
//!
//! [`Selection::NotFound`] のあとどうするかは [`miss_for`] が決める。分かれ目は
//! **面が 1 枚も見えていないか**（`enumeration_empty`）と **`TAKO_DISPLAY` を明示したか**
//! （`explicit`）で、`is_verification_gui` だけで決めてはいけない。
//!
//! - **面が 1 枚も見えない + 検証用 GUI**: **窓を開かずに終わる**。列挙が空なのは
//!   「置き先が無い」ではなく**まだ分からない**状態（下記）なので、ここで既定の面へ
//!   落とすと #1141 の目的が黙って破れる
//! - **面は見えているが当たらない + 暗黙の既定**: 既定の面へ落ちて起動は止めない。
//!   その機に置き先が無いということで（CI・他人の環境・仮想ディスプレイを配線していない
//!   Windows = FR-4.8.10）、**ここで開かないと検証そのものが回らなくなる**。代わりに
//!   [`Placement::fallback_notice`] で起動時に見える警告を出す
//! - **面は見えているが当たらない + `TAKO_DISPLAY` を明示 + 検証用 GUI**: 窓を開かずに
//!   終わる（#1697）。狙った面を見失っただけなので、既定の面へ落とす理由が無い
//! - **通常起動で `TAKO_DISPLAY` が外れた**: 常に既定の面へ落ちる。指定が外れるのは
//!   検証の都合であって、tako が起動できない理由ではない
//!
//! ## 列挙は空になりうる（#1160 の原因）
//!
//! macOS の `cx.displays()` は **`CGGetActiveDisplayList`**（gpui の `MacDisplay::all`）で、
//! gpui 自身が「眠っている機では active な一覧が返るとは限らない」と書いている。
//! 実測（2026-09-07）: ディスプレイスリープ中は `NSScreen` に 2 枚残ったままで
//! `CGDisplayIsActive` が両方 0 = **列挙が 0 件**になる。
//! `scripts/lib/virtual-display.sh status` には面が見えているのに `候補=[]` になるのはこれで、
//! 1 回引いただけで諦めると**起きかけの面を見落として既定の面へ落ちる**。
//!
//! なので [`retry_policy`] の予算のあいだ列挙し直す。**待つのは空のあいだだけ**で、
//! 面が見えているのに当たらないなら待っても答えは変わらない（読めている一覧に無い）。
//! 眠っている面を**起こす**のは tako ではなく `scripts/lib/virtual-display.sh ensure`
//! （面を用意する係）の担当で、ここは「現れるまで少し待つ」だけを行う。
//!
//! ## 名前が読めない瞬間がある（#1697）
//!
//! 名前は `system_profiler` から引く（[`display_names`]）ので、それが読めない起動では
//! 面は見えているのに**全部の面が `name=?`** になる（2026-09-24 の実測: 候補 2 枚とも
//! `name=?` で、うち 1 枚の uuid は `tako-vd` そのもの。直前の起動では同じ面が名前で
//! 解決できていた。応答待ちの打ち切りか出力の欠けかは未確定）。名前だけで探すと
//! ここで外れ、面が見えているので「落ちる」側へ倒れて**ユーザーのメイン画面へ窓が出た**。
//!
//! 塞ぎ方は 2 段で、どちらも `TAKO_1697_LEGACY=1` で #1697 前へ戻せる:
//!
//! - **照合を name → 記録済み uuid の 2 段にする**。`scripts/lib/virtual-display.sh ensure`
//!   が面を用意した直後にその面の uuid を [`record_dir`] へ残し、名前で当たらないときは
//!   **名前が読めない面に限って**その uuid で当てる（[`MatchKind::RecordedUuid`]）。
//!   名前が読めていて別の名前の面には当てない（記録より、いま読めた名前が正）
//! - **明示の指定を見失ったら検証用 GUI は開かない**（[`miss_for`] の `explicit`）。
//!   `TAKO_DISPLAY` を明示したのに当たらないのは「その機に置き先が無い」ではなく
//!   「狙った面を見失った」なので、既定の面へ落とすと #1141 の目的が黙って破れる。
//!   検証用 GUI が既定の面へ落ちるのは **`TAKO_DISPLAY` 未指定（暗黙の既定）**のときだけで、
//!   これは置き先を配線していない環境（CI・他人の機・Windows）のために残す

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// 常設の仮想ディスプレイの既定名。`TAKO_DISPLAY` 未指定の隔離起動はこれを探す。
///
/// 作るのは `scripts/lib/virtual-display.sh ensure`（tako 本体は作らない = 他人の
/// ディスプレイ構成を勝手に変えない）。
pub const DEFAULT_VIRTUAL_DISPLAY_NAME: &str = "tako-vd";

/// 窓を置くディスプレイを指定する環境変数。値は「名前 | UUID | index」
pub const ENV_DISPLAY: &str = "TAKO_DISPLAY";

/// #1160 の前（列挙を 1 回だけ引き、外れたら必ず既定の面へ落ちる）へ戻す env。
/// 検出力の A/B 用で、同一バイナリのまま旧挙動を再現できる
pub const ENV_LEGACY_1160: &str = "TAKO_1160_LEGACY";

/// 列挙を**空に見せる**回数を指定する診断 env（#1160）。
///
/// 本物のディスプレイを眠らせずに「起動の瞬間だけ列挙が空」を再現するために使う
/// （`TAKO_1160_INJECT_EMPTY=2` なら最初の 2 回だけ 0 件を返し、3 回目から実際の列挙）。
/// 大きな値を渡せば「やり直しても現れない」= 窓を開かずに終わる道も踏める。
/// **本番動作には影響しない**（明示されたときだけ効く）
pub const ENV_INJECT_EMPTY_1160: &str = "TAKO_1160_INJECT_EMPTY";

/// #1697 の前（名前だけで探し、`TAKO_DISPLAY` を明示していても面が見えていれば既定の面へ
/// 落ちる）へ戻す env。検出力の A/B 用で、同一バイナリのまま旧挙動を再現できる
pub const ENV_LEGACY_1697: &str = "TAKO_1697_LEGACY";

/// 面の名前が**読めない瞬間**を再現する診断 env（#1697）。
///
/// 立っていれば名前引き（[`display_names`]）を空として扱い、全部の面が `name=?` になる
/// （本物の `system_profiler` を止めずに #1697 の一覧を作る）。**本番動作には影響しない**
pub const ENV_INJECT_NO_NAMES_1697: &str = "TAKO_1697_INJECT_NO_NAMES";

/// 記録済み uuid の置き場を差し替える env（#1697）。
/// **シェル側（`scripts/lib/virtual-display.sh`）と同じ名前**を読む（番犬が突き合わせる）
pub const ENV_RECORD_DIR: &str = "TAKO_VD_RECORD_DIR";

/// 記録済み uuid の既定の置き場（ホームからの相対。macOS だけ）。
/// **シェル側と同じ綴り**（番犬が突き合わせる）
pub const RECORD_SUBDIR: &str = "Library/Caches/tako/virtual-display";

/// 列挙をやり直す間隔（#1160）
pub const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// 検証用 GUI がやり直す回数（#1160）。100ms × 20 = **最大 2 秒**。
///
/// ここで待つのは「起きかけ / 構成変更の途中」を跨ぐためで、**眠ったままの面が
/// 自分から起きてくるのを待つ器ではない**（それは `ensure` の担当）。
/// 待てば必ず現れるものではないので、上限は「起動が体感で止まらない」側に寄せてある
pub const VERIFICATION_RETRIES: u32 = 20;

/// 通常起動で `TAKO_DISPLAY` が外れたときやり直す回数（#1160）。100ms × 3 = 0.3 秒。
///
/// この道は既定の面へ落ちて**必ず起動する**ので、待つ意味は「起きかけを拾えたら拾う」まで。
/// ユーザーの起動を秒単位で待たせないため検証用より短くしてある
pub const FALLBACK_RETRIES: u32 = 3;

/// 検証用 GUI が置き先を用意できずに終わるときの終了コード（#1160）。
/// 他の起動失敗（`1`）と見分けられるように分けてある
pub const REFUSED_EXIT_CODE: i32 = 4;

/// 指定した面が列挙に無かったときの落とし所
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
    /// 既定の面（メイン画面）へ開く。通常起動で `TAKO_DISPLAY` が外れた道
    FallBack,
    /// 窓を開かずに終わる。検証用 GUI の道（ユーザーの画面に出すより開かない方がよい）
    Refuse,
}

impl Miss {
    /// 診断・JSON に出す語
    pub fn as_str(self) -> &'static str {
        match self {
            Miss::FallBack => "fall_back",
            Miss::Refuse => "refuse",
        }
    }
}

/// 列挙が空のあいだやり直す予算（#1160）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// 列挙をやり直す回数（`0` なら 1 回引いて終わり = #1160 前の挙動）
    pub retries: u32,
    /// やり直しの間隔
    pub interval: Duration,
}

impl RetryPolicy {
    /// やり直しに費やしうる最大の時間（診断に出す）
    pub fn budget(&self) -> Duration {
        self.interval * self.retries
    }
}

/// 列挙をやり直す予算を決める（#1160）。
///
/// `TAKO_1160_LEGACY=1` なら #1160 前（やり直さない）を返す。
/// 判定を 1 か所に閉じているので、tako-app・診断・テストが同じ予算を見る
pub fn retry_policy(verification_gui: bool) -> RetryPolicy {
    retry_policy_for(verification_gui, legacy_1160())
}

/// [`retry_policy`] の中身（env を引数へ出した純粋関数。両アームを 1 プロセスで検証できる）
fn retry_policy_for(verification_gui: bool, legacy: bool) -> RetryPolicy {
    RetryPolicy {
        retries: if legacy {
            0
        } else if verification_gui {
            VERIFICATION_RETRIES
        } else {
            FALLBACK_RETRIES
        },
        interval: RETRY_INTERVAL,
    }
}

/// 外したときの落とし所を決める（#1160 / #1697）。
///
/// 見るのは 3 つ: 検証用 GUI か / **面が 1 枚も見えていないか**（`enumeration_empty`）/
/// **`TAKO_DISPLAY` を明示したか**（`explicit`。[`explicit_request`]）。
///
/// - 通常起動 → 常に [`Miss::FallBack`]（起動を止める理由にならない。FR-4.8.4）
/// - 空 + 検証用 GUI → [`Miss::Refuse`]（窓を開かずに終わる）。列挙が空なのは
///   「置き先が無い」ではなく**まだ分からない**状態（ディスプレイスリープ）なので、
///   ここで既定の面 = ユーザーの画面へ落とすと #1141 の目的が黙って破れる（#1160）
/// - 面は見えているが当たらない + 明示 + 検証用 GUI → [`Miss::Refuse`]（#1697）。
///   狙った面を**見失った**のであって、その機に置き先が無いのではない
///   （#1697 の実測: 名前だけが読めない瞬間に、生きている `tako-vd` を見失った）
/// - 面は見えているが当たらない + 暗黙の既定 → [`Miss::FallBack`]。その機に置き先が
///   無いということで、**ここで開かないと検証そのものが回らなくなる**（CI・他人の環境・
///   仮想ディスプレイを配線していない Windows = FR-4.8.10 では、狙いが外れるのが正しい
///   挙動）。代わりに [`Placement::fallback_notice`] で起動時に見える警告を出す
pub fn miss_for(verification_gui: bool, enumeration_empty: bool, explicit: bool) -> Miss {
    miss_for_with(
        verification_gui,
        enumeration_empty,
        explicit,
        legacy_1160(),
        legacy_1697(),
    )
}

/// [`miss_for`] の中身（env を引数へ出した純粋関数）
fn miss_for_with(
    verification_gui: bool,
    enumeration_empty: bool,
    explicit: bool,
    legacy_1160: bool,
    legacy_1697: bool,
) -> Miss {
    // #1160 前は構えそのものが無い（必ず落ちる）
    if legacy_1160 || !verification_gui {
        return Miss::FallBack;
    }
    if enumeration_empty || (explicit && !legacy_1697) {
        Miss::Refuse
    } else {
        Miss::FallBack
    }
}

/// `TAKO_DISPLAY` が**明示されている**か（#1697）。空・空白だけは未指定と同じ
/// （[`requested_spec`] と同じ読み = 「狙った」と「落ちてよい」の判定がずれない）
pub fn explicit_request(env_display: Option<&str>) -> bool {
    env_display.is_some_and(|s| !s.trim().is_empty())
}

/// `TAKO_1160_LEGACY` が立っているか（プロセス内で 1 回だけ読む）
pub fn legacy_1160() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os(ENV_LEGACY_1160).is_some())
}

/// `TAKO_1697_LEGACY` が立っているか（プロセス内で 1 回だけ読む）
pub fn legacy_1697() -> bool {
    static LEGACY: OnceLock<bool> = OnceLock::new();
    *LEGACY.get_or_init(|| std::env::var_os(ENV_LEGACY_1697).is_some())
}

/// `TAKO_1697_INJECT_NO_NAMES` が立っているか（名前引きを空として扱う。#1697 の診断 env）
pub fn inject_no_names() -> bool {
    static INJECT: OnceLock<bool> = OnceLock::new();
    *INJECT.get_or_init(|| std::env::var_os(ENV_INJECT_NO_NAMES_1697).is_some())
}

/// 列挙を空に見せる残り回数（`TAKO_1160_INJECT_EMPTY` の値。未指定なら 0）。
/// 呼び出し側は「この回数だけ 0 件を返す」を自分で数える
pub fn inject_empty_count() -> u32 {
    static N: OnceLock<u32> = OnceLock::new();
    *N.get_or_init(|| {
        std::env::var(ENV_INJECT_EMPTY_1160)
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0)
    })
}

/// 名前引きにかける時間の上限。超えたら名前無しで進む（起動を待たせない）。
/// 名前を引ける実装を持つプラットフォームでしか使わない
#[cfg(target_os = "macos")]
const NAME_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

/// ディスプレイの矩形（論理ピクセル。左上原点のグローバル座標）。
///
/// **解決した時点の値を持ち回る**のが要点。あとから `cx.displays()` を引き直すと、
/// ディスプレイスリープや配置変更で空・別値になり得る（#1141 で実測: セルフテストの
/// 突き合わせが「面が見つからない」で黙って弱い検査へ落ちた）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayRect {
    /// 左端
    pub x: f32,
    /// 上端
    pub y: f32,
    /// 幅
    pub width: f32,
    /// 高さ
    pub height: f32,
}

impl DisplayRect {
    /// 指定の矩形がこの面に収まっているか（境界を含む）
    pub fn contains(&self, x: f32, y: f32, width: f32, height: f32) -> bool {
        x >= self.x
            && y >= self.y
            && x + width <= self.x + self.width
            && y + height <= self.y + self.height
    }
}

/// 選択の候補 1 枚。呼び出し側（tako-app）が GPUI の `cx.displays()` から組む
#[derive(Debug, Clone, PartialEq)]
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
    /// 解決した時点のこの面の矩形（取れなければ `None`）
    pub rect: Option<DisplayRect>,
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
#[derive(Debug, Clone, PartialEq)]
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
    /// 名前が読めない面を、`virtual-display.sh ensure` が残した uuid で当てた（#1697）
    RecordedUuid,
    /// `cx.displays()` の並び順（0 始まり）
    Index,
}

impl MatchKind {
    /// 診断・JSON に出す語
    pub fn as_str(self) -> &'static str {
        match self {
            MatchKind::Uuid => "uuid",
            MatchKind::Name => "name",
            MatchKind::RecordedUuid => "recorded_uuid",
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
///
/// 記録済み uuid は見ない（見るのは [`select_with_record`]）。
pub fn select(spec: &str, candidates: &[DisplayCandidate]) -> Selection {
    select_with(spec, candidates, None)
}

/// [`select`] + **記録済み uuid**（#1697）。当てる順は **UUID → 名前 → 記録済み uuid → index**。
///
/// 記録済み uuid（[`recorded_uuid`]）で当てるのは **名前が読めない面（`name=None`）だけ**。
/// 名前が読めていて別の名前の面は、uuid が記録と一致しても当てない（記録より、いま読めた
/// 名前が正。面の作り直しや改名のあとに古い記録で別の面へ出さない）。
/// `TAKO_1697_LEGACY=1` なら記録を見ない（= [`select`] と同じ = #1697 前）
pub fn select_with_record(
    spec: &str,
    candidates: &[DisplayCandidate],
    recorded_uuid: Option<&str>,
) -> Selection {
    select_with(spec, candidates, recorded_uuid.filter(|_| !legacy_1697()))
}

/// [`select`] / [`select_with_record`] の中身（env を読まない純粋関数）
fn select_with(
    spec: &str,
    candidates: &[DisplayCandidate],
    recorded_uuid: Option<&str>,
) -> Selection {
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
            let recorded = recorded_uuid?;
            candidates
                .iter()
                .find(|c| {
                    c.name.is_none()
                        && c.uuid
                            .as_deref()
                            .is_some_and(|u| u.eq_ignore_ascii_case(recorded))
                })
                .map(|c| (c, MatchKind::RecordedUuid))
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

/// 36 文字・8-4-4-4-12 の 16 進 + ハイフンか（#1697。記録の中身の検査と、
/// 「uuid の指定に記録を引かない」判定に使う）
pub fn is_uuid_shaped(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// 記録済み uuid の置き場（#1697）。
///
/// `TAKO_VD_RECORD_DIR` があればそれ、無ければ macOS は `~/`[`RECORD_SUBDIR`]、
/// それ以外は `None`（面を用意するヘルパが無い = 記録も無い）。
/// **書くのは `scripts/lib/virtual-display.sh ensure`**（面を用意する係）で、tako は読むだけ。
/// data dir に置かないのは、隔離起動ごとに data dir が一時 dir へ変わる（= 読めない）ため
pub fn record_dir() -> Option<PathBuf> {
    record_dir_from(
        std::env::var_os(ENV_RECORD_DIR),
        crate::paths::home_dir(),
        cfg!(target_os = "macos"),
    )
}

/// [`record_dir`] の中身（env とホームを引数へ出した純粋関数）
fn record_dir_from(env: Option<OsString>, home: Option<PathBuf>, macos: bool) -> Option<PathBuf> {
    if let Some(dir) = env.filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if !macos {
        return None;
    }
    home.map(|h| h.join(RECORD_SUBDIR))
}

/// 名前の指定 → 記録ファイル名（`<小文字の名前>.uuid`）。
/// 置き場の外を指せる名前（区切り・先頭のドット）と uuid / 空の指定は `None`
fn record_file_name(spec: &str) -> Option<String> {
    let name = spec.trim().to_ascii_lowercase();
    let safe = !name.is_empty()
        && !name.starts_with('.')
        && !is_uuid_shaped(&name)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    safe.then(|| format!("{name}.uuid"))
}

/// 記録ファイルの中身 → uuid（1 行目だけを見る。形が崩れていれば `None`）
fn parse_record(text: &str) -> Option<String> {
    let first = text.lines().next()?.trim();
    is_uuid_shaped(first).then(|| first.to_ascii_lowercase())
}

/// その名前の面について `virtual-display.sh ensure` が残した uuid（#1697）。
/// 記録が無い・読めない・形が崩れている・指定が名前でないなら `None`
pub fn recorded_uuid(spec: &str) -> Option<String> {
    recorded_uuid_in(&record_dir()?, spec)
}

/// [`recorded_uuid`] の置き場を引数へ出したもの（テストは一時 dir を渡す = HOME を読まない）
pub fn recorded_uuid_in(dir: &Path, spec: &str) -> Option<String> {
    let file = record_file_name(spec)?;
    parse_record(&std::fs::read_to_string(dir.join(file)).ok()?)
}

/// 外したときの理由（persist.log / 診断の 1 行。#1160 / #1697）。
///
/// 直し方が違うものを**別の文**にする: 一覧が空（ディスプレイスリープ）/ 名前を引けない
/// プラットフォーム / **名前が読めない面がある**（#1697。記録済み uuid の有無も書く）/ 素の不一致
pub fn miss_reason(candidates: &[DisplayCandidate], recorded_uuid: Option<&str>) -> String {
    miss_reason_for(candidates, recorded_uuid, name_lookup_supported())
}

/// [`miss_reason`] の中身（プラットフォームを引数へ出した純粋関数）
fn miss_reason_for(
    candidates: &[DisplayCandidate],
    recorded_uuid: Option<&str>,
    lookup_supported: bool,
) -> String {
    if candidates.is_empty() {
        "OS のディスプレイ一覧が空（ディスプレイスリープ中は面が在っても列挙から落ちる）".into()
    } else if !lookup_supported {
        // 名前を引けない環境（Windows）で名前を指定された、を切り分けられるようにする
        "該当なし（このプラットフォームでは名前を引けないので UUID か index で指定する）".into()
    } else if candidates.iter().any(|c| c.name.is_none()) {
        match recorded_uuid {
            Some(u) => {
                format!("該当なし（名前が読めない面があり、記録済みの uuid {u} にも当たらない）")
            }
            None => "該当なし（名前が読めない面があり、uuid の記録も無い。\
                     scripts/lib/virtual-display.sh ensure が記録する）"
                .into(),
        }
    } else {
        "該当なし".into()
    }
}

/// 起動時に決めた配置。`tako_check_health` が読む（設計原則 5: AI から状態が読める）
#[derive(Debug, Clone, PartialEq)]
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
    /// 列挙をやり直した回数（#1160。`0` なら 1 回で決まった）
    pub retries: u32,
    /// **窓を開かずに終わった**か（#1160）。`true` ならこの起動で窓は 1 枚も開いていない
    pub refused: bool,
    /// 外したときに**実際にどうしたか**（#1160）。当たった / 指定が無かったときは `None`。
    /// **起動時に 1 回決めた値**をそのまま持つ（診断側で再計算すると判定が散る）
    pub miss_policy: Option<Miss>,
    /// この起動が検証用 GUI（`TAKO_ISOLATED` / `TAKO_SELF_TEST` / `TAKO_VISUAL_TEST`）だったか。
    /// 「なぜユーザーの画面に出た / 出なかった」を後から説明するのに要る（#1160）。
    /// `requested` が `None` のとき（= 通常起動）は常に `false`
    /// （検証用の起動は [`requested_spec`] が必ず指定を返すため）
    pub verification: bool,
    /// `requested` が `TAKO_DISPLAY` の**明示**か（`false` なら検証用の暗黙の既定）。
    /// 外したときに「開かなかった / 既定へ落ちた」を分けた材料そのもの（#1697）
    pub explicit: bool,
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
            retries: 0,
            refused: false,
            miss_policy: None,
            verification: false,
            explicit: false,
        }
    }

    /// persist.log へ残す 1 行。**当たっても外れても必ず出す**（次に同じことを
    /// 調べる人が「そもそも狙ったのか」から確かめられるように）
    pub fn log_line(&self) -> String {
        // やり直したときだけ回数を添える（通常起動のログを増やさない）
        let retried = if self.retries > 0 {
            format!(" / やり直し={} 回", self.retries)
        } else {
            String::new()
        };
        match (&self.requested, &self.resolved) {
            (None, _) => "ディスプレイ指定なし: 既定の面へ開く".to_string(),
            (Some(spec), Some(d)) => format!(
                "ディスプレイ指定 {spec}: {} で解決 → {}{retried}",
                self.matched_by.map(MatchKind::as_str).unwrap_or("?"),
                d.label(),
            ),
            // #1160: 検証用 GUI は既定の面（= ユーザーの画面）へ落ちない。
            // 「開かなかった」と「既定へ落ちた」を**別の文**にしておく（persist.log を
            // 後から読む人が、窓が出たのかどうかを 1 行で判別できるように）
            (Some(spec), None) if self.refused => format!(
                "ディスプレイ指定 {spec}: 見つからないので窓を開かずに終了する\
                 （理由={} / 候補=[{}]{retried}）",
                self.reason.as_deref().unwrap_or("不明"),
                self.available.join(", "),
            ),
            (Some(spec), None) => format!(
                "ディスプレイ指定 {spec}: 見つからないので既定の面へ開く（理由={} / 候補=[{}]{retried}）",
                self.reason.as_deref().unwrap_or("不明"),
                self.available.join(", "),
            ),
        }
    }

    /// 診断（`tako_check_health` の `display_placement`）へ出す形（#1141 / #1160）。
    ///
    /// **形をここに置く理由**: 中身は `Placement` の状態そのものなので、
    /// `dispatch` 側で組み立てると GUI を立てないと検証できない
    /// （実際 #1160 の検証で、無関係な項目が機械の混み具合で落ちてセルフテストが
    /// 145b まで届かなくなった）。ここに置けば純粋なテストで全状態を固定できる。
    /// `describe` は `shell_integration` / `backend` と同じ作法
    pub fn describe(&self) -> serde_json::Value {
        serde_json::json!({
            "requested": self.requested,
            "resolved": self.resolved.as_ref().map(|d| serde_json::json!({
                "index": d.index,
                "id": d.id,
                "name": d.name,
                "uuid": d.uuid,
                "primary": d.primary,
                // 解決した時点の矩形（窓がここへ開いたかを GUI 無しで突き合わせられる）
                "rect": d.rect.map(|r| serde_json::json!({
                    "x": r.x, "y": r.y, "width": r.width, "height": r.height,
                })),
            })),
            "matched_by": self.matched_by.map(MatchKind::as_str),
            "reason": self.reason,
            "available": self.available,
            "name_lookup_supported": name_lookup_supported(),
            // #1160: 列挙のやり直し回数 / 窓を開かずに終わったか /
            // 外したときに実際にどうしたか（当たったときは null）/ 検証用の起動だったか
            "retries": self.retries,
            "refused": self.refused,
            "miss_policy": self.miss_policy.map(Miss::as_str),
            "verification": self.verification,
            // #1697: 指定が `TAKO_DISPLAY` の明示か（false = 検証用の暗黙の既定）
            "explicit": self.explicit,
        })
    }

    /// 診断の `issues` へ出す 1 件（狙って外したときだけ。当たっていれば `None`）。
    ///
    /// **重さを分ける**（#1160）: 既定の面へ落ちた = ユーザーの画面に窓が出ている状態なので
    /// `warning`、窓を開かずに終わった = 目的どおりの拒否なので `error`（起動しなかった
    /// 理由が要る）。当たっているときは黙る = 通常起動と同じ
    pub fn health_issue(&self) -> Option<serde_json::Value> {
        let spec = self.requested.as_deref()?;
        if self.resolved.is_some() {
            return None;
        }
        let reason = self.reason.as_deref().unwrap_or("不明");
        let available = self.available.join(", ");
        let (level, message) = if self.refused {
            // 一覧が空（#1160）と、面は見えているのに明示の指定を見失った（#1697）は直し方が違う
            let what = if self.available.is_empty() {
                "が列挙に出ない"
            } else {
                "がどの面にも当たらない"
            };
            (
                "error",
                format!(
                    "検証用 GUI の置き先 {spec} {what}ので窓を開かずに終了した\
                     （理由={reason} / 候補=[{available}] / やり直し={} 回）。\
                     scripts/lib/virtual-display.sh ensure で用意できる",
                    self.retries,
                ),
            )
        } else {
            (
                "warning",
                format!(
                    "ディスプレイ {spec} が見つからないので既定の面へ開いている\
                     （理由={reason} / 候補=[{available}] / やり直し={} 回）。\
                     scripts/lib/virtual-display.sh ensure で用意できる",
                    self.retries,
                ),
            )
        };
        Some(serde_json::json!({
            "level": level,
            "check": "display_placement",
            "message": message,
        }))
    }

    /// 検証用 GUI が**既定の面（= ユーザーの画面）へ開いた**ときに人へ見せる警告（#1160）。
    ///
    /// この道は残してある（面が見えているのに当たらない = その機に置き先が無い。
    /// ここで開かないと CI・他人の環境・Windows で検証が回らなくなる）が、
    /// **黙って縮退させない**: persist.log を読むまで気づけなかったのが症状の一部だった。
    /// 検証用でない起動や、当たった / 開かずに終わった起動では `None`
    pub fn fallback_notice(&self) -> Option<String> {
        if !self.verification || self.refused || self.resolved.is_some() {
            return None;
        }
        let spec = self.requested.as_deref()?;
        Some(format!(
            "検証用 GUI の置き先 {spec} が見つからないので、\
             ユーザーのメイン画面へ窓を開いた（理由={} / 候補={} 枚）。\
             面を用意する: scripts/lib/virtual-display.sh ensure",
            self.reason.as_deref().unwrap_or("不明"),
            self.available.len(),
        ))
    }

    /// 窓を開かずに終わるときに**人へ見せる**案内（#1160 / #1697）。
    ///
    /// persist.log を読むまで気づけないのが #1160 の症状だったので、起動時に
    /// stderr へ出す用の文をここで作る（文言の正はこのモジュール）。
    /// `refused` でなければ `None`
    pub fn refusal_notice(&self) -> Option<String> {
        if !self.refused {
            return None;
        }
        let spec = self.requested.as_deref().unwrap_or("?");
        if !self.available.is_empty() {
            // #1697: 面は見えているのに明示の指定を見失った。候補を全部出す
            // （どの面が tako-vd だったのかを人が 1 行で突き合わせられるように）
            return Some(format!(
                "検証用 GUI の置き先 {spec}（{ENV_DISPLAY} で指定）が OS のディスプレイ一覧の\
                 どの面にも当たらないので、窓を開かずに終了した（理由={} / 候補=[{}] / \
                 やり直し={} 回）。\n\
                 ユーザーのメイン画面へ検証用の窓を出さないため（#1141 / #1697）。\
                 面を用意して uuid を記録する: scripts/lib/virtual-display.sh ensure\n\
                 置き先を配線していない機では {ENV_DISPLAY} を外す\
                 （未指定なら既定の面へ開いて警告を出す）",
                self.reason.as_deref().unwrap_or("不明"),
                self.available.join(", "),
                self.retries,
            ));
        }
        Some(format!(
            "検証用 GUI の置き先 {spec} が OS のディスプレイ一覧に出ていないので、\
             窓を開かずに終了した（理由={} / 候補={} 枚 / やり直し={} 回）。\n\
             ユーザーのメイン画面へ検証用の窓を出さないため（#1141 / #1160）。\
             面を用意する: scripts/lib/virtual-display.sh ensure\n\
             ディスプレイスリープ中は面が在っても列挙から落ちる。\
             どうしてもメイン画面で起こすなら {ENV_DISPLAY} に実在する面を指定する",
            self.reason.as_deref().unwrap_or("不明"),
            self.available.len(),
            self.retries,
        ))
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
            rect: Some(DisplayRect {
                x: 0.0,
                y: 0.0,
                width: 1512.0,
                height: 982.0,
            }),
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

    #[test]
    fn 矩形の内外判定は境界を含む() {
        let r = DisplayRect {
            x: 1512.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        };
        // 実測値（#1141: tako-vd の中央へ開いた窓）
        assert!(r.contains(2312.0, 420.0, 960.0, 600.0));
        // 面の左上・右下にぴったり接する = 収まっている
        assert!(r.contains(1512.0, 0.0, 2560.0, 1440.0));
        // 1px でもはみ出したら外
        assert!(!r.contains(1511.0, 0.0, 960.0, 600.0), "左へはみ出す");
        assert!(!r.contains(1512.0, 841.0, 960.0, 600.0), "下へはみ出す");
        // メイン画面に開いた窓（#1141 の「外した」実測値）は tako-vd の外
        assert!(!r.contains(276.0, 191.0, 960.0, 600.0));
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
            ..Placement::not_requested()
        };
        assert!(hit.log_line().contains("tako-vd"));
        assert!(hit.log_line().contains("name"));

        let miss = Placement {
            requested: Some("nope".into()),
            resolved: None,
            matched_by: None,
            reason: Some("該当なし".into()),
            available: vec!["[0] id=1 name=Color LCD uuid=? (primary)".into()],
            ..Placement::not_requested()
        };
        let line = miss.log_line();
        assert!(line.contains("既定の面へ開く"), "{line}");
        assert!(line.contains("Color LCD"), "候補も残す: {line}");

        assert!(Placement::not_requested().log_line().contains("指定なし"));
    }

    // ── #1160: 列挙が空のときユーザーの画面へ落ちない ────────────────

    /// **面が 1 枚も見えないとき、検証用 GUI は既定の面へ落ちない**（#1160 の本体）。
    ///
    /// 列挙が空なのは「置き先が無い」ではなく**まだ分からない**状態
    /// （ディスプレイスリープ）なので、ここでユーザーのメイン画面へ窓を出すのは
    /// 「開かない」より悪い。`TAKO_1160_LEGACY=1` では `FallBack` に戻るのでこの検査が落ちる
    #[test]
    fn 列挙が空なら検証用guiは窓を開かない() {
        for explicit in [false, true] {
            assert_eq!(
                miss_for(true, true, explicit),
                Miss::Refuse,
                "検証用 GUI が既定の面（= ユーザーの画面）へ落ちる構えになっている \
                 explicit={explicit}"
            );
        }
    }

    /// **暗黙の既定で面が見えているのに当たらないときは落ちる**（#1160 で狭めた点）。
    ///
    /// その機に置き先が無いということで、`tako-vd` を配線していない環境
    /// （CI・他人の機・Windows = FR-4.8.10）はここを通る。**開かない構えにすると
    /// 検証そのものが回らなくなる**（`build-app.sh --verify` と Windows 実機の
    /// セルフテストが起動できなくなる）ので、落として警告を出す。
    /// `TAKO_DISPLAY` を明示したときは落ちない（#1697。下の検査）
    #[test]
    fn 面が見えているのに当たらないときは検証用でも既定の面へ開く() {
        assert_eq!(
            miss_for(true, false, false),
            Miss::FallBack,
            "置き先を配線していない環境で検証用 GUI が起動できなくなる"
        );
    }

    /// **通常起動は止めない**（`TAKO_DISPLAY` が外れても既定の面へ開く。空でも同じ）。
    /// 指定が外れるのは検証の都合であって、tako が起動できない理由ではない
    #[test]
    fn 通常起動は置き先が外れても既定の面へ開く() {
        for explicit in [false, true] {
            assert_eq!(
                miss_for(false, false, explicit),
                Miss::FallBack,
                "explicit={explicit}"
            );
            assert_eq!(
                miss_for(false, true, explicit),
                Miss::FallBack,
                "空でも通常起動は開く explicit={explicit}"
            );
        }
    }

    /// 列挙が空のあいだは**やり直す**（1 回引いて諦めるのが #1160 の原因）。
    /// `TAKO_1160_LEGACY=1` では 0 回に戻るので、この検査が落ちる
    #[test]
    fn 置き先が無いときは列挙をやり直す() {
        let v = retry_policy(true);
        assert!(
            v.retries > 0,
            "検証用 GUI が列挙をやり直さない（起動の瞬間だけ空になる面を拾えない）"
        );
        let n = retry_policy(false);
        assert!(n.retries > 0, "通常起動も 1 回で諦めない");
        assert!(
            v.retries > n.retries,
            "窓を開かずに終わる側の方が長く待つ（開いてしまうと取り返せない）: \
             検証={} 通常={}",
            v.retries,
            n.retries,
        );
    }

    /// やり直しの予算は「起動が体感で止まらない」範囲（上限 2 秒・下限 0.3 秒）
    #[test]
    fn やり直しの予算は起動を止めない範囲() {
        assert_eq!(
            retry_policy(true).budget(),
            Duration::from_millis(2000),
            "検証用 GUI の予算"
        );
        assert_eq!(
            retry_policy(false).budget(),
            Duration::from_millis(300),
            "通常起動の予算"
        );
    }

    /// A/B（`TAKO_1160_LEGACY=1`）の中身: やり直さず必ず既定の面へ落ちる = #1160 前
    #[test]
    fn legacyは1回引いて既定の面へ落ちる() {
        for verification in [true, false] {
            for empty in [true, false] {
                assert_eq!(
                    retry_policy_for(verification, true).retries,
                    0,
                    "verification={verification}"
                );
                for explicit in [false, true] {
                    assert_eq!(
                        miss_for_with(verification, empty, explicit, true, false),
                        Miss::FallBack,
                        "verification={verification} empty={empty} explicit={explicit}"
                    );
                }
            }
        }
        // 新挙動は同じ入力で構えが変わる（= A/B が本当に効いている）
        assert_ne!(
            miss_for_with(true, true, false, false, false),
            miss_for_with(true, true, false, true, false),
            "legacy と新挙動が同じなら A/B に検出力が無い"
        );
    }

    /// 既定の面へ落ちるときも**黙らない**（#1160 の 2 つ目の要望）。
    /// 検証用 GUI のときだけ警告を出す（通常起動の出力は増やさない）
    #[test]
    fn 検証用guiが既定の面へ落ちたら起動時に警告する() {
        let fell_back = Placement {
            requested: Some(DEFAULT_VIRTUAL_DISPLAY_NAME.into()),
            reason: Some("該当なし".into()),
            available: vec!["[0] id=1 name=Color LCD uuid=? (primary)".into()],
            miss_policy: Some(Miss::FallBack),
            verification: true,
            ..Placement::not_requested()
        };
        let notice = fell_back
            .fallback_notice()
            .expect("検証用 GUI が既定の面へ落ちたら警告が出る");
        assert!(notice.contains("メイン画面"), "{notice}");
        assert!(notice.contains("virtual-display.sh ensure"), "{notice}");
        // 通常起動では出さない
        let normal = Placement {
            verification: false,
            ..fell_back.clone()
        };
        assert!(
            normal.fallback_notice().is_none(),
            "通常起動の出力は増やさない"
        );
        // 当たったとき・開かずに終わったときも出さない（別の文がある）
        let hit = Placement {
            resolved: Some(fixture()[1].clone()),
            ..fell_back.clone()
        };
        assert!(hit.fallback_notice().is_none());
        let refused = Placement {
            refused: true,
            ..fell_back.clone()
        };
        assert!(refused.fallback_notice().is_none());
    }

    /// 列挙が 0 件でも `select` は落ちず、候補が空の `NotFound` になる
    /// （#1160 の症状そのもの: `候補=[]`）
    #[test]
    fn 列挙が空でも落ちずに候補が空の見つからないになる() {
        match select(DEFAULT_VIRTUAL_DISPLAY_NAME, &[]) {
            Selection::NotFound { spec, available } => {
                assert_eq!(spec, DEFAULT_VIRTUAL_DISPLAY_NAME);
                assert!(available.is_empty(), "候補は空: {available:?}");
            }
            other => panic!("空列挙で見つからない扱いにならない: {other:?}"),
        }
    }

    /// 「開かずに終わった」と「既定へ落ちた」は persist.log の**別の文**になる。
    /// 後から読む人が、窓が出たのかどうかを 1 行で判別できること
    #[test]
    fn 窓を開かずに終わった記録は既定へ落ちた記録と別の文になる() {
        let refused = Placement {
            requested: Some(DEFAULT_VIRTUAL_DISPLAY_NAME.into()),
            reason: Some("該当なし".into()),
            retries: VERIFICATION_RETRIES,
            refused: true,
            miss_policy: Some(Miss::Refuse),
            verification: true,
            ..Placement::not_requested()
        };
        let line = refused.log_line();
        assert!(line.contains("窓を開かずに終了する"), "{line}");
        assert!(
            !line.contains("既定の面へ開く"),
            "既定へ落ちたと読める文が混ざっている: {line}"
        );
        assert!(
            line.contains("やり直し=20 回"),
            "やり直した回数も残す: {line}"
        );

        // 人向けの案内（起動時に stderr へ出す分）。persist.log を読まなくても気づける
        let notice = refused.refusal_notice().expect("refused なら案内が出る");
        assert!(notice.contains(DEFAULT_VIRTUAL_DISPLAY_NAME), "{notice}");
        assert!(
            notice.contains("virtual-display.sh ensure"),
            "直し方を書く: {notice}"
        );
        // 落ちた側・指定なしの側では案内を出さない（通常起動の出力を汚さない）
        let fell_back = Placement {
            requested: Some("nope".into()),
            reason: Some("該当なし".into()),
            ..Placement::not_requested()
        };
        assert!(fell_back.refusal_notice().is_none());
        assert!(Placement::not_requested().refusal_notice().is_none());
    }

    /// 診断（`tako_check_health` の `display_placement`）の形を**全状態で固定する**（#1160）。
    ///
    /// これを GUI 側で組み立てていると、無関係な項目が機械の混み具合で落ちた日に
    /// セルフテストがここまで届かず、形が検証できないまま merge されうる（実際に踏んだ）。
    #[test]
    fn 診断の形は全状態で固定されている() {
        // ① 当たった
        let hit = Placement {
            requested: Some(DEFAULT_VIRTUAL_DISPLAY_NAME.into()),
            resolved: Some(fixture()[1].clone()),
            matched_by: Some(MatchKind::Name),
            retries: 3,
            verification: true,
            ..Placement::not_requested()
        };
        let v = hit.describe();
        assert_eq!(v["requested"], DEFAULT_VIRTUAL_DISPLAY_NAME);
        assert_eq!(v["resolved"]["id"], 13);
        assert_eq!(v["resolved"]["name"], "tako-vd");
        assert_eq!(v["resolved"]["rect"]["width"], 1512.0);
        assert_eq!(v["matched_by"], "name");
        assert_eq!(v["retries"], 3);
        assert_eq!(v["refused"], false);
        assert!(v["miss_policy"].is_null(), "当たったので構えは出さない");
        assert_eq!(v["verification"], true);
        assert!(
            hit.health_issue().is_none(),
            "当たっているときは issues に出さない（通常起動と同じ）"
        );

        // ② 既定の面へ落ちた（= ユーザーの画面に出ている）
        let fell_back = Placement {
            requested: Some(DEFAULT_VIRTUAL_DISPLAY_NAME.into()),
            reason: Some("該当なし".into()),
            available: vec!["[0] id=1 name=Color LCD uuid=? (primary)".into()],
            retries: 3,
            miss_policy: Some(Miss::FallBack),
            verification: true,
            ..Placement::not_requested()
        };
        let v = fell_back.describe();
        assert!(v["resolved"].is_null());
        assert_eq!(v["miss_policy"], "fall_back");
        assert_eq!(v["refused"], false);
        assert_eq!(v["available"].as_array().map(Vec::len), Some(1));
        let issue = fell_back.health_issue().expect("外したら申告する");
        assert_eq!(issue["level"], "warning", "窓は出ているので warning");
        assert_eq!(issue["check"], "display_placement");
        let msg = issue["message"].as_str().unwrap_or_default();
        assert!(msg.contains("既定の面へ開いている"), "{msg}");
        assert!(
            msg.contains("virtual-display.sh ensure"),
            "直し方も書く: {msg}"
        );

        // ③ 窓を開かずに終わった（#1160 の本体）
        let refused = Placement {
            requested: Some(DEFAULT_VIRTUAL_DISPLAY_NAME.into()),
            reason: Some("OS のディスプレイ一覧が空".into()),
            retries: VERIFICATION_RETRIES,
            refused: true,
            miss_policy: Some(Miss::Refuse),
            verification: true,
            ..Placement::not_requested()
        };
        let v = refused.describe();
        assert_eq!(v["refused"], true);
        assert_eq!(v["miss_policy"], "refuse");
        assert_eq!(v["retries"], 20);
        let issue = refused.health_issue().expect("拒否も申告する");
        assert_eq!(
            issue["level"], "error",
            "起動しなかった理由なので warning では埋もれる"
        );
        let msg = issue["message"].as_str().unwrap_or_default();
        assert!(msg.contains("窓を開かずに終了した"), "{msg}");
        assert!(
            !msg.contains("既定の面へ開いている"),
            "落ちたと読める文が混ざっている: {msg}"
        );

        // ④ 指定なし（通常起動）: 形は出るが申告はしない
        let none = Placement::not_requested();
        let v = none.describe();
        assert!(v["requested"].is_null());
        assert_eq!(v["retries"], 0);
        assert_eq!(v["verification"], false);
        assert!(none.health_issue().is_none());
    }

    /// やり直していないときログに回数を足さない（通常起動のログを増やさない）
    #[test]
    fn やり直していないときログに回数を足さない() {
        let hit = Placement {
            requested: Some("tako-vd".into()),
            resolved: Some(fixture()[1].clone()),
            matched_by: Some(MatchKind::Name),
            ..Placement::not_requested()
        };
        assert!(!hit.log_line().contains("やり直し"), "{}", hit.log_line());
        assert_eq!(Placement::not_requested().retries, 0);
        assert!(!Placement::not_requested().refused);
    }

    // ── #1697: 名前が読めない瞬間にユーザーの画面へ落ちない ──────────────

    /// 主画面のダミー uuid（実機の値は置かない = public リポ）
    const UUID_MAIN: &str = "AAAAAAAA-0000-4000-8000-000000000001";
    /// 仮想ディスプレイのダミー uuid。GPUI は小文字で、記録（JXA）は大文字で出るので
    /// 候補側は小文字・記録側は大文字で持ち、大文字小文字を無視して当たることも固定する
    const UUID_VD: &str = "BBBBBBBB-0000-4000-8000-000000000002";

    /// 2026-09-24 の実測の形（候補 2 枚とも `name=?`・うち 1 枚の uuid が tako-vd）
    fn names_unreadable() -> Vec<DisplayCandidate> {
        vec![
            cand(0, 1, None, Some(&UUID_MAIN.to_ascii_lowercase())),
            cand(1, 6, None, Some(&UUID_VD.to_ascii_lowercase())),
        ]
    }

    /// **#1697 の本体**: 名前が読めない一覧でも、記録済みの uuid で tako-vd へ解決する。
    /// `TAKO_1697_LEGACY=1` では記録を見ない（= 名前だけで探す）のでここが落ちる
    #[test]
    fn 名前が読めない一覧でも記録済みuuidで仮想ディスプレイへ解決する() {
        let s = select_with_record(
            DEFAULT_VIRTUAL_DISPLAY_NAME,
            &names_unreadable(),
            Some(UUID_VD),
        );
        let Selection::Selected {
            display,
            matched_by,
            spec,
        } = s
        else {
            panic!("名前が読めない瞬間に tako-vd を見失う（#1697 の症状）: {s:?}");
        };
        assert_eq!(display.id, 6, "記録済み uuid の面へ当たる");
        assert_eq!(matched_by, MatchKind::RecordedUuid);
        assert_eq!(spec, DEFAULT_VIRTUAL_DISPLAY_NAME);
        // persist.log の解決行から「何で当てたか」が読める
        let line = Placement {
            requested: Some(spec),
            resolved: Some(display),
            matched_by: Some(matched_by),
            ..Placement::not_requested()
        }
        .log_line();
        assert!(line.contains("recorded_uuid で解決"), "{line}");
    }

    /// 名前が読めていれば**記録より名前が勝つ**（記録は名前が読めないときの代わり）
    #[test]
    fn 名前が読めていれば記録済みuuidより名前が勝つ() {
        let cands = vec![
            cand(0, 1, None, Some(UUID_VD)),
            cand(1, 13, Some("tako-vd"), Some(UUID_MAIN)),
        ];
        match select_with_record("tako-vd", &cands, Some(UUID_VD)) {
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

    /// 名前と記録が**別の面を指す矛盾**: 記録の uuid の面に別の名前が読めているなら当てない
    /// （作り直し・改名のあとに古い記録で別の面へ出さない）。当たらないので見つからない
    #[test]
    fn 名前が読めて別名の面には記録済みuuidで当てない() {
        let cands = vec![
            cand(0, 1, Some("Color LCD"), Some(UUID_MAIN)),
            cand(1, 6, Some("External"), Some(UUID_VD)),
        ];
        assert!(
            matches!(
                select_with_record("tako-vd", &cands, Some(UUID_VD)),
                Selection::NotFound { .. }
            ),
            "名前が External と読めている面へ tako-vd の記録で当ててしまう"
        );
    }

    /// 記録が無い / 形が崩れていれば、名前が読めない一覧では従来どおり見つからない
    #[test]
    fn 記録が無ければ名前が読めない一覧では見つからない() {
        let cands = names_unreadable();
        let Selection::NotFound { available, .. } = select_with_record("tako-vd", &cands, None)
        else {
            panic!("記録が無いのに当たった");
        };
        assert_eq!(available.len(), 2, "候補は全部出す: {available:?}");
        assert!(
            matches!(select("tako-vd", &cands), Selection::NotFound { .. }),
            "select は記録を見ない"
        );
    }

    /// **明示の指定を見失ったら検証用 GUI は開かない**（#1697 の 2 段目）。
    /// 面が見えているので #1160 の構えでは「落ちる」側だった。
    /// `TAKO_1697_LEGACY=1` では `FallBack` に戻るのでここが落ちる
    #[test]
    fn 明示の指定を見失ったら検証用guiは窓を開かない() {
        assert_eq!(
            miss_for(true, false, true),
            Miss::Refuse,
            "TAKO_DISPLAY で明示した面を見失った検証用 GUI が既定の面（= ユーザーの画面）へ落ちる"
        );
    }

    /// A/B（`TAKO_1697_LEGACY=1`）の中身: 記録を見ず、明示でも面が見えていれば落ちる = #1697 前
    #[test]
    fn legacy1697は記録を見ず明示でも既定の面へ落ちる() {
        assert_eq!(
            miss_for_with(true, false, true, false, true),
            Miss::FallBack
        );
        // 列挙が空の拒否（#1160）は #1697 の A/B では戻さない
        assert_eq!(miss_for_with(true, true, true, false, true), Miss::Refuse);
        assert!(matches!(
            select_with(DEFAULT_VIRTUAL_DISPLAY_NAME, &names_unreadable(), None),
            Selection::NotFound { .. }
        ));
        // 新挙動は同じ入力で構えが変わる（= A/B が本当に効いている）
        assert_ne!(
            miss_for_with(true, false, true, false, false),
            miss_for_with(true, false, true, false, true),
            "legacy と新挙動が同じなら A/B に検出力が無い"
        );
    }

    /// 明示かどうかは `requested_spec` と同じ読み（空・空白だけは未指定）
    #[test]
    fn 明示の判定は空と空白を未指定として扱う() {
        assert!(!explicit_request(None));
        assert!(!explicit_request(Some("")));
        assert!(!explicit_request(Some("  ")));
        assert!(explicit_request(Some("tako-vd")));
        assert!(explicit_request(Some(UUID_VD)));
    }

    /// 見失って開かなかった記録・案内・診断は**候補を全部出し**、一覧が空の拒否と別の文になる
    #[test]
    fn 明示の指定を見失った拒否は候補を出して一覧が空の拒否と別の文になる() {
        let cands = names_unreadable();
        let refused = Placement {
            requested: Some(DEFAULT_VIRTUAL_DISPLAY_NAME.into()),
            reason: Some(miss_reason_for(&cands, None, true)),
            available: cands.iter().map(DisplayCandidate::label).collect(),
            refused: true,
            miss_policy: Some(Miss::Refuse),
            verification: true,
            explicit: true,
            ..Placement::not_requested()
        };
        let line = refused.log_line();
        assert!(line.contains("窓を開かずに終了する"), "{line}");
        assert!(!line.contains("既定の面へ開く"), "{line}");
        let notice = refused.refusal_notice().expect("refused なら案内が出る");
        assert!(notice.contains("どの面にも当たらない"), "{notice}");
        for label in &refused.available {
            assert!(
                notice.contains(label.as_str()),
                "候補 {label} が出ない: {notice}"
            );
        }
        assert!(notice.contains("virtual-display.sh ensure"), "{notice}");
        assert!(
            !notice.contains("一覧に出ていない"),
            "一覧が空の拒否（#1160）の文が混ざっている: {notice}"
        );
        let issue = refused.health_issue().expect("拒否は申告する");
        assert_eq!(issue["level"], "error");
        let msg = issue["message"].as_str().unwrap_or_default();
        assert!(msg.contains("どの面にも当たらない"), "{msg}");
        assert_eq!(refused.describe()["explicit"], true);
        assert!(
            refused.fallback_notice().is_none(),
            "開いていないので落ちた警告は出さない"
        );

        // 一覧が空の拒否（#1160）は従来の文のまま
        let blind = Placement {
            available: Vec::new(),
            ..refused.clone()
        };
        let notice = blind.refusal_notice().expect("refused なら案内が出る");
        assert!(notice.contains("一覧に出ていない"), "{notice}");
        let msg = blind.health_issue().expect("申告する")["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(msg.contains("列挙に出ない"), "{msg}");
    }

    /// 外した理由は直し方ごとに別の文（#1697 は名前が読めないことと記録の有無を書く）
    #[test]
    fn 外した理由は直し方ごとに別の文になる() {
        assert!(miss_reason_for(&[], None, true).contains("一覧が空"));
        let unreadable = names_unreadable();
        assert!(miss_reason_for(&unreadable, None, false).contains("名前を引けない"));
        let no_record = miss_reason_for(&unreadable, None, true);
        assert!(no_record.contains("名前が読めない面"), "{no_record}");
        assert!(no_record.contains("記録も無い"), "{no_record}");
        let stale = miss_reason_for(&unreadable, Some(UUID_VD), true);
        assert!(
            stale.contains(UUID_VD),
            "当たらなかった記録を名指す: {stale}"
        );
        assert_eq!(miss_reason_for(&fixture(), None, true), "該当なし");
    }

    /// 一時 dir を片付けるガード（パニックしても残骸を残さない）
    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 記録は `<小文字の名前>.uuid` に 1 行の uuid。読めない形・置き場の外を指す名前は無視する
    #[test]
    fn 記録は名前ごとに1行のuuidで壊れた記録は無視する() {
        let dir =
            TempDir(std::env::temp_dir().join(format!("tako-1697-record-{}", std::process::id())));
        std::fs::create_dir_all(&dir.0).expect("一時 dir");
        let file = dir.0.join("tako-vd.uuid");
        std::fs::write(&file, format!("{UUID_VD}\n")).expect("記録を書く");
        let got = recorded_uuid_in(&dir.0, "tako-vd");
        assert_eq!(got.as_deref(), Some(UUID_VD.to_ascii_lowercase().as_str()));
        // 指定の大文字小文字は無視する（名前の照合と同じ）
        assert_eq!(recorded_uuid_in(&dir.0, " TAKO-VD "), got);
        // 無い名前・置き場の外を指す名前・uuid の指定は引かない
        assert_eq!(recorded_uuid_in(&dir.0, "other"), None);
        for spec in ["../tako-vd", "a/b", ".hidden", "", UUID_VD] {
            assert_eq!(recorded_uuid_in(&dir.0, spec), None, "spec={spec:?}");
        }
        // 形の崩れた記録は無いのと同じ
        for broken in ["", "not-a-uuid\n", "BBBBBBBB-0000-4000-8000-00000000000\n"] {
            std::fs::write(&file, broken).expect("記録を書く");
            assert_eq!(
                recorded_uuid_in(&dir.0, "tako-vd"),
                None,
                "broken={broken:?}"
            );
        }
    }

    /// 置き場: env が勝つ / macOS はホーム配下 / それ以外は無い（HOME は読まずに引数で渡す）
    #[test]
    fn 記録の置き場はenvが勝ちmacosだけホーム配下() {
        let home = Some(PathBuf::from("/home-for-test"));
        assert_eq!(
            record_dir_from(Some("/x".into()), home.clone(), true),
            Some(PathBuf::from("/x"))
        );
        assert_eq!(
            record_dir_from(None, home.clone(), true),
            Some(PathBuf::from("/home-for-test").join(RECORD_SUBDIR))
        );
        assert_eq!(
            record_dir_from(Some("".into()), home.clone(), true),
            Some(PathBuf::from("/home-for-test").join(RECORD_SUBDIR)),
            "空の env は未指定と同じ"
        );
        assert_eq!(record_dir_from(None, home, false), None);
        assert_eq!(
            record_dir_from(Some("/x".into()), None, false),
            Some(PathBuf::from("/x")),
            "env はどの OS でも効く"
        );
    }

    #[test]
    fn uuidの形は8_4_4_4_12の16進() {
        assert!(is_uuid_shaped(UUID_VD));
        assert!(is_uuid_shaped(&UUID_VD.to_ascii_lowercase()));
        assert!(!is_uuid_shaped("tako-vd"));
        assert!(!is_uuid_shaped("BBBBBBBB-0000-4000-8000-00000000000Z"));
        assert!(!is_uuid_shaped("BBBBBBBB00000-4000-8000-000000000002"));
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
