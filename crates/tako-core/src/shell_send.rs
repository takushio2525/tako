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
//!
//! # 全文を画面に出せない寸法（#1940）
//!
//! 全文一致は「入力行の全文が画面に出る」ことを前提にしている。本番（10/9）で worker が
//! 立たなかった 4 件は worker ペインが **2〜3 行**まで潰れていて、zsh は 2 行では入力行を
//! 1 行の横スクロール（`<…`）、3 行では `>....` に畳むので、**全文が一度も画面に出ない**
//! （実測）。旧実装はそこで書き直しを使い切り、最後に**消していない前の本文へ**
//! 「本文 + Enter」を書き足していた = `…--permission-mode auto` + `export …` が
//! `autoexport` に化け、起動先が化けた引数と正しい引数で 2 回起動した（実測）。
//!
//! 直し方は 2 つ:
//!
//! 1. 寸法から**全文を出せないと分かるとき**は書き直さない（何度書いても一致しない）。
//!    書いた本文の反映が落ち着くのを待ってから Enter を 1 回だけ送り、実行は
//!    シェル統合の印（OSC 133;C = preexec）の回数で確かめる（画面に依らない状態の観測）
//! 2. 書き直しを使い切った後の書き切りは、**必ず Ctrl+C で行を捨ててから**書く
//!    （同じ入力を消さずに 2 回打たない）
//!
//! 旧挙動は同じバイナリで `TAKO_1940_LEGACY=1`（[`legacy_1940`]）。

/// tick の間隔（呼び出し側と共有する前提値。ミリ秒）
pub const TICK_MS: u64 = 500;

/// 画面が動かなくなったとみなす連続 tick 数（`WaitReady`）
const READY_SETTLE_TICKS: u32 = 1;
/// シェルの起動を待つ上限 tick 数（超えたら待たずに書く = 従来動作へ落ちる）
const READY_MAX_TICKS: u32 = 60;
/// 最初のプロンプトの印（OSC 133;A）を待つ上限 tick 数（#1940）。
///
/// direnv 等の precmd の後に印が出るので、印を待てば「シェルの起動フックが終わった
/// プロンプト」で書ける（フックの最中に書くと、kernel のエコーと先行入力になり、
/// 画面の変化を実行と取り違える = 準備 40 秒の direnv で実測）。印を出さない環境で
/// spawn が毎回遅れないよう上限で従来の据え置き判定へ落ちる
const READY_MARK_MAX_TICKS: u32 = 30;
/// エコーが**まったく進まなくなった**とみなす連続 tick 数。
///
/// 経過時間で打ち切ってはいけない。負荷が高いと器はエコーを 1 秒あたり数文字しか
/// 出さないことがあり（実機で 33 バイトの反映に 8 秒）、「まだ届いている最中」に
/// 書き直すと、残りが流れ込んで `Write-Output (TAKOMARW` + 書き直し分、のように
/// **混ざった行**ができる。進みが止まったときだけ壊れたと判断する
const ECHO_IDLE_TICKS: u32 = 4;
/// エコー待ちの絶対上限 tick 数。
///
/// 「進みが止まったら」だけを頼りにすると、**画面が常に動いている環境で永久に待つ**。
/// 時計入りプロンプトやスピナーを出すシェルが相手だと、書いたバイトが落ちていても
/// 据え置き判定が一度も成立せず、書き直しにも打ち切りにも進めない
/// （= 修正前より悪い。修正前は少なくとも 1 回は書けていた）。
/// 実測の最悪ケース（33 バイトの反映に 8 秒）に十分な余裕を取ったうえで頭を打つ
const ECHO_MAX_TICKS: u32 = 40;
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
/// 起動フックの最中に Enter した（= 先行入力になった）とき、実行の印を待つ上限 tick 数
/// （#1940）。direnv 等が終わって最初のプロンプトが出れば、先行入力はそこで実行される
/// （準備 40 秒の direnv で実測）。上限を過ぎたら確認できなかったとして終える
const TYPEAHEAD_MAX_TICKS: u32 = 140;
/// 先行入力のあと最初のプロンプトの印が来てから、実行の印を待つ tick 数（#1940）。
/// 2 つの印は同じ tick に届くとは限らない（プロンプトの描き直しが先に画面へ出て、
/// 実行の印が次の tick に来る順序を実測）。待たないと画面の変化だけで決着してしまう
const TYPEAHEAD_PROMPT_GRACE_TICKS: u32 = 4;
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

/// 入力行の全文を画面に出せる最小の行数（#1940）。
///
/// zsh は端末が 3 行未満だと入力行を 1 行の横スクロール（`<…`）で描く（2 行で実測）。
/// 3 行以上でも、本文が「プロンプト行を除いた残り」に収まらなければ `>....` に畳む
/// （3 行 × 63 桁で実測）ので、そちらは [`echo_unviewable`] が桁数から判断する
const MIN_ECHO_ROWS: usize = 3;

