//! マウス報告を要求する TUI へ左クリック / ドラッグを渡す（Issue #1961 / FR-2.5.15）
//!
//! 判断（渡すか / tako が取るか）とバイト列は `tako_core::mouse_forward` の 1 実装。
//! ここは GPUI のマウスイベントをその判断へ載せ、届け先（直接ペイン = PTY /
//! 器のペイン = `send-keys -H`）へ順番どおりに流す UI 層の状態だけを持つ。
//!
//! 器のペインは「内側アプリがマウスを要求しているか」を器へ問い合わせないと分からない
//! （外側の端末モードは器の `mouse on` で常に要求ありに見える = #1252）。問い合わせは
//! background で、答えが来るまでの押す・ドラッグ・離すは [`ClickForward::Resolving`] に
//! 溜めておき、答えに合わせて**内側アプリへ渡す**か **tako の選択として再生する**。
//! 問い合わせの往復（数 ms〜数十 ms）は人の手の速さより十分短い。

use tako_core::mouse_forward::{
    self as mf, ClickContext, ClickPhase, ClickRoute, MotionBudget, MouseReport, TakoReason,
    TmuxClickSource, TmuxMouseFlags,
};
use tako_core::scroll::ScrollTarget;

use super::*;

/// 押してから離すまでの 1 回ぶんの状態
pub(crate) enum ClickForward {
    /// 器のペイン: 内側アプリのマウス要求を問い合わせ中。答えが来るまでのイベントを溜める
    Resolving {
        pane: PaneId,
        events: Vec<HeldEvent>,
        ctrl: bool,
    },
    /// 内側アプリへ渡している（押したまま）
    Forwarding {
        pane: PaneId,
        sink: ClickSink,
        report: MouseReport,
        ctrl: bool,
        /// 最後に渡したセル（同じセルの移動は渡さない）
        sent: (usize, usize),
        /// 最後にポインタが居たセル（離すの位置。移動を捨てても離すで追いつく）
        pointer: (usize, usize),
        budget: MotionBudget,
    },
}

/// 問い合わせ中に溜めるイベント
#[derive(Clone, Copy, Debug)]
pub(crate) struct HeldEvent {
    phase: ClickPhase,
    col: usize,
    row: usize,
    side_right: bool,
    click_count: usize,
}

/// 届け先
pub(crate) enum ClickSink {
    /// 直接ペインの PTY
    Pty,
    /// 器のペイン（`send-keys -H`）
    Tmux,
}

/// 器のペインへの送信待ち（ペインごとに 1 本へ直列化 = 押す → 移動 → 離すの順を守る。
/// `pump_wheel` と同じ形）
#[derive(Default)]
pub(crate) struct ClickPump {
    queue: Vec<(ClickPhase, Vec<u8>)>,
    target: Option<ScrollTarget>,
    sending: bool,
}

