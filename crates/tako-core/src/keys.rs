//! keys — キー名から PTY バイト列への符号化（Issue #662）
//!
//! GUI のキーイベントを介さずに「Enter を押した」「↓ を押した」を PTY へ届けるための部品。
//! AI（MCP / CLI）が TUI の対話ダイアログ（AskUserQuestion・permission ダイアログ等）を
//! 操作するのに要る。
//!
//! # なぜ tako-app の変換と別に置くのか
//!
//! `tako-app::keybindings::keystroke_to_bytes` は GPUI の `Keystroke`（`key` + `key_char` +
//! `Modifiers`）を入力に取るため、GPUI 非依存の層からは呼べない。ここはキー名の**文字列**を
//! 入力に取り、同じバイト列を返す。二重実装の乖離は
//! `tako-app` 側のテスト（`新旧のキー符号化が一致する`）で固定している。
//!
//! # 修飾の扱い
//!
//! 名前に `ctrl-` / `shift-` / `alt-` を前置できる（`ctrl-c` / `shift-tab`）。
//! 修飾付き機能キーは xterm 標準の `CSI 1;mod X` / `CSI n;mod ~`、修飾付き
//! Enter / Tab / Backspace / Esc は CSI u（`CSI 13;2u` = Shift+Enter）で送る。
//! これは tako-app 側の既定（`CsiUMode::ModifiedOnly`）と同じ規律で、
//! Claude Code の Shift+Enter 改行（#28）が要求する形。

/// 端末側のモードに応じた符号化の出し分け。
///
/// `app_cursor` / `disambiguate` は **TUI が要求したときだけ真**にする
/// （`TerminalSession` から引ける）。既定は素の端末想定で、レガシー形式を送る
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEncoding {
    /// DECCKM（application cursor keys）。真なら矢印は `ESC O A`、偽なら `ESC [ A`
    pub app_cursor: bool,
    /// kitty keyboard protocol の disambiguate。真なら Esc 単押しを `CSI 27u` で送る
    /// （偽のまま CSI 27u を送ると、非対応アプリの入力欄に「27u」が文字として入る。
    /// tako-app 側で実機バグとして確認済みの罠なので既定では送らない）
    pub disambiguate: bool,
    /// **経路が CSI u を内側アプリまで運べるか**（#729）。
    ///
    /// `disambiguate` が「内側アプリが CSI u を読みたがっているか」なのに対し、
    /// こちらは「そもそも届くのか」。器（psmux）が握り潰す場合は偽になり、
    /// 修飾付きキーをレガシー形式へ落とす。
    /// 正は [`crate::backend::BackendCapabilities::extended_keys`]
    pub extended_keys: bool,
}

impl Default for KeyEncoding {
    fn default() -> Self {
        Self {
            app_cursor: false,
            disambiguate: false,
            // 既定は「運べる」。運べない器だけが明示的に倒す
            extended_keys: true,
        }
    }
}

/// 修飾キーの組み合わせ
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Mods {
    shift: bool,
    alt: bool,
    ctrl: bool,
}

impl Mods {
    /// xterm の修飾エンコード（1 = 修飾なし、+1 = Shift、+2 = Alt、+4 = Ctrl）
    fn encode(&self) -> u32 {
        1 + u32::from(self.shift) + (u32::from(self.alt) << 1) + (u32::from(self.ctrl) << 2)
    }
}

/// キー名を PTY バイト列へ符号化する。未知の名前は `None`。
///
/// 受け付ける名前（大文字小文字は区別しない）:
/// - 特殊キー: `enter`(=`return`) / `escape`(=`esc`) / `tab` / `backtab`(=Shift+Tab) /
///   `backspace` / `delete` / `space` / `up` / `down` / `left` / `right` /
///   `home` / `end` / `pageup` / `pagedown` / `insert` / `f1`〜`f12`
/// - 修飾前置: `ctrl-` / `shift-` / `alt-`（例 `ctrl-c` / `shift-tab` / `alt-v`）
/// - 1 文字リテラル: `a` / `1` / `?`（そのまま UTF-8 バイト列）
pub fn encode_key(name: &str, enc: KeyEncoding) -> Option<Vec<u8>> {
    let (mods, base) = split_modifiers(name);
    encode_base(&base, mods, enc)
}