/// A/B 用の逃げ道（`TAKO_1940_LEGACY=1`）。#1940 の修正を切って**旧挙動を再現**する
/// （寸法を見ずに書き直しを使い切り、消していない行へ本文 + Enter を書き足す）。
/// 実測（`tests/issue1940_launch_e2e.rs`）と番犬の検出力の確認に使う
pub fn legacy_1940() -> bool {
    static LEGACY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LEGACY.get_or_init(|| {
        matches!(
            std::env::var("TAKO_1940_LEGACY").ok().as_deref(),
            Some("1" | "true" | "on")
        )
    })
}

/// シェル統合（OSC 133）の印を受けた回数（#1940）。
///
/// 回数で持つのは「Enter を送った**後に**実行の印が来たか」を前後比較で見るため
/// （状態だけだと、速く終わるコマンドの Running を tick の間に取りこぼす）。
/// 統合が効いていないシェルではすべて 0 のまま = 画面の観測だけで判断する
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShellMarks {
    /// プロンプトの印（133;A）
    pub prompts: u64,
    /// コマンド実行の印（133;C = preexec）
    pub executed: u64,
    /// コマンド終了の印（133;D）
    pub finished: u64,
    /// 直近の 133;D の終了コード（コードの無い D は None）
    pub last_exit: Option<i32>,
}

/// 1 tick ぶんの観測（#1940）
#[derive(Debug, Clone, Copy)]
pub struct ShellObservation<'a> {
    /// ペインの可視行（行数 = ペインの行数）
    pub screen: &'a [String],
    /// ペインの桁数。0 = 不明（寸法による判断をしない）
    pub cols: usize,
    pub marks: ShellMarks,
    /// このプロセスの別ペインでシェル統合の印を観測済みか
    /// （[`crate::terminal::shell_marks_seen`]）。true なら、まだ印の無い新しいペインは
    /// 最初のプロンプトの印を待ってから書く。印を出さない環境では false のまま = 待たない
    pub marks_expected: bool,
}

impl<'a> ShellObservation<'a> {
    /// 画面だけの観測（寸法・印を持たない呼び出し元 = 旧来の `tick` 用）
    pub fn screen_only(screen: &'a [String]) -> Self {
        Self {
            screen,
            cols: 0,
            marks: ShellMarks::default(),
            marks_expected: false,
        }
    }
}

/// どうやって送達を確かめたか（#1940。診断ログ用の安定した語彙）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// 全文のエコーを見てから Enter し、実行を観測した（#640 の本来の経路）
    Echo,
    /// 全文を画面に出せない寸法だったので書き直さずに Enter し、実行の印で確かめた
    FoldedByMark,
    /// 全文を画面に出せない寸法で、実行の印も無い（画面の変化だけを見た = 中身は未確認）
    FoldedUnverified,
    /// 書き直しを使い切り、行を捨ててから書き切った（確認なし）
    WriteThrough,
    /// まだ決着していない / Enter が効いた気配が無いまま打ち切った
    Unconfirmed,
}

impl Confirmation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::FoldedByMark => "folded_mark",
            Self::FoldedUnverified => "folded_unverified",
            Self::WriteThrough => "write_through",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// 入力行の全文を画面に出せない寸法か（#1940）。
///
/// **出せないと確信できるときだけ true**（寸法が不明・ぎりぎりは false = 従来の
/// 書き直しで確かめる側へ倒す）。幅は全角を 2 セルとして数える
pub fn echo_unviewable(cols: usize, rows: usize, command: &str) -> bool {
    if cols == 0 || rows == 0 {
        return false;
    }
    let width: usize = command
        .chars()
        .map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0))
        .sum();
    rows < MIN_ECHO_ROWS || width > (rows - 1) * cols
}

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
    /// 書き直しを使い切った後、書き切りの前に行を捨てている（#1940）
    FinalClear,
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
    /// 本文を書いた時点の画面（#1940。全文を出せない寸法で「何か反映されたか」を見る）
    write_screen: Option<String>,
    /// Enter を送った時点のシェル統合の印（#1940。実行の印が増えたかの基準）
    submit_marks: Option<ShellMarks>,
    /// 全文を出せない寸法と判断して、書き直さずに Enter を送ったか（#1940）
    folded: bool,
    /// 最初のプロンプトの印より前に Enter した = 起動フックの最中の先行入力（#1940）。
    /// このあいだの画面の変化は kernel のエコーで、実行の証拠にならない
    typeahead: bool,
    /// 先行入力のあと最初のプロンプトの印を見た tick（`ticks_in_stage` の値）
    typeahead_prompt_at: Option<u32>,
    /// どう確かめたか（#1940。診断ログと起動失敗の判定に渡す）
    confirmation: Confirmation,
    /// `TAKO_1940_LEGACY=1` の旧挙動か（構築時に決める = tick 中に腕が変わらない）
    legacy: bool,
    /// 最初のプロンプトの印を待つか（#1940）。最初の観測で決める: 印を観測済みの
    /// プロセスで、**このペインにまだ印が 1 つも無い**（= 起動したてのシェル）ときだけ。
    /// 同じペインへの 2 本目のフロー（ssh 接続後の `cd` 等。相手は印を出さない遠隔の
    /// シェル）には掛けない
    wait_prompt_mark: Option<bool>,
}