impl TakoApp {
    /// 押したとき（`on_pane_mouse_down` の cmd+リンクの判定の後・tako の選択の前）。
    /// true = このクリックはこちらが持つ（tako の選択を始めない）
    pub(crate) fn begin_click_forward(
        &mut self,
        pane: PaneId,
        event: &MouseDownEvent,
        cell: (usize, usize, bool),
        focus_change: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        // 前の押下の離すが届いていない（窓の外で離した等）なら、先に離すを渡して畳む
        if matches!(self.click_forward, Some(ClickForward::Forwarding { .. })) {
            self.end_click_forward(None, cx);
        }
        let mirror = self.mirror_scroll_pane(pane);
        let scrolled_back = if mirror {
            self.scroll_ctls
                .get(&pane)
                .is_some_and(|c| c.mirror_scrolling())
        } else {
            self.terminals
                .get(&pane)
                .is_some_and(|s| s.display_offset() > 0)
        };
        let ctx = ClickContext {
            enabled: self.mouse_click_forward,
            legacy: mf::legacy(),
            link_modifier: crate::keybindings::link_modifier_active(&event.modifiers),
            bypass_modifier: mf::bypass_modifier(event.modifiers.alt, event.modifiers.shift),
            window_activation: event.first_mouse,
            focus_change,
            scrolled_back,
        };
        if let ClickRoute::Tako(reason) = mf::route(&ctx) {
            self.note_click(pane, Err(reason));
            return false;
        }
        let held = HeldEvent {
            phase: ClickPhase::Press,
            col: cell.0,
            row: cell.1,
            side_right: cell.2,
            click_count: event.click_count,
        };
        let ctrl = event.modifiers.control;
        if !mirror {
            // 直接ペイン: 端末モードからその場で分かる
            let report = self.terminals.get(&pane).and_then(|s| s.click_report());
            return match mf::decide(report, false) {
                Ok(report) => self.start_forwarding(pane, ClickSink::Pty, report, ctrl, held, cx),
                Err(reason) => {
                    self.note_click(pane, Err(reason));
                    false
                }
            };
        }
        // 器のペイン: 同じペインで問い合わせ中なら続けて溜める（素早いダブルクリック）
        if let Some(ClickForward::Resolving {
            pane: p, events, ..
        }) = self.click_forward.as_mut()
        {
            if *p == pane {
                events.push(held);
                return true;
            }
        }
        let Some(source) = self.tmux_click_source(pane) else {
            return false;
        };
        self.click_forward = Some(ClickForward::Resolving {
            pane,
            events: vec![held],
            ctrl,
        });
        cx.spawn(async move |this, cx| {
            let task = cx.background_executor().spawn(async move {
                let target = source.resolve();
                let flags = mf::tmux_mouse_flags(&target);
                (target, flags)
            });
            let (target, flags) = task.await;
            this.update(cx, |app, cx| {
                app.finish_click_resolve(pane, target, flags, cx)
            })
            .ok();
        })
        .detach();
        true
    }

    /// 器のペインの送り先。ミラーで解決済みならそれを使い回す（ネストの解決 = 子プロセスを省く。
    /// ホイールと同じ送り先になる）
    fn tmux_click_source(&self, pane: PaneId) -> Option<TmuxClickSource> {
        if let Some(target) = self.scroll_ctls.get(&pane).and_then(|c| c.target.clone()) {
            return Some(TmuxClickSource::Resolved(target));
        }
        TmuxClickSource::for_pane(
            self.tmux_view_panes.get(&pane),
            self.backend_sessions.get(&pane).map(String::as_str),
            &tako_core::tmux_backend::socket_name(),
        )
    }

    /// 器への問い合わせの答え。溜めたイベントを内側アプリへ渡すか、tako の選択として再生する
    fn finish_click_resolve(
        &mut self,
        pane: PaneId,
        target: ScrollTarget,
        flags: Option<TmuxMouseFlags>,
        cx: &mut Context<Self>,
    ) {
        // 先に覗いてから取る（別のペインの押下に置き換わっていたら、その状態は残す。
        // `take()` を先にすると、押している最中の別のクリックの状態まで捨ててしまう）
        let mine = matches!(
            &self.click_forward,
            Some(ClickForward::Resolving { pane: p, .. }) if *p == pane
        );
        if !mine {
            return;
        }
        let Some(ClickForward::Resolving { events, ctrl, .. }) = self.click_forward.take() else {
            return;
        };
        // ホイールの判定にも同じ答えを使う（次のホイールで問い合わせ直さずに済む）
        let ctl = self.scroll_ctls.entry(pane).or_default();
        ctl.target = Some(target.clone());
        if let Some(f) = flags {
            ctl.wants_mouse = Some(f.any);
            ctl.wants_sgr = f.sgr;
        }
        let decided = match flags {
            Some(f) => mf::decide(f.report(), f.in_mode),
            None => Err(TakoReason::Unresolved),
        };
        match decided {
            Ok(report) => self.replay_forward(pane, target, report, ctrl, &events, cx),
            Err(reason) => {
                self.note_click(pane, Err(reason));
                self.replay_selection(pane, &events, cx);
            }
        }
        cx.notify();
    }

