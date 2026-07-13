//! scroll — tmux バックエンド / ネスト tmux のスクロールバック制御（FR-2.5.13 拡張）
//!
//! バックエンドペイン（Phase 5.5）のスクロールバックは外側の alacritty ではなく
//! tmux サーバー側にある。さらにユーザーが自前 tmux セッションをペイン内で attach
//! している場合（ネスト。tako の主要ユースケース）、実体は**ネスト先サーバー**にある。
//!
//! 従来は SGR ホイールイベントを流し込んで tmux 既定バインドの copy-mode に
//! 任せていたが、これは ① 1 イベント = 5 行で「ばっ」と飛ぶ ② copy-mode に
//! 入りっぱなしでキー入力が飲まれる ③ copy-mode カーソルが画面に居座る、の
//! 3 症状を生む（2026-06-12 実機フィードバック）。本モジュールは tako 自身が
//! tmux コマンドで copy-mode を**正確な行数**で駆動し、キー入力時には呼び出し側
//! （UI / dispatch）が `cancel` で iTerm2 流の「打ったら最下部へ戻る」を実現する。
//!
//! マウスを要求しているアプリ（vim 等）への生 SGR 転送は従来どおり
//! `TerminalSession::scroll_wheel` 側の責務（`ScrollState::wants_mouse` で出し分け）。

use crate::tmux::run_tmux;

/// ペインのスクロール実体（どのサーバーのどのセッションがスクロールバックを持つか）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrollTarget {
    /// バックエンドセッション自身（素のシェル・claude 直下など）
    Backend { socket: String, session: String },
    /// ペイン内にネストしたユーザー tmux のセッション（クライアント tty 突き合わせで解決）。
    /// `socket` は `tmux -L` 名。None は既定サーバー
    Nested {
        socket: Option<String>,
        session: String,
    },
}

/// スクロール状態のスナップショット
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollState {
    /// スクロールバック内の現在位置（0 = 最下部、history = 最古）
    pub position: usize,
    /// スクロールバック行数
    pub history: usize,
    /// copy-mode 中か（true の間はキー入力が copy-mode に飲まれる → 呼び出し側が cancel）
    pub in_mode: bool,
    /// 対象ペインのアプリがマウスを要求しているか（true なら生 SGR 転送に任せる）
    pub wants_mouse: bool,
}

/// バックエンドペインのスクロール実体を解決する。
/// バックエンドペインの tty 上で動く tmux クライアント（= ユーザーがペイン内で
/// attach したネストセッション）が `nested_sockets` のサーバーに見つかれば Nested、
/// 無ければ Backend。通常運用は `&[None]`（既定サーバーのみ）を渡す
pub fn resolve_target(
    backend_socket: &str,
    backend_session: &str,
    nested_sockets: &[Option<&str>],
) -> ScrollTarget {
    if let Some(tty) = crate::tmux_backend::pane_tty(backend_socket, backend_session) {
        for socket in nested_sockets {
            for session in crate::tmux::list_sessions(*socket) {
                if session.client_ttys.iter().any(|t| t == &tty) {
                    return ScrollTarget::Nested {
                        socket: socket.map(|s| s.to_string()),
                        session: session.name,
                    };
                }
            }
        }
    }
    ScrollTarget::Backend {
        socket: backend_socket.to_string(),
        session: backend_session.to_string(),
    }
}

impl ScrollTarget {
    /// (`-L` ソケット, `-t` ターゲット)。セッション完全一致 + アクティブペイン。
    /// 末尾コロン必須: `=name` 単体はペインターゲットとして解決されない
    /// （copy-mode / send-keys が "can't find pane" になる。2026-06-12 検証）。
    /// ネスト先が分割されている場合はアクティブペインへの近似（制約は FR-2.17 メモ）
    pub(crate) fn locate(&self) -> (Option<&str>, String) {
        match self {
            ScrollTarget::Backend { socket, session } => {
                (Some(socket.as_str()), format!("={session}:"))
            }
            ScrollTarget::Nested { socket, session } => (socket.as_deref(), format!("={session}:")),
        }
    }
}

