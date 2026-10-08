// tako mod が tako へ送る報告の契約（#1879。設計書 .agent/plans/2026-10-tako-mod.md §4.2）。
// 受け手の正本は tako_core::claude_mod::ModReport。キーを増やすときは両方を同じコミットで直す。
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
  /** session.end を受けた最後の報告。tako はそのペインの報告を即座に捨てる */
  ended: boolean
}