/// キー名の並びを 1 本のバイト列へ符号化する。1 つでも未知の名前があれば
/// その名前を `Err` で返す（部分適用してから失敗する = 途中まで撃ってしまうのを防ぐ）
pub fn encode_keys(names: &[String], enc: KeyEncoding) -> Result<Vec<Vec<u8>>, String> {
    names
        .iter()
        .map(|n| encode_key(n, enc).ok_or_else(|| n.clone()))
        .collect()
}

/// 修飾前置を剥がす。`ctrl-shift-up` のような多重前置も受ける
fn split_modifiers(name: &str) -> (Mods, String) {
    let mut mods = Mods::default();
    let mut rest = name.trim();
    loop {
        let lower = rest.to_ascii_lowercase();
        // `ctrl-` を剥がしたいが、キー名自体が `-`（ハイフンキー）の場合を壊さないよう
        // 「前置 + 残りが空でない」ときだけ剥がす
        let stripped = ["ctrl-", "control-", "shift-", "alt-", "option-", "meta-"]
            .iter()
            .find_map(|p| lower.strip_prefix(p).map(|r| (*p, r.len())));
        match stripped {
            Some((prefix, rest_len)) if rest_len > 0 => {
                match prefix {
                    "ctrl-" | "control-" => mods.ctrl = true,
                    "shift-" => mods.shift = true,
                    // meta は Alt と同義（xterm の metaSendsEscape）
                    _ => mods.alt = true,
                }
                rest = &rest[rest.len() - rest_len..];
            }
            _ => break,
        }
    }
    (mods, rest.to_string())
}

fn encode_base(base: &str, mods: Mods, enc: KeyEncoding) -> Option<Vec<u8>> {
    let lower = base.to_ascii_lowercase();
    // `backtab` は Shift+Tab の別名
    let (lower, mods) = if lower == "backtab" {
        (
            "tab".to_string(),
            Mods {
                shift: true,
                ..mods
            },
        )
    } else {
        (lower, mods)
    };
    let m = mods.encode();

    // 経路が CSI u を運べないなら、届く形へ落とす（#729）。
    // 落とさないと器（psmux）が握り潰して**何も届かない** = キーが無反応になる
    if !enc.extended_keys {
        if let Some(bytes) = legacy_modified_mods(&lower, mods) {
            return Some(bytes);
        }
    }

    // 修飾付き Enter / Tab / Backspace / Esc はレガシー形式では区別できないので CSI u。
    // Esc 単押しは TUI が kitty disambiguate を要求しているときだけ CSI u
    let csi_u_code: Option<u32> = match lower.as_str() {
        "escape" | "esc" if enc.disambiguate || m > 1 => Some(27),
        "enter" | "return" if m > 1 => Some(13),
        "tab" if m > 1 => Some(9),
        "backspace" if m > 1 => Some(127),
        _ => None,
    };
    if let Some(code) = csi_u_code {
        return Some(if m > 1 {
            format!("\x1b[{code};{m}u").into_bytes()
        } else {
            format!("\x1b[{code}u").into_bytes()
        });
    }

    // Ctrl+英字 → C0 制御コード（ctrl-c = 0x03）
    if mods.ctrl {
        let mut chars = lower.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(vec![(c as u8) & 0x1f]);
            }
        }
    }

    // 矢印・Home / End は DECCKM で SS3 形式へ切り替わる。修飾付きは常に CSI 形式
    let cursor = |letter: char| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[1;{m}{letter}").into_bytes()
        } else if enc.app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if m > 1 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };

    let bytes: Vec<u8> = match lower.as_str() {
        "enter" | "return" => b"\r".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" | "esc" => b"\x1b".to_vec(),
        "backspace" => b"\x7f".to_vec(),
        "space" => b" ".to_vec(),
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "insert" => csi_tilde(2),
        "delete" | "del" => csi_tilde(3),
        "pageup" | "pgup" => csi_tilde(5),
        "pagedown" | "pgdn" => csi_tilde(6),
        "f1" => ss3_or_csi('P', m),
        "f2" => ss3_or_csi('Q', m),
        "f3" => ss3_or_csi('R', m),
        "f4" => ss3_or_csi('S', m),
        "f5" => csi_tilde(15),
        "f6" => csi_tilde(17),
        "f7" => csi_tilde(18),
        "f8" => csi_tilde(19),
        "f9" => csi_tilde(20),
        "f10" => csi_tilde(21),
        "f11" => csi_tilde(23),
        "f12" => csi_tilde(24),
        // 1 文字リテラル（数字キー・英字・記号）。修飾なし前提。
        // Shift 付きは呼び出し側が大文字を渡す（`shift-a` ではなく `A`）
        _ => {
            let mut chars = base.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => {
                    let mut out = Vec::new();
                    // Alt 単独 = meta（ESC 前置）。xterm の metaSendsEscape
                    if mods.alt {
                        out.push(0x1b);
                    }
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                    out
                }
                _ => return None,
            }
        }
    };
    Some(bytes)
}

