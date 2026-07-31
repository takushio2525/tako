//! #697: 蓋閉じ継続の**実機経路**を通しで確認する統合テスト。
//!
//! 単体テスト（`platform::lid` の中）は純粋関数と低レベル API 単発を見ているが、
//! ここでは「倒す → 記録が残る → tako が落ちる → 次回起動で元へ戻る」という
//! **安全性の肝**を実際の電源プランに対して通す。
//!
//! 統合テストは 1 ファイル 1 プロセスなので、`TAKO_DATA_DIR` を差し替えても
//! 他のテストへ漏れない（単体テスト側で環境変数を触るとレースになる）。
//!
//! **本番の記録ファイルには触らない**が、電源プランの値そのものは機械全体で共有なので、
//! どの経路で失敗しても最後に必ず元へ戻す。
#![cfg(windows)]

use tako_control::platform::lid;

/// 実行中に電源プランの蓋設定を読むための最小 FFI。
/// 製品コードの外から**独立に**観測することで、`lid` 自身の実装が正しいかを確かめる
mod probe {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const SUB_BUTTONS: Guid = Guid {
        data1: 0x4f97_1e89,
        data2: 0xeebd,
        data3: 0x4455,
        data4: [0xa8, 0xde, 0x9e, 0x59, 0x04, 0x0e, 0x73, 0x47],
    };
    const LIDCLOSE_ACTION: Guid = Guid {
        data1: 0x5ca8_3367,
        data2: 0x6e45,
        data3: 0x459f,
        data4: [0xa2, 0x7b, 0x47, 0x6b, 0x1d, 0x01, 0xc9, 0x36],
    };

    #[link(name = "powrprof")]
    extern "system" {
        fn PowerGetActiveScheme(user_root: *mut c_void, scheme: *mut *mut Guid) -> u32;
        fn PowerReadACValueIndex(
            root: *mut c_void,
            scheme: *const Guid,
            subgroup: *const Guid,
            setting: *const Guid,
            value: *mut u32,
        ) -> u32;
        fn PowerWriteACValueIndex(
            root: *mut c_void,
            scheme: *const Guid,
            subgroup: *const Guid,
            setting: *const Guid,
            value: u32,
        ) -> u32;
        fn PowerSetActiveScheme(user_root: *mut c_void, scheme: *const Guid) -> u32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(mem: *mut c_void) -> *mut c_void;
    }

    fn scheme() -> Guid {
        let mut ptr: *mut Guid = std::ptr::null_mut();
        // SAFETY: 出力先はスタック上のポインタ。成功時のみ読む
        let rc = unsafe { PowerGetActiveScheme(std::ptr::null_mut(), &mut ptr) };
        assert_eq!(rc, 0, "アクティブな電源プランが取れる");
        assert!(!ptr.is_null());
        // SAFETY: 直前に非 NULL を確認済み。API 仕様どおり LocalFree で解放する
        let g = unsafe { *ptr };
        // SAFETY: PowerGetActiveScheme が確保したバッファ
        unsafe { LocalFree(ptr as *mut c_void) };
        g
    }

    /// AC 側の蓋設定を読む（0 = 何もしない / 1 = スリープ / 2 = 休止 / 3 = シャットダウン）
    pub fn read_ac() -> u32 {
        let s = scheme();
        let mut v = 0u32;
        // SAFETY: 入力は生存する参照、出力先はスタック上の u32
        let rc = unsafe {
            PowerReadACValueIndex(
                std::ptr::null_mut(),
                &s,
                &SUB_BUTTONS,
                &LIDCLOSE_ACTION,
                &mut v,
            )
        };
        assert_eq!(rc, 0, "蓋設定が読める");
        v
    }

    /// 後始末専用。テストがどこで失敗しても既知の値へ戻せるようにする
    pub fn force_write_ac(value: u32) {
        let s = scheme();
        // SAFETY: 入力は生存する参照
        unsafe {
            PowerWriteACValueIndex(
                std::ptr::null_mut(),
                &s,
                &SUB_BUTTONS,
                &LIDCLOSE_ACTION,
                value,
            );
            PowerSetActiveScheme(std::ptr::null_mut(), &s);
        }
    }
}

/// この 3 本は **プロセス共有の `TAKO_DATA_DIR`** と **機械全体で 1 つの電源プラン**を
/// 触るので、並列に走らせるとお互いの後始末を踏む（`cargo test` は既定で並列）。
/// 各テストの先頭でこの錠を取って直列化する
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 直列化の錠を取る。**`Restore` より先に宣言する**こと
/// （変数は宣言と逆順に落ちるので、錠が最後に外れて後始末まで保護される）
fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 失敗しても必ず元の蓋設定へ戻すための番人
struct Restore(u32);
impl Drop for Restore {
    fn drop(&mut self) {
        probe::force_write_ac(self.0);
        assert_eq!(
            probe::read_ac(),
            self.0,
            "後始末: 蓋設定を元へ戻せていない（この機械はいま蓋を閉じてもスリープしないかもしれない）"
        );
    }
}