/// 現在のスクロール状態を取得する。セッション消滅・tmux 不在では None
pub fn scroll_state(target: &ScrollTarget) -> Option<ScrollState> {
    let (socket, t) = target.locate();
    let output = run_tmux(
        socket,
        &[
            "list-panes",
            "-t",
            &t,
            "-F",
            "#{pane_active}\t#{scroll_position}\t#{history_size}\t#{pane_in_mode}\t#{mouse_any_flag}",
        ],
    )
    .ok()?;
    output.lines().find_map(parse_pane_scroll_line)
}

/// list-panes 1 行のパース。アクティブペインのみ Some
fn parse_pane_scroll_line(line: &str) -> Option<ScrollState> {
    let mut f = line.split('\t');
    if f.next()? != "1" {
        return None;
    }
    // scroll_position は copy-mode 外だと空文字列
    let position = f.next()?.parse().unwrap_or(0);
    let history = f.next()?.parse().unwrap_or(0);
    let in_mode = f.next()? == "1";
    let wants_mouse = f.next()? == "1";
    Some(ScrollState {
        position,
        history,
        in_mode,
        wants_mouse,
    })
}

/// 相対スクロール（正 = 遡る）。実行後の状態を返す。
/// 遡るものが無い（alt screen の TUI 等で history 0）場合は copy-mode に
/// 入らない（入りっぱなしでキーが飲まれる事故を防ぐ）
pub fn scroll_by(target: &ScrollTarget, delta: i32) -> Option<ScrollState> {
    let state = scroll_state(target)?;
    if delta > 0 {
        if state.history == 0 {
            return Some(state);
        }
        if !state.in_mode {
            enter_copy_mode(target);
        }
        send_copy_command(target, "scroll-up", delta as usize);
    } else if delta < 0 {
        if !state.in_mode {
            // 最下部でさらに下 → 何もしない
            return Some(state);
        }
        // -e で入っているため最下部到達で copy-mode は自動解除される
        send_copy_command(target, "scroll-down", (-delta) as usize);
    }
    scroll_state(target)
}

/// 絶対位置へスクロールする（0 = 最下部 = copy-mode 解除）。実行後の状態を返す
pub fn scroll_to(target: &ScrollTarget, offset: usize) -> Option<ScrollState> {
    let state = scroll_state(target)?;
    let goal = offset.min(state.history);
    if goal == 0 {
        if state.in_mode {
            cancel(target);
        }
        return scroll_state(target);
    }
    if !state.in_mode {
        enter_copy_mode(target);
    }
    let current = scroll_state(target)?.position;
    match goal.cmp(&current) {
        std::cmp::Ordering::Greater => send_copy_command(target, "scroll-up", goal - current),
        std::cmp::Ordering::Less => send_copy_command(target, "scroll-down", current - goal),
        std::cmp::Ordering::Equal => {}
    }
    scroll_state(target)
}

/// copy-mode を解除して最下部へ戻す（キー入力前の iTerm2 流ジャンプ）。
/// copy-mode 外でのエラーは無害なので潰す
pub fn cancel(target: &ScrollTarget) {
    let (socket, t) = target.locate();
    let _ = run_tmux(socket, &["send-keys", "-t", &t, "-X", "cancel"]);
}

/// `-e` 付きで copy-mode に入る（最下部までスクロールダウンすると自動解除）
fn enter_copy_mode(target: &ScrollTarget) {
    let (socket, t) = target.locate();
    let _ = run_tmux(socket, &["copy-mode", "-e", "-t", &t]);
}