/// CSI u を運べない経路（psmux）向けのレガシー表現（#729）。
///
/// **GUI 経路（`tako-app::keystroke_to_bytes`）と共有するために公開している。**
/// 二重に書くと「手で押すと動くのに AI から送ると動かない（or 逆）」という
/// 再現の難しい差になる（`新旧のキー符号化が一致する` テストが番犬）
pub fn legacy_modified(key: &str, shift: bool, alt: bool, ctrl: bool) -> Option<Vec<u8>> {
    legacy_modified_mods(&key.to_ascii_lowercase(), Mods { shift, alt, ctrl })
}

/// [`legacy_modified`] の本体。
///
/// 対象は「本来 CSI u で送るキー」だけ。器が CSI u を握り潰すと**何も届かない**ので、
/// 修飾を完全には表現できなくても届く形へ落とす方が良い。
/// どの形が psmux を通るかは実測で確かめてある（Issue #729 の表）。
fn legacy_modified_mods(lower: &str, mods: Mods) -> Option<Vec<u8>> {
    let m = mods.encode();
    match lower {
        // 修飾付き Enter の意図は「送信せず改行」。meta-Enter（`ESC CR`）は
        // Claude Code が Option / Alt+Enter として受け付ける標準形で、psmux も通す。
        // Shift / Ctrl / Alt はここで区別を失うが、いずれも改行が欲しい打鍵なので
        // 1 つに畳んで良い（畳まないと全滅する）
        "enter" | "return" if m > 1 => Some(b"\x1b\r".to_vec()),
        // Shift+Tab = backtab。psmux を通ることを実測済み
        "tab" if m > 1 => Some(b"\x1b[Z".to_vec()),
        // 修飾を表現する術が無いキーは修飾を落として素の形で送る。
        // CSI u のままだと握り潰されて無反応になるので、素の方がまだ使える
        "backspace" if m > 1 => Some(b"\x7f".to_vec()),
        // 単押し（disambiguate 由来の `CSI 27u`）も修飾付きも素の Esc へ
        "escape" | "esc" => Some(b"\x1b".to_vec()),
        _ => None,
    }
}

/// F1〜F4 は修飾なしで SS3（`ESC O P`）、修飾付きで CSI（`ESC [ 1;2P`）
fn ss3_or_csi(letter: char, m: u32) -> Vec<u8> {
    if m > 1 {
        format!("\x1b[1;{m}{letter}").into_bytes()
    } else {
        format!("\x1bO{letter}").into_bytes()
    }
}

