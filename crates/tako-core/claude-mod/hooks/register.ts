// tako mod（#1877 / S1 #1879 / S3 #1881）: このペインの Claude Code の状態を集めて tako へ報告し、
// tako の応答（帯・サイドバーの材料）で Claude Code の画面に tako の状況を出す。
// 設計の正本は .agent/plans/2026-10-tako-mod.md §5（規約）と §7 S3（帯・サイドバー）。
//
// 画面に出すもの（S3）:
// - 帯（プロンプトの上の 1 行）: このペインの名前・タブ・worker 数と要注意の数。ctx / 使用制限は
//   tako が閾値を超えたと判断したときだけ（statusLine を持つ利用者の画面で二重にならない既定）。
//   幅（bodyColumns）に収まらなければ優先度の低い区切りから落とし、それでも溢れたら末尾を切る
//   = **必ず 1 行**。権限ダイアログ・質問の表示中は Claude Code が帯ごと隠すので「承認待ち」は出さない
// - `/tako`: サイドバーのペイン（$.ui.open）に詳細。頼まれずには開かない（開くのはコマンドだけ）
// - 帯を隠すトグル: 正本は tako の ui.json の band.hidden（#1960）。$.store の `band`（{ hidden, at }）は
//   この読み込み元の写しで、`/tako band on|off`・ペインのボタンで変えたら報告の band.toggled_at で tako へ
//   渡り、tako が ui.json へ取り込む。tako からは応答の view.band_request で届く。時刻の新しいものが勝つ
// 何を出すかの判断は tako 側（tako_core::claude_mod::band_view）。ここは幅に合わせて詰めて描くだけ
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

import type { TakoLimitsSeen, TakoModReport, TakoRateLimit, TakoTurn, TakoView, TakoWorker } from '../types'

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
/** tako の応答（帯の材料）がこれより古くなったら帯を消す（tako の鮮度 FRESH_FOR と同じ 45 秒） */
const VIEW_FRESH_MS = 45_000
/** 帯の区切りの区切り文字（幅 1 の ASCII だけ。曖昧幅の文字は端末の設定で 2 桁になりうる） */
const SEP = ' | '
/** 帯のペイン名・タブ名を切り詰める上限（桁） */
const NAME_MAX = 24
/** サイドバーのペインの id（$.ui.open）。描画の requestId もこれ */
const PANE_ID = 'tako'
/** $.store のキー（帯のトグル） */
const BAND_KEY = 'band'
/** 利用者が Claude Code の /plugin で止める id（tako setup が skills/tako に置いた写し = #1959） */
const SKILLS_ID = 'tako@skills-dir'

// $.state（セッションの値。描画が購読するので、書けば読み手が描き直される）
const VIEW = { plugin: 'tako', key: 'view' } as const
const HIDDEN = { plugin: 'tako', key: 'bandHidden' } as const
// 使用制限の窓ごとの観測時刻（#1903）。$.state に置くのはホットリロード（tako の更新で mod が
// 書き換わる）をまたいで残すため。モジュール変数だと読み込み直しのたびに全窓を「今」の観測にしてしまう
const LIMITS_SEEN = { plugin: 'tako', key: 'limitsSeen' } as const

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
// 帯（S3）: 最後に受け取った tako の材料の時刻・JSON（同じなら $.state を書かない = 無駄に描き直さない）
let viewAt = 0
let lastViewJson: string | undefined
let lang = 'en'
// `/tako` の説明文を登録した言語（tako の表示言語が応答で分かったら登録し直す）
let commandLang: string | undefined
// 帯のトグル（$.store の写し）と、それを変えた時刻（中継・他のセッションとの突き合わせ）
let bandHidden = false
let bandToggledAt: number | undefined
// 直近の描画（報告で tako へ返す。何を描いたかは区切りの種類だけ）
let bandShown = false
let bandColumns: number | undefined
let bandSegments: string[] = []
let lastBandKey: string | undefined
// classic 系のイベント（classic.PreToolUse 等）がこのセッションの mod へ届いているか。
// 組織アカウントでは 1 つも届かないことがある（2.1.294 で実測。FR-2.42.7）。
// 届かないときの権限待ちは tool.check の判定から確かなものだけを拾う
let classicEvents = false
// 利用者が /plugin で止めたかを最後に見た時刻（#1959。heartbeat と同じ間隔で見直す）
let lastDisabledCheck = 0

