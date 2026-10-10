// tako mod S7-3（#1962）: 帯を next で包む 1 行・カスタムボタン・使用制限 / ctx のバーを `claude plugin test` で確かめる。
// tako 本体は呼ばない: `process.run` をテストの hook で受け、応答（tako 側の材料 = view）を返す。
// テストの `ui.render` の hook は「tako より内側の他の mod」（帯に自分の行を描く）の代役。
// 描画は terminal と desktop の両 surface で同じ本体を回す
import type { On } from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'

type Sent = { argv: readonly string[]; report: Record<string, unknown> }
type Reply = { exitCode: number; view?: unknown }
type World = {
  sent: Sent[]
  runs: string[][]
  commands: string[]
  fills: { text: string; mode?: string }[]
  tails: (string | undefined)[]
}

const SURFACES = ['terminal', 'desktop'] as const
// 80 / 144 / 300 桁の端末で帯の本文に使える桁（右端の 5 桁は Claude Code の `[-]`）
const WIDTHS = [75, 139, 295] as const

const USAGE = {
  startedAt: 0,
  context: { tokens: 60000, window: 1000000, percent: 6 },
  rateLimits: [],
  cost: { usd: 0.007 },
}

// ui.json の既定（tako_core::claude_mod_ui::UiConfig::default）+ 足したボタン
function uiJson(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    schema_version: 1,
    band: { hidden: false, segments: ['pane', 'tab', 'workers', 'attention', 'card', 'buttons'] },
    usage_bar: { place: 'prompt_hint', items: ['five_hour', 'seven_day', 'ctx'] },
    buttons: [{ id: 'compact', label: 'compact', hotkey: 'c', action: { kind: 'slash', command: 'compact' } }],
    cards: { place: 'auto' },
    chat: { place: 'auto', bubble: 'gui_only', code_copy: true, tool_summary: 'gui_only' },
    colors: { accent: 'suggestion', warn: 'warning', dim: 'subtle' },
    ...extra,
  }
}

const MORE_BUTTONS = [
  { id: 'compact', label: 'compact', hotkey: 'c', action: { kind: 'slash', command: 'compact' } },
  { id: 'split-right', label: 'split right', hotkey: 's', action: { kind: 'tako', op: 'split-right' } },
  { id: 'make', label: 'make test', hotkey: 'm', action: { kind: 'shell', command: 'make test' } },
  { id: 'go-on', label: '続けて', hotkey: 'g', action: { kind: 'prompt', text: '続けてください' } },
]
const ARGS = {
  'split-right': ['split', '--pane', '7', '--right'],
  make: ['run-interactive', '--pane', '7', '--down', '--ratio', '0.35', '--auto-close', 'never', '--', 'make test'],
}

// S7 の材料（ui.json と tako の判断が載った新しい tako の応答）。長い名前と要注意で帯が混む
function view(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    pane: 7,
    pane_title: 'オーケストレーターの master ペイン（長い名前の切り詰めを見る）',
    tab_title: 'tako-wt-1962 の作業タブ（これも長い）',
    lang: 'ja',
    worker_count: 3,
    attention: 1,
    workers: [{ pane: 9, name: 'fix-1962', state: 'waiting', attention: 'permission' }],
    warnings: [],
    ctx: { tokens: 60000, window: 1000000, percent: 6 },
    rate_limits: [
      { kind: 'five_hour', percent: 4, resets_at: 1791477000 },
      { kind: 'seven_day', percent: 22.4, resets_at: 1791651600 },
    ],
    thresholds: { ctx_percent: 80, limit_percent: 80 },
    ui: uiJson(),
    usage_bar: { draw: true },
    button_args: {},
    ...extra,
  }
}

