//! メインスレッドのスタック予約量（抽象境界。#1133）
//!
//! ## なぜ要るか
//!
//! GPUI アプリの debug ビルドは **1 関数のスタックフレームが数百 KiB になる**。
//! `div().child(…)` 形のビルダーは中間値が大量に生まれ、`-O0` では
//! LLVM のスタックスロット再利用（stack coloring）が走らないので、
//! **関数内のローカル全部の合計**がそのままフレームになるためである。
//! main `8a94263` の実測（macOS arm64・プロローグの `sub sp` を読んだ値）:
//!
//! | 関数 | フレーム |
//! |---|---|
//! | `tako_control::mcp::catalog::tools` | 828.4 KiB |
//! | `TakoApp::render_tmux_view` | 564.4 KiB |
//! | `TakoApp::render_git_view` | 482.8 KiB |
//! | `self_test::run` の async ブロック（poll） | 478.7 KiB |
//! | `TakoApp::render_preview_pane` | 326.7 KiB |
//! | `TakoApp::render_status_bar` | 199.6 KiB |
//! | `<TakoApp as Render>::render` | 135.7 KiB |
//!
//! `render` → 右パネル → プレビューのように**入れ子で呼ぶ**ので、実行時の谷は
//! 単独のフレームではなく連鎖の合計になる。macOS / Linux はメインスレッドの
//! スタックが既定 8 MiB なのでこれが収まるが、**Windows/MSVC は既定 1 MiB**
//! （PE ヘッダの `SizeOfStackReserve` = 0x100000）しかない。
//!
//! 実際に #1133 では、隔離 GUI セルフテストが項目 80 で
//! `thread 'main' has overflowed its stack` を出して**プロセスごと落ちた**
//! （判定行も `FAILED` も出ないので、原因が分からない形で死ぬ）。
//! そのときフレームの伸びは 7 commit で **+12 KiB** しかなく、
//! 「どの commit が悪い」ではなく**予約量そのものが足りていなかった**。
//!
//! ## どう直すか
//!
//! 実行ファイルへ「このバイナリはこれだけスタックを使う」と宣言する。
//! MSVC のリンカ引数 `/stack:<予約バイト数>` で、`tako-app` / `tako-cli` の
//! `build.rs` が emit する（写しは番犬
//! `crates/tako-control/tests/windows_stack_reserve_watchdog.rs` が拘束する）。
//! Windows は予約したぶんを**アドレス空間だけ**確保して物理メモリは触った分しか
//! コミットしないので、64bit では 8 MiB の予約に実コストは無い。
//!
//! GPUI 本家（Zed）も同じ理由で同じ値を宣言している（`crates/zed/build.rs`:
//! `println!("cargo:rustc-link-arg=/stack:{}", 8 * 1024 * 1024)` に
//! `todo(windows): This is to avoid stack overflow.` のコメントつき）。
//! tako はこの宣言だけを持っていなかった。
//!
//! ## 二段構え（`dpi` と同じ作法）
//!
//! 1. **macOS でも走る番犬**: `build.rs` の宣言が落ちていないこと
//! 2. **実プロセス**: セルフテストが起動直後に [`shortfall_note`] で実測し、
//!    足りなければ **項目 80 まで行かずに `FAILED` を出す**（沈黙の死を、
//!    原因の書いてある失敗へ替える）
//!
//! 判定は純粋関数なので **macOS 上から Windows 側の数値と文言を検証できる**。

/// リンカへ宣言するメインスレッドのスタック予約量。
///
/// macOS / Linux の既定（8 MiB）へ揃える。実測の谷が 1 MiB 前後なので 8 倍の余裕がある。
/// この値を変えたら `tako-app` / `tako-cli` の `build.rs` も対で直すこと（番犬が落ちる）。
pub const REQUIRED_MAIN_STACK_BYTES: u64 = 8 * 1024 * 1024;

/// これを下回るスタックでは走らせない下限。
///
/// [`REQUIRED_MAIN_STACK_BYTES`] と分けてあるのは、**OS が報告する値は宣言値とは
/// 端数が違う**ため。実測: macOS のメインスレッドは 8 MiB を予約しているのに
/// `pthread_get_stacksize_np` は **8,372,224 B（7.984 MiB）**を返す（ガードページぶん）。
/// 「宣言は 8 MiB / 拒否は 4 MiB 未満」にすれば、既定 1 MiB の Windows は確実に捕まえ、
/// 端数では騒がない。
pub const MIN_MAIN_STACK_BYTES: u64 = 4 * 1024 * 1024;

/// Windows/MSVC のリンカ既定（PE ヘッダの `SizeOfStackReserve` = 0x100000）。
/// #1133 で落ちていたときの実際の値で、文言の説明に使う
pub const WINDOWS_DEFAULT_MAIN_STACK_BYTES: u64 = 1024 * 1024;

/// `build.rs` が emit するリンカ引数。build.rs 側の写しをこの 1 実装が拘束する
pub fn windows_link_arg() -> String {
    format!("/stack:{REQUIRED_MAIN_STACK_BYTES}")
}

/// この予約量では tako を走らせられないか
pub fn is_too_small(reserve: u64) -> bool {
    reserve < MIN_MAIN_STACK_BYTES
}