fn send_copy_command(target: &ScrollTarget, command: &str, count: usize) {
    if count == 0 {
        return;
    }
    let (socket, t) = target.locate();
    let _ = run_tmux(
        socket,
        &[
            "send-keys",
            "-t",
            &t,
            "-N",
            &count.to_string(),
            "-X",
            command,
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{SpawnCommand, SpawnOptions};
    use crate::tmux_backend::{available, kill_server, wrap_options};

    struct Cleanup(Vec<String>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            for socket in &self.0 {
                kill_server(socket);
            }
        }
    }

    fn lines_command() -> String {
        "i=0; while [ $i -lt 100 ]; do echo LINE-$i; i=$((i+1)); done; exec sleep 60".into()
    }

    fn wait_until(session: &crate::TerminalSession, pred: impl Fn(&[String]) -> bool) -> bool {
        for _ in 0..100 {
            if pred(&session.visible_lines()) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }

    /// バックエンドペインを scroll_by / scroll_to で正確な行数だけ駆動できる e2e
    /// （バグ (3) スクロールバー・(a) ヌルヌル化・CLI `tako scroll` の土台）
    #[test]
    #[cfg(unix)]
    fn バックエンドのスクロールを正確な行数で駆動できる() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-scr-{}", std::process::id());
        let _cleanup = Cleanup(vec![socket.clone()]);
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), lines_command()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-scr"))
                .expect("spawn できる");
        assert!(
            wait_until(&session, |lines| lines
                .iter()
                .any(|l| l.trim_end() == "LINE-99")),
            "出力が揃わない"
        );
        let target = ScrollTarget::Backend {
            socket: socket.clone(),
            session: "tako-e2e-scr".into(),
        };
        // 解決: ネスト無しなので Backend のまま
        assert_eq!(
            resolve_target(&socket, "tako-e2e-scr", &[]),
            target,
            "ネスト候補なしでは Backend に解決される"
        );
        // +10 行遡る → 位置 10・先頭行が 10 行分古くなる（77 - 10 = LINE-67）
        let state = scroll_by(&target, 10).expect("状態が取れる");
        assert_eq!(state.position, 10);
        assert!(state.in_mode);
        assert!(
            wait_until(&session, |lines| lines
                .first()
                .is_some_and(|l| l.trim_end() == "LINE-67")),
            "ビューが 10 行遡っていない。先頭: {:?}",
            session.visible_lines().first()
        );
        // -4 行戻す → 位置 6
        let state = scroll_by(&target, -4).expect("状態が取れる");
        assert_eq!(state.position, 6);
        // 絶対位置 30 へ
        let state = scroll_to(&target, 30).expect("状態が取れる");
        assert_eq!(state.position, 30);
        // 0 = 最下部へ戻す（copy-mode 解除）
        let state = scroll_to(&target, 0).expect("状態が取れる");
        assert_eq!(state.position, 0);
        assert!(!state.in_mode, "最下部復帰で copy-mode が解除される");
        assert!(
            wait_until(&session, |lines| lines
                .iter()
                .any(|l| l.trim_end() == "LINE-99")),
            "最下部へ戻らない"
        );
    }

    /// 遡るものが無い alt-screen TUI では copy-mode に入らない
    /// （入りっぱなしでキーが飲まれる事故 = 実機症状の再発防止）
    #[test]
    #[cfg(unix)]
    fn 履歴ゼロではcopy_modeに入らない() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let socket = format!("tako-coretest-scr0-{}", std::process::id());
        let _cleanup = Cleanup(vec![socket.clone()]);
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: "/bin/sh".into(),
                args: vec!["-c".into(), r"printf '\033[?1049h'; exec cat -v".into()],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &socket, "tako-e2e-scr0"))
                .expect("spawn できる");
        let target = ScrollTarget::Backend {
            socket: socket.clone(),
            session: "tako-e2e-scr0".into(),
        };
        // alt-screen 切替（\033[?1049h）の完了を待つ。
        // 切替前は history > 0（通常画面のシェル履歴）だが、
        // alt-screen に入ると history == 0 になる
        let mut state = None;
        for _ in 0..100 {
            state = scroll_state(&target);
            if state.is_some_and(|s| s.history == 0) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            state.is_some_and(|s| s.history == 0),
            "alt-screen 切替が完了しない: {state:?}"
        );
        let after = scroll_by(&target, 5).expect("状態が取れる");
        assert!(!after.in_mode, "履歴ゼロなのに copy-mode に入った");
        // 念のため: その後のキー入力が普通に届く
        session.write(b"ok".to_vec());
        assert!(
            wait_until(&session, |lines| lines.iter().any(|l| l.contains("ok"))),
            "キー入力が届かない"
        );
    }

    /// ネスト tmux（ユーザー自前サーバーを tako ペイン内で attach）の解決と駆動の e2e。
    /// 実機の claude（master-* セッション内）のスクロールバック遡りと同型
    #[test]
    #[cfg(unix)]
    fn ネスト先セッションを解決して駆動できる() {
        if !available() {
            eprintln!("skip: tmux が無い環境");
            return;
        }
        let backend = format!("tako-coretest-scrn-{}", std::process::id());
        let nested = format!("tako-coretest-scrn-in-{}", std::process::id());
        let _cleanup = Cleanup(vec![backend.clone(), nested.clone()]);
        let conf_path = std::env::temp_dir().join(format!("tako-scrn-conf-{nested}"));
        std::fs::write(&conf_path, crate::tmux_backend::NESTED_TMUX_SNIPPET)
            .expect("ネスト conf を書ける");
        let options = SpawnOptions {
            command: Some(SpawnCommand {
                program: crate::tmux::tmux_bin().to_string(),
                args: vec![
                    "-u".into(),
                    "-L".into(),
                    nested.clone(),
                    "-f".into(),
                    conf_path.display().to_string(),
                    "new-session".into(),
                    "-A".into(),
                    "-s".into(),
                    "nest".into(),
                    lines_command(),
                ],
            }),
            cwd: Some(std::env::temp_dir()),
            env: vec![],
        };
        let (session, _rx) =
            crate::TerminalSession::spawn(80, 24, wrap_options(options, &backend, "tako-e2e-scrn"))
                .expect("ネスト構成を spawn できる");
        assert!(
            wait_until(&session, |lines| lines
                .iter()
                .any(|l| l.trim_end() == "LINE-99")),
            "ネスト構成が立ち上がらない"
        );
        // tty 突き合わせでネスト先セッションに解決される
        let target = resolve_target(&backend, "tako-e2e-scrn", &[Some(&nested)]);
        assert_eq!(
            target,
            ScrollTarget::Nested {
                socket: Some(nested.clone()),
                session: "nest".into(),
            },
            "ネストクライアントが検出される"
        );
        // ネスト先の copy-mode を正確な行数で駆動 → 外側のビューに反映される
        let state = scroll_by(&target, 10).expect("状態が取れる");
        assert_eq!(state.position, 10);
        // ネスト tmux はステータスバー分ペイン高が 1 行縮むため、先頭行は ±1 の幅で見る
        // （位置 10 ぴったりは上の state.position で検証済み）
        assert!(
            wait_until(&session, |lines| top_line_number_of(lines)
                .is_some_and(|n| (66..=68).contains(&n))),
            "ネスト越しにビューが遡っていない。先頭: {:?}",
            session.visible_lines().first()
        );
        // キー入力前の cancel で最下部へ戻る（iTerm2 流）
        cancel(&target);
        let state = scroll_state(&target).expect("状態が取れる");
        assert!(!state.in_mode);
        assert!(
            wait_until(&session, |lines| lines
                .iter()
                .any(|l| l.trim_end() == "LINE-99")),
            "cancel で最下部へ戻らない"
        );
    }

    fn top_line_number_of(lines: &[String]) -> Option<usize> {
        lines.first().and_then(|l| {
            l.trim_end()
                .strip_prefix("LINE-")
                .and_then(|n| n.parse().ok())
        })
    }

    #[test]
    fn list_panes行のパース() {
        // アクティブ + copy-mode 外（scroll_position 空）
        assert_eq!(
            parse_pane_scroll_line("1\t\t120\t0\t0"),
            Some(ScrollState {
                position: 0,
                history: 120,
                in_mode: false,
                wants_mouse: false,
            })
        );
        // copy-mode 中 + マウス要求
        assert_eq!(
            parse_pane_scroll_line("1\t15\t300\t1\t1"),
            Some(ScrollState {
                position: 15,
                history: 300,
                in_mode: true,
                wants_mouse: true,
            })
        );
        // 非アクティブペインは読まない
        assert_eq!(parse_pane_scroll_line("0\t15\t300\t1\t0"), None);
        // 欄が欠けたら None
        assert_eq!(parse_pane_scroll_line("1\t15"), None);
    }
}
