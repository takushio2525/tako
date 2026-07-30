//! shell_send — 素のシェルへ「起動コマンド」を送り届ける送達確認ステートマシン（#640）
//!
//! # なぜ要るのか
//!
//! ペインを作った直後に起動コマンドを PTY へ書きっぱなしにすると、**器（psmux）が
//! まだ入力を読み始めていない間に書いたバイトが落ちる**。Windows 実機の実測（#640）:
//!
//! | 書き込み時刻（PTY 起動から） | 到達 |
//! |---|---|
//! | 0ms / 500ms | **全損**（1 バイトも届かない） |
//! | 1500ms | 途中欠落（`WriteOutput TKOMARK640` = 'A' が消える） |
//! | 3000ms | 途中欠落（'A','K' が消える） |
//! | 6000ms | 全到達 |
//!
//! 器を挟まない直接 spawn では 20/20 全到達、psmux 経由では同条件で欠落が出る。
//! つまり **落とすのは器の側**で、tako から直せるのは「書いた結果を確かめる」ことだけ。
//!
//! #640 で報告された 4 つの症状は、いずれもこの欠落 1 つで説明がつく:
//!
//! 1. **完全消失** — 全損モード。画面は素のプロンプトのまま何時間でも止まる
//! 2. **大幅遅延** — 途中欠落でシェルが構文的に未完の行を受け取り、継続プロンプト（`>>`）で
//!    待つ。後から届いた別の入力がくっついて初めて実行される
//! 3. **文字単位のドロップ** — 中抜けモードそのもの（実機で `$env:CAUDE_CO` = `L` の欠落を目撃）
//! 4. **Enter だけ欠落** — 本文と Enter は独立に落ちる。本文が全文入っても Enter が落ちれば
//!    実行されない（そのため本文の確認と実行の確認を分けてある）
//!
//! **欠落は末尾の切り詰めではなく中抜け**なので、送達確認は先頭一致でも末尾一致でもなく
//! **全文一致**でなければならない。
//!
//! # 設計
//!
//! 純粋なステートマシンにしてある（画面の行を渡すと次の操作が返るだけ）。
//! PTY もタイマーも持たないので、欠落の再現をテストで書ける。
//!
//! 1. `WaitReady` — 画面に何か出て、1 tick 変化しなくなるまで待つ
//! 2. `WaitEcho` — 本文だけ書き（Enter は付けない）、**エコーが画面に出るまで**待つ。
//!    出なければ Ctrl+C で行を消してから書き直す
//! 3. `WaitSubmitted` — エコー一致を確認してから Enter を単独で送り、画面が動くのを見る
//!
//! **Enter は「送った本文が画面に見えている」ときしか送らない**。これにより、
//! 欠けた行がそのまま実行されることが構造的に起きない。
//!
//! 再送のたびに Ctrl+C を打つのは、本文がまだ実行されていない `WaitEcho` の間だけ。
//! この経路は**新規に作ったペイン**の起動コマンド専用で、そこでは Ctrl+C は
//! 「入力行を捨てる」以上のことをしない（PSReadLine も POSIX シェルも同じ）。

/// tick の間隔（呼び出し側と共有する前提値。ミリ秒）
pub const TICK_MS: u64 = 500;

/// 画面が動かなくなったとみなす連続 tick 数（`WaitReady`）
const READY_SETTLE_TICKS: u32 = 1;
/// シェルの起動を待つ上限 tick 数（超えたら待たずに書く = 従来動作へ落ちる）
const READY_MAX_TICKS: u32 = 60;
/// エコーが**まったく進まなくなった**とみなす連続 tick 数。
///
/// 経過時間で打ち切ってはいけない。負荷が高いと器はエコーを 1 秒あたり数文字しか
/// 出さないことがあり（実機で 33 バイトの反映に 8 秒）、「まだ届いている最中」に
/// 書き直すと、残りが流れ込んで `Write-Output (TAKOMARW` + 書き直し分、のように
/// **混ざった行**ができる。進みが止まったときだけ壊れたと判断する
const ECHO_IDLE_TICKS: u32 = 4;
/// 本文の書き直し回数の上限。
///
/// 実機（psmux + 並行負荷あり）では欠落が 12 秒続いた試行がある。1 回の書き直しに
/// 約 2.5 秒かかるので、その程度の窓は越えられるだけの回数を持たせる
const MAX_REWRITES: u32 = 10;
/// Ctrl+C 後、行が消えきったとみなす連続 tick 数
const RECOVER_SETTLE_TICKS: u32 = 2;
/// Ctrl+C の効きを待つ上限 tick 数（効かない受け手でも先へ進めるため）
const RECOVER_MAX_TICKS: u32 = 20;
/// Enter を送ってから画面の変化を待つ tick 数
const SUBMIT_WAIT_TICKS: u32 = 6;
/// Enter の再送回数の上限。
///
/// 実機で「本文は全文正しく入ったのに Enter だけ落ちて実行されない」試行が出ている
/// （#640 の 4 番目の症状）。本文の欠落と Enter の欠落は独立に起きるので、
/// 本文が通った後も Enter 単独で撃ち直せる回数を確保する。
/// 余分な Enter はシェルなら空行、起動済み TUI なら空送信で無害
const MAX_ENTER_RESENDS: u32 = 4;

