//! **#1926 の番犬**: App Nap の扱いが「効かない止め方」「寿命の間の握りっぱなし」
//! 「利用者が待つ読み込みを間引かせたまま」のどれにも戻らないようにする。
//!
//! ## 何が起きていたのか
//!
//! 起動時の `sleep_guard::disable_app_nap`（#173）は `defaults write /proc/<pid>/Info
//! NSAppSleepDisabled …` で、macOS には `/proc` が無いので一度も効いていなかった。前面に無い
//! tako は起動から約 30 秒で優先度 46 → 4 に落ち、background の重い処理は E コアへ寄せられる。
//!
//! ## 選んだ形（実測・release・隔離 GUI・Apple M5 Max。数字は `.agent/architecture.md`）
//!
//! - **アプリ全体では止めない**: 止めると誰も待っていない裏の処理まで P コアへ載り、ペインへ流れる
//!   30 万行の出力で 110〜319 → 550〜575 mJ。アイドル時は 341〜365 → 856〜1,154 µW。
//!   エージェント稼働中は #173 のアサーションを tako 自身が握り、OS が App Nap を外す（優先度 28 のまま）
//! - **利用者が待つ読み込みだけ止める**: 117 ページの PDF は 7.2〜8.0 秒（E コア）→ 3.0 秒（P コア）
//!
//! ## ここで止める 4 つ
//!
//! 1. [`効かないapp_napの止め方を残さない`] — `disable_app_nap` / `NSAppSleepDisabled` が本番へ戻る形
//! 2. [`読み込みのbackgroundはuser_workを握って回す`] — PDF のラスタライズ（開く / ズーム）・Markdown の
//!    組み立て / 描き直しの closure が握らない・処理の後で握る・`let _ =` でその場で落とす形
//! 3. [`user_workは処理の間だけ握る`] — `forget` / static / フィールドに持って寿命の間握る形
//! 4. [`読み込みの腕と実経路はコードと同じ名前で戻す`] — A/B の腕と実経路テストの判定がずれる形
//!
//! 落ちるときは **file:line で名指し**する。実 GUI での A/B は
//! `scripts/test-preview-load-app-nap-1926.sh`（`TAKO_1926_LEGACY=1` で ① と ② が名指しで FAILED）。

use std::path::{Path, PathBuf};

use tako_core::source_scan::fn_head_name;

// 本番コードの範囲取りは 1 実装（#1420）。**切らずにテスト領域だけを潰す**
#[path = "common/production_range.rs"]
mod production_range;

use production_range::code_view::{code_view, without_comments, without_comments_checked};

const SIDEBAR: &str = "crates/tako-app/src/sidebar.rs";
const PREVIEW_RENDER: &str = "crates/tako-app/src/preview_render.rs";
const USER_WORK: &str = "crates/tako-app/src/platform/user_work.rs";
const SCRIPT: &str = "scripts/test-preview-load-app-nap-1926.sh";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("リポジトリルート")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel} を読めない: {e}"))
}

/// 本番コードのコメントを潰した眺め（バイト位置は元のファイルと同じ）
fn production_view(rel: &str) -> String {
    let src = read(rel);
    let prod = production_range::production(&src, rel);
    let code = code_view(&prod);
    let view = without_comments_checked(&prod, rel);
    assert_eq!(
        code.len(),
        view.len(),
        "{rel}:1 2 つの眺めのバイト長が食い違う（範囲を使い回せない）"
    );
    view
}

/// `at`（バイト位置）が何行目か
fn line_at(text: &str, at: usize) -> usize {
    text[..at].bytes().filter(|b| *b == b'\n').count() + 1
}

/// 関数 1 本の本体（開始のバイト位置と本文）。名前で最初に見つかったもの
fn body(view: &str, rel: &str, name: &str) -> (usize, String) {
    let mut offset = 0;
    for line in view.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if fn_head_name(line.trim_start()) != Some(name) {
            continue;
        }
        let open = view[start..].find('{').expect("関数の本体") + start;
        let mut depth = 0usize;
        for (i, byte) in view[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return (open, view[open..open + i + 1].to_string());
                    }
                }
                _ => {}
            }
        }
    }
    panic!("{rel}:1 `fn {name}` が見つからない（改名したならこの番犬も直す）");
}

/// `dir` 配下の `*.rs`（リポジトリ相対パス）
fn rs_files(dir: &str) -> Vec<String> {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(reader) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in reader.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let root = repo_root();
    let mut paths = Vec::new();
    collect(&root.join(dir), &mut paths);
    let mut out: Vec<String> = paths
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap_or(p)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    out.sort();
    assert!(
        !out.is_empty(),
        "{dir} に .rs が無い（置き場を変えたならこの番犬も直す）"
    );
    out
}

/// 各クレートの `src/`（本番コードの置き場）
fn crate_src_dirs() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(repo_root().join("crates"))
        .expect("crates/")
        .flatten()
        .filter(|e| e.path().join("src").is_dir())
        .map(|e| format!("crates/{}/src", e.file_name().to_string_lossy()))
        .collect();
    out.sort();
    assert!(
        out.iter().any(|d| d == "crates/tako-control/src"),
        "crates/*/src の列挙に tako-control が無い（置き場を変えたならこの番犬も直す）"
    );
    out
}