// エンジンの下の世界。reply.view で tako の応答を差し替える。inner = 内側の他の mod が帯に描く行（無ければエンジンだけ）
function world(on: On, reply: Reply, inner: string | null = 'other-mod line'): World {
  const w: World = { sent: [], runs: [], commands: [], fills: [], tails: [] }
  on('process.run', ($, e) => {
    if (e.argv[1] === 'mod' && e.argv[2] === 'report') {
      w.sent.push({ argv: e.argv, report: JSON.parse(e.init?.stdin ?? '{}') as Record<string, unknown> })
      const body = reply.view === undefined ? { accepted: 'stored', tako: {} } : { accepted: 'stored', tako: { view: reply.view } }
      const stdout = reply.exitCode === 0 ? JSON.stringify(body) : ''
      return { value: { exitCode: reply.exitCode, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
    }
    w.runs.push([...e.argv])
    const exitCode = e.argv.includes('fail-me') ? 1 : 0
    return { value: { exitCode, stdout: '', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('session.usage', () => ({ value: USAGE }))
  on('session.model', () => ({ value: 'claude-haiku-5-5' }))
  on('session.id', () => ({ value: 'session-1' }))
  on('session.version', () => ({ value: { version: '2.1.294' } }))
  on('ui.log', () => ({ value: undefined }))
  on('command.register', ($, e) => ({ value: { command: e.name } }))
  on('command.list', () => ({
    value: [
      { name: 'compact', description: 'compact', source: 'builtin' },
      { name: 'context', description: 'context', source: 'builtin' },
    ],
  }))
  on('command.run', ($, e) => {
    w.commands.push(e.command)
    return { text: '' }
  })
  on('prompt.fill', ($, e) => {
    w.fills.push({ text: e.text, mode: e.mode })
    return { isFilled: true }
  })
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('session.end', ($, e) => ({ sessionId: e.sessionId }))
  // 内側の他の mod（帯に自分の行を描く）。入力欄の下の行は受け取った tail を控えて描く。
  // inner が無ければ帯はエンジン自身の描画（core が next の答えに返す { type: 'engine' }。テストに core は無い）
  on('ui.render', ($, e) => {
    const { Box, Text } = $.ui.resolve(e)
    if (e.component === 'PromptHint') {
      w.tails.push(e.props.tail)
      return Text({ children: `${e.props.hint}${e.props.tail === undefined ? '' : ` ${e.props.tail}`}` })
    }
    if (e.component === 'AbovePrompt') return inner === null ? ENGINE : Text({ children: inner })
    return Box({ key: 'engine-own' })
  })
  return w
}

const START = { cwd: '/tmp', surface: 'terminal', isInteractive: true } as const
const IN_TAKO = { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' }
// core が自分で描くときの next の答え（下に他の mod が居ない帯）
const ENGINE = { type: 'engine' as const, ref: 0 }

function band(bodyColumns: number) {
  return {
    plugin: 'tako',
    component: 'AbovePrompt' as const,
    props: { hasSurvey: false, isWorking: false, maxRows: 1, bodyColumns, scroll: { offset: 0, bodyRows: 1 }, view: {} },
  }
}

function hint(tail?: string) {
  return {
    plugin: 'tako',
    component: 'PromptHint' as const,
    props: tail === undefined ? { isDraft: false, isWorking: false, hint: '? for shortcuts' } : { isDraft: false, isWorking: false, hint: '? for shortcuts', tail },
  }
}

// 端末の桁数（register.ts と同じ数え方。全角・CJK は 2 桁）
function cells(text: string): number {
  let width = 0
  for (const ch of text) {
    const c = ch.codePointAt(0) ?? 0
    const wide =
      (c >= 0x1100 && c <= 0x115f) ||
      (c >= 0x2e80 && c <= 0xa4cf) ||
      (c >= 0xac00 && c <= 0xd7a3) ||
      (c >= 0xf900 && c <= 0xfaff) ||
      (c >= 0xfe30 && c <= 0xfe4f) ||
      (c >= 0xff00 && c <= 0xff60) ||
      (c >= 0xffe0 && c <= 0xffe6) ||
      (c >= 0x20000 && c <= 0x3fffd)
    width += wide ? 2 : 1
  }
  return width
}

type Drawn = { type: string; props?: Record<string, unknown>; children?: Drawn[] | string[] }

// tako の行（Text + Button の横並び）の桁: Text の文字 + ボタンごとに間の 1 桁と `[ ` ` ]`
async function rowCells(found: { text?: string } | undefined, buttons: { props: Record<string, unknown> }[]): Promise<number> {
  return cells(found?.text ?? '') + buttons.reduce((sum, b) => sum + 1 + 4 + cells(String(b.props.label)), 0)
}

async function started(on: On, $: Parameters<Parameters<typeof test>[1]>[0], reply: Reply, inner: string | null = 'other-mod line') {
  const w = world(on, reply, inner)
  const clock = mock.clock(on, { now: 1791470000000 })
  mock.env(on, IN_TAKO)
  mock.store(on)
  await $.session.start(START)
  await clock.advance(1_000)
  return { w, clock }
}

describe('tako mod の帯（S7-3: next を包む 1 行）', () => {
  test('他の mod の行を上に残し、tako の行をその下に 1 行で足す（両 surface）', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(139), surface })
      expect(await ui.find({ type: 'Text', text: 'other-mod line' })).toBeDefined()
      const tree = (await ui.drawn()) as Drawn
      expect(tree.type).toBe('Box')
      expect(tree.props?.flexDirection).toBe('column')
      const rows = tree.children as Drawn[]
      expect(rows.length).toBe(2)
      // 2 行目 = tako の行（Text + ボタンの横並び）
      expect(rows[1]?.props?.flexDirection).toBe('row')
      expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeDefined()
      expect(await ui.find({ type: 'Button', key: 'tako-button-compact' })).toBeDefined()
      await ui.unmount()
    }
    await clock.advance(1_000)
    const renders = w.sent.at(-1)?.report.renders as Record<string, unknown>
    expect(renders).toEqual(expect.objectContaining({ band: true, buttons: 1, band_hook: true }))
  })

  test('下に他の mod が居なければ（エンジンだけ）tako の行だけを返す', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() }, null)
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    const tree = (await ui.drawn()) as Drawn
    expect(tree.props?.flexDirection).toBe('row')
    expect(await ui.find({ key: 'engine-own' })).toBeUndefined()
    await ui.unmount()
  })

  test('隠している・tako の外・調査票の表示中は他の mod の行だけを残す（next へ譲る）', async ($, on) => {
    const { w } = await started(on, $, { exitCode: 0, view: view({ band_request: { hidden: true, at: 1791470005000 } }) })
    const mounted = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await mounted.find({ type: 'Text', text: 'other-mod line' })).toBeDefined()
    expect(await mounted.find({ type: 'Text', text: /^tako \| / })).toBeUndefined()
    await mounted.unmount()
    expect(w.sent.length).toBeGreaterThan(0)
  })

  test('80 / 144 / 300 桁・帯 1 行でボタン込み 1 行に収まり、狭いほど優先度の低いものから落ちる', async ($, on) => {
    const { w, clock } = await started(on, $, {
      exitCode: 0,
      view: view({ ui: uiJson({ buttons: MORE_BUTTONS }), button_args: ARGS }),
    })
    for (const surface of SURFACES) {
      const seen: Record<number, { segments: string[]; buttons: number }> = {}
      for (const columns of WIDTHS) {
        const ui = await $.ui.mount({ ...band(columns), surface })
        const line = await ui.find({ type: 'Text', text: /^tako/ })
        expect(line?.text ?? '').not.toContain('\n')
        const buttons = (await ui.findAll({ type: 'Button' })) as unknown as { props: Record<string, unknown> }[]
        expect(await rowCells(line, buttons)).toBeLessThanOrEqual(columns)
        await clock.advance(1_000)
        const reported = w.sent.at(-1)?.report.band as { columns: number; segments: string[] }
        const renders = w.sent.at(-1)?.report.renders as { buttons: number }
        expect(reported.columns).toBe(columns)
        expect(renders.buttons).toBe(buttons.length)
        seen[columns] = { segments: reported.segments, buttons: buttons.length }
        await ui.unmount()
      }
      // 300 桁は全部（区切り + ボタン 4 つ）、80 桁はタブと worker 数・後ろのボタンから落ちる
      expect(seen[295]).toEqual({ segments: ['tako', 'pane', 'tab', 'workers', 'attention', 'buttons'], buttons: 4 })
      expect(seen[75]?.segments.slice(0, 3)).toEqual(['tako', 'pane', 'attention'])
      expect(seen[75]?.segments).not.toContain('tab')
      expect(seen[75]?.buttons ?? 0).toBeLessThan(4)
      expect(seen[75]?.buttons ?? 0).toBeGreaterThanOrEqual(1)
      expect(seen[139]?.buttons ?? 0).toBeGreaterThanOrEqual(seen[75]?.buttons ?? 0)
    }
  })

  test('極端に狭い幅ではボタンを全部落とし、tako の行は末尾を切って 1 行', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view({ ui: uiJson({ buttons: MORE_BUTTONS }), button_args: ARGS }) })
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(12), surface })
      expect(await ui.findAll({ type: 'Button' })).toHaveLength(0)
      const found = await ui.find({ type: 'Text', text: /^tako/ })
      expect(found?.props.wrap).toBe('truncate-end')
      await ui.unmount()
    }
    await clock.advance(1_000)
    expect((w.sent.at(-1)?.report.renders as { buttons: number }).buttons).toBe(0)
  })

  test('ボタンが 0 個・band.segments に buttons が無い・Claude Code に無いコマンドは描かない', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view({ ui: uiJson({ buttons: [] }) }) }
    const { clock } = await started(on, $, reply)
    let ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.findAll({ type: 'Button' })).toHaveLength(0)
    expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeDefined()
    await ui.unmount()
    reply.view = view({ ui: uiJson({ band: { hidden: false, segments: ['pane', 'attention'] } }) })
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.findAll({ type: 'Button' })).toHaveLength(0)
    await ui.unmount()
    // /cost は command.list に無い（このテストの世界）= 描かない。tako の操作は引数が届かなければ描かない
    reply.view = view({
      ui: uiJson({
        buttons: [
          { id: 'cost', label: 'cost', hotkey: 'o', action: { kind: 'slash', command: 'cost' } },
          { id: 'split-right', label: 'split right', hotkey: 's', action: { kind: 'tako', op: 'split-right' } },
        ],
      }),
      button_args: {},
    })
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.findAll({ type: 'Button' })).toHaveLength(0)
    await ui.unmount()
  })
})