// effort の値を読む。turn.step は文字列（2.1.294 の実測: "medium"）、classic 系は { level } で運ぶ
function effortLevel(value: unknown): string | undefined {
  if (typeof value === 'string') return value
  if (typeof value === 'object' && value !== null && 'level' in value) {
    const level = (value as { level: unknown }).level
    if (typeof level === 'string') return level
  }
  return undefined
}

// 使用制限に観測時刻を付ける（#1903）。窓ごとに % か resetsAt が変わったときだけ今の時刻を打ち、
// 変わらなければ前に打った時刻のまま送る。heartbeat で打ち直すと、1 時間放置したペインの古い % も
// 「今」の観測に見え、tako が同じアカウントの値を最新の観測で束ねられない（設計書 §6）。
// $.state を読めない・書けないときは今の時刻を打つ（報告そのものは止めない = §5 の「失敗で騒がない」）
async function stampLimits($: EngineInterface, limits: readonly SessionRateLimit[], at: number): Promise<TakoRateLimit[]> {
  let seen: TakoLimitsSeen = {}
  try {
    seen = (await $.state.get(LIMITS_SEEN)).value ?? {}
  } catch (err) {
    $.ui.log(`tako mod: limitsSeen unreadable: ${String(err).slice(0, 200)}`, { to: 'debug' })
  }
  const next: TakoLimitsSeen = {}
  let changed = Object.keys(seen).length !== limits.length
  const out = limits.map(limit => {
    const prev = seen[limit.kind]
    const same = prev !== undefined && prev.percent_used === limit.percentUsed && prev.resets_at === limit.resetsAt
    if (!same) changed = true
    const observed = same ? prev.observed_at : at
    next[limit.kind] =
      limit.resetsAt === undefined
        ? { percent_used: limit.percentUsed, observed_at: observed }
        : { percent_used: limit.percentUsed, resets_at: limit.resetsAt, observed_at: observed }
    return { kind: limit.kind, percent_used: limit.percentUsed, resets_at: limit.resetsAt, observed_at: observed }
  })
  if (changed) {
    try {
      await $.state.set(LIMITS_SEEN, next)
    } catch (err) {
      $.ui.log(`tako mod: limitsSeen unwritable: ${String(err).slice(0, 200)}`, { to: 'debug' })
    }
  }
  return out
}

// 利用者が Claude Code 側で tako mod を止めたか（/plugin の disable = settings の enabledPlugins に
// false。#1959）。env の注入（inline）で読まれていても従う = 注入が利用者の選択を上書きしない。
// 読めなければ止めていない扱い（報告は止めない = §5 の「失敗で騒がない」）
async function userDisabled($: EngineInterface): Promise<boolean> {
  try {
    const enabled = (await $.settings.read()).enabledPlugins
    return typeof enabled === 'object' && enabled !== null && (enabled as Record<string, unknown>)[SKILLS_ID] === false
  } catch (err) {
    $.ui.log(`tako mod: settings unreadable: ${String(err).slice(0, 200)}`, { to: 'debug' })
    return false
  }
}

// 休眠に入る（#1959）: 理由を 1 回だけ報告して timer を止め、帯の材料を消す
// （tako は画面の読み取りへ落ちる。次に起きるのは session.start = claude の起動し直しかホットリロード）
async function goDormant($: EngineInterface, reason: 'user_disabled'): Promise<void> {
  timer?.cancel()
  timer = undefined
  await send($, false, FINAL_TIMEOUT_MS, reason)
  cli = undefined
  await setView($, null)
}

