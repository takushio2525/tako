// tako mod が tako へ送る報告の契約（#1879。設計書 .agent/plans/2026-10-tako-mod.md §4.2）と、
// tako が応答で返す帯・サイドバーの材料（#1881。§7 S3）、mod の $.state の契約。
// 受け手 / 送り手の正本は tako_core::claude_mod::{ModReport, ModBand, BandView}。
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
  /** この値を読んだ時刻（epoch ms）。アカウント単位の値を束ねるときに新しい方を採る */
  observed_at: number
}

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
  /** `tako mod band on|off` の中継（at は epoch ms。$.store の時刻より新しいときだけ従う） */
  band_request?: { hidden: boolean; at: number }
}

declare module 'claude-code' {
  interface PluginState {
    tako: {
      /** 最後に受け取った tako の材料（無い・古い = null で帯を描かない） */
      view: TakoView | null
      /** 帯を隠すトグル（$.store の band の写し。描画が購読する） */
      bandHidden: boolean
    }
  }
}
