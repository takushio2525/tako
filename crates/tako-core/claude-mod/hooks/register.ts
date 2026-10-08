// tako mod（#1877 / S1 #1879）: このペインの Claude Code の状態を集めて tako へ報告する。
// 描画はしない（帯・ペインは S3）。設計の正本は .agent/plans/2026-10-tako-mod.md §5。
//
// 守っていること（§5 の規約。番犬 crates/tako-control/tests/issue1879_claude_mod_watchdog.rs が走査する）:
// - 観測だけで判断を奪わない: すべての on(...) は next(e) へ流し、.catch(($, e, next) => next(e)) を付ける
//   （mod が壊れても tool call と権限ダイアログを止めない。権限を勝手に許可しない）
// - フックの中で CLI を待たない: フックは状態を書き換えて dirty を立てるだけ。CLI は 1 秒の flush が叩く
//   （例外は session.end の最後の 1 回だけ。待たないとプロセスが先に終わって届かない）
// - tako の外では休眠: TAKO_PANE_ID か TAKO_CLI が無ければ何も呼ばない
//   （CLI は接続できないと control.json へ落ちるので、tako の外から叩くと別のインスタンスへ繋がりうる）
// - 失敗で騒がない: CLI の失敗は $.ui.log(…, { to: 'debug' }) だけ
// - $ を渡す補助関数はファイル最上位の関数宣言（validate の規則）
// - 会話の本文・プロンプト・ツールの引数は報告に載せない（ツール名だけ）
import type { EngineInterface, Register, SessionRateLimit, Timer } from 'claude-code'

import type { TakoModReport, TakoRateLimit, TakoTurn } from '../types'

// 展開時（tako_core::claude_mod::install）に tako の版へ置き換わる
const MOD_VERSION = '__TAKO_MOD_VERSION__'
/** 変化があるときの報告の最短間隔 */
const FLUSH_MS = 1_000
/** 変化が無くても送る間隔（tako は 45 秒で失効させる = 2 回の取りこぼしまで許す） */
const HEARTBEAT_MS = 15_000
/** CLI 1 回の上限。tako が落ちていれば 10 ms 未満で exit 1 になる */
const REPORT_TIMEOUT_MS = 5_000
/** session.end の最後の報告だけは待つので、上限は短く */
const FINAL_TIMEOUT_MS = 2_000

// モジュール変数。ホットリロードで消えるが、そのとき session.start が再発火して張り直す
let cli: string | undefined
let timer: Timer | undefined
let dirty = false
let sending = false
let lastSentAt = 0
let turn: TakoTurn = 'idle'
let pendingTool: string | undefined
let effort: string | undefined
let lastTurn: { duration_ms: number; reason: string } | undefined
// classic 系のイベント（classic.PreToolUse 等）がこのセッションの mod へ届いているか。
// 組織アカウントでは 1 つも届かないことがある（2.1.294 で実測。設計書 §1.3 の追記）。
// 届かないときの権限待ちは tool.check の判定から確かなものだけを拾う
let classicEvents = false

function rateLimit(limit: SessionRateLimit, at: number): TakoRateLimit {
  return { kind: limit.kind, percent_used: limit.percentUsed, resets_at: limit.resetsAt, observed_at: at }
}

// 報告を組む。欠けた値（最初の API 応答の前の ctx% など）は undefined のまま = JSON から落ちる
async function buildReport($: EngineInterface, ended: boolean): Promise<TakoModReport> {
  const at = await $.clock.now()
  const usage = await $.session.usage()
  const version = await $.session.version()
  return {
    schema: 1,
    mod_version: MOD_VERSION,
    claude_version: version.version,
    session_id: await $.session.id(),
    at,
    model: await $.session.model(),
    effort,
    context: { tokens: usage.context.tokens, window: usage.context.window, percent: usage.context.percent },
    rate_limits: usage.rateLimits.map(limit => rateLimit(limit, at)),
    cost_usd: usage.cost?.usd,
    turn,
    pending_tool: pendingTool,
    last_turn: lastTurn,
    classic_events: classicEvents,
    config_dir: await $.env.get('CLAUDE_CONFIG_DIR'),
    ended,
  }
}

// 1 回送る。応答（tako 側のスナップショット）は S3 から使う
async function send($: EngineInterface, ended: boolean, timeoutMs: number): Promise<void> {
  const path = cli
  if (path === undefined) return
  dirty = false
  try {
    const report = await buildReport($, ended)
    lastSentAt = report.at
    const done = await $.process.run([path, 'mod', 'report'], { stdin: JSON.stringify(report), timeoutMs })
    if (done.exitCode !== 0) {
      $.ui.log(`tako mod: report exit ${done.exitCode}: ${done.stderr.trim().slice(0, 200)}`, { to: 'debug' })
    }
  } catch (err) {
    $.ui.log(`tako mod: report failed: ${String(err).slice(0, 200)}`, { to: 'debug' })
  }
}

