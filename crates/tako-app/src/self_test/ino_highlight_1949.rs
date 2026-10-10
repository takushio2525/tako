//! visual-test `ino-highlight`（FR-3.2 / Issue #1949）: `.ino`（Arduino のスケッチ）が C++ の
//! 構文で塗られ、構文セットの常駐（#815）が `.cpp` と同じ寿命で手放されるか。
//!
//! 場面: 一時 dir のスケッチ（`sketch/sketch.ino`）を dispatch の `OpenFile`（表示種別は指定しない
//! = 拡張子からの振り分けを通す）で開く。
//! ① 複数の構文色で塗られる ② 同じ中身の `.cpp` と span が 1 つ残らず一致する（= C++ の構文で
//! 塗っている。構文名は応答に無いので塗りで比べる。応答へ出すのは #1948）③ 塗った直後は構文セットが
//! 載っていて、**開いたまま触らずに置くと 2 秒 tick が猶予（`SYNTAX_IDLE_GRACE` = 30 秒）の後に
//! 手放す**（実時間で測る。手放しても表示中の色は残る）④ 閉じたら猶予を待たずに手放す。
//! live ヒープ（`live_heap`）は「開く前 → `.ino` を塗った後 → 手放した後 → `.cpp` を塗った後」を
//! `TAKO_VISUAL_HEAP:` で出すだけで判定しない（量は環境で揺れる）。
//!
//! 判定は新しい挙動を無条件に主張する。`TAKO_1949_LEGACY=1` は `.ino` を Plain Text で塗るので
//! ① が FAILED = 同一バイナリでの A/B。画像は判定の前に `TAKO_VISUAL_DUMP_DIR` へ
//! `ino-highlight.png` で落とす（新旧で dir を分けて回す）。単独実行は `TAKO_VISUAL_ONLY=ino-highlight`

use super::*;
use std::time::Instant;

const SECTION: &str = "ino-highlight";

/// 授業で書く形のスケッチ（`#include` / `#define` / クラス / 文字列 / 数値 / コメント）
const SKETCH: &str = "\
#include <Arduino.h>
#define LED_PIN 13 // 内蔵 LED

class Blinker {
 public:
  explicit Blinker(uint8_t pin) : pin_(pin) {}
  void toggle() { digitalWrite(pin_, !digitalRead(pin_)); }
 private:
  uint8_t pin_;
};

Blinker blinker(LED_PIN);

