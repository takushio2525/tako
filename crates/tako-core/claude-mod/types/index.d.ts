// tako mod が tako へ送る報告の契約（#1879。設計書 .agent/plans/2026-10-tako-mod.md §4.2）と、
// tako が応答で返す帯・サイドバーの材料（#1881。§7 S3）・定型の UI 設定（#1960。§9.7）、mod の $.state の契約。
// 受け手 / 送り手の正本は tako_core::claude_mod::{ModReport, ModBand, BandView} と tako_core::claude_mod_ui::UiConfig。
// キーを増やすときは両方を同じコミットで直す。
//
// **会話の本文・プロンプト・ツールの引数は載せない**（AGENTS.md の絶対ルール）。
// pending_tool はツール名だけ。番犬 crates/tako-control/tests/issue1879_claude_mod_watchdog.rs が
// 本文系のキーが紛れ込んでいないかを走査する。

/** ターンの状態。permission = 権限ダイアログ表示中、question = AskUserQuestion 表示中 */
export type TakoTurn = 'idle' | 'busy' | 'permission' | 'question'

export type TakoContext = {
  /** 最初の API 応答までは欠ける（0 埋めしない = 未観測） */
  tokens?: number
  window: number
  percent?: number
}

export type TakoRateLimit = {
  /** five_hour / seven_day / ゲートウェイの spend_limit など */
  kind: string
  percent_used: number
  resets_at?: string
  /**
   * この値を観測した時刻（epoch ms）。% か resets_at が変わったときだけ打ち直す（heartbeat では
   * 前の時刻のまま = #1903）。tako はアカウント単位の値を束ねるときに新しい方を採る
   */
  observed_at: number
}

/** $.state の limitsSeen（窓の種類 → 最後に値が変わった観測）。ホットリロードをまたいで残す */
export type TakoLimitsSeen = Record<string, { percent_used: number; resets_at?: string; observed_at: number }>

export type TakoModReport = {
  schema: 1
  mod_version: string
  claude_version?: string
  session_id?: string
  /** 報告を組んだ時刻（epoch ms） */
  at: number
  model?: string
  effort?: string
  context?: TakoContext
  rate_limits: TakoRateLimit[]
  cost_usd?: number
  turn: TakoTurn
  pending_tool?: string
  last_turn?: { duration_ms: number; reason: string }
  /**
   * classic 系のイベント（PermissionRequest 等）がこのセッションの mod へ届いているか。
   * false のとき permission は ask ルール由来と ExitPlanMode だけ（ルールの無い ask は拾わない）
   */
  classic_events: boolean
  /** CLAUDE_CONFIG_DIR（使用制限はアカウント単位なので束ねる鍵にする。ログには出さない） */
  config_dir?: string
  /** 帯の状態（#1881。描いた文字列は載せない = 区切りの種類だけ） */
  band?: TakoBand
  /**
   * 休眠に入った理由（#1959）。user_disabled = 利用者が Claude Code の /plugin で tako@skills-dir を
   * 止めた（env の注入で読まれていても従う）。これを最後に mod は報告を止める
   */
  dormant?: 'user_disabled'
  /** session.end を受けた最後の報告。tako はそのペインの報告を即座に捨てる */
  ended: boolean
}

/** 帯（プロンプトの上の 1 行）の状態（tako_core::claude_mod::ModBand） */
export type TakoBand = {
  /** 利用者が帯を隠している（$.store の band） */
  hidden: boolean
  /** 直近の描画で帯を描いたか */
  shown: boolean
  /** 直近の描画の帯の本文の桁（AbovePrompt の bodyColumns） */
  columns?: number
  /** 描いた区切りの種類（左から。tako / pane / tab / workers / attention / ctx / five_hour …） */
  segments: string[]
  /** トグルを最後に変えた時刻（epoch ms） */
  toggled_at?: number
}

/** worker の状態（tako_core::claude_mod::WorkerState） */
export type TakoWorkerState = 'busy' | 'idle' | 'waiting' | 'limited' | 'failed' | 'unknown'
/** 要注意の理由（tako_core::claude_mod::Attention） */
export type TakoAttention = 'permission' | 'question' | 'dialog' | 'limited' | 'failed'

