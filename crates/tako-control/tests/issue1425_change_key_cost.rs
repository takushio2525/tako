//! #1425 の A/B 計測 — 変化が無い tick のコストが実際に落ちていること
//!
//! `save_layout` は 2 秒 tick と dispatch ごとに呼ばれるのに、変化検出が
//! **JSON 直列化の結果の文字列比較**だったので、変化が無くても毎回
//! 「全ペインの `PaneMeta` 構築 + 全体の直列化」を払っていた（#1001 C6・
//! perf レポート §H6 の実測は `メインスレッド専有: save_layout が 32ms`）。
//!
//! ## 何で測るか
//!
//! 時計ではなく**確保回数・確保バイト数**で測る（機械の混み具合に依らず
//! 同じソースなら同じ数字になる。所要時間は参考として印字だけする）。
//! この 1 本だけのテストファイルにしてあるのは、グローバルアロケータの
//! カウンタが**同じ実行ファイル内の他テストの確保まで数えてしまう**ため。

use std::alloc::{GlobalAlloc, Layout, System};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use tako_control::layout::{self, LayoutExtras, PaneMetaRef, WindowFrame};
use tako_core::{Pane, PaneId, PaneOrigin, SplitDirection, Workspace};

// --------------------------------------------------------------------------
// 数えるアロケータ
// --------------------------------------------------------------------------

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        if COUNTING.load(Ordering::Relaxed) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size.saturating_sub(l.size()), Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn measure<T>(f: impl FnOnce() -> T) -> (usize, usize, T) {
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::Relaxed);
    let out = f();
    COUNTING.store(false, Ordering::Relaxed);
    (
        ALLOCS.load(Ordering::Relaxed),
        BYTES.load(Ordering::Relaxed),
        out,
    )
}

// --------------------------------------------------------------------------
// 実測シーン（22 ペイン = perf レポートが実測した規模）
// --------------------------------------------------------------------------

const PANES: usize = 22;

/// ペイン付帯情報の実体（借用元）。本番では backend_sessions / terminals などが持つ
struct Owned {
    session: String,
    cwd: PathBuf,
    claude: String,
    agent: String,
    conv: String,
    preview: PathBuf,
    webview: String,
}

fn scene() -> (Workspace, Vec<PaneId>, HashMap<u64, Owned>) {
    let root = Pane::new(PaneOrigin::User);
    let mut ids = vec![root.id()];
    let mut ws = Workspace::new("作業", root);
    let tab1 = ws.active_tab_id();
    // 2 タブへ散らす（本番の「1 グループ = 1 タブ」に寄せる）
    let second_root = Pane::new(PaneOrigin::User);
    ids.push(second_root.id());
    let tab2 = ws.create_tab("監視", second_root);
    for i in 2..PANES {
        let pane = Pane::new(if i % 2 == 0 {
            PaneOrigin::Cli
        } else {
            PaneOrigin::Mcp
        });
        let id = pane.id();
        let even = i % 2 == 0;
        let anchor = ids[usize::from(!even)];
        let tab = if even { tab1 } else { tab2 };
        ws.get_tab_mut(tab)
            .expect("タブがある")
            .tree_mut()
            .split(anchor, SplitDirection::Right, pane)
            .expect("分割できる");
        ids.push(id);
    }
    let owned = ids
        .iter()
        .map(|id| {
            let n = id.as_u64();
            (
                n,
                Owned {
                    session: format!("tako-{n}"),
                    cwd: PathBuf::from(format!("/srv/work/project-{n}")),
                    claude: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
                    agent: "codex".to_string(),
                    conv: format!("conv-{n}"),
                    preview: PathBuf::from(format!("/srv/work/project-{n}/README.md")),
                    webview: format!("https://example.invalid/{n}"),
                },
            )
        })
        .collect();
    (ws, ids, owned)
}

fn extras() -> LayoutExtras {
    LayoutExtras {
        window: Some(WindowFrame {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
            state: "windowed".into(),
        }),
        window_frames: vec![(1, None)],
        collapsed: vec![2],
        webview_dock: vec!["https://example.invalid/dock".into()],
    }
}

/// 変化が無い tick のコストが、旧経路（capture + 直列化）より桁で小さい
#[test]
fn 変化が無いtickは確保も直列化もほぼ払わない() {
    let (ws, _ids, owned) = scene();
    let extras = extras();
    let meta = |pane: PaneId| match owned.get(&pane.as_u64()) {
        Some(o) => PaneMetaRef {
            session: Some(o.session.as_str()),
            cwd: Some(o.cwd.as_path()),
            claude_session_id: Some(o.claude.as_str()),
            agent_resume: Some((o.agent.as_str(), Some(o.conv.as_str()))),
            logged_history: Some(1234),
            preview: Some((o.preview.as_path(), "markdown")),
            webview: Some(Cow::Borrowed(o.webview.as_str())),
        },
        None => PaneMetaRef::default(),
    };

    // 旧経路（`TAKO_1425_LEGACY=1` が本番で通す道）: 変化が無くても毎回払う
    let legacy = || {
        let mut layout = layout::capture(&ws, &|p| meta(p).to_meta(), extras.window.clone());
        extras.apply(&mut layout);
        serde_json::to_string(&layout).expect("直列化できる").len()
    };
    // 新経路: 変化検出キーだけ
    let fresh = || layout::change_key(&ws, &meta, &extras);

    // 1 回ぶんの確保（ウォームアップ後に測る = 遅延初期化の分を混ぜない）
    let _ = legacy();
    let _ = fresh();
    let (legacy_allocs, legacy_bytes, json_len) = measure(legacy);
    let (fresh_allocs, fresh_bytes, _) = measure(fresh);

    // 所要時間は参考（機械差があるので判定には使わない）
    const ROUNDS: u32 = 200;
    let t0 = Instant::now();
    for _ in 0..ROUNDS {
        std::hint::black_box(legacy());
    }
    let legacy_ms = t0.elapsed().as_secs_f64() * 1000.0 / f64::from(ROUNDS);
    let t1 = Instant::now();
    for _ in 0..ROUNDS {
        std::hint::black_box(fresh());
    }
    let fresh_ms = t1.elapsed().as_secs_f64() * 1000.0 / f64::from(ROUNDS);

    println!("#1425 A/B（{PANES} ペイン・JSON {json_len} バイト）");
    println!(
        "  legacy（capture + 直列化）: 確保 {legacy_allocs} 回 / {legacy_bytes} バイト / {legacy_ms:.4} ms"
    );
    println!(
        "  new  （変化検出キーのみ）: 確保 {fresh_allocs} 回 / {fresh_bytes} バイト / {fresh_ms:.4} ms"
    );

    assert!(
        legacy_allocs > 100,
        "旧経路の確保が少なすぎる（シーンが痩せていて A/B にならない）: {legacy_allocs}"
    );
    assert!(
        fresh_allocs * 10 < legacy_allocs,
        "変化検出キーの確保が旧経路の 1/10 未満になっていない: \
         new={fresh_allocs} legacy={legacy_allocs}（#1001 C6 が効いていない）"
    );
    assert!(
        fresh_bytes * 10 < legacy_bytes,
        "変化検出キーの確保バイトが旧経路の 1/10 未満になっていない: \
         new={fresh_bytes} legacy={legacy_bytes}"
    );
    // ペインあたり 1 回も確保していない（= PaneMeta を組んでいない証拠）
    assert!(
        fresh_allocs < PANES,
        "ペイン数ぶんの確保が残っている = 借用のまま流せていない: {fresh_allocs}"
    );
}