describe('tako mod のカスタムボタン（S7-3）', () => {
  test('押すと語彙どおりに実行し、種類と成否だけを報告する', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view({ ui: uiJson({ buttons: MORE_BUTTONS }), button_args: ARGS }) })
    const ui = await $.ui.mount({ ...band(295), surface: 'terminal' })
    // slash = $.command.run（/compact）
    await ui.press({ key: 'tako-button-compact' })
    expect(w.commands).toEqual(['compact'])
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.last_press).toEqual(expect.objectContaining({ kind: 'slash', ok: true }))
    // tako = tako が組んだ CLI の引数をそのまま
    await ui.press({ key: 'tako-button-split-right' })
    expect(w.runs.at(-1)).toEqual(['/opt/tako/tako', 'split', '--pane', '7', '--right'])
    // shell = 新しいペインで実行（引数は tako が組む。ここでシェルの文字列を組まない）
    await ui.press({ key: 'tako-button-make' })
    expect(w.runs.at(-1)).toEqual(['/opt/tako/tako', ...ARGS.make])
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.last_press).toEqual(expect.objectContaining({ kind: 'shell', ok: true }))
    // prompt = 入力欄へ入れるだけ（送らない）
    await ui.press({ key: 'tako-button-go-on' })
    expect(w.fills).toEqual([{ text: '続けてください', mode: 'insert' }])
    await clock.advance(1_000)
    const press = w.sent.at(-1)?.report.last_press as Record<string, unknown>
    expect(press).toEqual(expect.objectContaining({ kind: 'prompt', ok: true }))
    // 報告にラベル・コマンド・文を載せない
    expect(Object.keys(press).sort()).toEqual(['at', 'kind', 'ok'])
    expect(JSON.stringify(w.sent.at(-1)?.report)).not.toContain('続けてください')
    expect(JSON.stringify(w.sent.at(-1)?.report)).not.toContain('make test')
    await ui.unmount()
  })

  test('CLI が失敗したら ok: false を報告する', async ($, on) => {
    const failing = [{ id: 'bad', label: 'bad', hotkey: 'b', action: { kind: 'shell', command: 'fail-me' } }]
    const { w, clock } = await started(on, $, {
      exitCode: 0,
      view: view({ ui: uiJson({ buttons: failing }), button_args: { bad: ['run-interactive', '--', 'fail-me'] } }),
    })
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    await ui.press({ key: 'tako-button-bad' })
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.last_press).toEqual(expect.objectContaining({ kind: 'shell', ok: false }))
    await ui.unmount()
  })
})