impl ShellSendFlow {
    pub fn new(command: impl Into<String>) -> Self {
        Self::with_legacy(command, legacy_1940())
    }

    /// 腕を明示して作る（テストが env を触らずに新旧を比べる口）
    pub fn with_legacy(command: impl Into<String>, legacy: bool) -> Self {
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
            write_screen: None,
            submit_marks: None,
            folded: false,
            typeahead: false,
            typeahead_prompt_at: None,
            confirmation: Confirmation::Unconfirmed,
            legacy,
            wait_prompt_mark: None,
        }
    }

    /// どう確かめたか（#1940。決着前は `Unconfirmed`）
    pub fn confirmation(&self) -> Confirmation {
        self.confirmation
    }

    /// Enter を送った時点のシェル統合の印（#1940）。起動先がすぐ終わったか
    /// （= 起動に失敗したか）を、呼び出し側が「この後に終了の印が来たか」で見る基準。
    /// Enter を送っていなければ None
    pub fn submit_marks(&self) -> Option<ShellMarks> {
        self.submit_marks
    }

    /// 現在のステージ名（診断ログ用。本文は出さない）
    pub fn stage_name(&self) -> &'static str {
        match self.stage {
            Stage::WaitReady => "シェル準備待ち",
            Stage::Recovering => "行クリア",
            Stage::WaitEcho => "エコー待ち",
            Stage::WaitSubmitted => "実行確認",
            Stage::FinalClear => "書き切り前の行クリア",
            Stage::Done => "完了",
        }
    }

    /// 本文の長さ（診断ログ用。本文そのものは出さない）
    pub fn command_len(&self) -> usize {
        self.command.len()
    }

    /// 送る本文（末尾の改行は落としてある）。
    ///
    /// **診断ログへは出さない**（AGENTS.md の絶対ルール。ログ用途は `command_len` と
    /// `stage_name` を使う）。これは「何が積まれたか」を検証する経路のための口で、
    /// セルフテストが起動コマンドの中身（モデル・effort）を突き合わせるのに使う
    pub fn command(&self) -> &str {
        &self.command
    }

    /// 書き直した回数（診断ログ用）
    pub fn rewrites(&self) -> u32 {
        self.rewrites
    }

    /// 1 tick 進める。`screen` はペインの可視行（寸法・シェル統合の印を持たない旧来の口）
    pub fn tick(&mut self, screen: &[String]) -> ShellSendAction {
        self.observe(&ShellObservation::screen_only(screen))
    }

    /// 1 tick 進める（#1940）。画面に加えてペインの寸法とシェル統合の印を見る
    pub fn observe(&mut self, obs: &ShellObservation) -> ShellSendAction {
        self.ticks_in_stage = self.ticks_in_stage.saturating_add(1);
        let wait_mark = *self.wait_prompt_mark.get_or_insert(
            !self.legacy && obs.marks_expected && obs.marks == ShellMarks::default(),
        );
        match self.stage {
            Stage::WaitReady => self.tick_ready(obs.screen, wait_mark && obs.marks.prompts == 0),
            Stage::Recovering => self.tick_recovering(obs.screen),
            Stage::WaitEcho => self.tick_echo(obs),
            Stage::WaitSubmitted => self.tick_submitted(obs),
            Stage::FinalClear => self.tick_final_clear(obs.screen),
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
        let now = squash_screen(screen);
        self.echo_baseline_count = count_occurrences(&now, &self.needle);
        self.write_screen = Some(now);
        self.last_echo_screen = None;
        self.echo_idle_ticks = 0;
        ShellSendAction::Write(self.command.clone().into_bytes())
    }

    /// Enter を単独で送り、実行確認へ進む。基準（画面・印）はここで控える
    fn submit(&mut self, now: String, marks: ShellMarks) -> ShellSendAction {
        self.submit_baseline = Some(now);
        self.submit_marks = Some(marks);
        self.typeahead = self.wait_prompt_mark == Some(true) && marks.prompts == 0;
        self.enter(Stage::WaitSubmitted);
        // Enter は本文と分けて送る（#623: まとめて書くと「送信」と解釈されない
        // 経路があり、分離した方がどの受け手でも素直に通る）
        ShellSendAction::Write(vec![b'\r'])
    }

    /// シェルが動き出したか。画面に何か出たうえで、内容が据え置きになるまで待つ。
    /// **プロンプトの形は問わない**（シェルも OS も固定できないため）。
    /// `awaiting_mark` = 最初のプロンプトの印をまだ待っている（#1940。上限つき）
    fn tick_ready(&mut self, screen: &[String], awaiting_mark: bool) -> ShellSendAction {
        let now = squash_screen(screen);
        if !now.is_empty() {
            if self.last_screen.as_deref() == Some(now.as_str()) {
                self.settled_ticks = self.settled_ticks.saturating_add(1);
            } else {
                self.settled_ticks = 0;
            }
        }
        self.last_screen = Some(now);
        // #1940: 起動フック（direnv 等）の最中は画面が据え置きでも準備ができていない。
        // 印を待つペインでは印が来るまで（上限つきで）書かない
        let settled = self.settled_ticks >= READY_SETTLE_TICKS
            && (!awaiting_mark || self.ticks_in_stage >= READY_MARK_MAX_TICKS);
        // 何も出ないシェル（エコーしない・出力しない）でも従来どおり進むための上限
        if settled || self.ticks_in_stage >= READY_MAX_TICKS {
            self.enter(Stage::WaitEcho);
            return self.write_command(screen);
        }
        ShellSendAction::Wait
    }

    /// 画面が落ち着いたか（Ctrl+C の後に行が消えきったか）。据え置きの観測は
    /// `last_echo_screen` / `echo_idle_ticks` を使い回す
    fn line_cleared(&mut self, screen: &[String]) -> bool {
        let now = squash_screen(screen);
        if self.last_echo_screen.as_deref() == Some(now.as_str()) {
            self.echo_idle_ticks = self.echo_idle_ticks.saturating_add(1);
        } else {
            self.echo_idle_ticks = 0;
            self.last_echo_screen = Some(now);
        }
        self.echo_idle_ticks >= RECOVER_SETTLE_TICKS || self.ticks_in_stage >= RECOVER_MAX_TICKS
    }

    /// Ctrl+C を送った後。**行が消えきるのを待ってから**書き直す。
    /// 消える前に書くと、遅れて届いた残りと混ざって余計に壊れる
    fn tick_recovering(&mut self, screen: &[String]) -> ShellSendAction {
        if !self.line_cleared(screen) {
            return ShellSendAction::Wait;
        }
        self.enter(Stage::WaitEcho);
        self.write_command(screen)
    }

    /// 書き直しを使い切った後（#1940）。**行を捨てきってから**本文 + Enter を書き切る。
    /// 旧実装はここを飛ばして、最後に書き直した本文が残る行へ書き足していた
    /// （`…auto` + `export …` = `autoexport` の化け）
    fn tick_final_clear(&mut self, screen: &[String]) -> ShellSendAction {
        if !self.line_cleared(screen) {
            return ShellSendAction::Wait;
        }
        self.write_through()
    }

    /// 確認は諦めるが、**従来どおり本文 + Enter は書き切る**（#640 以前の動作が下限）。
    /// ここで何も送らないと、確認できない環境で機能が丸ごと止まってしまう
    fn write_through(&mut self) -> ShellSendAction {
        self.stage = Stage::Done;
        self.verified = false;
        self.confirmation = Confirmation::WriteThrough;
        let mut bytes = self.command.clone().into_bytes();
        bytes.push(b'\r');
        ShellSendAction::Write(bytes)
    }

    /// 行を捨てて撃ち直すか、使い切っていれば行を捨ててから書き切る
    fn recover(&mut self) -> ShellSendAction {
        // Ctrl+C の効きは改めて観測する（エコー待ちの据え置き観測を持ち込むと、
        // 行が消える前に「落ち着いた」と判断してしまう）
        self.last_echo_screen = None;
        self.echo_idle_ticks = 0;
        if self.rewrites >= MAX_REWRITES {
            if self.legacy {
                // 旧挙動（`TAKO_1940_LEGACY=1`）: 行を捨てずに書き足す
                return self.write_through();
            }
            self.enter(Stage::FinalClear);
            return ShellSendAction::Write(vec![CTRL_C]);
        }
        self.rewrites += 1;
        self.enter(Stage::Recovering);
        // 欠けた本文が入力行に残っている可能性があるので、書き直す前に必ず捨てる
        ShellSendAction::Write(vec![CTRL_C])
    }

    /// 書いた本文が画面に出たか。出たら Enter、出なければ Ctrl+C して書き直す
    fn tick_echo(&mut self, obs: &ShellObservation) -> ShellSendAction {
        let now = squash_screen(obs.screen);
        // 「増えたか」で見る（画面に残る過去の失敗行を自分のエコーと誤認しないため）
        if self.needle.is_empty()
            || count_occurrences(&now, &self.needle) > self.echo_baseline_count
        {
            self.verified = true;
            self.confirmation = Confirmation::Echo;
            return self.submit(now, obs.marks);
        }
        // まだ届いている最中か（画面が動いているうちは待つ）
        if self.last_echo_screen.as_deref() == Some(now.as_str()) {
            self.echo_idle_ticks = self.echo_idle_ticks.saturating_add(1);
        } else {
            self.echo_idle_ticks = 0;
            self.last_echo_screen = Some(now.clone());
        }
        if self.echo_idle_ticks < ECHO_IDLE_TICKS && self.ticks_in_stage < ECHO_MAX_TICKS {
            return ShellSendAction::Wait;
        }
        // #1940: 全文を画面に出せない寸法では、何度書き直しても一致しない。書いた本文が
        // 何か画面へ反映された（= シェルが受け取っている）なら、書き直さずに Enter を
        // 1 回だけ送る。実行は印（OSC 133;C）で確かめる。何も反映されていない（全損）なら
        // 消すものが無いので、従来どおり捨てて書き直してよい
        let reflected = self.write_screen.as_deref() != Some(now.as_str());
        if !self.legacy && reflected && echo_unviewable(obs.cols, obs.screen.len(), &self.command) {
            self.folded = true;
            return self.submit(now, obs.marks);
        }
        self.recover()
    }

    /// Enter が効いたか。画面が動かなければ Enter を単独で撃ち直す
    fn tick_submitted(&mut self, obs: &ShellObservation) -> ShellSendAction {
        // #1940: シェル統合の実行の印（preexec）が Enter の後に増えた = 実行された。
        // 画面の見え方（畳まれた入力行）に依らない
        let executed = !self.legacy
            && self
                .submit_marks
                .is_some_and(|base| obs.marks.executed > base.executed);
        let now = squash_screen(obs.screen);
        if self.typeahead && !executed {
            if obs.marks.prompts == 0 {
                // まだ起動フックの最中。画面の変化は kernel のエコーなので見ない。
                // Enter も撃ち直さない（先行入力に余分な Enter が積もり、起動した
                // エージェントの入力へ流れ込む）
                if self.ticks_in_stage < TYPEAHEAD_MAX_TICKS {
                    return ShellSendAction::Wait;
                }
                self.stage = Stage::Done;
                self.verified = false;
                self.confirmation = Confirmation::Unconfirmed;
                return ShellSendAction::Done { verified: false };
            }
            // プロンプトが出た。先行入力はこの直後に実行される（印は少し遅れて届きうる）
            let seen_at = *self.typeahead_prompt_at.get_or_insert(self.ticks_in_stage);
            if self.ticks_in_stage.saturating_sub(seen_at) < TYPEAHEAD_PROMPT_GRACE_TICKS {
                return ShellSendAction::Wait;
            }
            // プロンプトが出たのに実行の印が来ない = 先行入力が実行されなかった。
            // ここからは通常の確かめ方（画面の変化・Enter の撃ち直し）へ戻る
            self.typeahead = false;
            self.submit_baseline = Some(now);
            self.ticks_in_stage = 0;
            return ShellSendAction::Wait;
        }
        if executed || self.submit_baseline.as_deref() != Some(now.as_str()) {
            self.stage = Stage::Done;
            if self.folded {
                // 中身は画面で確かめられていない。実行の印があれば「実行された」までは言える
                self.verified = executed;
                self.confirmation = if executed {
                    Confirmation::FoldedByMark
                } else {
                    Confirmation::FoldedUnverified
                };
            }
            return ShellSendAction::Done {
                verified: self.verified,
            };
        }
        if self.ticks_in_stage < SUBMIT_WAIT_TICKS {
            return ShellSendAction::Wait;
        }
        if self.enter_resends >= MAX_ENTER_RESENDS {
            // 本文は通ったが実行された気配が無いまま打ち切る。ここを verified 扱いにすると
            // 「動かないのに成功と記録される」ので、確認できなかったこととして残す
            self.stage = Stage::Done;
            self.verified = false;
            self.confirmation = Confirmation::Unconfirmed;
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
    fn 画面が常に動く環境でも書き直しへ進める() {
        // 時計入りプロンプトやスピナーが相手だと据え置き判定が一度も成立しない。
        // 「進みが止まったら」だけを頼りにすると永久に待ち、書いたバイトが落ちていても
        // 書き直しにも打ち切りにも進めない（修正前より悪い）
        let mut f = ShellSendFlow::new(CMD);
        let animated = |n: usize| vec![format!("PS C:\\Users\\x> [{n}]")];
        f.tick(&animated(0));
        // WaitReady は上限（READY_MAX_TICKS）で抜ける
        let mut wrote = false;
        for n in 1..=(READY_MAX_TICKS as usize + 2) {
            if write_of(&f.tick(&animated(n))).is_some() {
                wrote = true;
                break;
            }
        }
        assert!(wrote, "動き続ける画面でも上限で本文を書き始める");
        // エコーは一度も現れず、画面は動き続ける → 上限で書き直しへ進む
        let mut recovered = false;
        for n in 100..(100 + ECHO_MAX_TICKS as usize + 2) {
            if write_of(&f.tick(&animated(n))).as_deref() == Some("\u{3}") {
                recovered = true;
                break;
            }
        }
        assert!(recovered, "据え置きが成立しなくても上限で書き直しへ進む");
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

    // ---- #1940: 全文を画面に出せない寸法 / 書き切りの前の行クリア / 実行の印 ----

    /// 本番（10/9）と同じ形の起動コマンド
    const LAUNCH: &str =
        "export CLAUDE_CONFIG_DIR='/tmp/x'; TAKO_ORCHESTRATOR_ROLE='worker:tako:l' \
                          claude --model claude-opus-5-5 --effort xhigh --remote-control \
                          --permission-mode auto";

    fn obs<'a>(screen: &'a [String], cols: usize, marks: ShellMarks) -> ShellObservation<'a> {
        ShellObservation {
            screen,
            cols,
            marks,
            marks_expected: false,
        }
    }

    /// zsh が 2 行のペインで描く 1 行の横スクロール（`<` + 末尾）。全文は出ない
    fn folded(tail_len: usize) -> Vec<String> {
        let tail: String = LAUNCH
            .chars()
            .rev()
            .take(tail_len)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        vec![
            "direnv: export ~CLAUDE_CONFIG_DIR".into(),
            format!("<{tail}"),
        ]
    }

    fn two_rows_prompt() -> Vec<String> {
        vec![
            "direnv: export ~CLAUDE_CONFIG_DIR".into(),
            "[test@host:work]$".into(),
        ]
    }

    /// 1 試行ぶんの書き込みを全部集めて、**同じ本文を消さずに 2 回打っていない**ことを見る
    /// （本文の書き込みの間には必ず Ctrl+C が挟まる）
    fn assert_no_overlapping_writes(writes: &[Vec<u8>]) {
        let mut pending_body = false;
        for (i, w) in writes.iter().enumerate() {
            if w.as_slice() == [CTRL_C] {
                pending_body = false;
            } else if w.starts_with(LAUNCH.as_bytes()) {
                assert!(
                    !pending_body,
                    "{i} 回目の書き込みが、消していない本文へ本文を書き足している: {:?}",
                    writes
                        .iter()
                        .map(|w| String::from_utf8_lossy(w)
                            .chars()
                            .take(12)
                            .collect::<String>())
                        .collect::<Vec<_>>()
                );
                pending_body = !w.ends_with(b"\r");
            } else if w.as_slice() == b"\r" {
                pending_body = false;
            }
        }
    }

    #[test]
    fn 全文を出せない寸法の判定() {
        let w = LAUNCH.chars().count();
        assert!(
            w > 125,
            "前提: 本番の起動コマンドは 1 行に収まらない（{w} 桁）"
        );
        assert!(
            echo_unviewable(125, 2, LAUNCH),
            "2 行は zsh が 1 行表示に畳む"
        );
        assert!(
            echo_unviewable(63, 3, LAUNCH),
            "3 行 × 63 桁は `>....` に畳む"
        );
        assert!(!echo_unviewable(63, 30, LAUNCH), "行数が足りれば全文が出る");
        assert!(!echo_unviewable(0, 2, LAUNCH), "寸法が不明なら判断しない");
        assert!(!echo_unviewable(80, 0, LAUNCH), "寸法が不明なら判断しない");
        // 全角は 2 セル。半角なら収まる幅でも、全角なら収まらない
        assert!(!echo_unviewable(10, 3, "aaaaaaaaaaaaaaaaaaaa"));
        assert!(echo_unviewable(10, 3, "ああああああああああああ"));
    }

    #[test]
    fn 畳まれた入力行では書き直さずにenterを1回だけ送り実行の印で確かめる() {
        let mut f = ShellSendFlow::with_legacy(LAUNCH, false);
        let none = ShellMarks {
            prompts: 1,
            ..ShellMarks::default()
        };
        let p = two_rows_prompt();
        f.observe(&obs(&p, 125, none));
        let a = f.observe(&obs(&p, 125, none));
        assert_eq!(
            write_of(&a).as_deref(),
            Some(LAUNCH),
            "据え置き後に本文を書く"
        );
        // 反映されたが畳まれていて全文は出ない。落ち着くまでは待つ
        let shown = folded(70);
        let mut writes = vec![];
        let mut enter_at = None;
        for i in 0..(ECHO_MAX_TICKS + 5) {
            match f.observe(&obs(&shown, 125, none)) {
                ShellSendAction::Wait => {}
                ShellSendAction::Write(b) => {
                    writes.push(b.clone());
                    if b == b"\r" {
                        enter_at = Some(i);
                        break;
                    }
                }
                ShellSendAction::Done { .. } => panic!("Enter の前に終わった"),
            }
        }
        assert_eq!(f.rewrites(), 0, "畳まれた寸法では書き直さない");
        assert_eq!(
            writes,
            vec![b"\r".to_vec()],
            "Ctrl+C を挟まず Enter を 1 回だけ送る"
        );
        assert!(
            enter_at.unwrap() >= ECHO_IDLE_TICKS - 1,
            "反映が落ち着くまで Enter を送らない"
        );
        // 画面は変わらないが、実行の印（preexec）が増えた = 実行された
        let ran = ShellMarks {
            prompts: 1,
            executed: 1,
            ..ShellMarks::default()
        };
        assert_eq!(
            f.observe(&obs(&shown, 125, ran)),
            ShellSendAction::Done { verified: true }
        );
        assert_eq!(f.confirmation(), Confirmation::FoldedByMark);
    }

    #[test]
    fn 畳まれた入力行で印が無ければ画面の変化で決着し未確認として残す() {
        let mut f = ShellSendFlow::with_legacy(LAUNCH, false);
        let m = ShellMarks::default();
        let p = two_rows_prompt();
        f.observe(&obs(&p, 125, m));
        f.observe(&obs(&p, 125, m));
        let shown = folded(70);
        let mut sent_enter = false;
        for _ in 0..(ECHO_MAX_TICKS + 5) {
            if let ShellSendAction::Write(b) = f.observe(&obs(&shown, 125, m)) {
                assert_eq!(b, b"\r", "書き直さない");
                sent_enter = true;
                break;
            }
        }
        assert!(sent_enter);
        let after = vec!["<tail".into(), "FAKE-STARTED".into()];
        assert_eq!(
            f.observe(&obs(&after, 125, m)),
            ShellSendAction::Done { verified: false },
            "中身を確かめていないので verified にしない"
        );
        assert_eq!(f.confirmation(), Confirmation::FoldedUnverified);
    }

    #[test]
    fn 畳まれた寸法でも1バイトも届いていなければ捨てて書き直す() {
        // 全損（#640）は寸法に関係なく起きる。何も反映されていないなら消すものが無いので
        // 書き直してよい（Enter だけ送っても空行が実行されるだけ）
        let mut f = ShellSendFlow::with_legacy(LAUNCH, false);
        let m = ShellMarks::default();
        let p = two_rows_prompt();
        f.observe(&obs(&p, 125, m));
        f.observe(&obs(&p, 125, m));
        let mut first = None;
        for _ in 0..(ECHO_MAX_TICKS + 5) {
            if let ShellSendAction::Write(b) = f.observe(&obs(&p, 125, m)) {
                first = Some(b);
                break;
            }
        }
        assert_eq!(first, Some(vec![CTRL_C]), "まず行を捨てる");
        assert_eq!(f.rewrites(), 1);
    }

    #[test]
    fn 書き直しを使い切った後の書き切りは行を捨ててから書く() {
        // #1940 の主症状: 最後に書き直した本文が残る行へ本文 + Enter を書き足し、
        // `…auto` + `export …` = `autoexport` に化けた（実測）
        for legacy in [false, true] {
            let mut f = ShellSendFlow::with_legacy(LAUNCH, legacy);
            let mut writes = vec![];
            for _ in 0..1000 {
                match f.tick(&prompt()) {
                    ShellSendAction::Write(b) => writes.push(b),
                    ShellSendAction::Done { .. } => break,
                    ShellSendAction::Wait => {}
                }
            }
            assert_eq!(f.rewrites(), MAX_REWRITES);
            let last = writes.last().unwrap();
            assert!(last.ends_with(b"\r") && last.starts_with(LAUNCH.as_bytes()));
            let before_last = &writes[writes.len() - 2];
            if legacy {
                // 旧挙動（A/B の対照）: 書き直した本文を消さずに書き足す
                assert!(
                    before_last.starts_with(LAUNCH.as_bytes()),
                    "旧挙動は行を捨てない"
                );
            } else {
                assert_eq!(before_last, &vec![CTRL_C], "書き切りの直前に行を捨てる");
                assert_no_overlapping_writes(&writes);
                assert_eq!(f.confirmation(), Confirmation::WriteThrough);
            }
        }
    }

    #[test]
    fn 旧挙動は畳まれた寸法でも書き直しを使い切る() {
        // A/B の対照（`TAKO_1940_LEGACY=1`）が本当に旧挙動を再現していること
        let mut f = ShellSendFlow::with_legacy(LAUNCH, true);
        let m = ShellMarks::default();
        let p = two_rows_prompt();
        let shown = folded(70);
        f.observe(&obs(&p, 125, m));
        f.observe(&obs(&p, 125, m));
        for _ in 0..1000 {
            if matches!(
                f.observe(&obs(&shown, 125, m)),
                ShellSendAction::Done { .. }
            ) {
                break;
            }
        }
        assert_eq!(f.rewrites(), MAX_REWRITES);
    }

    #[test]
    fn 印を観測済みのプロセスでは新しいペインの最初のプロンプトの印を待ってから書く() {
        let mut f = ShellSendFlow::with_legacy(LAUNCH, false);
        let loading = vec!["direnv: loading ~/work/.envrc".into()];
        let expect = |marks| ShellObservation {
            screen: &loading,
            cols: 63,
            marks,
            marks_expected: true,
        };
        // 画面は据え置きだが、起動フック（direnv）の最中 = 印がまだ無い
        for _ in 0..(READY_MARK_MAX_TICKS - 1) {
            assert_eq!(
                f.observe(&expect(ShellMarks::default())),
                ShellSendAction::Wait
            );
        }
        let prompted = ShellMarks {
            prompts: 1,
            ..ShellMarks::default()
        };
        let a = f.observe(&expect(prompted));
        assert_eq!(write_of(&a).as_deref(), Some(LAUNCH), "印が来たら書く");

        // 上限: 印が来なくても READY_MARK_MAX_TICKS で従来の据え置き判定へ落ちる
        let mut g = ShellSendFlow::with_legacy(LAUNCH, false);
        let mut wrote_at = None;
        for i in 0..=READY_MAX_TICKS {
            if write_of(&g.observe(&expect(ShellMarks::default()))).is_some() {
                wrote_at = Some(i);
                break;
            }
        }
        assert_eq!(
            wrote_at,
            Some(READY_MARK_MAX_TICKS - 1),
            "上限で待つのをやめる"
        );

        // 印を観測していないプロセス（統合が効かない環境）では待たない
        let mut h = ShellSendFlow::with_legacy(LAUNCH, false);
        h.observe(&obs(&loading, 63, ShellMarks::default()));
        assert!(write_of(&h.observe(&obs(&loading, 63, ShellMarks::default()))).is_some());
    }

    #[test]
    fn 既に印のあるペインへの2本目のフローは印を待たない() {
        // ssh 接続後の `cd`（#1006）は遠隔のシェルへ打つ。遠隔のシェルは印を出さないので
        // 待つと毎回上限まで遅れる
        let mut f = ShellSendFlow::with_legacy("cd /srv/app", false);
        let local = ShellMarks {
            prompts: 1,
            executed: 1,
            ..ShellMarks::default()
        };
        let remote = vec!["user@remote:~$".into()];
        let o = ShellObservation {
            screen: &remote,
            cols: 80,
            marks: local,
            marks_expected: true,
        };
        f.observe(&o);
        assert_eq!(write_of(&f.observe(&o)).as_deref(), Some("cd /srv/app"));
    }

    #[test]
    fn 起動フックの最中の先行入力は画面の変化を実行とみなさずenterも撃ち直さない() {
        // 準備に 40 秒かかる direnv の実測: 上限で書くと kernel のエコーと先行入力になり、
        // Enter の改行のエコーで画面が動く。これを実行と取り違えると「完了」と言った後に
        // まだ何も起動していない（実行は direnv の後）
        let mut f = ShellSendFlow::with_legacy(LAUNCH, false);
        let loading = vec!["direnv: loading ~/work/.envrc".into()];
        fn o(screen: &[String], marks: ShellMarks) -> ShellObservation<'_> {
            ShellObservation {
                screen,
                cols: 63,
                marks,
                marks_expected: true,
            }
        }
        let zero = ShellMarks::default();
        let mut wrote = false;
        for _ in 0..=READY_MAX_TICKS {
            if write_of(&f.observe(&o(&loading, zero))).is_some() {
                wrote = true;
                break;
            }
        }
        assert!(wrote, "印が来なくても上限で書く");
        let echoed = vec!["direnv: loading ~/work/.envrc".into(), LAUNCH.to_string()];
        assert_eq!(
            write_of(&f.observe(&o(&echoed, zero))).as_deref(),
            Some("\r")
        );
        // 改行のエコーで画面が動くが、印はまだ 0 = 起動フックの最中
        let moved = vec![LAUNCH.to_string(), String::new()];
        for _ in 0..(SUBMIT_WAIT_TICKS * 5) {
            assert_eq!(
                f.observe(&o(&moved, zero)),
                ShellSendAction::Wait,
                "画面の変化では決着しない・Enter も撃ち直さない"
            );
        }
        // direnv が終わって最初のプロンプト → 先行入力が実行される（印は次の tick）
        let prompted = ShellMarks { prompts: 1, ..zero };
        assert_eq!(f.observe(&o(&moved, prompted)), ShellSendAction::Wait);
        let ran = ShellMarks {
            prompts: 1,
            executed: 1,
            ..zero
        };
        assert_eq!(
            f.observe(&o(&moved, ran)),
            ShellSendAction::Done { verified: true }
        );
        assert_eq!(f.confirmation(), Confirmation::Echo);
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