// 1 秒ごとの flush。変化があれば送り、無くても 15 秒に 1 回は送る（heartbeat）
async function tick($: EngineInterface): Promise<void> {
  if (cli === undefined || sending) return
  const now = await $.clock.now()
  if (!dirty && now - lastSentAt < HEARTBEAT_MS) return
  sending = true
  try {
    await send($, false, REPORT_TIMEOUT_MS)
  } finally {
    sending = false
  }
}

// session.start（ホットリロードでも来る）で入口を決める。tako の外なら休眠のまま
async function wake($: EngineInterface): Promise<void> {
  timer?.cancel()
  timer = undefined
  const pane = await $.env.get('TAKO_PANE_ID')
  const path = await $.env.get('TAKO_CLI')
  if (pane === undefined || pane === '' || path === undefined || path === '') {
    cli = undefined
    return
  }
  cli = path
  dirty = true
  timer = $.clock.every(FLUSH_MS, () => {
    void tick($)
  })
}

// 最後の報告（ended: true）。tako はそのペインの報告を即座に捨てる
async function finish($: EngineInterface): Promise<void> {
  timer?.cancel()
  timer = undefined
  await send($, true, FINAL_TIMEOUT_MS)
  cli = undefined
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    const started = await next(e)
    await wake($)
    return started
  }).catch(($, e, next) => next(e))

  on('turn.start', ($, e, next) => {
    turn = 'busy'
    pendingTool = undefined
    dirty = true
    return next(e)
  }).catch(($, e, next) => next(e))

  // classic 系が届くかの印（すべてのツール呼び出しで tool.check より先に来る）
  on('classic.PreToolUse', ($, e, next) => {
    classicEvents = true
    return next(e)
  }).catch(($, e, next) => next(e))

  // ダイアログの表示と同時に来る（設計書 §1.3）。AskUserQuestion もここを通るのでツール名で分ける
  on('classic.PermissionRequest', ($, e, next) => {
    classicEvents = true
    turn = e.tool_name === 'AskUserQuestion' ? 'question' : 'permission'
    pendingTool = e.tool_name
    if (e.effort !== undefined) effort = e.effort.level
    dirty = true
    return next(e)
  }).catch(($, e, next) => next(e))

  // classic 系が届かない環境の代わりの手がかり。判定が ask でも auto モードは分類器が黙って
  // 通しうるので、**ダイアログが確かに出るものだけ**を拾う: ask ルールに当たった呼び出し
  // （auto モードより優先される）と、人に計画の承認を求める ExitPlanMode
  on('tool.check', async ($, e, next) => {
    const verdict = await next(e)
    if (!classicEvents && verdict.decision === 'ask' && (verdict.rule !== undefined || e.tool === 'ExitPlanMode')) {
      turn = 'permission'
      pendingTool = e.tool
      dirty = true
    }
    return verdict
  }).catch(($, e, next) => next(e))

  // 承認・拒否・回答の瞬間を知らせるイベントは無いので、そのツールの呼び出しが返ったところで戻す。
  // AskUserQuestion は呼び出しの開始と同時に質問が出る（classic 系が届かなくても分かる）
  on('tool.call', async ($, e, next) => {
    if (e.tool === 'AskUserQuestion') {
      turn = 'question'
      pendingTool = e.tool
      dirty = true
    }
    const result = await next(e)
    if ((turn === 'permission' || turn === 'question') && e.tool === pendingTool) {
      turn = 'busy'
      pendingTool = undefined
      dirty = true
    }
    return result
  }).catch(($, e, next) => next(e))

  // サブエージェントのターン（agentId あり）は数えない（#1021 の isSidechain 除外と同じ理由）
  on('turn.complete', async ($, e, next) => {
    const done = await next(e)
    if (e.agentId === undefined) {
      turn = 'idle'
      pendingTool = undefined
      lastTurn = { duration_ms: e.durationMs, reason: e.reason }
      dirty = true
    }
    return done
  }).catch(($, e, next) => next(e))

  // effort はターンの終わりの Stop が運ぶ（effort を持たないモデルでは来ない）
  on('classic.Stop', ($, e, next) => {
    if (e.effort !== undefined && e.effort.level !== effort) {
      effort = e.effort.level
      dirty = true
    }
    return next(e)
  }).catch(($, e, next) => next(e))

  // /clear と /resume は同じプロセスで別のセッションが続く（session.start は来ない）。
  // 次の報告が新しい session_id を運ぶので、状態だけ戻して報告は続ける
  on('session.end', async ($, e, next) => {
    if (e.reason === 'clear' || e.reason === 'resume') {
      turn = 'idle'
      pendingTool = undefined
      lastTurn = undefined
      dirty = true
      return next(e)
    }
    await finish($)
    return next(e)
  }).catch(($, e, next) => next(e))
}