/// 受け入れ条件 1・2: 倒れること、そして落ちても次回起動で戻ること
#[test]
fn 倒したあと落ちても次回起動の残留復元で元へ戻る() {
    let _serial = serial();
    let dir = std::env::temp_dir().join(format!("tako-lid-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
    // SAFETY: 統合テストは 1 ファイル 1 プロセス。この差し替えは他テストへ漏れない
    unsafe { std::env::set_var("TAKO_DATA_DIR", &dir) };

    let original = probe::read_ac();
    let _guard = Restore(original);
    let record = dir.join("lid-guard.json");

    // --- 倒す ---
    let changed = lid::set_stay_awake(true, false).expect("倒せる");
    assert!(changed, "状態が変わったと報告される");
    assert!(lid::is_active(), "保持中として見える");
    assert_eq!(
        probe::read_ac(),
        0,
        "電源プランの蓋設定が「何もしない」になっている"
    );
    assert!(record.exists(), "元値の記録がディスクに置かれている");

    // 同じ要求は何もしない（毎 tick 呼ばれる前提の冪等性）
    assert_eq!(
        lid::set_stay_awake(true, false),
        Ok(false),
        "再要求は no-op"
    );

    // --- tako がここで落ちたとみなす（記録を残したまま）---
    // 次回起動の残留復元。他インスタンスなし・隔離モードでない前提
    let msg = lid::clear_residual(false, false).expect("復元が走る");
    assert!(msg.is_some(), "復元したことが報告される: {msg:?}");
    assert_eq!(probe::read_ac(), original, "蓋設定が元値へ戻っている");
    assert!(!record.exists(), "記録が破棄されている");
    assert!(!lid::is_active(), "保持していない状態に戻る");

    // SAFETY: 上で設定した変数を戻す
    unsafe { std::env::remove_var("TAKO_DATA_DIR") };
    let _ = std::fs::remove_dir_all(&dir);
}

/// 正常終了では**その場で**元へ戻すこと（起動時の残留復元は最後の砦であって一次手段ではない）。
///
/// 蓋の設定は電源プランに書かれる永続設定なので、電源要求と違って OS が回収してくれない。
/// ここが抜けると「tako を終了したら蓋を閉じてもスリープしない PC」が出来上がる
#[test]
fn 正常終了で蓋設定が元へ戻る() {
    let _serial = serial();
    let dir = std::env::temp_dir().join(format!("tako-lid-exit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
    // SAFETY: 統合テストは 1 ファイル 1 プロセス
    unsafe { std::env::set_var("TAKO_DATA_DIR", &dir) };

    let original = probe::read_ac();
    let _guard = Restore(original);

    lid::set_stay_awake(true, false).expect("倒せる");
    assert_eq!(probe::read_ac(), 0, "倒れている");

    // アプリの終了フック（Cmd+Q / Dock 終了 / OS シャットダウンのどれでもここを通る）
    tako_control::sleep_guard::cleanup_on_exit();

    assert_eq!(
        probe::read_ac(),
        original,
        "終了時に蓋設定が元へ戻っている（次回起動を待たない）"
    );
    assert!(!dir.join("lid-guard.json").exists(), "記録も破棄されている");

    // SAFETY: 同上
    unsafe { std::env::remove_var("TAKO_DATA_DIR") };
    let _ = std::fs::remove_dir_all(&dir);
}

/// 受け入れ条件 3: ユーザーが自分で設定を変えていたら、解除で勝手に戻さない
#[test]
fn ユーザーが蓋設定を変えていたら解除で上書きしない() {
    let _serial = serial();
    let dir = std::env::temp_dir().join(format!("tako-lid-user-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("一時ディレクトリを作れる");
    // SAFETY: 同上
    unsafe { std::env::set_var("TAKO_DATA_DIR", &dir) };

    let original = probe::read_ac();
    let _guard = Restore(original);

    lid::set_stay_awake(true, false).expect("倒せる");
    assert_eq!(probe::read_ac(), 0);

    // ユーザーが設定画面で「蓋を閉じたら休止状態」へ変えた、という状況を作る
    const HIBERNATE: u32 = 2;
    probe::force_write_ac(HIBERNATE);

    // tako の解除では触らない（ユーザーの選択が勝つ）
    lid::set_stay_awake(false, false).expect("解除は成功扱い");
    assert_eq!(
        probe::read_ac(),
        HIBERNATE,
        "ユーザーが変えた値を tako が巻き戻してはいけない"
    );

    // SAFETY: 同上
    unsafe { std::env::remove_var("TAKO_DATA_DIR") };
    let _ = std::fs::remove_dir_all(&dir);
}