    /// 溜めたイベントを内側アプリへ渡す（器のペイン）
    fn replay_forward(
        &mut self,
        pane: PaneId,
        target: ScrollTarget,
        report: MouseReport,
        ctrl: bool,
        events: &[HeldEvent],
        cx: &mut Context<Self>,
    ) {
        self.click_pumps.entry(pane).or_default().target = Some(target);
        if let Some(session) = self.terminals.get(&pane) {
            session.clear_selection();
        }
        let mut pressed = false;
        let mut sent = (0, 0);
        let mut pointer = (0, 0);
        for e in events {
            pointer = (e.col, e.row);
            match e.phase {
                ClickPhase::Press => {
                    if self.emit_tmux(pane, report, ClickPhase::Press, pointer, ctrl) {
                        pressed = true;
                        sent = pointer;
                        self.note_click(pane, Ok(()));
                    }
                }
                ClickPhase::Drag if pressed && pointer != sent => {
                    if self.emit_tmux(pane, report, ClickPhase::Drag, pointer, ctrl) {
                        sent = pointer;
                    }
                }
                ClickPhase::Drag => {}
                ClickPhase::Release if pressed => {
                    self.emit_tmux(pane, report, ClickPhase::Release, pointer, ctrl);
                    pressed = false;
                }
                ClickPhase::Release => {}
            }
        }
        if pressed {
            self.click_forward = Some(ClickForward::Forwarding {
                pane,
                sink: ClickSink::Tmux,
                report,
                ctrl,
                sent,
                pointer,
                budget: MotionBudget::default(),
            });
        }
        self.pump_click(pane, cx);
    }