/// MiB 表記（診断行の見た目を 1 実装に寄せる）
pub fn format_mib(bytes: u64) -> String {
    format!("{:.2}MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// 予約量が足りないときだけ、原因と直し方を書いた 1 行を返す。
///
/// セルフテストの `FAILED` 行にそのまま載る文言。**Issue 番号と実測値を必ず含める**
/// （沈黙の死を置き換える目的なので、ログ 1 行で何をすればいいか分かる必要がある）
pub fn shortfall_note(reserve: u64) -> Option<String> {
    if !is_too_small(reserve) {
        return None;
    }
    Some(format!(
        "メインスレッドのスタック予約が足りない（#1133）: 実測 {} / 下限 {} / 宣言 {}。\
         Windows/MSVC の既定は {} で、GPUI の描画フレーム（実測 135〜564KiB）と\
         セルフテストの poll フレーム（実測 479KiB）を積むと足りずに\
         プロセスごと落ちる。tako-app / tako-cli の build.rs が `{}` を\
         emit しているか確認すること",
        format_mib(reserve),
        format_mib(MIN_MAIN_STACK_BYTES),
        format_mib(REQUIRED_MAIN_STACK_BYTES),
        format_mib(WINDOWS_DEFAULT_MAIN_STACK_BYTES),
        windows_link_arg(),
    ))
}

/// 実行中スレッドのスタック予約量を OS から採る。分からない環境では `None`。
///
/// **呼ぶ場所がメインスレッドでなければ意味が無い**（`cargo test` のテストスレッドは
/// Rust の既定 2 MiB なので、ここの単体テストは「取れるか」までしか見ない）
pub fn current_thread_reserve() -> Option<u64> {
    imp::current_thread_reserve()
}

#[cfg(target_vendor = "apple")]
mod imp {
    pub(super) fn current_thread_reserve() -> Option<u64> {
        // SAFETY: 引数を取らない自スレッドの問い合わせで、返り値はサイズのみ
        let size = unsafe { libc::pthread_get_stacksize_np(libc::pthread_self()) };
        (size > 0).then_some(size as u64)
    }
}

#[cfg(windows)]
mod imp {
    // kernel32 の直宣言。tako-core は `windows` クレートに依存していないので
    // （依存させると macOS のビルドグラフにも載る）、必要な 1 本だけ引く
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    }

    pub(super) fn current_thread_reserve() -> Option<u64> {
        let mut low: usize = 0;
        let mut high: usize = 0;
        // SAFETY: 自スレッドの予約範囲を 2 つの out ポインタへ書くだけの API
        // （Windows 8 以降。tako の下限は 10.0.17763）
        unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
        high.checked_sub(low).filter(|n| *n > 0).map(|n| n as u64)
    }
}

#[cfg(not(any(target_vendor = "apple", windows)))]
mod imp {
    pub(super) fn current_thread_reserve() -> Option<u64> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rsへ渡す宣言は要求量と一致する() {
        assert_eq!(windows_link_arg(), "/stack:8388608");
        assert_eq!(REQUIRED_MAIN_STACK_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn windowsの既定1mibは下限を割る() {
        assert!(is_too_small(WINDOWS_DEFAULT_MAIN_STACK_BYTES));
        assert!(is_too_small(MIN_MAIN_STACK_BYTES - 1));
        assert!(!is_too_small(MIN_MAIN_STACK_BYTES));
    }

    #[test]
    fn macosが報告する端数つき8mibは下限を割らない() {
        // 実測値。宣言（8 MiB）ちょうどを下限にすると macOS が誤検知になる
        assert!(!is_too_small(8_372_224));
        assert!(!is_too_small(REQUIRED_MAIN_STACK_BYTES));
    }

    #[test]
    fn 不足の文言は実測値と直し方を含む() {
        let note = shortfall_note(WINDOWS_DEFAULT_MAIN_STACK_BYTES).expect("不足なら Some");
        assert!(note.contains("#1133"), "Issue 番号が無い: {note}");
        assert!(note.contains("1.00MiB"), "実測値が無い: {note}");
        assert!(note.contains("/stack:8388608"), "直し方が無い: {note}");
        assert!(note.contains("build.rs"), "どこを見るかが無い: {note}");
        assert_eq!(shortfall_note(REQUIRED_MAIN_STACK_BYTES), None);
        assert_eq!(shortfall_note(8_372_224), None);
    }

    #[test]
    fn mib表記は小数2桁() {
        assert_eq!(format_mib(1024 * 1024), "1.00MiB");
        assert_eq!(format_mib(8_372_224), "7.98MiB");
    }

    #[test]
    fn 実行中スレッドの予約量を採れる() {
        // テストスレッドは Rust の既定（2 MiB）なので**量は問わない**。
        // 「OS へ問い合わせられるか」だけを見る（メインスレッドの実測は
        // セルフテストが起動直後に行う）
        if let Some(n) = current_thread_reserve() {
            assert!(n >= 64 * 1024, "予約量が小さすぎる: {n}");
        } else if cfg!(any(target_vendor = "apple", windows)) {
            panic!("macOS / Windows では OS から予約量を採れなければならない");
        }
    }
}
