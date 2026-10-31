//! 本人だけが読み書きできるファイル・置き場（抽象境界 B32。#1632）
//!
//! ## なぜ要るか
//!
//! remote の管理トークン・QR（トークン入り URL）・devices.json などの秘密は、unix では
//! 置き場を 0700・ファイルを 0600 にして同じ PC の別ユーザーから隠している（#104 / P0-3）。
//! #1632 まで Windows はこの腕が no-op で、置き場も中身も親（`%APPDATA%\tako`）の ACL を
//! 受け継ぐだけだった。既定のプロファイルなら本人 + SYSTEM + Administrators に落ち着くが、
//! 移動プロファイル・フォルダのリダイレクト（`%APPDATA%` がサーバーの共有に置かれる）・
//! 継承の広いドメイン環境では、その広さがそのまま秘密に及ぶ。しかも縮退として宣言されていなかった。
//!
//! ## 作り
//!
//! | | unix | Windows |
//! |---|---|---|
//! | 置き場（[`restrict_dir`]） | 0700 | 継承を切った DACL（本人 + SYSTEM。OI / CI で中へ受け継がせる） |
//! | ファイルを作る（[`create`]） | 0600 で作る（既に在れば mode は変えない = `OpenOptions` の仕様） | 作って中身を書く前に、継承を切った DACL（本人 + SYSTEM） |
//! | 無いときだけ作る（[`create_new`]） | 0600 で作る（在れば `AlreadyExists`） | 作って中身を書く前に、継承を切った DACL（本人 + SYSTEM） |
//! | 既に在るファイル（[`restrict`]） | 0600 | 継承を切った DACL（本人 + SYSTEM） |
//! | 読み戻し（[`inspect`]） | mode | DACL の SDDL |
//!
//! Windows の DACL は SDDL `D:P(A;;FA;;;<本人の SID>)(A;;FA;;;SY)` で、`P` が継承を切る
//! （`PROTECTED_DACL_SECURITY_INFORMATION`）。DACL ごと置き換えるので、前の版が書いた緩い ACE
//! （明示の Everyone など）も残らない。置き場に付けると、境界を通らずに中へ作られたファイル
//! （画像の保存・監査ログ・shortcuts.json）も同じ 2 者だけを受け継ぐ = unix の 0700 の置き場と同じ守り方。
//! 既に中に在るものは `SetNamedSecurityInfoW` が受け継ぎの分を張り替える。
//! SYSTEM を残すのは OS の保守（バックアップ・索引・ウイルス対策）を止めないため（OpenSSH の秘密鍵の
//! 検査と同じ線引き）。Administrators は入れない（必要なら所有権を取って入れる = 黙っては読めない）。
//!
//! **Windows で DACL を当てられなくても呼び手を止めない**: 在るものに当てられなかったとき
//! （ACL を持たない置き場 = FAT / exFAT・一部のネットワーク共有、DACL を書き換える権利が無い）は
//! `Ok` を返し、修正前と同じ継承のまま続ける（[`create`] / [`create_new`] は開けたファイルを返す）。
//! IPC のトークン（`load_or_create_token`）が Err になると GUI は IPC / MCP ごと立たず、
//! control.json が書けないと CLI が GUI を見つけられない（#2581）ので、ACL が無いだけの置き場で
//! それを起こさない。**無いもの**は unix の chmod と同じく Err（`NotFound`）。unix の腕の失敗は
//! これまでどおりそのまま返る（呼び手の `?` も修正前と同じ）。効いているかは [`inspect`] で読み戻せる
//! （Windows の CI が Win32 と icacls の 2 通りで確かめる = `remote::tests::issue1632_*` / `issue2581_*`）。
//!
//! Windows の腕は、既に同じ DACL（継承を切った本人 + SYSTEM の 2 つだけ）が当たっていれば当て直さない。
//! 置き場へ当てるたびに `SetNamedSecurityInfoW` が中身全体へ受け継ぎを配り直すので、data dir
//! （control.json の置き場 = GUI の起動ごとに呼ぶ）では 2 回目以降をその手間なしで済ませる（#2581）。
//!
//! 通るもの: remote の秘密（#1632）・IPC の接続情報 control.json と `instances/`・IPC / MCP の常設
//! トークン `token`・usage の鍵の塩 `machine_id`・`remote/remote_shortcuts.json`・スマホから
//! 受け取ったファイル・ペインの最後の出力の置き場（#2581）。unix の腕の chmod / mode は、それぞれが
//! 前に直に当てていたものと同じ（置き場 0700・ファイル 0600。仕上がりの mode は修正前と一致）。
//! control.json・トークン・受け取ったファイルは作る時点から 0600 で開く（前は書き終えてから
//! 0600 にしていたので、その間の隙が無くなった）。A/B は `TAKO_1632_LEGACY=1`（Windows は DACL に触らない =
//! #1632 / #2581 前。unix は変わらない）。番犬 `crates/tako-control/tests/issue1632_private_file_watchdog.rs` が、
//! Windows の腕が no-op へ戻ること・秘密の書き手が境界を通らずに書くこと・境界の外で本人だけの
//! mode を当てる新しい書き手を `file:line` で落とす。