    /// 溜めたイベントを tako の選択として再生する（内側アプリが要求していなかった）。
    /// `on_pane_mouse_down` → `on_mouse_move` → `on_mouse_up` と同じ結果になる
    fn replay_selection(&mut self, pane: PaneId, events: &[HeldEvent], cx: &mut Context<Self>) {
        let Some(session) = self.terminals.get(&pane) else {
            return;
        };
        let mut selecting = false;
        for e in events {
            match e.phase {
                ClickPhase::Press => {
                    let kind = match e.click_count {
                        1 => SelectionKind::Simple,
                        2 => SelectionKind::Word,
                        _ => SelectionKind::Line,
                    };
                    session.clear_selection();
                    session.start_selection(kind, e.col, e.row, e.side_right);
                    selecting = true;
                }
                ClickPhase::Drag if selecting => {
                    session.extend_selection(e.col, e.row, e.side_right)
                }
                ClickPhase::Release if selecting => {
                    selecting = false;
                    // iTerm2 流の copy-on-select（`on_mouse_up` と同じ）
                    if let Some(text) = session.selection_text() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
                _ => {}
            }
        }
        if selecting {
            self.selecting = Some(pane);
        }
    }

    /// 直接ペインで押すを渡して、押したままの状態へ入る
    fn start_forwarding(
        &mut self,
        pane: PaneId,
        sink: ClickSink,
        report: MouseReport,
        ctrl: bool,
        held: HeldEvent,
        _cx: &mut Context<Self>,
    ) -> bool {
        let cell = (held.col, held.row);
        let Some(bytes) = mf::click_report_bytes(report, ClickPhase::Press, cell.0, cell.1, ctrl)
        else {
            // 報告の形で表せない座標（X10 の 223 桁超え）は渡さない = tako の選択
            self.note_click(pane, Err(TakoReason::OutOfRange));
            return false;
        };
        let Some(session) = self.terminals.get(&pane) else {
            return false;
        };
        session.clear_selection();
        session.write_click_report(bytes);
        self.click_stats.count(ClickPhase::Press);
        self.note_click(pane, Ok(()));
        self.click_forward = Some(ClickForward::Forwarding {
            pane,
            sink,
            report,
            ctrl,
            sent: cell,
            pointer: cell,
            budget: MotionBudget::default(),
        });
        true
    }

    /// 押したままの移動（`on_mouse_move` の左ボタン押下中）。true = こちらが持つ
    pub(crate) fn drag_click_forward(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let pane = match self.click_forward.as_ref() {
            Some(ClickForward::Resolving { pane, .. })
            | Some(ClickForward::Forwarding { pane, .. }) => *pane,
            None => return false,
        };
        let Some((col, row, side_right)) = self.cell_clamped(pane, position) else {
            return true;
        };
        let mut emit = None;
        match self.click_forward.as_mut() {
            Some(ClickForward::Resolving { events, .. }) => {
                let held = HeldEvent {
                    phase: ClickPhase::Drag,
                    col,
                    row,
                    side_right,
                    click_count: 1,
                };
                // 移動は最後の 1 つだけ要る（絶対座標）
                match events.last_mut() {
                    Some(last) if last.phase == ClickPhase::Drag => *last = held,
                    _ => events.push(held),
                }
            }
            Some(ClickForward::Forwarding {
                report,
                ctrl,
                sent,
                pointer,
                budget,
                sink,
                ..
            }) => {
                *pointer = (col, row);
                if report.drag && *sent != (col, row) && budget.take(std::time::Instant::now()) {
                    *sent = (col, row);
                    emit = Some((matches!(sink, ClickSink::Tmux), *report, *ctrl));
                }
            }
            None => {}
        }
        if let Some((tmux, report, ctrl)) = emit {
            self.emit(pane, tmux, report, ClickPhase::Drag, (col, row), ctrl, cx);
        }
        true
    }

    /// 離したとき（`on_mouse_up`）/ 離すが届かないまま戻ってきたとき（ボタンなしの移動）。
    /// `position` が無ければ最後にポインタが居たセルで離す。true = こちらが持っていた
    pub(crate) fn end_click_forward(
        &mut self,
        position: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) -> bool {
        let pane = match self.click_forward.as_ref() {
            Some(ClickForward::Resolving { pane, .. })
            | Some(ClickForward::Forwarding { pane, .. }) => *pane,
            None => return false,
        };
        let cell = position.and_then(|p| self.cell_clamped(pane, p));
        match self.click_forward.take() {
            Some(ClickForward::Resolving {
                pane,
                mut events,
                ctrl,
            }) => {
                // 答えが来るまで離すも溜める（答えで渡すか選択として再生する）。
                // 離した後のボタンなしの移動でもここへ来るので、離すは重ねない
                let last = events.last().copied();
                if !last.is_some_and(|e| e.phase == ClickPhase::Release) {
                    let (col, row, side_right) = cell.unwrap_or_else(|| {
                        last.map(|e| (e.col, e.row, e.side_right))
                            .unwrap_or((0, 0, false))
                    });
                    events.push(HeldEvent {
                        phase: ClickPhase::Release,
                        col,
                        row,
                        side_right,
                        click_count: 1,
                    });
                }
                self.click_forward = Some(ClickForward::Resolving { pane, events, ctrl });
            }
            Some(ClickForward::Forwarding {
                pane,
                sink,
                report,
                ctrl,
                pointer,
                ..
            }) => {
                let at = cell.map(|(c, r, _)| (c, r)).unwrap_or(pointer);
                // 離すは捨てない（捨てると内側アプリが押しっぱなしだと思い込む）
                self.emit(
                    pane,
                    matches!(sink, ClickSink::Tmux),
                    report,
                    ClickPhase::Release,
                    at,
                    ctrl,
                    cx,
                );
            }
            None => {}
        }
        cx.notify();
        true
    }

    /// 1 イベントを届け先へ渡す
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &mut self,
        pane: PaneId,
        tmux: bool,
        report: MouseReport,
        phase: ClickPhase,
        cell: (usize, usize),
        ctrl: bool,
        cx: &mut Context<Self>,
    ) {
        if tmux {
            if self.emit_tmux(pane, report, phase, cell, ctrl) {
                self.pump_click(pane, cx);
            }
            return;
        }
        let Some(bytes) = mf::click_report_bytes(report, phase, cell.0, cell.1, ctrl) else {
            return;
        };
        if let Some(session) = self.terminals.get(&pane) {
            session.write_click_report(bytes);
            self.click_stats.count(phase);
        }
    }