void setup() {
  pinMode(LED_PIN, OUTPUT);
  Serial.begin(9600);
  Serial.println(\"start\");
}

void loop() {
  blinker.toggle();
  delay(500);
}
";

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

/// 開いて塗り終わるのを待ち、塗れたか（複数の色）とその時点の span を返す
async fn open_and_paint(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
    pane: Option<u64>,
    path: &std::path::Path,
) -> (PaneId, bool, Vec<preview::Line>) {
    let opened = window
        .update(cx, |app, _, cx| {
            let opened = tako_control::dispatch(
                app,
                tako_control::protocol::Request::OpenFile {
                    pane,
                    path: path.display().to_string(),
                    mode: None,
                    direction: pane
                        .is_none()
                        .then_some(tako_control::protocol::Direction::Right),
                    focus: Some(true),
                    new_tab: false,
                    line: None,
                    column: None,
                },
                PaneOrigin::Cli,
            )
            .unwrap_or_else(|e| fail(&format!("visual-test {SECTION}: 開けない: {e:?}")));
            cx.notify();
            (
                opened["pane"].as_u64(),
                opened["mode"].as_str().map(str::to_string),
            )
        })
        .unwrap_or_else(|_| fail(&format!("visual-test {SECTION}: OpenFile")));
    let (Some(raw), mode) = opened else {
        fail(&format!(
            "visual-test {SECTION}: OpenFile 応答に pane が無い"
        ))
    };
    check(
        mode.as_deref() == Some("code"),
        &format!("visual-test {SECTION}: 拡張子からの振り分けが code（実際: {mode:?}）"),
    );
    let pane = PaneId::from_raw(raw);
    check(
        wait_for_preview_maps(any, window, cx, pane, false).await,
        &format!("visual-test {SECTION}: 平文 paint"),
    );
    window
        .update(cx, |app, _, cx| app.drain_pending_highlights(cx))
        .ok();
    let painted = wait_for_preview_highlight(any, window, cx, pane).await;
    let lines = window
        .update(cx, |app, _, _| {
            match app.previews.get(&pane).map(|s| &s.content) {
                Some(preview::PreviewContent::Code(lines)) => lines.clone(),
                _ => Vec::new(),
            }
        })
        .unwrap_or_default();
    (pane, painted, lines)
}

fn color_count(lines: &[preview::Line]) -> usize {
    lines
        .iter()
        .flatten()
        .filter_map(|span| span.color)
        .map(|c| (c.r, c.g, c.b))
        .collect::<std::collections::HashSet<_>>()
        .len()
}

pub(super) async fn ino_highlight_visual(
    any: AnyWindowHandle,
    window: WindowHandle<TakoApp>,
    cx: &mut AsyncApp,
) {
    inject_section_failure(SECTION);
    ensure_fresh_scene(window, cx, SECTION).await;
    let dir = std::env::temp_dir().join(format!("tako-visual-ino-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ino = dir.join("sketch").join("sketch.ino");
    let cpp = dir.join("cpp").join("sketch.cpp");
    for path in [&ino, &cpp] {
        std::fs::create_dir_all(path.parent().expect("親 dir")).expect("一時 dir");
        std::fs::write(path, SKETCH).expect("スケッチを書ける");
    }

    // 何も塗っていない場面から始める（構文セットは未ロード）
    let resident_before = preview::syntax_resident();
    let (heap_before, _) = live_heap();

    let opened_at = Instant::now();
    let (pane, painted, ino_lines) = open_and_paint(any, window, cx, None, &ino).await;
    let (heap_ino, _) = live_heap();
    let colors = color_count(&ino_lines);
    // 画像は判定の前に落とす（A/B の旧 = Plain Text でも絵が残るように）
    if let Ok(dump) = std::env::var("TAKO_VISUAL_DUMP_DIR") {
        if let Some((frame, _)) = capture_frame(any, cx) {
            let dump = std::path::Path::new(&dump);
            let _ = std::fs::create_dir_all(dump);
            let path = dump.join("ino-highlight.png");
            if frame.save(&path).is_ok() {
                println!("TAKO_VISUAL_DUMP_FILE: {}", path.display());
            }
        }
    }
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} painted={painted} colors={colors} lines={} \
         legacy={} resident_before={resident_before}",
        ino_lines.len(),
        preview::ino_highlight_legacy()
    );
    check(
        painted && colors > 1,
        "① .ino が複数の構文色で塗られる (#1949)",
    );
    check(
        preview::syntax_resident(),
        "③ 塗った直後は構文セットが載っている (#815)",
    );

    // ③ 開いたまま触らずに置く。猶予の起点（最後の借用）は開いた後なので、手放すのは
    // 必ず `opened_at + 猶予` 以降。上限は 2 秒 tick と負荷のぶれを見込んで広めに取る
    let grace = preview::SYNTAX_IDLE_GRACE;
    let limit = grace + Duration::from_secs(20);
    let released_after = loop {
        cx.background_executor()
            .timer(Duration::from_millis(500))
            .await;
        if !preview::syntax_resident() {
            break Some(opened_at.elapsed());
        }
        if opened_at.elapsed() > limit {
            break None;
        }
    };
    let (heap_released, _) = live_heap();
    let still_colored = window
        .update(cx, |app, _, _| {
            match app.previews.get(&pane).map(|s| &s.content) {
                Some(preview::PreviewContent::Code(lines)) => color_count(lines) > 1,
                _ => false,
            }
        })
        .unwrap_or(false);
    println!(
        "TAKO_VISUAL_PIXEL: {SECTION} released_after={released_after:?} grace={grace:?} \
         still_colored={still_colored}"
    );
    check(
        released_after.is_some_and(|t| t >= grace),
        &format!(
            "③ 開いたまま放置すると猶予（{grace:?}）の後に手放す（実際: {released_after:?}）(#815)"
        ),
    );
    check(still_colored, "③ 手放しても表示中の色はそのまま残る (#815)");

    // ② 同じ中身の `.cpp` を同じペインで開き、塗りを突き合わせる（構文セットは載せ直しになる）
    let (_, cpp_painted, cpp_lines) =
        open_and_paint(any, window, cx, Some(pane.as_u64()), &cpp).await;
    let (heap_cpp, _) = live_heap();
    println!(
        "TAKO_VISUAL_HEAP: {SECTION} before={:.2}MB ino_painted=+{:.2}MB released=-{:.2}MB \
         cpp_painted=+{:.2}MB",
        mb(heap_before),
        mb(heap_ino.saturating_sub(heap_before)),
        mb(heap_ino.saturating_sub(heap_released)),
        mb(heap_cpp.saturating_sub(heap_released)),
    );
    check(cpp_painted, "② 比べる .cpp が塗られる");
    check(
        cpp_lines == ino_lines,
        "② .ino と同じ中身の .cpp が 1 span も違わず同じ塗り = C++ の構文 (#1949)",
    );

    // ④ 閉じたらテキストのプレビューが 0 枚 = 次の tick で猶予を待たずに手放す
    window
        .update(cx, |app, _, cx| {
            let _ = tako_control::dispatch(
                app,
                tako_control::protocol::Request::Close {
                    pane: Some(pane.as_u64()),
                    force: true,
                    caller_role: None,
                },
                PaneOrigin::Cli,
            );
            cx.notify();
        })
        .ok();
    let closed_at = Instant::now();
    let released_on_close = loop {
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
        if !preview::syntax_resident() {
            break Some(closed_at.elapsed());
        }
        if closed_at.elapsed() > Duration::from_secs(10) {
            break None;
        }
    };
    println!("TAKO_VISUAL_PIXEL: {SECTION} released_on_close={released_on_close:?}");
    check(
        released_on_close.is_some_and(|t| t < grace),
        "④ 閉じたら猶予を待たずに手放す (#815)",
    );
    let _ = std::fs::remove_dir_all(&dir);
}