use std::fs::File;
use std::io;
use std::path::Path;

/// A/B の旧経路（`TAKO_1632_LEGACY=1`）。Windows は DACL に触らない（#1632 前）。unix は変わらない
pub const ENV_LEGACY: &str = "TAKO_1632_LEGACY";

/// 読み戻した結果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    /// 本人（Windows はそれに加えて SYSTEM）のほかに誰も触れないか
    pub owner_only: bool,
    /// 親の ACL を受け継ぎうるか（Windows で DACL が継承を切っていない。unix は概念が無いので常に偽）
    pub inherits: bool,
    /// 読み戻した形（unix は `0600` などの 8 進、Windows は DACL の SDDL）
    pub detail: String,
}

/// 置き場を本人だけが入れる形にする（unix は 0700、Windows は継承を切った本人 + SYSTEM の DACL を
/// 中へ受け継がせる形で）
pub fn restrict_dir(dir: &Path) -> io::Result<()> {
    imp::restrict_dir(dir)
}

/// 本人だけが読み書きできる新しいファイルを書き込み用に開く（中身を書く前に絞る）。
/// 既に在れば切り詰める
pub fn create(path: &Path) -> io::Result<File> {
    imp::create(path)
}

/// [`create`] の「無いときだけ作る」版（既に在れば `AlreadyExists`。他のプロセスと競って先に
/// 書いた方を使う呼び手のため = usage の鍵の塩）
pub fn create_new(path: &Path) -> io::Result<File> {
    imp::create_new(path)
}

/// 既に在るファイルを本人だけが読み書きできる形にする
pub fn restrict(path: &Path) -> io::Result<()> {
    imp::restrict(path)
}

/// 今の形を読み戻す
pub fn inspect(path: &Path) -> io::Result<Inspection> {
    imp::inspect(path)
}

/// Windows の腕が組む DACL の種類
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    /// 中に後から作られるもの（ファイル = OI / サブディレクトリ = CI）へ受け継がせる
    Dir,
}

/// 本人 + SYSTEM だけの、継承を切った DACL の SDDL（`D:P` = 保護）
#[cfg_attr(not(windows), allow(dead_code))]
fn owner_only_sddl(user_sid: &str, kind: Kind) -> String {
    let inherit = match kind {
        Kind::File => "",
        Kind::Dir => "OICI",
    };
    format!("D:P(A;{inherit};FA;;;{user_sid})(A;{inherit};FA;;;SY)")
}