    /// 器のペインの送信待ちへ積む（送るのは `pump_click`）。移動は送信中に最新 1 つへ畳む
    fn emit_tmux(
        &mut self,
        pane: PaneId,
        report: MouseReport,
        phase: ClickPhase,
        cell: (usize, usize),
        ctrl: bool,
    ) -> bool {
        let Some(bytes) = mf::click_report_bytes(report, phase, cell.0, cell.1, ctrl) else {
            return false;
        };
        let pump = self.click_pumps.entry(pane).or_default();
        match pump.queue.last_mut() {
            Some((ClickPhase::Drag, last)) if phase == ClickPhase::Drag => *last = bytes,
            _ => pump.queue.push((phase, bytes)),
        }
        self.click_stats.count(phase);
        true
    }

    /// 器のペインへの送信（ペインごとに 1 本。完了したら残りを送る）
    fn pump_click(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        let Some(pump) = self.click_pumps.get_mut(&pane) else {
            return;
        };
        if pump.sending || pump.queue.is_empty() {
            return;
        }
        let Some(target) = pump.target.clone() else {
            pump.queue.clear();
            return;
        };
        let bytes: Vec<u8> = pump.queue.drain(..).flat_map(|(_, b)| b).collect();
        pump.sending = true;
        cx.spawn(async move |this, cx| {
            let task = cx
                .background_executor()
                .spawn(async move { mf::send_tmux(&target, &bytes) });
            let result = task.await;
            this.update(cx, |app, cx| {
                if let Err(e) = result {
                    // GUI の stderr は誰も読めない（#1399）ので persist.log へ。
                    // バイト列（座標）は出さない。器が消えた等の理由だけ
                    tako_control::diag::persist_log(&format!(
                        "クリックを器へ渡せない（#1961）: pane={} 理由={e}",
                        pane.as_u64()
                    ));
                }
                if let Some(pump) = app.click_pumps.get_mut(&pane) {
                    pump.sending = false;
                }
                app.pump_click(pane, cx);
            })
            .ok();
        })
        .detach();
    }

    /// 最後に押したクリックの行き先を残す（`tako mouse` の `last`）
    fn note_click(&mut self, pane: PaneId, result: Result<(), TakoReason>) {
        self.click_stats.last = Some(match result {
            Ok(()) => (pane.as_u64(), true, "forwarded"),
            Err(reason) => (pane.as_u64(), false, reason.as_str()),
        });
    }

    /// ペインを閉じたときの後片付け（送信待ち・押下中の状態を捨てる）
    pub(crate) fn forget_click_forward(&mut self, pane: PaneId) {
        self.click_pumps.remove(&pane);
        let held = match self.click_forward.as_ref() {
            Some(ClickForward::Resolving { pane: p, .. })
            | Some(ClickForward::Forwarding { pane: p, .. }) => *p == pane,
            None => false,
        };
        if held {
            self.click_forward = None;
        }
    }

    /// AI のクリック（CLI `tako click` / MCP `tako_send_input` の `click`）の直接ペインの経路。
    /// GUI と同じ判断・同じバイト列を通る
    pub(crate) fn click_direct_pane(
        &mut self,
        pane: PaneId,
        col: usize,
        row: usize,
    ) -> Result<MouseReport, String> {
        let session = self
            .terminals
            .get(&pane)
            .ok_or_else(|| format!("ペイン {} は端末ではない", pane.as_u64()))?;
        let report =
            mf::decide(session.click_report(), false).map_err(mf::not_forwarded_message)?;
        let bytes = mf::click_pair(report, col, row)?;
        session.write_click_report(bytes);
        Ok(report)
    }
}
