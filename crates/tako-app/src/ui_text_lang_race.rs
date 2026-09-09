//! #1274 の競合そのものを再現するテスト
//!
//! 表示言語は `tako_core::i18n` の**プロセス全体の AtomicU8**。`ui_text` の相対比較
//! テストはロックを取らずに言語依存の文字列を 2 回読んでいたので、**読み取りの
//! あいだに別スレッドのテストが言語を切り替える**と別言語同士を比べて落ちていた。
//!
//! ここには 3 本置く:
//!
//! 1. `相対比較の2点のあいだで言語が変わると別言語を比べる` — 競合の**注入**。
//!    2 点のあいだで実際に言語を切り替え、旧形の `assert_eq!` が成立しないことを
//!    決定的に（時間に依らず）示す
//! 2. `言語を切り替えるスレッドと並行しても相対比較は破れない` — **実競合**。
//!    別スレッドが言語を切り替え続ける横で、修正後の形（ヘルパの区間の中）なら
//!    破れないこと。`TAKO_1274_LEGACY=1` で修正前の形（ロック外の比較）へ切り替わり、
//!    そのときは落ちる（A/B）
//! 3. `ヘルパの区間では別スレッドが言語を切り替えられない` — **機構の証明**。
//!    切替が同じロックを通る以上、区間の中で言語が動かないことはロックが保証する
//!
//! このファイルが `ui_text/` の**外**にあるのは、1 と 2 の legacy 経路が
//! 「ロック外で言語依存の関数を読む」形そのものだから。`ui_text/lang_watchdog.rs`
//! はその形を落とす番犬なので、再現側を同じ走査対象へ置くと自分自身に噛みつく。
//! 宣言は `ui_text/mod.rs` から `#[path]` で行う（`main.rs` へ `#[cfg(test)]` を
//! 足すと、そこを本番コードの終端とみなす別の番犬が空振りする）。

use super::{path_menu, sidebar, tests_support};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use tako_control::platform::os_integration::FileManager;
use tako_core::i18n::{self, Lang};

/// 修正前の `ui_text::path_menu::tests::共通項目はファイルツリーと同一文言` と
/// **同じ並び**の比較。呼び出し側がロックを握っていなければ、2 点の読み取りの
/// あいだに言語が変わって別言語同士を比べる
fn 共通項目を比べる() {
    assert_eq!(path_menu::open_default(), sidebar::menu_open_default());
    assert_eq!(path_menu::open_with(), sidebar::menu_open_with());
    assert_eq!(path_menu::copy_rel(), sidebar::menu_copy_rel());
    assert_eq!(path_menu::copy_abs(), sidebar::menu_copy_abs());
    for fm in [FileManager::Finder, FileManager::Explorer] {
        assert_eq!(path_menu::reveal(fm), sidebar::menu_reveal(fm));
    }
}

/// 本体が panic しても切替スレッドを止める（回しっぱなしで CPU を焼かない）
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// 競合の注入。**時間に依らず決定的**に、旧形の相対比較が別言語同士の比較に
/// なりうることを示す。ここが `assert_ne!` で通るあいだ、ロック外の相対比較は
/// 「たまたま通っていただけ」である
#[test]
fn 相対比較の2点のあいだで言語が変わると別言語を比べる() {
    let _guard = tests_support::lang_guard();
    i18n::set_lang(Lang::Ja);
    let 一点目 = path_menu::open_default();
    // ここが競合の窓。別スレッドのテスト（check_ja_en 等）が入ると言語が変わる
    i18n::set_lang(Lang::En);
    let 二点目 = sidebar::menu_open_default();
    assert_ne!(
        一点目, 二点目,
        "2 点のあいだで言語を変えたのに同じ文字列だった。\
         日英が同一文言になったなら #1274 の再現条件が消えている（要見直し）"
    );
}

/// 実競合。別スレッドが**他のテストと同じ経路で**言語を切り替え続ける横で、
/// 修正後の形（`for_each_lang` の区間の中で比較する）が破れないことを見る。
///
/// A/B: `TAKO_1274_LEGACY=1` を付けると比較が修正前の形（ロック外）になり、
/// 切替スレッドの割り込みを拾って落ちる
#[test]
fn 言語を切り替えるスレッドと並行しても相対比較は破れない() {
    /// 比較の反復回数。legacy 経路は 1 回あたり 6 か所の窓を持つので、
    /// 割り込みは十分に当たる（実測は Issue #1274 のコメント）
    const 反復: usize = 200_000;

    let legacy = std::env::var("TAKO_1274_LEGACY").is_ok_and(|v| v == "1");
    let stop = Arc::new(AtomicBool::new(false));
    let _stopper = StopOnDrop(stop.clone());
    // 切替スレッドが 1 度も走らないまま本体が終わる（= 検査が空振りする）ことを
    // 時間ではなくバリアで防ぐ。1 往復してから本体を走らせる
    let 準備 = Arc::new(Barrier::new(2));

    let 切替 = {
        let stop = stop.clone();
        let 準備 = 準備.clone();
        std::thread::spawn(move || {
            // 他のカタログテストと同じ経路（= 必ずロックを取る）で Ja / En を往復する
            tests_support::for_each_lang(|| {});
            let mut 回数 = 1u64;
            準備.wait();
            while !stop.load(Ordering::Relaxed) {
                tests_support::for_each_lang(|| {});
                回数 += 1;
            }
            回数
        })
    };

    準備.wait();
    for _ in 0..反復 {
        if legacy {
            共通項目を比べる();
        } else {
            tests_support::for_each_lang(共通項目を比べる);
        }
    }

    stop.store(true, Ordering::Relaxed);
    let 切替回数 = 切替.join().expect("切替スレッドが panic した");
    println!("[#1274] legacy={legacy} 反復={反復} 切替={切替回数}");
    assert!(切替回数 >= 1, "切替スレッドが走っていない");
}

/// 機構の証明。ヘルパの区間を握っているあいだ、**別スレッドは言語を切り替えられない**
/// （切替は同じロックを通るので待たされる）。ここは確率ではなくロックの保証
#[test]
fn ヘルパの区間では別スレッドが言語を切り替えられない() {
    let 門 = Arc::new(Barrier::new(2));
    let 切替 = {
        let 門 = 門.clone();
        std::thread::spawn(move || {
            // 本体がロックを握ってから走り出す
            門.wait();
            // ここでロック待ちになる（本体の区間が終わるまで進めない）
            tests_support::with_lang(Lang::Ja, || {});
        })
    };

    tests_support::with_lang(Lang::En, || {
        門.wait();
        for _ in 0..100_000 {
            assert_eq!(
                i18n::lang(),
                Lang::En,
                "ヘルパの区間の中で言語が切り替わった（ロックが効いていない）"
            );
        }
    });

    切替.join().expect("切替スレッドが panic した");
}