// --- 1) 効かない止め方を残さない ----------------------------------------------------

/// `disable_app_nap`（関数名）と `NSAppSleepDisabled`（`defaults` で書く既定値）を、どのクレートの
/// `src/` にも置かない。`NSAppSleepDisabled` はアプリのドメインへ書いて**次の起動から**効く値で、
/// 走っているプロセスの App Nap は変わらない（#173 の実装は `/proc/<pid>/Info` という存在しない
/// ドメインへ書いて失敗していた）。App Nap を止める口は `platform::user_work` の activity だけ
#[test]
fn 効かないapp_napの止め方を残さない() {
    let mut hits = Vec::new();
    for dir in crate_src_dirs() {
        for rel in rs_files(&dir) {
            // コメントだけを潰す（文字列リテラルの中の `NSAppSleepDisabled` は拾う）
            let view = without_comments(&read(&rel));
            for needle in ["disable_app_nap", "NSAppSleepDisabled"] {
                for (at, _) in view.match_indices(needle) {
                    hits.push(format!("{rel}:{} `{needle}`", line_at(&view, at)));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "効かない App Nap の止め方が戻っている（#1926）:\n  {}\n\
         `defaults write … NSAppSleepDisabled` は走っているプロセスに効かない（#173 の実装は \
         `/proc/<pid>/Info` へ書いて一度も効いていなかった）。アプリ全体では止めない方針で、\
         利用者が待つ処理だけ `platform::user_work::UserWork` で止める",
        hits.join("\n  ")
    );
}

// --- 2) 読み込みの background は UserWork を握る ------------------------------------

/// PDF のラスタライズ（開く `spawn_preview_load` / ズーム `ensure_pdf_raster_quality`）と
/// Markdown の組み立て（開く `spawn_preview_load` / 編集を抜けた描き直し `spawn_md_resume`）は、
/// background の closure の中で**処理の前に** `UserWork::begin_load()` を名前のある束縛で握る。
/// 握らないと前面に無い tako で E コアへ寄せられる（117 ページの PDF が 3.0 秒 → 7.2〜8.0 秒）
#[test]
fn 読み込みのbackgroundはuser_workを握って回す() {
    for (rel, name, work) in [
        (SIDEBAR, "spawn_preview_load", "preview::load_pdf_with_key("),
        (SIDEBAR, "spawn_preview_load", "preview::load_for_reload("),
        (SIDEBAR, "spawn_md_resume", "preview::markdown_from_text("),
        (
            PREVIEW_RENDER,
            "ensure_pdf_raster_quality",
            "preview::rasterize_pdf(",
        ),
    ] {
        let view = production_view(rel);
        let (at, f) = body(&view, rel, name);
        let Some(call) = f.find(work) else {
            panic!(
                "{rel}:{} `fn {name}` が `{work}` を呼んでいない（形を変えたならこの番犬も直す）",
                line_at(&view, at)
            );
        };
        let call_line = line_at(&view, at + call);
        // 処理を包む background の closure の頭（処理より前で最後に現れる spawn）
        let Some(spawn) = f[..call].rfind(".spawn(async move {") else {
            panic!(
                "{rel}:{call_line} `fn {name}` の `{work}` が background の closure \
                 （`.spawn(async move {{`）の中に無い（#1926。形を変えたならこの番犬も直す）"
            );
        };
        let closure = &f[spawn..call];
        let held = closure.find("UserWork::begin_load()").is_some_and(|begin| {
            let head = closure[..begin].rsplit('\n').next().unwrap_or("");
            head.contains("let _") && !head.contains("let _ =")
        });
        assert!(
            held,
            "{rel}:{call_line} `fn {name}` の `{work}` が `UserWork` を握らずに読み込む（#1926）。\
             処理の前に `let _work = UserWork::begin_load();` で握る（`let _ =` はその場で落ちる）。\
             握らないと前面に無い tako で E コアへ寄せられ、117 ページの PDF が \
             3.0 秒 → 7.2〜8.0 秒になる"
        );
    }
}

// --- 3) 処理の間だけ握る ------------------------------------------------------------

/// `UserWork` は処理の closure の中で `let _名前 = UserWork::begin…();` と握り、処理の終わりで
/// 落とす形だけを許す。`std::mem::forget`・static・構造体のフィールド・`Option` に持つと
/// 寿命の間握りっぱなしになり、#1926 で選ばなかった「アプリ全体で App Nap を止める」へ
/// 黙って戻る（誰も待っていない裏の処理まで P コアへ載り、電力が 2〜5 倍になる）
#[test]
fn user_workは処理の間だけ握る() {
    let mut bad = Vec::new();
    for rel in rs_files("crates/tako-app/src") {
        if rel == USER_WORK {
            continue;
        }
        // コードだけの眺め（コメントと文字列を潰す。行番号は原文と同じ）
        let view = code_view(&read(&rel));
        for (n, line) in view.lines().enumerate() {
            let mut rest = line;
            let mut col = 0;
            while let Some(at) = rest.find("UserWork") {
                let before = &line[..col + at];
                let after = &rest[at + "UserWork".len()..];
                col += at + "UserWork".len();
                rest = after;
                // 取り込み（`use crate::platform::user_work::UserWork;`）は持たないので許す
                if before.trim_start().starts_with("use ") {
                    continue;
                }
                let head = before
                    .trim_end_matches("crate::platform::user_work::")
                    .trim();
                let named = head
                    .strip_prefix("let _")
                    .and_then(|h| h.strip_suffix('='))
                    .is_some_and(|name| {
                        let name = name.trim();
                        !name.is_empty()
                            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    });
                let call = after.starts_with("::begin()") || after.starts_with("::begin_load()");
                if !(named && call) {
                    bad.push(format!("{rel}:{} `{}`", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "`UserWork` を処理の間だけでなく持ち続ける形がある（#1926）:\n  {}\n\
         握るのは処理の closure の中で `let _work = UserWork::begin_load();`（または `begin()`）だけ。\
         寿命の間握ると、選ばなかった「アプリ全体で App Nap を止める」へ黙って戻る\
         （ペインへ流れる 30 万行の出力で 110〜319 → 550〜575 mJ）",
        bad.join("\n  ")
    );
    let view = production_view(USER_WORK);
    if let Some(at) = view.find("forget(") {
        panic!(
            "{USER_WORK}:{} `UserWork` の実装が `forget` で握りっぱなしにしている（#1926）",
            line_at(&view, at)
        );
    }
}

// --- 4) A/B の腕と実経路 ------------------------------------------------------------

/// 読み込みの口（`begin_load`）は `TAKO_1926_LEGACY` で旧挙動（握らない）へ戻り、握る本体は
/// 塗り（`begin`）と同じ 1 実装（`hold`）を通る。実経路テストは同じ名前の腕で GUI を起こし、
/// 「P コアで走ったか」で判定する（所要の実時間は判定に使わない = `.agent/conventions.md`）
#[test]
fn 読み込みの腕と実経路はコードと同じ名前で戻す() {
    let view = production_view(USER_WORK);
    let (load_at, load) = body(&view, USER_WORK, "begin_load");
    for needle in ["legacy_1926()", "Self::hold()"] {
        assert!(
            load.contains(needle),
            "{USER_WORK}:{} `UserWork::begin_load` が `{needle}` を通っていない（#1926）",
            line_at(&view, load_at)
        );
    }
    let (legacy_at, legacy) = body(&view, USER_WORK, "legacy_1926");
    assert!(
        legacy.contains("\"TAKO_1926_LEGACY\""),
        "{USER_WORK}:{} 旧挙動へ戻す腕が `TAKO_1926_LEGACY` を読んでいない（#1926。\
         実経路テストの A/B が効かなくなる）",
        line_at(&view, legacy_at)
    );

    let script = read(SCRIPT);
    let line_of = |needle: &str| script.find(needle).map_or(1, |at| line_at(&script, at));
    // 無いときは「在るべき場所」の目印の行を名指す（消えた文字列からは行を数えられない）
    for (needle, anchor, why) in [
        (
            "TAKO_1926_LEGACY=$TAKO_1926_LEGACY",
            "launch_isolated_gui ",
            "A/B の腕を GUI へ渡す（コードの読む名前と同じ）",
        ),
        (
            "wait_napped",
            "echo \"① ",
            "間引かれたのを状態で待ってから測る",
        ),
        (
            "hw.nperflevels",
            "uname -s",
            "E コアの無い機は未実測で終える",
        ),
        (
            "sleep-guard set --mode on",
            "echo \"⑤ ",
            "前提（スリープ防止のアサーションを握っている間は間引かれない）を確かめる",
        ),
    ] {
        assert!(
            script.contains(needle),
            "{SCRIPT}:{} `{needle}` が無い（#1926）。{why}",
            line_of(anchor)
        );
    }
    // 「P コアで走った」の下限（#1916 と同じ値。実測は修正後 0.848〜0.971・旧挙動 0.000〜0.002）
    assert!(
        script.contains("P_CORE_MIN=0.5\n"),
        "{SCRIPT}:{} P コアの比率の下限が 0.5 でない（#1926）",
        line_of("P_CORE_MIN=")
    );
    let helper = script
        .find("on_p_cores() {")
        .map(|at| script[at..].lines().next().unwrap_or(""))
        .unwrap_or("");
    assert!(
        helper.contains(">= $P_CORE_MIN"),
        "{SCRIPT}:{} `on_p_cores` が下限 `P_CORE_MIN` と比べていない（#1926）",
        line_of("on_p_cores() {")
    );
    let judged = script.matches("on_p_cores \"$").count();
    assert!(
        judged >= 2,
        "{SCRIPT}:{} P コアの比率で判定している箇所が {judged} か所しか無い（#1926。① 開いた PDF と \
         ② ズームした PDF の両方を `on_p_cores` で判定する）",
        line_of("on_p_cores() {")
    );
}