describe('tako mod の使用制限・ctx のバー（S7-3）', () => {
  test('入力欄の下の行の末尾に描き、他の mod の tail の後ろへ足して next へ流す', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    let ui = await $.ui.mount({ ...hint(), surface: 'terminal' })
    expect(w.tails.at(-1)).toBe('5h ▁ 4% 7d ▂ 22% ctx ▁ 6%')
    expect((await ui.find({ type: 'Text' }))?.text).toBe('? for shortcuts 5h ▁ 4% 7d ▂ 22% ctx ▁ 6%')
    await ui.unmount()
    ui = await $.ui.mount({ ...hint('other-tail'), surface: 'terminal' })
    expect(w.tails.at(-1)).toBe('other-tail · 5h ▁ 4% 7d ▂ 22% ctx ▁ 6%')
    await ui.unmount()
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.renders).toEqual(expect.objectContaining({ usage_bar: 'prompt_hint', hint_hook: true }))
  })

  test('tako が描かないと決めたら（statusLine が出している・off）描かず、報告もしない', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view({ usage_bar: { draw: false, reason: 'status_line' } }) }
    const { w, clock } = await started(on, $, reply)
    const ui = await $.ui.mount({ ...hint(), surface: 'terminal' })
    expect(w.tails.at(-1)).toBeUndefined()
    await ui.unmount()
    await clock.advance(1_000)
    const renders = w.sent.at(-1)?.report.renders as Record<string, unknown>
    expect(renders.usage_bar).toBeUndefined()
  })

  test('desktop は tail を描かないので描かない・値が 1 つも無ければ描かない', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view() }
    const { w, clock } = await started(on, $, reply)
    let ui = await $.ui.mount({ ...hint(), surface: 'desktop' })
    expect(w.tails.at(-1)).toBeUndefined()
    await ui.unmount()
    reply.view = view({ ctx: undefined, rate_limits: [] })
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...hint(), surface: 'terminal' })
    expect(w.tails.at(-1)).toBeUndefined()
    await ui.unmount()
    await clock.advance(1_000)
    expect((w.sent.at(-1)?.report.renders as Record<string, unknown>).usage_bar).toBeUndefined()
  })

  test('置き場が帯なら tako の行の中に描き、入力欄の下の行には描かない', async ($, on) => {
    const { w, clock } = await started(on, $, {
      exitCode: 0,
      view: view({ ui: uiJson({ usage_bar: { place: 'band', items: ['ctx', 'five_hour'] } }) }),
    })
    const b = await $.ui.mount({ ...band(295), surface: 'terminal' })
    expect((await b.find({ type: 'Text', text: /^tako/ }))?.text).toContain('ctx ▁ 6% 5h ▁ 4%')
    await b.unmount()
    const h = await $.ui.mount({ ...hint(), surface: 'terminal' })
    expect(w.tails.at(-1)).toBeUndefined()
    await h.unmount()
    await clock.advance(1_000)
    expect((w.sent.at(-1)?.report.renders as Record<string, unknown>).usage_bar).toBe('band')
    expect((w.sent.at(-1)?.report.band as { segments: string[] }).segments).toContain('usage')
  })

  test('利用者の statusLine の有無（中身は載せない）を報告する', async ($, on) => {
    const w = world(on, { exitCode: 0, view: view() })
    on('settings.read', () => ({ value: { statusLine: { type: 'command', command: 'secret-statusline.sh' } } }))
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.status_line).toBe(true)
    expect(JSON.stringify(w.sent.at(-1)?.report)).not.toContain('secret-statusline')
  })
})

describe('A/B（TAKO_1877_S7_LEGACY = band_style s3）', () => {
  test('S3 の描き方へ戻る: 他の mod の行を包まず・ボタンもバーも描かず・renders を報告しない', async ($, on) => {
    const s3 = view({ band_style: 's3', ui: uiJson({ buttons: MORE_BUTTONS }), usage_bar: undefined, button_args: undefined })
    const { w, clock } = await started(on, $, { exitCode: 0, view: s3 })
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(295), surface })
      const tree = (await ui.drawn()) as Drawn
      expect(tree.type).toBe('Text')
      expect(await ui.find({ type: 'Text', text: 'other-mod line' })).toBeUndefined()
      expect(await ui.findAll({ type: 'Button' })).toHaveLength(0)
      await ui.unmount()
    }
    const h = await $.ui.mount({ ...hint(), surface: 'terminal' })
    expect(w.tails.at(-1)).toBeUndefined()
    await h.unmount()
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.renders).toBeUndefined()
  })
})