// 報告を組む。欠けた値（最初の API 応答の前の ctx% など）は undefined のまま = JSON から落ちる
async function buildReport($: EngineInterface, ended: boolean, dormant?: 'user_disabled'): Promise<TakoModReport> {
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
    rate_limits: await stampLimits($, usage.rateLimits, at),
    cost_usd: usage.cost?.usd,
    turn,
    pending_tool: pendingTool,
    last_turn: lastTurn,
    classic_events: classicEvents,
    config_dir: await $.env.get('CLAUDE_CONFIG_DIR'),
    band: { hidden: bandHidden, shown: bandShown, columns: bandColumns, segments: bandSegments, toggled_at: bandToggledAt },
    dormant,
    ended,
  }
}

// 1 回送る。応答（tako 側のスナップショット = 帯・サイドバーの材料）を $.state へ写す
async function send($: EngineInterface, ended: boolean, timeoutMs: number, dormant?: 'user_disabled'): Promise<void> {
  const path = cli
  if (path === undefined) return
  dirty = false
  try {
    await syncStore($)
    const report = await buildReport($, ended, dormant)
    lastSentAt = report.at
    const done = await $.process.run([path, 'mod', 'report'], { stdin: JSON.stringify(report), timeoutMs })
    if (done.exitCode !== 0) {
      $.ui.log(`tako mod: report exit ${done.exitCode}: ${done.stderr.trim().slice(0, 200)}`, { to: 'debug' })
    } else if (!ended && dormant === undefined) {
      await absorb($, done.stdout)
    }
  } catch (err) {
    $.ui.log(`tako mod: report failed: ${String(err).slice(0, 200)}`, { to: 'debug' })
  }
}