/// 行を捨てるキー。PSReadLine では `CopyOrCancelLine`、POSIX シェルでは SIGINT で
/// どちらも「入力行を捨てて新しいプロンプト」になる
const CTRL_C: u8 = 0x03;

/// 1 tick ぶんの指示
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellSendAction {
    /// 何もしない
    Wait,
    /// このバイト列を PTY へ書く
    Write(Vec<u8>),
    /// 完了。`verified=false` は「確認できないまま従来どおり書き切った」
    Done { verified: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    WaitReady,
    /// Ctrl+C を送った直後（次 tick で本文を書き直す）
    Recovering,
    WaitEcho,
    WaitSubmitted,
    Done,
}

/// 素のシェルへの 1 コマンド送達フロー
#[derive(Debug)]
pub struct ShellSendFlow {
    /// 送る本文（末尾の改行は落としてある。Enter は分離して送る）
    command: String,
    /// 空白を除いた照合用の本文
    needle: String,
    stage: Stage,
    /// 現ステージに入ってからの tick 数
    ticks_in_stage: u32,
    /// `WaitReady` の据え置き観測
    last_screen: Option<String>,
    settled_ticks: u32,
    rewrites: u32,
    enter_resends: u32,
    /// 本文を書く直前に画面へ既に写っていた本文の出現数。
    ///
    /// 書き直しのたびに Ctrl+C で捨てた行（`… ^C`）が履歴として画面に残るため、
    /// 「画面のどこかに本文がある」だけで判定すると**過去の失敗行**を自分のエコーと
    /// 取り違える。実機で、入力行が `W` の 1 文字しか無いのに Enter を撃つ誤判定が出た。
    /// 書いた後に出現数が増えたことまで見る
    echo_baseline_count: usize,
    /// 前 tick のエコー観測（進んでいるか止まったかの判定用）
    last_echo_screen: Option<String>,
    /// エコーがまったく進まなかった連続 tick 数
    echo_idle_ticks: u32,
    /// Enter を送った時点の画面（変化検出の基準）
    submit_baseline: Option<String>,
    verified: bool,
}

impl ShellSendFlow {
    pub fn new(command: impl Into<String>) -> Self {
        let command = command.into().trim_end_matches(['\r', '\n']).to_string();
        Self {
            needle: squash(&command),
            command,
            stage: Stage::WaitReady,
            ticks_in_stage: 0,
            last_screen: None,
            settled_ticks: 0,
            rewrites: 0,
            enter_resends: 0,
            echo_baseline_count: 0,
            last_echo_screen: None,
            echo_idle_ticks: 0,
            submit_baseline: None,
            verified: false,
        }
    }

    /// 現在のステージ名（診断ログ用。本文は出さない）
    pub fn stage_name(&self) -> &'static str {
        match self.stage {
            Stage::WaitReady => "シェル準備待ち",
            Stage::Recovering => "行クリア",
            Stage::WaitEcho => "エコー待ち",
            Stage::WaitSubmitted => "実行確認",
            Stage::Done => "完了",
        }
    }

    /// 本文の長さ（診断ログ用。本文そのものは出さない）
    pub fn command_len(&self) -> usize {
        self.command.len()
    }

    /// 書き直した回数（診断ログ用）
    pub fn rewrites(&self) -> u32 {
        self.rewrites
    }

    /// 1 tick 進める。`screen` はペインの可視行
    pub fn tick(&mut self, screen: &[String]) -> ShellSendAction {
        self.ticks_in_stage = self.ticks_in_stage.saturating_add(1);
        match self.stage {
            Stage::WaitReady => self.tick_ready(screen),
            Stage::Recovering => self.tick_recovering(screen),
            Stage::WaitEcho => self.tick_echo(screen),
            Stage::WaitSubmitted => self.tick_submitted(screen),
            Stage::Done => ShellSendAction::Done {
                verified: self.verified,
            },
        }
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.ticks_in_stage = 0;
    }

    /// 本文を書く。同時に「今そこに何個写っているか」を控えて、自分のエコーだけを
    /// 数えられるようにする
    fn write_command(&mut self, screen: &[String]) -> ShellSendAction {
        self.echo_baseline_count = count_occurrences(&squash_screen(screen), &self.needle);
        self.last_echo_screen = None;
        self.echo_idle_ticks = 0;
        ShellSendAction::Write(self.command.clone().into_bytes())
    }

    /// シェルが動き出したか。画面に何か出たうえで、内容が据え置きになるまで待つ。
    /// **プロンプトの形は問わない**（シェルも OS も固定できないため）
    fn tick_ready(&mut self, screen: &[String]) -> ShellSendAction {
        let now = squash_screen(screen);
        if !now.is_empty() {
            if self.last_screen.as_deref() == Some(now.as_str()) {
                self.settled_ticks = self.settled_ticks.saturating_add(1);
            } else {
                self.settled_ticks = 0;
            }
        }
        self.last_screen = Some(now);
        let settled = self.settled_ticks >= READY_SETTLE_TICKS;
        // 何も出ないシェル（エコーしない・出力しない）でも従来どおり進むための上限
        if settled || self.ticks_in_stage >= READY_MAX_TICKS {
            self.enter(Stage::WaitEcho);
            return self.write_command(screen);
        }
        ShellSendAction::Wait
    }

    /// Ctrl+C を送った後。**行が消えきるのを待ってから**書き直す。
    /// 消える前に書くと、遅れて届いた残りと混ざって余計に壊れる
    fn tick_recovering(&mut self, screen: &[String]) -> ShellSendAction {
        let now = squash_screen(screen);
        if self.last_echo_screen.as_deref() == Some(now.as_str()) {
            self.echo_idle_ticks = self.echo_idle_ticks.saturating_add(1);
        } else {
            self.echo_idle_ticks = 0;
            self.last_echo_screen = Some(now);
        }
        if self.echo_idle_ticks < RECOVER_SETTLE_TICKS && self.ticks_in_stage < RECOVER_MAX_TICKS {
            return ShellSendAction::Wait;
        }
        self.enter(Stage::WaitEcho);
        self.write_command(screen)
    }

    /// 書いた本文が画面に出たか。出たら Enter、出なければ Ctrl+C して書き直す
    fn tick_echo(&mut self, screen: &[String]) -> ShellSendAction {
        let now = squash_screen(screen);
        // 「増えたか」で見る（画面に残る過去の失敗行を自分のエコーと誤認しないため）
        if self.needle.is_empty()
            || count_occurrences(&now, &self.needle) > self.echo_baseline_count
        {
            self.verified = true;
            self.submit_baseline = Some(now);
            self.enter(Stage::WaitSubmitted);
            // Enter は本文と分けて送る（#623: まとめて書くと「送信」と解釈されない
            // 経路があり、分離した方がどの受け手でも素直に通る）
            return ShellSendAction::Write(vec![b'\r']);
        }
        // まだ届いている最中か（画面が動いているうちは待つ）
        if self.last_echo_screen.as_deref() == Some(now.as_str()) {
            self.echo_idle_ticks = self.echo_idle_ticks.saturating_add(1);
        } else {
            self.echo_idle_ticks = 0;
            self.last_echo_screen = Some(now);
        }
        if self.echo_idle_ticks < ECHO_IDLE_TICKS {
            return ShellSendAction::Wait;
        }
        if self.rewrites >= MAX_REWRITES {
            // 確認は諦めるが、**従来どおり本文 + Enter は書き切る**（#640 以前の動作が下限）。
            // ここで何も送らないと、確認できない環境で機能が丸ごと止まってしまう
            self.stage = Stage::Done;
            self.verified = false;
            let mut bytes = self.command.clone().into_bytes();
            bytes.push(b'\r');
            return ShellSendAction::Write(bytes);
        }
        self.rewrites += 1;
        self.enter(Stage::Recovering);
        // Ctrl+C の効きは改めて観測する（エコー待ちの据え置き観測を持ち込むと、
        // 行が消える前に「落ち着いた」と判断してしまう）
        self.last_echo_screen = None;
        self.echo_idle_ticks = 0;
        // 欠けた本文が入力行に残っている可能性があるので、書き直す前に必ず捨てる
        ShellSendAction::Write(vec![CTRL_C])
    }

    /// Enter が効いたか。画面が動かなければ Enter を単独で撃ち直す
    fn tick_submitted(&mut self, screen: &[String]) -> ShellSendAction {
        let now = squash_screen(screen);
        if self.submit_baseline.as_deref() != Some(now.as_str()) {
            self.stage = Stage::Done;
            return ShellSendAction::Done { verified: true };
        }
        if self.ticks_in_stage < SUBMIT_WAIT_TICKS {
            return ShellSendAction::Wait;
        }
        if self.enter_resends >= MAX_ENTER_RESENDS {
            // 本文は通ったが実行された気配が無いまま打ち切る。ここを verified 扱いにすると
            // 「動かないのに成功と記録される」ので、確認できなかったこととして残す
            self.stage = Stage::Done;
            self.verified = false;
            return ShellSendAction::Done { verified: false };
        }
        self.enter_resends += 1;
        self.ticks_in_stage = 0;
        ShellSendAction::Write(vec![b'\r'])
    }
}