export type TakoWorker = {
  pane: number
  name: string
  state: TakoWorkerState
  attention?: TakoAttention
}

/** tako が `tako mod report` の応答（tako.view）で返す帯・サイドバーの材料（tako_core::claude_mod::BandView） */
export type TakoView = {
  pane: number
  pane_title?: string
  tab_title?: string
  /** tako の表示言語（ja / en） */
  lang: string
  worker_count: number
  attention: number
  /** 要注意を先に、上限まで */
  workers: TakoWorker[]
  /** 帯に出す警告（tako が閾値を超えたと判断したものだけ） */
  warnings: { kind: string; percent: number; resets_at?: number }[]
  /** サイドバー用（閾値に関わらず） */
  ctx?: TakoContext
  rate_limits: { kind: string; percent: number; resets_at?: number }[]
  thresholds: { ctx_percent: number; limit_percent: number }
  /**
   * 帯のトグルの中継（at は epoch ms。$.store の時刻より新しいときだけ従う）。正本は ui.json の
   * band.hidden / toggled_at（#1960）で、`tako mod band on|off`・`tako mod ui set band.hidden`・
   * この mod の報告の取り込みのどれで変わっても同じ形で届く
   */
  band_request?: { hidden: boolean; at: number }
  /** 定型の UI 設定（#1960。tako が検証した値だけが届く。古い tako の応答には無い） */
  ui?: TakoUi
}

/** 帯に並べる区切り（tako_core::claude_mod_ui::BandSegment） */
export type TakoBandSegment = 'pane' | 'tab' | 'workers' | 'attention' | 'ctx' | 'limits' | 'buttons' | 'card'
/** カード・チャットを誰が描くか */
export type TakoPlace = 'auto' | 'mod' | 'tako'
/** チャット風の描き方をいつ出すか */
export type TakoChatShow = 'always' | 'gui_only' | 'off'
/** 色は Claude Code のテーマのキーだけ（ThemeKey の部分集合） */
export type TakoThemeColor =
  | 'text'
  | 'inactive'
  | 'subtle'
  | 'suggestion'
  | 'remember'
  | 'success'
  | 'error'
  | 'warning'
  | 'merged'
  | 'claude'
  | 'permission'
  | 'planMode'
  | 'autoAccept'
  | 'ide'
/** ボタンの動作（4 種の定型だけ。任意の JS は持たない） */
export type TakoButtonAction =
  | { kind: 'slash'; command: 'compact' | 'clear' | 'context' | 'cost' | 'model' | 'effort' | 'tako' }
  | {
      kind: 'tako'
      op:
        | 'split-right'
        | 'split-down'
        | 'session-restart-harness'
        | 'session-restart-handoff'
        | 'limit-resume-on'
        | 'limit-resume-off'
        | 'background'
        | 'close'
        | 'open-cwd'
    }
  | { kind: 'shell'; command: string }
  | { kind: 'prompt'; text: string }
/** カスタムボタン（8 個まで・label 16 桁・hotkey は数字か英小文字 1 字で重複なし） */
export type TakoButton = { id: string; label: string; hotkey: string; action: TakoButtonAction }

/** 定型の UI 設定（tako_core::claude_mod_ui::UiConfig = `<data_dir>/claude-mod/ui.json`） */
export type TakoUi = {
  schema_version: 1
  band: { hidden: boolean; segments: TakoBandSegment[]; toggled_at?: number }
  usage_bar: { place: 'prompt_hint' | 'band' | 'off'; items: ('five_hour' | 'seven_day' | 'ctx')[] }
  buttons: TakoButton[]
  cards: { place: TakoPlace }
  chat: { place: TakoPlace; bubble: TakoChatShow; code_copy: boolean; tool_summary: TakoChatShow }
  colors: { accent: TakoThemeColor; warn: TakoThemeColor; dim: TakoThemeColor }
}

declare module 'claude-code' {
  interface PluginState {
    tako: {
      /** 最後に受け取った tako の材料（無い・古い = null で帯を描かない） */
      view: TakoView | null
      /** 帯を隠すトグル（$.store の band の写し。描画が購読する） */
      bandHidden: boolean
      /** 使用制限の窓ごとの観測時刻（#1903。描画は読まない） */
      limitsSeen: TakoLimitsSeen
    }
  }
}
