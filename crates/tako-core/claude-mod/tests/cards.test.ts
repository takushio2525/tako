// tako mod S7-4（#1963）: コマンドカードを mod で描く（会話の行のカード・帯の `カード N` と Copy / Run）を
// `claude plugin test` で確かめる。tako 本体は呼ばない: `process.run` をテストの hook で受け、応答（tako 側の
// 材料 = view。カードは view.cards）を返す。`ui.copy` もテストの hook で受ける（本物のクリップボードへ書かない）。
// テストの `ui.render` の hook は「tako より内側の他の mod / エンジン」の代役。描画は terminal と desktop の両 surface
import type { On } from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'

type Sent = { argv: readonly string[]; report: Record<string, unknown> }
type Reply = { exitCode: number; view?: unknown }
type World = {
  sent: Sent[]
  runs: string[][]
  copies: { text: string; surface?: string }[]
  groups: boolean[]
}

const SURFACES = ['terminal', 'desktop'] as const
const CLI = '/opt/tako/tako'
// 157 字・改行なしの長いコマンド（設計書 §9.2 の実測と同じ長さ。画面では折り返しても論理文字列は 1 行）
const LONG =
  'cargo build -p tako-cli && TAKO_ISOLATED=1 TAKO_DATA_DIR=/tmp/tako-s7-very-long-data-dir-name ./target/debug/tako mod --json | jq ".panes[] | {pane, reason}"'
const MULTI = 'cd /tmp \\\n  && ls -la\necho done'

const USAGE = {
  startedAt: 0,
  context: { tokens: 60000, window: 1000000, percent: 6 },
  rateLimits: [],
  cost: { usd: 0.007 },
}

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

// tako_core::claude_mod::cards_view と同じ形（引数は tako が組む = card_cli_args）
function cardItem(id: number, commands: string[], label?: string, runs: Record<number, unknown> = {}): Record<string, unknown> {
  return {
    id,
    ...(label === undefined ? {} : { label }),
    commands: commands.map((text, i) => ({
      text,
      copy_args: ['show-command', '--copy', '--card', String(id), '--index', String(i + 1)],
      run_args: ['show-command', '--run', '--card', String(id), '--index', String(i + 1)],
      ...(runs[i + 1] === undefined ? {} : { run: runs[i + 1] }),
    })),
    dismiss_args: ['show-command', '--dismiss', '--card', String(id)],
  }
}

const HOTKEYS = { copy: 'y', run: 'r', next: 'n', dismiss: 'x' }

function cards(items: Record<string, unknown>[]): Record<string, unknown> {
  return { hotkeys: HOTKEYS, items }
}

function view(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    pane: 7,
    pane_title: 'master',
    lang: 'ja',
    worker_count: 0,
    attention: 0,
    workers: [],
    warnings: [],
    ctx: { tokens: 60000, window: 1000000, percent: 6 },
    rate_limits: [],
    thresholds: { ctx_percent: 80, limit_percent: 80 },
    ui: uiJson(),
    usage_bar: { draw: false, reason: 'off' },
    button_args: {},
    cards: cards([cardItem(4, ['npm test']), cardItem(5, [LONG, MULTI], 'ビルドと確認')]),
    ...extra,
  }
}