/// 読み戻した DACL の SDDL を判定する（純粋関数 = macOS でも検査できる）。
///
/// 許可の ACE の受け手がすべて本人か SYSTEM で、本人への許可が在るときだけ `owner_only`。
/// 拒否の ACE は狭める側なので問わない。NULL DACL（`NO_ACCESS_CONTROL` = 誰でも）は偽
#[cfg_attr(not(windows), allow(dead_code))]
fn judge_sddl(sddl: &str, user_sid: &str) -> Inspection {
    let Some(start) = sddl.find("D:") else {
        return Inspection {
            owner_only: false,
            inherits: true,
            detail: sddl.to_string(),
        };
    };
    let dacl = &sddl[start + 2..];
    let flags_end = dacl.find('(').unwrap_or(dacl.len());
    let flags = &dacl[..flags_end];
    let null_dacl = flags.contains("NO_ACCESS_CONTROL");
    let mut grants_user = false;
    let mut others = false;
    for ace in sddl_aces(&dacl[flags_end..]) {
        let fields: Vec<&str> = ace.split(';').collect();
        let allows = matches!(fields.first().copied(), Some("A" | "OA" | "XA" | "ZA"));
        if !allows {
            continue;
        }
        let trustee = fields.get(5).copied().unwrap_or("");
        if is_user(trustee, user_sid) {
            grants_user = true;
        } else if trustee != "SY" {
            others = true;
        }
    }
    Inspection {
        owner_only: !null_dacl && grants_user && !others,
        inherits: !flags.contains('P'),
        detail: sddl.to_string(),
    }
}

/// ACE の並び（`(…)(…)`）を 1 つずつ切り出す。条件つき ACE は中に括弧を持つので深さで数える
#[cfg_attr(not(windows), allow(dead_code))]
fn sddl_aces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut begin = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => {
                if depth == 0 {
                    begin = i + 1;
                }
                depth += 1;
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    out.push(&s[begin..i]);
                }
            }
            // 次の節（`S:` の SACL など）。頼んでいないので来ないが、来ても DACL に混ぜない
            ':' if depth == 0 => break,
            _ => {}
        }
    }
    out
}

/// SDDL の受け手が本人か。組み込みの Administrator / Guest のアカウントは SID ではなく
/// 別名（`LA` / `LG`）で書き出される
#[cfg_attr(not(windows), allow(dead_code))]
fn is_user(trustee: &str, user_sid: &str) -> bool {
    trustee == user_sid
        || (trustee == "LA" && user_sid.starts_with("S-1-5-21-") && user_sid.ends_with("-500"))
        || (trustee == "LG" && user_sid.starts_with("S-1-5-21-") && user_sid.ends_with("-501"))
}

/// 読み戻した DACL が [`owner_only_sddl`] で当てる形そのもの（継承を切った、本人と SYSTEM への
/// フルアクセスの 2 つだけ・受け継がせ方も同じ）か。そうなら当て直さない（#2581）。
///
/// [`judge_sddl`] より厳しい: 本人だけでも権利が欠けている・受け継がせ方が違う・ACE が多いなら偽
/// （= 当て直して揃える）
#[cfg_attr(not(windows), allow(dead_code))]
fn already_applied(sddl: &str, user_sid: &str, kind: Kind) -> bool {
    let Some(start) = sddl.find("D:") else {
        return false;
    };
    let dacl = &sddl[start + 2..];
    let flags_end = dacl.find('(').unwrap_or(dacl.len());
    let flags = &dacl[..flags_end];
    if !flags.contains('P') || flags.contains("NO_ACCESS_CONTROL") {
        return false;
    }
    let inherit = match kind {
        Kind::File => "",
        Kind::Dir => "OICI",
    };
    let aces = sddl_aces(&dacl[flags_end..]);
    let trustees: Vec<&str> = aces
        .iter()
        .filter_map(|ace| full_access_trustee(ace, inherit))
        .collect();
    aces.len() == 2
        && trustees.len() == 2
        && trustees.iter().any(|t| is_user(t, user_sid))
        && trustees.contains(&"SY")
}

/// `A;<inherit>;FA;;;<受け手>` の形の ACE なら受け手を返す
#[cfg_attr(not(windows), allow(dead_code))]
fn full_access_trustee<'a>(ace: &'a str, inherit: &str) -> Option<&'a str> {
    let fields: Vec<&'a str> = ace.split(';').collect();
    (fields.len() == 6
        && fields[0] == "A"
        && fields[1] == inherit
        && fields[2] == "FA"
        && fields[3].is_empty()
        && fields[4].is_empty())
    .then(|| fields[5])
}

#[cfg(unix)]
mod imp {
    use super::Inspection;
    use std::fs::File;
    use std::io;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    pub fn restrict_dir(dir: &Path) -> io::Result<()> {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }

    pub fn create(path: &Path) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
    }

    pub fn create_new(path: &Path) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
    }

    pub fn restrict(path: &Path) -> io::Result<()> {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }

    pub fn inspect(path: &Path) -> io::Result<Inspection> {
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o7777;
        Ok(Inspection {
            owner_only: mode & 0o077 == 0,
            inherits: false,
            detail: format!("{mode:04o}"),
        })
    }
}

#[cfg(windows)]
mod imp {
    use super::{Inspection, Kind};
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::null_mut;
    use std::sync::OnceLock;

    // `windows-sys` を足さず必要な関数だけ宣言する方針は `platform::install_info` / `user_path` と同じ
    const SE_FILE_OBJECT: u32 = 1;
    const DACL_SECURITY_INFORMATION: u32 = 0x0000_0004;
    const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
    const SDDL_REVISION_1: u32 = 1;
    const TOKEN_QUERY: u32 = 0x0008;
    /// `TOKEN_INFORMATION_CLASS::TokenUser`
    const TOKEN_USER: u32 = 1;
    const ERROR_SUCCESS: u32 = 0;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> i32;
        fn GetTokenInformation(
            token: *mut c_void,
            class: u32,
            info: *mut c_void,
            len: u32,
            ret_len: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: *mut c_void, out: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl: *const u16,
            revision: u32,
            sd: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd: *mut c_void,
            revision: u32,
            info: u32,
            out: *mut *mut u16,
            len: *mut u32,
        ) -> i32;
        fn GetSecurityDescriptorDacl(
            sd: *mut c_void,
            present: *mut i32,
            dacl: *mut *mut c_void,
            defaulted: *mut i32,
        ) -> i32;
        fn SetNamedSecurityInfoW(
            name: *const u16,
            object_type: u32,
            info: u32,
            owner: *mut c_void,
            group: *mut c_void,
            dacl: *mut c_void,
            sacl: *mut c_void,
        ) -> u32;
        fn GetNamedSecurityInfoW(
            name: *const u16,
            object_type: u32,
            info: u32,
            owner: *mut *mut c_void,
            group: *mut *mut c_void,
            dacl: *mut *mut c_void,
            sacl: *mut *mut c_void,
            sd: *mut *mut c_void,
        ) -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    pub fn restrict_dir(dir: &Path) -> io::Result<()> {
        keep_going(apply(dir, Kind::Dir))
    }