// 1 秒ごとの flush。変化があれば送り、無くても 15 秒に 1 回は送る（heartbeat）
async function tick($: EngineInterface): Promise<void> {
  if (cli === undefined || sending) return
  const now = await $.clock.now()
  // tako が応答しなくなったら（落ちた・別の tako へ繋がらない）古い材料で描き続けない
  if (lastViewJson !== undefined && lastViewJson !== 'null' && now - viewAt > VIEW_FRESH_MS) {
    await setView($, null)
  }
  // 走っている間に /plugin で止められたら従う（#1959。見直しは heartbeat と同じ間隔）
  const recheck = now - lastDisabledCheck >= HEARTBEAT_MS
  if (!recheck && !dirty && now - lastSentAt < HEARTBEAT_MS) return
  sending = true
  try {
    if (recheck) {
      lastDisabledCheck = now
      if (await userDisabled($)) {
        await goDormant($, 'user_disabled')
        return
      }
    }
    if (dirty || now - lastSentAt >= HEARTBEAT_MS) await send($, false, REPORT_TIMEOUT_MS)
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
  // 利用者が /plugin で止めていれば、理由を 1 回だけ伝えて休眠する（#1959）
  lastDisabledCheck = await $.clock.now()
  if (await userDisabled($)) {
    await goDormant($, 'user_disabled')
    return
  }
  dirty = true
  // ホットリロードでは $.state の材料が残っている。次の応答まで（最大 45 秒）はそれで描く
  viewAt = await $.clock.now()
  timer = $.clock.every(FLUSH_MS, () => {
    void tick($)
  })
  // 帯の準備は報告の後ろに置き、失敗しても報告（S1 / S2）を止めない
  try {
    await syncStore($)
    await registerCommand($)
  } catch (err) {
    $.ui.log(`tako mod: band setup failed: ${String(err).slice(0, 200)}`, { to: 'debug' })
  }
}

// `/tako` を登録する（同じ名前の登録は置き換わる = 言語が変わったら登録し直す）。
// 説明文の頭に tako を付けない（Claude Code が一覧にプラグイン名を添える）
async function registerCommand($: EngineInterface): Promise<void> {
  commandLang = lang
  await $.command.register({
    name: 'tako',
    description: words(lang).command,
    argumentHint: '[band on|off]',
    immediate: true,
  })
}

// 最後の報告（ended: true）。tako はそのペインの報告を即座に捨てる
async function finish($: EngineInterface): Promise<void> {
  timer?.cancel()
  timer = undefined
  await send($, true, FINAL_TIMEOUT_MS)
  cli = undefined
}

// --- 帯とサイドバー（S3 #1881）------------------------------------------------------

type Words = {
  tab: string
  workers: (n: number) => string
  attention: (n: number) => string
  noWorkers: string
  more: (n: number) => string
  ctxPending: string
  resetsIn: (d: string) => string
  state: (w: TakoWorker) => string
  thresholds: (ctx: number, limit: number) => string
  hide: string
  show: string
  noView: string
  hidden: string
  shown: string
  usage: string
  notPlaced: (reason: string) => string
  command: string
}

const WORDS: Record<'ja' | 'en', Words> = {
  ja: {
    tab: 'タブ',
    workers: n => `worker ${n}`,
    attention: n => `要注意 ${n}`,
    noWorkers: 'worker なし',
    more: n => `ほか ${n} 本`,
    ctxPending: '最初の応答の前',
    resetsIn: d => `リセットまで ${d}`,
    state: w => {
      if (w.attention === 'permission') return '承認待ち'
      if (w.attention === 'question') return '質問待ち'
      if (w.attention === 'dialog') return '選択待ち'
      if (w.state === 'limited') return '使用制限'
      if (w.state === 'failed') return '異常終了'
      if (w.state === 'busy') return '作業中'
      if (w.state === 'idle') return '待機'
      return '不明'
    },
    thresholds: (c, l) => `帯に ctx / 使用制限を出すのは ${c}% / ${l}% 以上`,
    hide: '帯を隠す',
    show: '帯を出す',
    noView: 'tako から応答が無い（tako のペインの外か、tako が止まっている）',
    hidden: '帯を隠した（/tako band on で戻す）',
    shown: '帯を出した',
    usage: '使い方: /tako（サイドバー）・/tako band on|off（帯）',
    notPlaced: r => `サイドバーを置けない: ${r}`,
    command: 'このペインの tako の状況をサイドバーに出す（band on|off で帯を出す / 隠す）',
  },
  en: {
    tab: 'tab',
    workers: n => `workers ${n}`,
    attention: n => `attention ${n}`,
    noWorkers: 'no workers',
    more: n => `${n} more`,
    ctxPending: 'before the first reply',
    resetsIn: d => `resets in ${d}`,
    state: w => {
      if (w.attention === 'permission') return 'needs approval'
      if (w.attention === 'question') return 'has a question'
      if (w.attention === 'dialog') return 'waiting on a choice'
      if (w.state === 'limited') return 'rate-limited'
      if (w.state === 'failed') return 'exited with an error'
      if (w.state === 'busy') return 'working'
      if (w.state === 'idle') return 'idle'
      return 'unknown'
    },
    thresholds: (c, l) => `the band shows ctx / limits from ${c}% / ${l}%`,
    hide: 'Hide band',
    show: 'Show band',
    noView: 'No reply from tako (outside a tako pane, or tako has stopped)',
    hidden: 'band hidden (/tako band on to show it)',
    shown: 'band shown',
    usage: 'usage: /tako (sidebar), /tako band on|off (band)',
    notPlaced: r => `the sidebar cannot be placed: ${r}`,
    command: "Show this pane's tako status in a sidebar (band on|off shows / hides the band)",
  },
}

function words(code: string): Words {
  return code === 'ja' ? WORDS.ja : WORDS.en
}

// 端末の桁数（全角・CJK は 2 桁。Claude Code の配置と同じ数え方の近似）
function isWide(c: number): boolean {
  return (
    (c >= 0x1100 && c <= 0x115f) ||
    (c >= 0x2e80 && c <= 0xa4cf) ||
    (c >= 0xac00 && c <= 0xd7a3) ||
    (c >= 0xf900 && c <= 0xfaff) ||
    (c >= 0xfe30 && c <= 0xfe4f) ||
    (c >= 0xff00 && c <= 0xff60) ||
    (c >= 0xffe0 && c <= 0xffe6) ||
    (c >= 0x20000 && c <= 0x3fffd)
  )
}

function cellWidth(text: string): number {
  let width = 0
  for (const ch of text) width += isWide(ch.codePointAt(0) ?? 0) ? 2 : 1
  return width
}

// max 桁に収める（溢れたら末尾を … にする）
function clipCells(text: string, max: number): string {
  if (cellWidth(text) <= max) return text
  let out = ''
  let width = 0
  for (const ch of text) {
    const w = isWide(ch.codePointAt(0) ?? 0) ? 2 : 1
    if (width + w > max - 1) break
    out += ch
    width += w
  }
  return `${out}…`
}

function limitName(kind: string): string {
  if (kind === 'five_hour') return '5h'
  if (kind === 'seven_day') return '7d'
  if (kind.startsWith('seven_day_')) return `7d ${kind.slice('seven_day_'.length)}`
  return kind.replace(/_/g, ' ')
}

/** 帯の区切り 1 つ。rank が小さいほど最後まで残す（0 / 1 = tako とペイン名は落とさない） */
type BandSegment = { kind: string; label: string; tone?: 'warning'; rank: number }

// 帯の区切りを左から並べる（何を出すかは tako の view が決めている）
function segmentsOf(view: TakoView): BandSegment[] {
  const w = words(view.lang)
  const name = view.pane_title ?? `pane ${view.pane}`
  const out: BandSegment[] = [
    { kind: 'tako', label: 'tako', rank: 0 },
    { kind: 'pane', label: clipCells(name, NAME_MAX), rank: 1 },
  ]
  if (view.tab_title !== undefined && view.tab_title !== '' && view.tab_title !== name) {
    out.push({ kind: 'tab', label: `${w.tab} ${clipCells(view.tab_title, NAME_MAX)}`, rank: 6 })
  }
  if (view.worker_count > 0) out.push({ kind: 'workers', label: w.workers(view.worker_count), rank: 5 })
  if (view.attention > 0) out.push({ kind: 'attention', label: w.attention(view.attention), tone: 'warning', rank: 2 })
  view.warnings.forEach((warn, i) => {
    const pct = `${Math.round(warn.percent)}%`
    if (warn.kind === 'ctx') {
      out.push({ kind: 'ctx', label: `ctx ${pct}`, tone: 'warning', rank: 3 })
    } else {
      // 使用制限は % の大きい順に来る。小さい方から落とす
      out.push({ kind: warn.kind, label: `${limitName(warn.kind)} ${pct}`, tone: 'warning', rank: 4 + i / 100 })
    }
  })
  return out
}

function lineWidth(segments: readonly BandSegment[]): number {
  return segments.reduce((sum, seg, i) => sum + cellWidth(seg.label) + (i === 0 ? 0 : SEP.length), 0)
}

// columns 桁に収まるまで、優先度の低い区切りから落とす（並びは変えない）。
// tako とペイン名だけでも溢れる幅では、描画側の wrap="truncate-end" が末尾を切る = 必ず 1 行
function fitBand(view: TakoView, columns: number): BandSegment[] {
  let kept = segmentsOf(view)
  const order = [...kept].sort((a, b) => b.rank - a.rank)
  for (const drop of order) {
    if (lineWidth(kept) <= columns || drop.rank <= 1) break
    kept = kept.filter(seg => seg !== drop)
  }
  return kept
}

// 直近の描画を控え、変わったら次の報告で tako へ返す
function noteBand(shown: boolean, columns: number | undefined, segments: string[]): void {
  const key = JSON.stringify([shown, columns, segments])
  if (key === lastBandKey) return
  lastBandKey = key
  bandShown = shown
  bandColumns = columns
  bandSegments = segments
  dirty = true
}

// 残り時間（リセットまで）。端末の時刻帯に依らない書き方にする
function untilText(ms: number): string {
  const minutes = Math.max(0, Math.round(ms / 60_000))
  const days = Math.floor(minutes / 1440)
  const hours = Math.floor((minutes % 1440) / 60)
  const mins = minutes % 60
  if (days > 0) return `${days}d${hours}h`
  if (hours > 0) return `${hours}h${mins}m`
  return `${mins}m`
}

type SavedBand = { hidden: boolean; at: number }

function savedBand(value: unknown): SavedBand | undefined {
  if (typeof value !== 'object' || value === null) return undefined
  const v = value as { hidden?: unknown; at?: unknown }
  if (typeof v.hidden !== 'boolean' || typeof v.at !== 'number') return undefined
  return { hidden: v.hidden, at: v.at }
}

// tako の材料を $.state へ（同じなら書かない）
async function setView($: EngineInterface, view: TakoView | null): Promise<void> {
  const json = JSON.stringify(view)
  if (json === lastViewJson) return
  lastViewJson = json
  await $.state.set(VIEW, view)
}

// トグルを変える（$.store はこの読み込み元の写し。正本の ui.json へは報告の toggled_at で渡る = #1960。
// $.state は描画の購読のための写し）
async function setHidden($: EngineInterface, hidden: boolean, at: number): Promise<void> {
  bandToggledAt = at
  if (hidden !== bandHidden) {
    bandHidden = hidden
    dirty = true
  }
  await $.store.set(BAND_KEY, { hidden, at })
  await $.state.set(HIDDEN, hidden)
}

// $.store を読み直す（同じ設定 dir の別のセッションが切り替えたぶんを拾う）。
// 読めなくても報告は続ける（帯のトグルが前の値のままになるだけ）
async function syncStore($: EngineInterface): Promise<void> {
  let stored: unknown
  try {
    stored = await $.store.get(BAND_KEY)
  } catch {
    return
  }
  const saved = savedBand(stored)
  if (saved === undefined || (bandToggledAt !== undefined && saved.at <= bandToggledAt)) return
  bandToggledAt = saved.at
  if (saved.hidden !== bandHidden) {
    bandHidden = saved.hidden
    dirty = true
  }
  await $.state.set(HIDDEN, saved.hidden)
}

// サイドバーのボタン。押した時点のトグルを反転する（描いた時点の値は使わない）
async function toggleBand($: EngineInterface): Promise<void> {
  await setHidden($, !bandHidden, await $.clock.now())
}

// `tako mod report` の応答を取り込む。view が無い（S3 の A/B・古い tako）なら何も描かない
async function absorb($: EngineInterface, stdout: string): Promise<void> {
  let view: TakoView | null = null
  try {
    const reply = JSON.parse(stdout) as { tako?: { view?: TakoView | null } }
    view = reply.tako?.view ?? null
  } catch {
    view = null
  }
  if (view !== null) {
    viewAt = await $.clock.now()
    lang = view.lang
    if (commandLang !== lang) {
      try {
        await registerCommand($)
      } catch {
        // 説明文が前の言語のままになるだけ
      }
    }
    const request = view.band_request
    if (request !== undefined && request.at > (bandToggledAt ?? 0)) {
      await setHidden($, request.hidden, request.at)
    }
  }
  await setView($, view)
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

  // effort は各ステップの turn.step が運ぶ（関数フックなので classic 系が届かない組織アカウントでも
  // 来る = FR-2.42.7 で欠けていた effort を埋める。#1880）。サブエージェントのステップは数えない。
  // turn.step はモデルの出力を流すストリームのフック: 値を控えたら `yield*` で**そのまま素通し**し、
  // チャンクには触らない（壊れても .catch が素通しへ戻す）
  on('turn.step', async function* ($, e, next) {
    const step = e as unknown as { effort?: unknown; agentId?: unknown }
    const level = effortLevel(step.effort)
    if (step.agentId === undefined && level !== undefined && level !== effort) {
      effort = level
      dirty = true
    }
    return yield* next(e)
  }).catch(async function* ($, e, next) {
    return yield* next(e)
  })

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

  // 帯（プロンプトの上の 1 行）。tako の外・材料が無い（古い）・隠している・調査票が使っている
  // ときは何も描かない。権限ダイアログ・質問の表示中はそもそも Claude Code が帯を出さない
  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const view = (await $.state.get(VIEW)).value ?? null
    const hidden = (await $.state.get(HIDDEN)).value ?? false
    const columns = e.props.bodyColumns
    if (cli === undefined || view === null || hidden || e.props.hasSurvey) {
      noteBand(false, columns, [])
      return next(e)
    }
    const segments = fitBand(view, columns)
    noteBand(true, columns, segments.map(seg => seg.kind))
    const { Text } = $.ui.resolve(e)
    const parts: Array<ReturnType<typeof Text> | string> = []
    segments.forEach((seg, i) => {
      if (i > 0) parts.push(SEP)
      parts.push(seg.tone === 'warning' ? Text({ color: 'warning', children: seg.label }) : seg.label)
    })
    return Text({ dimColor: true, wrap: 'truncate-end', children: parts })
  }).catch(($, e, next) => next(e))

  // `/tako` のサイドバー（詳細）
  on('ui.render', { component: 'Pane', requestId: PANE_ID }, async ($, e, next) => {
    const view = (await $.state.get(VIEW)).value ?? null
    const hidden = (await $.state.get(HIDDEN)).value ?? false
    const w = words(view?.lang ?? lang)
    const { Box, Button, Text } = $.ui.resolve(e)
    const toggle = Button({
      key: 'band-toggle',
      label: hidden ? w.show : w.hide,
      onPress: () => toggleBand($),
    })
    if (view === null) {
      return Box({ flexDirection: 'column', children: [Text({ dimColor: true, children: w.noView }), toggle] })
    }
    const now = await $.clock.now()
    const rows: Array<ReturnType<typeof Text>> = []
    const name = view.pane_title ?? `pane ${view.pane}`
    rows.push(Text({ bold: true, wrap: 'truncate-end', children: name }))
    if (view.tab_title !== undefined) rows.push(Text({ dimColor: true, wrap: 'truncate-end', children: `${w.tab} ${view.tab_title}` }))
    const ctx = view.ctx
    rows.push(
      Text({
        children:
          ctx?.percent === undefined
            ? `ctx - (${w.ctxPending})`
            : `ctx ${ctx.percent}% (${ctx.tokens ?? '-'} / ${ctx.window})`,
      }),
    )
    for (const limit of view.rate_limits) {
      const reset = limit.resets_at === undefined ? '' : `  ${w.resetsIn(untilText(limit.resets_at * 1000 - now))}`
      const line = `${limitName(limit.kind)} ${Math.round(limit.percent)}%${reset}`
      rows.push(limit.percent >= view.thresholds.limit_percent ? Text({ color: 'warning', children: line }) : Text({ children: line }))
    }
    rows.push(
      Text({
        bold: true,
        children:
          view.worker_count === 0 ? w.noWorkers : `${w.workers(view.worker_count)}  ${w.attention(view.attention)}`,
      }),
    )
    for (const worker of view.workers) {
      const line = `${worker.attention === undefined ? '  ' : '! '}${worker.name}  ${w.state(worker)}`
      rows.push(
        worker.attention === undefined
          ? Text({ wrap: 'truncate-end', children: line })
          : Text({ color: 'warning', wrap: 'truncate-end', children: line }),
      )
    }
    if (view.worker_count > view.workers.length) {
      rows.push(Text({ dimColor: true, children: w.more(view.worker_count - view.workers.length) }))
    }
    rows.push(Text({ dimColor: true, children: w.thresholds(view.thresholds.ctx_percent, view.thresholds.limit_percent) }))
    return Box({ flexDirection: 'column', children: [...rows, toggle] })
  }).catch(($, e, next) => next(e))

  // `/tako`（サイドバーを開く）・`/tako band on|off`（帯のトグル）。tako の外では登録していない。
  // 出力の行には Claude Code がプラグイン名（tako:）を前置するので、文言には付けない。
  // 出力の行はモデルも読むので短くする（サイドバーを開いたときは何も出さない）
  on('command.run', { command: 'tako' }, async ($, e, next) => {
    if (cli === undefined) return next(e)
    const w = words(lang)
    const args = (typeof e.args === 'string' ? e.args : '').trim().split(/\s+/).filter(arg => arg !== '')
    if (args[0] === 'band' && (args[1] === undefined || args[1] === 'on' || args[1] === 'off') && args.length <= 2) {
      const want = args[1] === 'on' ? false : args[1] === 'off' ? true : !bandHidden
      await setHidden($, want, await $.clock.now())
      return { text: want ? w.hidden : w.shown }
    }
    if (args.length > 0) return { text: w.usage }
    const opened = await $.ui.open({ id: PANE_ID, title: 'tako' })
    return opened.isPlaced ? {} : { text: w.notPlaced(opened.reason) }
  }).catch(($, e, next) => next(e))
}