/// このモジュールが受け付けるキー名の一覧（MCP / CLI の説明文と補完に使う。
/// 説明を 2 箇所に書かないための単一ソース）
pub const KEY_NAMES: &[&str] = &[
    "enter",
    "escape",
    "tab",
    "backtab",
    "backspace",
    "delete",
    "space",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "insert",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(name: &str) -> Vec<u8> {
        encode_key(name, KeyEncoding::default()).expect(name)
    }

    #[test]
    fn 特殊キーはレガシーバイト列を返す() {
        assert_eq!(enc("enter"), b"\r");
        assert_eq!(enc("return"), b"\r");
        assert_eq!(enc("tab"), b"\t");
        assert_eq!(enc("escape"), b"\x1b");
        assert_eq!(enc("esc"), b"\x1b");
        assert_eq!(enc("backspace"), b"\x7f");
        assert_eq!(enc("space"), b" ");
        assert_eq!(enc("up"), b"\x1b[A");
        assert_eq!(enc("down"), b"\x1b[B");
        assert_eq!(enc("right"), b"\x1b[C");
        assert_eq!(enc("left"), b"\x1b[D");
        assert_eq!(enc("home"), b"\x1b[H");
        assert_eq!(enc("end"), b"\x1b[F");
        assert_eq!(enc("pageup"), b"\x1b[5~");
        assert_eq!(enc("pagedown"), b"\x1b[6~");
        assert_eq!(enc("delete"), b"\x1b[3~");
    }

    #[test]
    fn 大文字小文字を区別しない() {
        assert_eq!(enc("Enter"), b"\r");
        assert_eq!(enc("ESC"), b"\x1b");
        assert_eq!(enc("Down"), b"\x1b[B");
    }

    #[test]
    fn ctrl英字はc0制御コードになる() {
        assert_eq!(enc("ctrl-c"), vec![0x03]);
        assert_eq!(enc("ctrl-C"), vec![0x03]);
        assert_eq!(enc("ctrl-d"), vec![0x04]);
        assert_eq!(enc("control-a"), vec![0x01]);
    }

    /// ダイアログ操作の主経路: 数字キーはそのまま 1 バイトで届く
    #[test]
    fn 一文字リテラルはそのまま送る() {
        assert_eq!(enc("1"), b"1");
        assert_eq!(enc("9"), b"9");
        assert_eq!(enc("a"), b"a");
        assert_eq!(enc("A"), b"A");
        assert_eq!(enc("?"), b"?");
    }

    #[test]
    fn 未知のキー名はnoneを返す() {
        assert!(encode_key("nosuchkey", KeyEncoding::default()).is_none());
        assert!(encode_key("", KeyEncoding::default()).is_none());
        // 複数文字の未知名は 1 文字リテラルにも落ちない
        assert!(encode_key("enterr", KeyEncoding::default()).is_none());
    }

    #[test]
    fn 修飾付き機能キーはxterm形式() {
        assert_eq!(enc("shift-up"), b"\x1b[1;2A");
        assert_eq!(enc("ctrl-up"), b"\x1b[1;5A");
        assert_eq!(enc("shift-delete"), b"\x1b[3;2~");
    }

    /// #28: Shift+Enter はレガシー形式で区別できないので CSI u
    #[test]
    fn 修飾付きenterとtabはcsi_uで送る() {
        assert_eq!(enc("shift-enter"), b"\x1b[13;2u");
        assert_eq!(enc("ctrl-enter"), b"\x1b[13;5u");
        assert_eq!(enc("shift-tab"), b"\x1b[9;2u");
        assert_eq!(enc("backtab"), b"\x1b[9;2u");
        assert_eq!(enc("shift-backspace"), b"\x1b[127;2u");
    }

    /// Esc 単押しの CSI 27u は TUI が kitty disambiguate を要求したときだけ。
    /// 既定で送ると非対応アプリの入力欄に「27u」が文字として入る（tako-app の実機バグ）
    #[test]
    fn esc単押しのcsi_uはdisambiguate時のみ() {
        let dis = KeyEncoding {
            disambiguate: true,
            ..Default::default()
        };
        assert_eq!(encode_key("escape", dis).unwrap(), b"\x1b[27u");
        assert_eq!(enc("escape"), b"\x1b");
        // 修飾付きは disambiguate に関係なく CSI u
        assert_eq!(enc("shift-escape"), b"\x1b[27;2u");
        // Enter / Tab は disambiguate でもレガシーのまま（tako-app と同じ規律）
        assert_eq!(encode_key("enter", dis).unwrap(), b"\r");
        assert_eq!(encode_key("tab", dis).unwrap(), b"\t");
    }

    /// DECCKM 中の矢印は SS3。修飾付きは CSI のまま（xterm 準拠）
    #[test]
    fn app_cursorモードの矢印はss3形式() {
        let app = KeyEncoding {
            app_cursor: true,
            ..Default::default()
        };
        assert_eq!(encode_key("up", app).unwrap(), b"\x1bOA");
        assert_eq!(encode_key("down", app).unwrap(), b"\x1bOB");
        assert_eq!(encode_key("home", app).unwrap(), b"\x1bOH");
        assert_eq!(encode_key("shift-up", app).unwrap(), b"\x1b[1;2A");
        // 矢印以外は影響を受けない
        assert_eq!(encode_key("pageup", app).unwrap(), b"\x1b[5~");
    }

    #[test]
    fn 機能キーf1からf12() {
        assert_eq!(enc("f1"), b"\x1bOP");
        assert_eq!(enc("f4"), b"\x1bOS");
        assert_eq!(enc("f5"), b"\x1b[15~");
        assert_eq!(enc("f12"), b"\x1b[24~");
        assert_eq!(enc("shift-f1"), b"\x1b[1;2P");
    }

    /// Windows / Linux の Alt = meta（ESC 前置）。#575 と同じ規律
    #[test]
    fn alt付き印字文字はesc前置() {
        assert_eq!(enc("alt-v"), b"\x1bv");
        assert_eq!(enc("meta-v"), b"\x1bv");
    }

    #[test]
    fn encode_keysは未知名を報告する() {
        let ok = encode_keys(&["1".into(), "enter".into()], KeyEncoding::default()).unwrap();
        assert_eq!(ok, vec![b"1".to_vec(), b"\r".to_vec()]);

        let err = encode_keys(&["1".into(), "bogus".into()], KeyEncoding::default()).unwrap_err();
        assert_eq!(err, "bogus");
    }

    /// CSI u を運べない経路（psmux）用の符号化
    fn legacy() -> KeyEncoding {
        KeyEncoding {
            extended_keys: false,
            ..Default::default()
        }
    }

    fn enc_legacy(name: &str) -> Vec<u8> {
        encode_key(name, legacy()).expect(name)
    }

    /// **#729 の本命**: 器が CSI u を握り潰す経路では、修飾付き Enter を
    /// meta-Enter（`ESC CR`）で送る。CSI u のままだと内側アプリへ**何も届かない**
    /// （psmux 実測）ので、Shift+Enter が無反応になる
    #[test]
    fn csi_uを運べない経路では修飾付きenterをesc_crで送る() {
        assert_eq!(enc_legacy("shift-enter"), b"\x1b\r");
        // Ctrl / Alt も同じ「送信せず改行」へ畳む（畳まないと全滅する）
        assert_eq!(enc_legacy("ctrl-enter"), b"\x1b\r");
        assert_eq!(enc_legacy("alt-enter"), b"\x1b\r");
        // Shift+Tab は backtab（psmux を通ることを実測済み）
        assert_eq!(enc_legacy("shift-tab"), b"\x1b[Z");
        assert_eq!(enc_legacy("backtab"), b"\x1b[Z");
        // 修飾を表現できないキーは素の形へ落とす（無反応よりまし）
        assert_eq!(enc_legacy("shift-backspace"), b"\x7f");
        assert_eq!(enc_legacy("shift-escape"), b"\x1b");
    }

    /// **回帰防止**: 修飾なしのキーは経路の能力に関係なく従来どおり。
    /// 特に **Enter 単独が `\r` のまま**でなければ「送信できない」に化ける
    #[test]
    fn csi_uを運べなくても修飾なしのキーは変わらない() {
        for name in [
            "enter",
            "return",
            "tab",
            "escape",
            "backspace",
            "space",
            "up",
            "down",
            "left",
            "right",
            "home",
            "end",
            "pageup",
            "pagedown",
            "delete",
            "f1",
            "f5",
        ] {
            assert_eq!(
                enc_legacy(name),
                enc(name),
                "修飾なし {name} が経路の能力で変わってしまった"
            );
        }
        assert_eq!(
            enc_legacy("enter"),
            b"\r",
            "Enter 単独は送信のままでなければならない"
        );
        // Ctrl+英字（C0）も不変
        assert_eq!(enc_legacy("ctrl-c"), vec![0x03]);
        // 修飾付き矢印は元から CSI u ではない（xterm 形式）ので不変
        assert_eq!(enc_legacy("shift-up"), b"\x1b[1;2A");
        assert_eq!(enc_legacy("shift-delete"), b"\x1b[3;2~");
    }

    /// 運べる経路（tmux / 直接ペイン）は従来どおり CSI u。macOS の挙動を変えない
    #[test]
    fn 運べる経路は従来どおりcsi_uを送る() {
        assert_eq!(enc("shift-enter"), b"\x1b[13;2u");
        assert_eq!(enc("shift-tab"), b"\x1b[9;2u");
        assert!(KeyEncoding::default().extended_keys, "既定は運べる側");
    }

    /// 一覧に載っている名前はすべて符号化できる（説明文と実装の乖離を防ぐ）
    #[test]
    fn key_names一覧は全て符号化できる() {
        for name in KEY_NAMES {
            assert!(
                encode_key(name, KeyEncoding::default()).is_some(),
                "{name} が符号化できない"
            );
        }
    }
}