/// 空白をすべて落とした文字列。ターミナルの折り返しは**文字を足さず改行位置を変える**だけ
/// なので、行を連結して空白を落とせば折り返しに関係なく照合できる
fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// `haystack` に `needle` が重ならずに何回現れるか
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.matches(needle).count()
}

fn squash_screen(screen: &[String]) -> String {
    let mut out = String::new();
    for line in screen {
        out.extend(line.chars().filter(|c| !c.is_whitespace()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: &str = "& claude --model claude-opus-5 --effort max";

    /// プロンプトが出た画面
    fn prompt() -> Vec<String> {
        vec!["PS C:\\Users\\x>".into()]
    }

    /// プロンプト + エコー（折り返しなし）
    fn echoed(cmd: &str) -> Vec<String> {
        vec![format!("PS C:\\Users\\x> {cmd}")]
    }

    fn write_of(a: &ShellSendAction) -> Option<String> {
        match a {
            ShellSendAction::Write(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }

    #[test]
    fn 画面が落ち着くまで本文を書かない() {
        let mut f = ShellSendFlow::new(CMD);
        // まだ何も出ていない
        assert_eq!(f.tick(&[]), ShellSendAction::Wait);
        // 出たが前回と違う（描画中）
        assert_eq!(f.tick(&["PowerShell 7".into()]), ShellSendAction::Wait);
        // 据え置き → ここで初めて書く
        let a = f.tick(&["PowerShell 7".into()]);
        assert_eq!(write_of(&a).as_deref(), Some(CMD));
    }

    #[test]
    fn エコーを確認してから_enterを分離して送る() {
        let mut f = ShellSendFlow::new(CMD);
        f.tick(&prompt());
        let a = f.tick(&prompt());
        assert_eq!(write_of(&a).as_deref(), Some(CMD), "据え置き後に本文を書く");
        // エコーが出た → Enter だけを送る（本文と混ぜない）
        let a = f.tick(&echoed(CMD));
        assert_eq!(write_of(&a).as_deref(), Some("\r"));
        // 画面が動いた → 完了
        let after = vec![format!("PS C:\\Users\\x> {CMD}"), "起動中".into()];
        assert_eq!(
            f.tick(&after),
            ShellSendAction::Done { verified: true },
            "実行が観測できたら送達確認済みで完了"
        );
    }

    #[test]
    fn 全損したらctrlcで捨ててから書き直す() {
        // #640 の主症状: 書いたバイトが 1 つも届かない
        let mut f = ShellSendFlow::new(CMD);
        f.tick(&prompt());
        assert!(write_of(&f.tick(&prompt())).is_some(), "1 回目の本文");
        // 画面がまったく動かないまま ECHO_IDLE_TICKS 経過（= 1 バイトも届いていない）
        for _ in 0..ECHO_IDLE_TICKS {
            assert_eq!(f.tick(&prompt()), ShellSendAction::Wait);
        }
        let a = f.tick(&prompt());
        assert_eq!(write_of(&a).as_deref(), Some("\u{3}"), "まず行を捨てる");
        // 行が消えきる（画面据え置き）のを待ってから書き直す
        for _ in 0..RECOVER_SETTLE_TICKS {
            assert_eq!(f.tick(&prompt()), ShellSendAction::Wait);
        }
        let a = f.tick(&prompt());
        assert_eq!(write_of(&a).as_deref(), Some(CMD), "落ち着いてから書き直す");
        assert_eq!(f.rewrites(), 1);
    }

    #[test]
    fn エコーが進んでいる間は書き直さない() {
        // 実機の失敗: 負荷が高いと器はエコーを 1 秒あたり数文字しか出さない。
        // 経過時間だけで打ち切ると、届いている最中に書き直して行が混ざる
        // （`Write-Output (TAKOMARW` + 書き直し分、を実測）
        let mut f = ShellSendFlow::new(CMD);
        f.tick(&prompt());
        assert!(write_of(&f.tick(&prompt())).is_some());
        // 1 文字ずつしか進まない画面を、打ち切り閾値の何倍も流す
        for n in 1..=(ECHO_IDLE_TICKS * 5) as usize {
            let partial: String = CMD.chars().take(n).collect();
            assert_eq!(
                f.tick(&echoed(&partial)),
                ShellSendAction::Wait,
                "進んでいる間は待つ（n={n}）"
            );
        }
        assert_eq!(f.rewrites(), 0, "書き直していない");
        // 最後まで出たら Enter
        assert_eq!(write_of(&f.tick(&echoed(CMD))).as_deref(), Some("\r"));
    }

    #[test]
    fn 途中欠落したエコーはenterを撃たずに書き直す() {
        // #640 の実測: `WriteOutput TAKOMARK640` が `WriteOutput TKOMARK640` になる
        // （中の 1 文字が落ちる）。これを「届いた」と扱うと欠けた行が実行される
        let mut f = ShellSendFlow::new(CMD);
        f.tick(&prompt());
        f.tick(&prompt());
        let broken = CMD.replacen("claude", "clade", 1);
        for _ in 0..ECHO_IDLE_TICKS {
            assert_eq!(
                f.tick(&echoed(&broken)),
                ShellSendAction::Wait,
                "欠けたエコーは一致とみなさない"
            );
        }
        let a = f.tick(&echoed(&broken));
        assert_eq!(
            write_of(&a).as_deref(),
            Some("\u{3}"),
            "欠けたまま Enter を撃たず、行を捨てて撃ち直す"
        );
    }

    #[test]
    fn 過去の失敗行を自分のエコーと取り違えない() {
        // 実機で出た誤判定: 書き直しのたびに Ctrl+C で捨てた行が画面に残るため、
        // 入力行にはまだ `W` の 1 文字しか無いのに「エコーがある」と誤認して
        // Enter を撃ってしまった（結果、起動コマンドは実行されない）
        let mut f = ShellSendFlow::new(CMD);
        // 1 回目の書き込み → 欠落 → Ctrl+C → 書き直し、で画面に捨てた行が積まれた状態
        let history = vec![
            format!("PS C:\\Users\\x> {CMD}^C"),
            "PS C:\\Users\\x> W".into(),
        ];
        assert_eq!(f.tick(&history), ShellSendAction::Wait);
        // 据え置き判定で本文を書く（このとき履歴の 1 件を数えておく）
        assert!(write_of(&f.tick(&history)).is_some());
        // 入力行は `W` のまま = まだ届いていない。過去行があっても Enter を撃たない
        for _ in 0..ECHO_IDLE_TICKS {
            assert_eq!(
                f.tick(&history),
                ShellSendAction::Wait,
                "履歴の一致では送達とみなさない"
            );
        }
        assert_eq!(
            write_of(&f.tick(&history)).as_deref(),
            Some("\u{3}"),
            "撃たずに書き直しへ進む"
        );
        // 行が消えきるのを待って書き直し、自分のエコーが増えたら通す
        for _ in 0..RECOVER_SETTLE_TICKS {
            f.tick(&history);
        }
        assert!(write_of(&f.tick(&history)).is_some(), "書き直す");
        let mut with_echo = history.clone();
        with_echo.push(format!("PS C:\\Users\\x> {CMD}"));
        assert_eq!(write_of(&f.tick(&with_echo)).as_deref(), Some("\r"));
    }

    #[test]
    fn 折り返したエコーでも一致する() {
        let mut f = ShellSendFlow::new(CMD);
        f.tick(&prompt());
        f.tick(&prompt());
        // 端末幅で 3 行に折り返された画面
        let wrapped = vec![
            "PS C:\\Users\\x> & claude --mod".into(),
            "el claude-opus-5 --effo".into(),
            "rt max".into(),
        ];
        let a = f.tick(&wrapped);
        assert_eq!(
            write_of(&a).as_deref(),
            Some("\r"),
            "折り返しは一致を妨げない"
        );
    }

    #[test]
    fn 書き直しの上限に達したら従来どおり本文とenterを書き切る() {
        // 「確認できない環境で機能ごと止まる」ことがないようにする（#640 以前の動作が下限）
        let mut f = ShellSendFlow::new(CMD);
        f.tick(&prompt());
        f.tick(&prompt());
        let mut last = ShellSendAction::Wait;
        for _ in 0..200 {
            last = f.tick(&prompt());
            if matches!(last, ShellSendAction::Done { .. }) {
                break;
            }
        }
        assert_eq!(f.rewrites(), MAX_REWRITES);
        // 最後の書き込みは本文 + Enter（従来動作）
        assert!(matches!(last, ShellSendAction::Done { verified: false }));
    }

    #[test]
    fn enterが落ちたら単独で撃ち直す() {
        let mut f = ShellSendFlow::new(CMD);
        f.tick(&prompt());
        f.tick(&prompt());
        assert_eq!(write_of(&f.tick(&echoed(CMD))).as_deref(), Some("\r"));
        // 画面が動かない = Enter が届いていない
        for _ in 0..(SUBMIT_WAIT_TICKS - 1) {
            assert_eq!(f.tick(&echoed(CMD)), ShellSendAction::Wait);
        }
        assert_eq!(
            write_of(&f.tick(&echoed(CMD))).as_deref(),
            Some("\r"),
            "Enter を単独で再送する"
        );
    }

    #[test]
    fn enterが最後まで効かなければ未確認として終わる() {
        // 「本文は入ったが実行されない」を成功扱いにすると、動かないのに記録だけ残る
        let mut f = ShellSendFlow::new(CMD);
        f.tick(&prompt());
        f.tick(&prompt());
        f.tick(&echoed(CMD));
        let mut last = ShellSendAction::Wait;
        for _ in 0..200 {
            last = f.tick(&echoed(CMD)); // 画面が一度も動かない
            if matches!(last, ShellSendAction::Done { .. }) {
                break;
            }
        }
        assert_eq!(last, ShellSendAction::Done { verified: false });
    }

    #[test]
    fn 日本語や記号を含む本文でも照合できる() {
        let cmd = "& claude --append-system-prompt \"日本語の指示 (#640) 100% 'ok'\"";
        let mut f = ShellSendFlow::new(cmd);
        f.tick(&prompt());
        f.tick(&prompt());
        // 折り返しで日本語の途中に改行が入っても一致する
        let wrapped = vec![
            "PS C:\\Users\\x> & claude --append-system-prompt \"日本".into(),
            "語の指示 (#640) 100% 'ok'\"".into(),
        ];
        assert_eq!(write_of(&f.tick(&wrapped)).as_deref(), Some("\r"));
    }

    #[test]
    fn 末尾改行つきで渡しても本文とenterは分離される() {
        let mut f = ShellSendFlow::new(format!("{CMD}\r\n"));
        f.tick(&prompt());
        let a = f.tick(&prompt());
        assert_eq!(
            write_of(&a).as_deref(),
            Some(CMD),
            "末尾の改行は落として本文だけ書く"
        );
    }

    #[test]
    fn 出力が無いシェルでも上限で書き始める() {
        let mut f = ShellSendFlow::new(CMD);
        // 画面が空のまま = 据え置き判定が働かない
        for _ in 0..(READY_MAX_TICKS - 1) {
            assert_eq!(f.tick(&[]), ShellSendAction::Wait);
        }
        assert_eq!(write_of(&f.tick(&[])).as_deref(), Some(CMD));
    }
}