// copy: 'ok' = 写せた / 'refused' = 他の mod の ui.copy フックが拒んだ
function world(on: On, reply: Reply, copy: 'ok' | 'refused' = 'ok', inner: string | null = null): World {
  const w: World = { sent: [], runs: [], copies: [], groups: [] }
  on('process.run', ($, e) => {
    if (e.argv[1] === 'mod' && e.argv[2] === 'report') {
      w.sent.push({ argv: e.argv, report: JSON.parse(e.init?.stdin ?? '{}') as Record<string, unknown> })
      const body = reply.view === undefined ? { accepted: 'stored', tako: {} } : { accepted: 'stored', tako: { view: reply.view } }
      const stdout = reply.exitCode === 0 ? JSON.stringify(body) : ''
      return { value: { exitCode: reply.exitCode, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
    }
    w.runs.push([...e.argv])
    return { value: { exitCode: 0, stdout: '{}', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('ui.copy', ($, e) => {
    w.copies.push({ text: e.text, surface: e.surface })
    return { value: copy === 'ok' ? { isCopied: true } : { isCopied: false, reason: 'refused' } }
  })
  on('session.usage', () => ({ value: USAGE }))
  on('session.model', () => ({ value: 'claude-haiku-5-5' }))
  on('session.id', () => ({ value: 'session-1' }))
  on('session.version', () => ({ value: { version: '2.1.294' } }))
  on('ui.log', () => ({ value: undefined }))
  on('command.register', ($, e) => ({ value: { command: e.name } }))
  on('command.list', () => ({ value: [{ name: 'compact', description: 'compact', source: 'builtin' }] }))
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('session.end', ($, e) => ({ sessionId: e.sessionId }))
  on('tool.call', () => ({ result: { content: [] } }))
  // 内側（エンジン / 他の mod）。ToolGroup は受け取った isExpanded を控える。ToolUse は inner があれば他の mod の行
  on('ui.render', ($, e) => {
    const { Box, Text } = $.ui.resolve(e)
    if (e.component === 'ToolGroup') {
      w.groups.push(e.props.isExpanded)
      return Text({ children: e.props.isExpanded ? 'expanded group' : 'Called tako' })
    }
    if (e.component === 'ToolUse') return inner === null ? ENGINE : Text({ children: inner })
    if (e.component === 'AbovePrompt') return ENGINE
    return Box({ key: 'engine-own' })
  })
  return w
}

const START = { cwd: '/tmp', surface: 'terminal', isInteractive: true } as const
const IN_TAKO = { TAKO_PANE_ID: '7', TAKO_CLI: CLI }
const ENGINE = { type: 'engine' as const, ref: 0 }
const TOOL = 'mcp__tako__tako_show_command'

function band(bodyColumns: number) {
  return {
    plugin: 'tako',
    component: 'AbovePrompt' as const,
    props: { hasSurvey: false, isWorking: false, maxRows: 1, bodyColumns, scroll: { offset: 0, bodyRows: 1 }, view: {} },
  }
}

function toolUse(output: unknown, input: Record<string, unknown> = { commands: [LONG, MULTI], label: 'ビルドと確認' }, tool = TOOL) {
  return {
    plugin: 'tako',
    component: 'ToolUse' as const,
    props: { tool_use_id: 'toolu_1', tool, input, isRunning: false, isErrored: false, isInterrupted: false, output },
  }
}

function toolGroup(calls: { tool: string; input: unknown }[], isExpanded = false) {
  return {
    plugin: 'tako',
    component: 'ToolGroup' as const,
    props: {
      calls: calls.map((c, i) => ({ tool_use_id: `toolu_${i}`, tool: c.tool, input: c.input, isRunning: false, isErrored: false, isInterrupted: false })),
      isActive: false,
      isExpanded,
    },
  }
}

// MCP の結果の形（content の text に tako の応答の JSON）
function mcpOutput(id: number): unknown {
  return [{ type: 'text', text: JSON.stringify({ card: { id, pane: 7, commands: [LONG, MULTI], count: 2, runs: [null, null] }, pane_cards: 2 }) }]
}

async function started(on: On, $: Parameters<Parameters<typeof test>[1]>[0], reply: Reply, copy: 'ok' | 'refused' = 'ok', inner: string | null = null) {
  const w = world(on, reply, copy, inner)
  const clock = mock.clock(on, { now: 1791470000000 })
  mock.env(on, IN_TAKO)
  mock.store(on)
  await $.session.start(START)
  await clock.advance(1_000)
  return { w, clock }
}

const CARD_KEYS = ['tako-card-copy', 'tako-card-run', 'tako-card-next', 'tako-card-dismiss'] as const

// 描かれたカードのボタンの key（ElementQuery の key は文字列だけ）
async function cardButtons(ui: { find: (q: { type?: string; key?: string }) => Promise<unknown> }): Promise<string[]> {
  const out: string[] = []
  for (const key of CARD_KEYS) if ((await ui.find({ type: 'Button', key })) !== undefined) out.push(key)
  return out
}

function rendersOf(w: World): Record<string, unknown> {
  return (w.sent.at(-1)?.report.renders ?? {}) as Record<string, unknown>
}

describe('帯のカード（#1963）', () => {
  test('いちばん新しいカードの 1 件目を `カード N` とホットキーつきの Copy / Run / 次 / 閉じるで出し、押せるカードを renders.cards で返す', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(139), surface })
      const line = await ui.find({ type: 'Text', text: /^tako/ })
      expect(line?.text).toContain('カード 5 (1/2)')
      expect(line?.text).toContain('cargo build -p tako-cli')
      expect(line?.text ?? '').not.toContain('\n')
      for (const [op, hotkey] of [['copy', 'y'], ['run', 'r'], ['next', 'n'], ['dismiss', 'x']] as const) {
        const b = await ui.find({ type: 'Button', key: `tako-card-${op}` })
        expect(b?.props.hotkey).toBe(hotkey)
        expect(b?.props.plain).toBe(true)
      }
      // カスタムボタン（compact）は今のまま並ぶ
      expect(await ui.find({ type: 'Button', key: 'tako-button-compact' })).toBeDefined()
      await ui.unmount()
    }
    await clock.advance(1_000)
    const renders = rendersOf(w)
    expect(renders.cards).toEqual([4, 5])
    // カスタムボタンの数はカードのボタンを数えない（#1962 の意味のまま）
    expect(renders.buttons).toBe(1)
  })

  test('Copy は論理文字列（157 字・改行なし）を $.ui.copy へそのまま渡す', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    await ui.press({ key: 'tako-card-copy' })
    expect(w.copies).toEqual([{ text: LONG, surface: 'terminal' }])
    expect(LONG.length).toBe(157)
    expect(w.copies[0]?.text).not.toContain('\n')
    // CLI へは落ちない
    expect(w.runs).toEqual([])
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.last_press).toEqual(expect.objectContaining({ kind: 'card_copy', ok: true }))
    expect(JSON.stringify(w.sent.at(-1)?.report)).not.toContain('cargo build')
    await ui.unmount()
  })

  test('他の mod が ui.copy を拒んだら tako の show-command --copy へ落とす', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() }, 'refused')
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    await ui.press({ key: 'tako-card-copy' })
    expect(w.copies.length).toBe(1)
    expect(w.runs).toEqual([[CLI, 'show-command', '--copy', '--card', '5', '--index', '1']])
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.last_press).toEqual(expect.objectContaining({ kind: 'card_copy_cli', ok: true }))
    await ui.unmount()
  })

  test('Run と閉じるは tako が組んだ引数をそのまま叩き、次は選択を進める（新しいカード → 古いカード）', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    let ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    await ui.press({ key: 'tako-card-run' })
    expect(w.runs.at(-1)).toEqual([CLI, 'show-command', '--run', '--card', '5', '--index', '1'])
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.last_press).toEqual(expect.objectContaining({ kind: 'card_run', ok: true }))
    // 次 → カード 5 の 2 件目（複数行コマンドは 1 行目だけを帯に出す）
    await ui.press({ key: 'tako-card-next' })
    await ui.unmount()
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    let line = await ui.find({ type: 'Text', text: /^tako/ })
    expect(line?.text).toContain('カード 5 (2/2)')
    expect(line?.text).toContain('cd /tmp \\')
    expect(line?.text).not.toContain('ls -la')
    await ui.press({ key: 'tako-card-copy' })
    expect(w.copies.at(-1)?.text).toBe(MULTI)
    // 次 → カード 4（古いカード）→ 次 → カード 5 の 1 件目へ戻る
    await ui.press({ key: 'tako-card-next' })
    await ui.unmount()
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    line = await ui.find({ type: 'Text', text: /^tako/ })
    expect(line?.text).toContain('カード 4')
    await ui.press({ key: 'tako-card-dismiss' })
    expect(w.runs.at(-1)).toEqual([CLI, 'show-command', '--dismiss', '--card', '4'])
    await ui.press({ key: 'tako-card-next' })
    await ui.unmount()
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect((await ui.find({ type: 'Text', text: /^tako/ }))?.text).toContain('カード 5 (1/2)')
    await ui.unmount()
  })

  test('新しいカードが届いたら帯の選択をその 1 件目へ移す', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view() }
    const { clock } = await started(on, $, reply)
    let ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    await ui.press({ key: 'tako-card-next' })
    await ui.unmount()
    reply.view = view({ cards: cards([cardItem(4, ['npm test']), cardItem(5, [LONG, MULTI]), cardItem(9, ['make lint'])]) })
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    const line = await ui.find({ type: 'Text', text: /^tako/ })
    expect(line?.text).toContain('カード 9')
    expect(line?.text).toContain('make lint')
    await ui.unmount()
  })

  test('カードが 1 件だけなら「次」を出さない', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view({ cards: cards([cardItem(3, ['ls'])]) }) })
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Button', key: 'tako-card-next' })).toBeUndefined()
    expect(await ui.find({ type: 'Button', key: 'tako-card-run' })).toBeDefined()
    await ui.unmount()
    await clock.advance(1_000)
    expect(rendersOf(w).cards).toEqual([3])
  })

  test('狭くて押せる形に収まらなければカードは帯に描かず、renders.cards は空（tako が GPUI の帯へ出す）', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(24), surface })
      expect(await cardButtons(ui)).toEqual([])
      expect((await ui.find({ type: 'Text', text: /^tako/ }))?.text ?? '').not.toContain('カード')
      await ui.unmount()
    }
    await clock.advance(1_000)
    expect(rendersOf(w).cards).toEqual([])
  })

  test('カードが 0 枚・band.segments に card が無い・帯を隠しているときは描かず、renders.cards は空', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view({ cards: cards([]) }) }
    const { w, clock } = await started(on, $, reply)
    let ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await cardButtons(ui)).toEqual([])
    await ui.unmount()
    await clock.advance(1_000)
    expect(rendersOf(w).cards).toEqual([])
    reply.view = view({ ui: uiJson({ band: { hidden: false, segments: ['pane', 'buttons'] } }) })
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await cardButtons(ui)).toEqual([])
    await ui.unmount()
    await clock.advance(1_000)
    expect(rendersOf(w).cards).toEqual([])
    reply.view = view({ band_request: { hidden: true, at: 1791470100000 } })
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await cardButtons(ui)).toEqual([])
    await ui.unmount()
    await clock.advance(1_000)
    expect(rendersOf(w).cards).toEqual([])
  })

  test('tako が描かせない（view.cards が無い = cards.place tako・A/B）なら描かず、renders.cards を送らない', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view({ cards: undefined }) })
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await cardButtons(ui)).toEqual([])
    await ui.unmount()
    await clock.advance(1_000)
    expect(rendersOf(w).cards).toBeUndefined()
    expect(rendersOf(w).band).toBe(true)
  })
})