    pub fn create(path: &Path) -> io::Result<File> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        // 呼び手はこの後で中身を書く。絞れなかったら修正前と同じ継承へ落ちる（モジュール冒頭）
        let _ = apply(path, Kind::File);
        Ok(file)
    }

    pub fn create_new(path: &Path) -> io::Result<File> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        // 呼び手はこの後で中身を書く。絞れなかったら修正前と同じ継承へ落ちる（モジュール冒頭）
        let _ = apply(path, Kind::File);
        Ok(file)
    }

    pub fn restrict(path: &Path) -> io::Result<()> {
        keep_going(apply(path, Kind::File))
    }

    /// 在るものに当てられなかったときは修正前の継承のまま続ける（モジュール冒頭）。
    /// 無いものは unix の chmod と同じく Err を返す
    fn keep_going(applied: io::Result<()>) -> io::Result<()> {
        match applied {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn inspect(path: &Path) -> io::Result<Inspection> {
        Ok(super::judge_sddl(&read_sddl(path)?, &user_sid()?))
    }

    /// 今の DACL を SDDL の文字列で読む
    fn read_sddl(path: &Path) -> io::Result<String> {
        let name = wide_path(path);
        let mut dacl: *mut c_void = null_mut();
        let mut sd: *mut c_void = null_mut();
        // SAFETY: name は NUL 終端の生存するバッファ、出力ポインタはスタック上の変数。
        // 所有者・グループ・SACL は頼まないので null を渡す（規定の呼び方）
        let rc = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut sd,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let mut text: *mut u16 = null_mut();
        // SAFETY: sd は直前の呼び出しが返した記述子。長さの出力は任意なので null
        let ok = unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                sd,
                SDDL_REVISION_1,
                DACL_SECURITY_INFORMATION,
                &mut text,
                null_mut(),
            )
        };
        let sddl = if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: 成功時の text は LocalAlloc された NUL 終端の文字列
            Ok(unsafe { take_local_wide(text) })
        };
        // SAFETY: GetNamedSecurityInfoW が LocalAlloc した記述子を 1 度だけ解放する（dacl はその内側）
        unsafe { LocalFree(sd) };
        sddl
    }

    /// 継承を切った本人 + SYSTEM だけの DACL を当てる（`TAKO_1632_LEGACY=1` では触らない）
    fn apply(path: &Path, kind: Kind) -> io::Result<()> {
        if std::env::var(super::ENV_LEGACY).is_ok_and(|v| v == "1") {
            return Ok(());
        }
        let user_sid = user_sid()?;
        // 既に同じ形なら当て直さない（置き場は当てるたびに中身全体へ受け継ぎを配り直す。#2581）
        if read_sddl(path).is_ok_and(|now| super::already_applied(&now, &user_sid, kind)) {
            return Ok(());
        }
        let sddl = wide(&super::owner_only_sddl(&user_sid, kind));
        let mut sd: *mut c_void = null_mut();
        // SAFETY: sddl は NUL 終端の生存するバッファ、sd はスタック上の変数。大きさの出力は任意
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = set_dacl_of(path, sd);
        // SAFETY: 変換が LocalAlloc した記述子を 1 度だけ解放する
        unsafe { LocalFree(sd) };
        result
    }

    fn set_dacl_of(path: &Path, sd: *mut c_void) -> io::Result<()> {
        let mut present: i32 = 0;
        let mut dacl: *mut c_void = null_mut();
        let mut defaulted: i32 = 0;
        // SAFETY: sd は生存する記述子、出力ポインタはスタック上の変数
        let ok = unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        if present == 0 || dacl.is_null() {
            return Err(io::Error::other("DACL を組めなかった"));
        }
        let name = wide_path(path);
        // SAFETY: name は NUL 終端の生存するバッファ、dacl は sd の内側で sd はまだ解放していない。
        // 所有者・グループ・SACL は変えないので null
        let rc = unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null_mut(),
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        Ok(())
    }

    /// このプロセスのユーザーの SID（`S-1-5-21-…`）。プロセスの間は変わらないので 1 度だけ引く
    fn user_sid() -> io::Result<String> {
        static CACHE: OnceLock<String> = OnceLock::new();
        if let Some(sid) = CACHE.get() {
            return Ok(sid.clone());
        }
        let sid = query_user_sid()?;
        Ok(CACHE.get_or_init(|| sid).clone())
    }

    fn query_user_sid() -> io::Result<String> {
        let mut token: *mut c_void = null_mut();
        // SAFETY: GetCurrentProcess は閉じなくてよい疑似ハンドル、token はスタック上の変数
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let result = token_user_sid(token);
        // SAFETY: OpenProcessToken が成功したハンドルだけを 1 度閉じる
        unsafe { CloseHandle(token) };
        result
    }

    fn token_user_sid(token: *mut c_void) -> io::Result<String> {
        let mut len: u32 = 0;
        // SAFETY: 大きさの問い合わせ（バッファ null・長さ 0 は規定の呼び方。失敗して len に要る大きさが入る）
        unsafe { GetTokenInformation(token, TOKEN_USER, null_mut(), 0, &mut len) };
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        // TOKEN_USER の先頭はポインタなので 8 バイト境界に揃えた領域を取る
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: buf は len バイト以上を確保済み
        let ok = unsafe {
            GetTokenInformation(
                token,
                TOKEN_USER,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                &mut len,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // TOKEN_USER = { User: SID_AND_ATTRIBUTES { Sid: PSID, Attributes: u32 } }。SID の実体は buf の内側
        // SAFETY: 成功した GetTokenInformation が先頭に SID へのポインタを書いている
        let sid = unsafe { *(buf.as_ptr() as *const *mut c_void) };
        let mut text: *mut u16 = null_mut();
        // SAFETY: sid は buf の内側を指す有効な SID、text はスタック上の変数
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: 成功時の text は LocalAlloc された NUL 終端の文字列
        Ok(unsafe { take_local_wide(text) })
    }

    /// LocalAlloc された NUL 終端の UTF-16 を String にして解放する
    ///
    /// # Safety
    /// `p` は LocalAlloc された NUL 終端の UTF-16 で、以後どこからも使わないこと
    unsafe fn take_local_wide(p: *mut u16) -> String {
        let mut n = 0usize;
        // SAFETY: 呼び手の約束（NUL 終端）
        while unsafe { *p.add(n) } != 0 {
            n += 1;
        }
        // SAFETY: p から n 要素は NUL より手前の初期化済みの領域
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) });
        // SAFETY: 呼び手の約束（LocalAlloc された領域を 1 度だけ解放する）
        unsafe { LocalFree(p.cast()) };
        s
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "S-1-5-21-111-222-333-1001";

    #[test]
    fn 本人とsystemだけの継承を切ったsddlを組む() {
        assert_eq!(
            owner_only_sddl(USER, Kind::File),
            "D:P(A;;FA;;;S-1-5-21-111-222-333-1001)(A;;FA;;;SY)"
        );
        // 置き場は中に後から作られるもの（ファイル / サブディレクトリ）へ受け継がせる
        assert_eq!(
            owner_only_sddl(USER, Kind::Dir),
            "D:P(A;OICI;FA;;;S-1-5-21-111-222-333-1001)(A;OICI;FA;;;SY)"
        );
    }

    #[test]
    fn 組んだsddlは判定で本人だけと読める() {
        for kind in [Kind::File, Kind::Dir] {
            let got = judge_sddl(&owner_only_sddl(USER, kind), USER);
            assert!(got.owner_only && !got.inherits, "{kind:?}: {got:?}");
        }
    }

    #[test]
    fn 書き出しの形の違いがあっても本人だけと読める() {
        // SetNamedSecurityInfoW の後は AI（自動継承）の印が付いて返る
        let got = judge_sddl(
            "D:PAI(A;OICI;FA;;;S-1-5-21-111-222-333-1001)(A;OICI;FA;;;SY)",
            USER,
        );
        assert!(got.owner_only && !got.inherits, "{got:?}");
        // 継承を切った置き場の中に作られたファイル = 受け継いだ 2 者だけ
        let got = judge_sddl(
            "D:AI(A;ID;FA;;;S-1-5-21-111-222-333-1001)(A;ID;FA;;;SY)",
            USER,
        );
        assert!(got.owner_only && got.inherits, "{got:?}");
        // 拒否の ACE は狭める側なので問わない
        let got = judge_sddl(
            "D:P(D;;FA;;;WD)(A;;FA;;;S-1-5-21-111-222-333-1001)(A;;FA;;;SY)",
            USER,
        );
        assert!(got.owner_only, "{got:?}");
        // 組み込みの Administrator アカウント（RID 500）は別名で書き出される
        let got = judge_sddl("D:P(A;;FA;;;LA)(A;;FA;;;SY)", "S-1-5-21-111-222-333-500");
        assert!(got.owner_only, "{got:?}");
    }

    #[test]
    fn 他者が読める形は本人だけと読まない() {
        let cases = [
            // #1632 前の Windows: %APPDATA% から受け継いだ Administrators
            "D:AI(A;ID;FA;;;SY)(A;ID;FA;;;BA)(A;ID;FA;;;S-1-5-21-111-222-333-1001)",
            // 明示の Everyone の読み取りが残っている
            "D:P(A;;FA;;;S-1-5-21-111-222-333-1001)(A;;FA;;;SY)(A;;FR;;;WD)",
            // 別のユーザー
            "D:P(A;;FA;;;S-1-5-21-111-222-333-1002)(A;;FA;;;SY)",
            // 本人への許可が無い（SYSTEM だけ）
            "D:P(A;;FA;;;SY)",
            // NULL DACL = 誰でも
            "D:NO_ACCESS_CONTROL",
            // DACL の節が無い
            "",
            // 条件つき ACE の受け手が他者（中の括弧で切り出しが崩れない）
            "D:P(A;;FA;;;S-1-5-21-111-222-333-1001)(XA;;FR;;;WD;(Member_of {SID(BA)}))",
            // 作成者の所有者（中へ受け継がせると作った人へ渡る）
            "D:P(A;OICI;FA;;;S-1-5-21-111-222-333-1001)(A;OICIIO;FA;;;CO)",
        ];
        for sddl in cases {
            let got = judge_sddl(sddl, USER);
            assert!(
                !got.owner_only,
                "本人だけと読んではいけない: {sddl} → {got:?}"
            );
        }
        // 別名の取り違え: RID 500 でない本人に LA を本人と読まない
        assert!(!judge_sddl("D:P(A;;FA;;;LA)(A;;FA;;;SY)", USER).owner_only);
    }

    #[test]
    fn 当てる形そのものだけを当て済みと読む() {
        // 組んだ形・当てた後の書き出しの形（AI の印・別名）は当て済み
        assert!(already_applied(
            &owner_only_sddl(USER, Kind::File),
            USER,
            Kind::File
        ));
        assert!(already_applied(
            "D:PAI(A;OICI;FA;;;SY)(A;OICI;FA;;;S-1-5-21-111-222-333-1001)",
            USER,
            Kind::Dir
        ));
        assert!(already_applied(
            "D:PAI(A;;FA;;;LA)(A;;FA;;;SY)",
            "S-1-5-21-111-222-333-500",
            Kind::File
        ));
        let cases = [
            // 受け継がせ方が違う（ファイルの形を置き場に・その逆）
            (owner_only_sddl(USER, Kind::File), Kind::Dir),
            (owner_only_sddl(USER, Kind::Dir), Kind::File),
            // 継承を切っていない（置き場の中で受け継いだだけ）
            (
                "D:AI(A;ID;FA;;;S-1-5-21-111-222-333-1001)(A;ID;FA;;;SY)".into(),
                Kind::File,
            ),
            // 本人だけでも権利が欠けている
            (
                "D:P(A;;FR;;;S-1-5-21-111-222-333-1001)(A;;FA;;;SY)".into(),
                Kind::File,
            ),
            // ACE が多い（拒否でも揃え直す）
            (
                "D:P(D;;FA;;;WD)(A;;FA;;;S-1-5-21-111-222-333-1001)(A;;FA;;;SY)".into(),
                Kind::File,
            ),
            // SYSTEM が無い・本人が無い
            (
                "D:P(A;;FA;;;S-1-5-21-111-222-333-1001)(A;;FA;;;S-1-5-21-111-222-333-1001)".into(),
                Kind::File,
            ),
            ("D:P(A;;FA;;;SY)(A;;FA;;;SY)".into(), Kind::File),
            ("D:NO_ACCESS_CONTROL".into(), Kind::File),
            (String::new(), Kind::File),
        ];
        for (sddl, kind) in cases {
            assert!(
                !already_applied(&sddl, USER, kind),
                "当て済みと読んではいけない: {sddl} ({kind:?})"
            );
        }
    }

    /// 使い捨ての置き場（スコープで消える = #1312）
    fn scratch(name: &str) -> crate::test_residue::ScratchDir {
        crate::test_residue::ScratchDir::new(&format!("1632-{name}"))
    }

    /// 実ファイルで作る・絞る・読み戻す（Windows の CI では実 DACL を通る）
    #[test]
    fn 作ったファイルと絞った置き場は本人だけと読み戻せる() {
        let held = scratch("fs");
        let dir = held.path().to_path_buf();
        restrict_dir(&dir).expect("置き場を絞れる");
        let made = dir.join("made.txt");
        {
            use std::io::Write;
            let mut f = create(&made).expect("作れる");
            f.write_all(b"secret").expect("書ける");
        }
        assert_eq!(std::fs::read_to_string(&made).unwrap(), "secret");
        for (label, path) in [("dir", &dir), ("made", &made)] {
            let got = inspect(path).expect("読み戻せる");
            println!(
                "TAKO1632_CORE {label} owner_only={} inherits={} {}",
                got.owner_only, got.inherits, got.detail
            );
            assert!(got.owner_only && !got.inherits, "{label}: {got:?}");
        }
    }

    /// 既に在る緩いファイルを絞ると、緩さが残らない
    #[test]
    fn 既に在る緩いファイルを絞ると本人だけになる() {
        let dir = scratch("loose");
        let loose = dir.join("loose.txt");
        std::fs::write(&loose, "x").unwrap();
        restrict(&loose).expect("絞れる");
        let got = inspect(&loose).expect("読み戻せる");
        println!(
            "TAKO1632_CORE loose owner_only={} inherits={} {}",
            got.owner_only, got.inherits, got.detail
        );
        assert!(got.owner_only && !got.inherits, "{got:?}");
    }

    #[test]
    fn 無いものは読み戻しも絞りもエラー() {
        let dir = scratch("missing");
        let missing = dir.join("missing.txt");
        assert!(inspect(&missing).is_err());
        assert!(restrict(&missing).is_err());
        assert!(restrict_dir(&dir.join("missing-dir")).is_err());
        assert!(create_new(&dir.join("no-parent").join("x")).is_err());
    }

    /// 無いときだけ作る口: 作ったものは本人だけ・在れば作らない（#2581。usage の鍵の塩）
    #[test]
    fn 無いときだけ作る口は本人だけで作り在れば断る() {
        let dir = scratch("create-new");
        let fresh = dir.join("salt");
        {
            use std::io::Write;
            let mut f = create_new(&fresh).expect("無いので作れる");
            f.write_all(b"salt").expect("書ける");
        }
        let got = inspect(&fresh).expect("読み戻せる");
        println!(
            "TAKO2581_CORE create_new owner_only={} inherits={} {}",
            got.owner_only, got.inherits, got.detail
        );
        assert!(got.owner_only && !got.inherits, "{got:?}");
        let again = create_new(&fresh).expect_err("在るので作らない");
        assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_to_string(&fresh).unwrap(),
            "salt",
            "中身は残る"
        );
    }

    /// 中身の多い置き場を絞る: 中に在ったものも本人だけを受け継ぎ、2 回目は形が変わらない（#2581。
    /// control.json の置き場 = data dir は GUI の起動ごとに絞る）。配り直しにかかった時間を CI の記録に出す
    #[test]
    fn 中身の多い置き場を絞ると中も受け継ぎ2回目は形が変わらない() {
        let held = scratch("many");
        let dir = held.path().to_path_buf();
        let mut files = 0usize;
        for d in 0..20 {
            let sub = dir.join(format!("d{d}"));
            std::fs::create_dir_all(&sub).unwrap();
            for f in 0..50 {
                std::fs::write(sub.join(format!("f{f}.txt")), "x").unwrap();
                files += 1;
            }
        }
        let child = dir.join("d7").join("f9.txt");
        let before = inspect(&child).expect("読み戻せる");
        let first = std::time::Instant::now();
        restrict_dir(&dir).expect("絞れる");
        let first_ms = first.elapsed().as_millis();
        let shaped = inspect(&dir).expect("読み戻せる");
        let again = std::time::Instant::now();
        restrict_dir(&dir).expect("2 回目も通る");
        let again_ms = again.elapsed().as_millis();
        let reshaped = inspect(&dir).expect("読み戻せる");
        let inner = inspect(&child).expect("読み戻せる");
        println!(
            "TAKO2581_CORE many files={files} first_ms={first_ms} again_ms={again_ms} dir={} child_before={} child_after owner_only={} inherits={} {}",
            shaped.detail, before.detail, inner.owner_only, inner.inherits, inner.detail
        );
        assert!(shaped.owner_only && !shaped.inherits, "{shaped:?}");
        assert_eq!(shaped.detail, reshaped.detail, "2 回目で形が変わった");
        if cfg!(windows) {
            // Windows は置き場の ACL が中を守らない（辿る権利の検査が無い）ので、中身へ配り直す
            assert!(inner.owner_only && inner.inherits, "{inner:?}");
        } else {
            // unix は置き場の 0700 が中へ辿らせない。中のファイルの mode は変えない（修正前と同じ）
            assert_eq!(before.detail, inner.detail);
        }
    }
}