describe('会話の行のカード（#1963）', () => {
  test('tako_show_command を含むグループを展開し、含まないグループ・展開済みは触らない', async ($, on) => {
    const { w } = await started(on, $, { exitCode: 0, view: view() })
    let ui = await $.ui.mount({ ...toolGroup([{ tool: 'Read', input: {} }, { tool: TOOL, input: { commands: ['ls'] } }]), surface: 'terminal' })
    expect(w.groups.at(-1)).toBe(true)
    await ui.unmount()
    ui = await $.ui.mount({ ...toolGroup([{ tool: 'Read', input: {} }, { tool: TOOL, input: { action: 'list' } }]), surface: 'terminal' })
    expect(w.groups.at(-1)).toBe(false)
    await ui.unmount()
    ui = await $.ui.mount({ ...toolGroup([{ tool: 'mcp__other__tako_show_command_x', input: {} }]), surface: 'terminal' })
    expect(w.groups.at(-1)).toBe(false)
    await ui.unmount()
  })

  test('tako_show_command の行を `カード N`・ラベル・コマンドの Code・実行記録・帯の案内に描き替える（両 surface）', async ($, on) => {
    const runs = { 1: { pane: 12, state: 'exited', exit_code: 0, count: 1 } }
    await started(on, $, { exitCode: 0, view: view({ cards: cards([cardItem(5, [LONG, MULTI], 'ビルドと確認', runs)]) }) })
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...toolUse(mcpOutput(5)), surface })
      const tree = (await ui.drawn()) as { type: string; props?: Record<string, unknown> }
      expect(tree.type).toBe('Box')
      expect(tree.props?.borderStyle).toBe('round')
      const title = await ui.find({ type: 'Text', text: /^tako/ })
      expect(title?.text).toContain('カード 5')
      expect(title?.text).toContain('ビルドと確認')
      const codes = await ui.findAll({ type: 'Code' })
      expect(codes.map(c => c.props.source)).toEqual([LONG, MULTI])
      expect(codes[0]?.props.wrap).toBe('wrap')
      expect(await ui.find({ type: 'Text', text: '→ ペイン 12 終了 0' })).toBeDefined()
      expect(await ui.find({ type: 'Text', text: /y: コピー \/ r: 実行/ })).toBeDefined()
      await ui.unmount()
    }
  })

  test('結果が JSON の文字列でも id を拾う・他の mod が描いた行はその下に足す', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() }, 'ok', 'other-mod row')
    const ui = await $.ui.mount({ ...toolUse(JSON.stringify({ card: { id: 4 } }), { commands: ['npm test'] }), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: 'other-mod row' })).toBeDefined()
    expect((await ui.find({ type: 'Text', text: /^tako/ }))?.text).toContain('カード 4')
    expect((await ui.findAll({ type: 'Code' })).map(c => c.props.source)).toEqual(['npm test'])
    await ui.unmount()
  })

  test('store に無いカード（閉じた）は引数から描いて「閉じた」と添え、帯の案内を出さない', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() })
    const ui = await $.ui.mount({ ...toolUse(mcpOutput(2), { commands: ['echo gone'], label: '古い' }), surface: 'terminal' })
    expect((await ui.find({ type: 'Text', text: /^tako/ }))?.text).toContain('このカードは閉じた')
    expect((await ui.findAll({ type: 'Code' })).map(c => c.props.source)).toEqual(['echo gone'])
    expect(await ui.find({ type: 'Text', text: /y: コピー/ })).toBeUndefined()
    await ui.unmount()
  })

  test('tako が描かせない・エラーで終わった・ほかのツール・list の呼び出しはエンジンの行のまま', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view() }
    const { clock } = await started(on, $, reply)
    for (const target of [
      { ...toolUse(mcpOutput(5)), props: { ...toolUse(mcpOutput(5)).props, isErrored: true } },
      toolUse(mcpOutput(5), { action: 'list' }),
      toolUse('ok', { command: 'ls' }, 'Bash'),
    ]) {
      const ui = await $.ui.mount({ ...target, surface: 'terminal' })
      expect(await ui.find({ type: 'Code' })).toBeUndefined()
      await ui.unmount()
    }
    reply.view = view({ cards: undefined })
    await clock.advance(15_000)
    const ui = await $.ui.mount({ ...toolUse(mcpOutput(5)), surface: 'terminal' })
    expect(await ui.find({ type: 'Code' })).toBeUndefined()
    await ui.unmount()
    const g = await $.ui.mount({ ...toolGroup([{ tool: TOOL, input: { commands: ['ls'] } }]), surface: 'terminal' })
    expect(await g.find({ type: 'Text', text: 'Called tako' })).toBeDefined()
    await g.unmount()
  })
})

describe('報告（#1963）', () => {
  test('tako_show_command の呼び出しが返ったらすぐ報告する（heartbeat を待たない）', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    await clock.advance(2_000)
    const before = w.sent.length
    await $.tool.call({ tool: TOOL, tool_use_id: 'toolu_9', commands: ['ls'] } as never)
    await clock.advance(1_000)
    expect(w.sent.length).toBe(before + 1)
  })
})
