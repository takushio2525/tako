// tako mod S7-6（#1964）: チャット風の描画（吹き出し・コードのコピー・ツールの要約）を `claude plugin test` で
// 確かめる。tako 本体は呼ばない: `process.run` をテストの hook で受け、応答（tako 側の材料 = view。チャットは
// view.chat）を返す。`ui.copy` もテストの hook で受ける（本物のクリップボードへ書かない）。
// テストの `ui.render` の hook は「tako より内側の他の mod / エンジン」の代役。描画は terminal と desktop の両 surface
import type { On } from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'

type Sent = { argv: readonly string[]; report: Record<string, unknown> }
type Reply = { exitCode: number; view?: unknown }
type World = {
  sent: Sent[]
  copies: { text: string; surface?: string }[]
  groups: boolean[]
  toasts: string[]
}

const SURFACES = ['terminal', 'desktop'] as const
const CLI = '/opt/tako/tako'
// 157 字・改行なしの長いコマンド（画面では折り返しても論理文字列は 1 行 = Copy に改行が混ざらない）
const LONG =
  'cargo build -p tako-cli && TAKO_ISOLATED=1 TAKO_DATA_DIR=/tmp/tako-s7-very-long-data-dir-name ./target/debug/tako mod --json | jq ".panes[] | {pane, reason}"'
const ANSWER = ['Here is the build command:', '', '```bash', LONG, '```', '', 'Then:', '', '~~~python', 'print("hi")', 'x = 1', '~~~'].join('\n')

const USAGE = {
  startedAt: 0,
  context: { tokens: 60000, window: 1000000, percent: 6 },
  rateLimits: [],
  cost: { usd: 0.007 },
}

function uiJson(chat: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    schema_version: 1,
    band: { hidden: false, segments: ['pane', 'tab', 'workers', 'attention', 'card', 'buttons'] },
    usage_bar: { place: 'prompt_hint', items: ['five_hour', 'seven_day', 'ctx'] },
    buttons: [{ id: 'compact', label: 'compact', hotkey: 'c', action: { kind: 'slash', command: 'compact' } }],
    cards: { place: 'auto' },
    chat: { place: 'mod', bubble: 'gui_only', code_copy: true, tool_summary: 'gui_only', ...chat },
    colors: { accent: 'suggestion', warn: 'warning', dim: 'subtle' },
  }
}

// tako_core::claude_mod::chat_view と同じ形（GUI モードで mod がチャットを持つ組）
const GUI_CHAT = { mode: 'gui', bubble: true, code_copy: true, tool_summary: true, hotkey: 'o', notify: false }
// ターミナル表示の既定（コードのコピーだけ・通知しない）
const TERMINAL_CHAT = { mode: 'terminal', bubble: false, code_copy: true, tool_summary: false, hotkey: 'o', notify: false }

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
    chat: GUI_CHAT,
    ...extra,
  }
}

// copy: 'ok' = 写せた / 'refused' = 他の mod の ui.copy フックが拒んだ。inner = 内側の他の mod の行
function world(on: On, reply: Reply, copy: 'ok' | 'refused' = 'ok', inner: string | null = null): World {
  const w: World = { sent: [], copies: [], groups: [], toasts: [] }
  on('process.run', ($, e) => {
    if (e.argv[1] === 'mod' && e.argv[2] === 'report') {
      w.sent.push({ argv: e.argv, report: JSON.parse(e.init?.stdin ?? '{}') as Record<string, unknown> })
      const body = reply.view === undefined ? { accepted: 'stored', tako: {} } : { accepted: 'stored', tako: { view: reply.view } }
      const stdout = reply.exitCode === 0 ? JSON.stringify(body) : ''
      return { value: { exitCode: reply.exitCode, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
    }
    return { value: { exitCode: 0, stdout: '{}', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('ui.copy', ($, e) => {
    w.copies.push({ text: e.text, surface: e.surface })
    return { value: copy === 'ok' ? { isCopied: true } : { isCopied: false, reason: 'refused' } }
  })
  on('ui.toast', ($, e) => {
    w.toasts.push(typeof e.text === 'string' ? e.text : JSON.stringify(e.text))
    return { value: undefined }
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
  on('turn.complete', ($, e) => ({ text: e.answer }))
  // 内側（エンジン / 他の mod）。ToolGroup は受け取った isExpanded を控える
  on('ui.render', ($, e) => {
    const { Text } = $.ui.resolve(e)
    if (e.component === 'ToolGroup') {
      w.groups.push(e.props.isExpanded)
      return Text({ children: e.props.isExpanded ? 'expanded group' : 'Ran 1 shell command' })
    }
    if (inner !== null && (e.component === 'UserMessage' || e.component === 'AssistantMessage' || e.component === 'ToolUse')) {
      return Text({ children: inner })
    }
    return ENGINE
  })
  return w
}

const START = { cwd: '/tmp', surface: 'terminal', isInteractive: true } as const
const IN_TAKO = { TAKO_PANE_ID: '7', TAKO_CLI: CLI }
const ENGINE = { type: 'engine' as const, ref: 0 }

function band(bodyColumns: number) {
  return {
    plugin: 'tako',
    component: 'AbovePrompt' as const,
    props: { hasSurvey: false, isWorking: false, maxRows: 1, bodyColumns, scroll: { offset: 0, bodyRows: 1 }, view: {} },
  }
}

function userMessage(text: string, kind = 'composer') {
  return {
    plugin: 'tako',
    component: 'UserMessage' as const,
    requestId: 'msg-u1',
    viewport: { columns: 100, rows: 40 },
    props: { text, origin: { kind }, isExpanded: true },
  }
}

function assistantMessage(text: string, extra: Record<string, unknown> = {}) {
  return {
    plugin: 'tako',
    component: 'AssistantMessage' as const,
    requestId: 'msg-a1',
    props: { text, isFirstOfReply: true, ...extra },
  }
}

function toolUse(tool: string, input: Record<string, unknown>, state: Record<string, boolean> = {}) {
  return {
    plugin: 'tako',
    component: 'ToolUse' as const,
    props: { tool_use_id: 'toolu_1', tool, input, isRunning: false, isErrored: false, isInterrupted: false, output: { stdout: 'made' }, ...state },
  }
}

function toolGroup(calls: { tool: string; input: unknown }[]) {
  return {
    plugin: 'tako',
    component: 'ToolGroup' as const,
    props: {
      calls: calls.map((c, i) => ({ tool_use_id: `toolu_${i}`, tool: c.tool, input: c.input, isRunning: false, isErrored: false, isInterrupted: false })),
      isActive: false,
      isExpanded: false,
    },
  }
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

function rendersOf(w: World): Record<string, unknown> {
  return (w.sent.at(-1)?.report.renders ?? {}) as Record<string, unknown>
}

describe('吹き出し（#1964）', () => {
  test('利用者の発話を枠つきの右寄せの吹き出しに描く（両 surface）', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() })
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...userMessage('ビルドして'), surface })
      const tree = (await ui.drawn()) as { type: string; props?: Record<string, unknown>; children?: unknown[] }
      expect(tree.type).toBe('Box')
      expect(tree.props?.justifyContent).toBe('flex-end')
      expect(tree.props?.paddingLeft).toBe(20)
      const bubble = await ui.find({ type: 'Box', props: { borderStyle: 'round' } } as never)
      expect(bubble).toBeDefined()
      expect(await ui.find({ type: 'Text', text: 'ビルドして' })).toBeDefined()
      await ui.unmount()
    }
  })

  test('Remote Control の発話も吹き出し・通知や他のエージェントの行はエンジンのまま', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() })
    let ui = await $.ui.mount({ ...userMessage('スマホから', 'bridge'), surface: 'terminal' })
    expect(await ui.find({ type: 'Box', props: { borderStyle: 'round' } } as never)).toBeDefined()
    await ui.unmount()
    for (const kind of ['task-notification', 'peer', 'plugin']) {
      ui = await $.ui.mount({ ...userMessage('通知', kind), surface: 'terminal' })
      expect(((await ui.drawn()) as { type: string }).type).toBe('engine')
      await ui.unmount()
    }
  })

  test('他の mod が描いた発話の行は捨てずに吹き出しの中へ包む', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() }, 'ok', 'other-mod row')
    const ui = await $.ui.mount({ ...userMessage('ビルドして'), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: 'other-mod row' })).toBeDefined()
    expect(await ui.find({ type: 'Box', props: { borderStyle: 'round' } } as never)).toBeDefined()
    expect(await ui.find({ type: 'Text', text: 'ビルドして' })).toBeUndefined()
    await ui.unmount()
  })

  test('ターミナル表示の組（bubble 偽）・tako が描かせない・tako の外では吹き出しにしない', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view({ chat: TERMINAL_CHAT }) }
    const { clock } = await started(on, $, reply)
    let ui = await $.ui.mount({ ...userMessage('ビルドして'), surface: 'terminal' })
    expect(((await ui.drawn()) as { type: string }).type).toBe('engine')
    await ui.unmount()
    reply.view = view({ chat: undefined })
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...userMessage('ビルドして'), surface: 'terminal' })
    expect(((await ui.drawn()) as { type: string }).type).toBe('engine')
    await ui.unmount()
  })
})

describe('コードのコピー（#1964）', () => {
  test('コードブロックごとに Copy を並べ、押すと論理文字列（157 字・改行なし）をそのまま写す', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...assistantMessage(ANSWER), surface })
      const b1 = await ui.find({ type: 'Button', key: 'tako-code-msg-a1-0' })
      const b2 = await ui.find({ type: 'Button', key: 'tako-code-msg-a1-1' })
      expect(b1?.props.label).toBe('コピー 1 bash')
      expect(b2?.props.label).toBe('コピー 2 python')
      await ui.press({ key: 'tako-code-msg-a1-0' })
      await ui.press({ key: 'tako-code-msg-a1-1' })
      await ui.unmount()
    }
    expect(w.copies.map(c => c.text)).toEqual([LONG, 'print("hi")\nx = 1', LONG, 'print("hi")\nx = 1'])
    expect(w.copies[0]?.text).not.toContain('\n')
    expect(w.copies[0]?.surface).toBe('terminal')
    expect(w.copies[2]?.surface).toBe('desktop')
    expect(w.toasts.at(-1)).toBe('コードをコピーしました')
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.last_press).toEqual(expect.objectContaining({ kind: 'code_copy', ok: true }))
    // 報告にコードの本文は載らない
    expect(JSON.stringify(w.sent.at(-1)?.report)).not.toContain('cargo build')
  })

  test('他の mod が ui.copy を拒んだらその選択を尊重し、知らせて失敗を報告する', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() }, 'refused')
    const ui = await $.ui.mount({ ...assistantMessage(ANSWER), surface: 'terminal' })
    await ui.press({ key: 'tako-code-msg-a1-0' })
    await ui.unmount()
    expect(w.copies.length).toBe(1)
    expect(w.toasts.at(-1)).toContain('コピーできませんでした')
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.last_press).toEqual(expect.objectContaining({ kind: 'code_copy', ok: false }))
  })

  test('コードの無い返答・要約の行・閉じていない（生成中の）ブロック・code_copy 偽はエンジンの行のまま', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view() }
    const { clock } = await started(on, $, reply)
    for (const target of [
      assistantMessage('ok'),
      assistantMessage(ANSWER, { isSummary: true }),
      assistantMessage('Here:\n```bash\nnpm test'),
      assistantMessage('inline ```not a fence``` here'),
    ]) {
      const ui = await $.ui.mount({ ...target, surface: 'terminal' })
      expect(((await ui.drawn()) as { type: string }).type).toBe('engine')
      await ui.unmount()
    }
    reply.view = view({ chat: { ...GUI_CHAT, code_copy: false } })
    await clock.advance(15_000)
    const ui = await $.ui.mount({ ...assistantMessage(ANSWER), surface: 'terminal' })
    expect(await ui.find({ type: 'Button' })).toBeUndefined()
    await ui.unmount()
  })

  test('他の mod が描いた返答の行は上に残し、その下に Copy を足す', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() }, 'ok', 'other-mod reply')
    const ui = await $.ui.mount({ ...assistantMessage(ANSWER), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: 'other-mod reply' })).toBeDefined()
    expect(await ui.find({ type: 'Button', key: 'tako-code-msg-a1-0' })).toBeDefined()
    await ui.unmount()
  })

  test('帯の「最新のコードをコピー」は最新の返答の最後のコードをキーで写す（tako が選んだホットキー）', async ($, on) => {
    const { w, clock } = await started(on, $, { exitCode: 0, view: view() })
    // 返答の前は出ない
    let ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Button', key: 'tako-code-latest' })).toBeUndefined()
    await ui.unmount()
    await $.turn.complete({ answer: ANSWER, durationMs: 10, isAborted: false, turnId: 't1', reason: 'answer' } as never)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    const b = await ui.find({ type: 'Button', key: 'tako-code-latest' })
    expect(b?.props.hotkey).toBe('o')
    expect(b?.props.plain).toBe(true)
    expect(b?.props.label).toBe('最後のコードをコピー')
    await ui.press({ key: 'tako-code-latest' })
    expect(w.copies.at(-1)?.text).toBe('print("hi")\nx = 1')
    await ui.unmount()
    await clock.advance(1_000)
    expect((w.sent.at(-1)?.report.band as { segments: string[] }).segments).toContain('code')
    // サブエージェントのターンは数えない・次の返答にコードが無ければ消える
    await $.turn.complete({ answer: 'ok', durationMs: 10, isAborted: false, turnId: 't2', reason: 'answer', agentId: 'a1' } as never)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Button', key: 'tako-code-latest' })).toBeDefined()
    await ui.unmount()
    await $.turn.complete({ answer: 'ok', durationMs: 10, isAborted: false, turnId: 't3', reason: 'answer' } as never)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Button', key: 'tako-code-latest' })).toBeUndefined()
    await ui.unmount()
  })

  test('狭い帯ではカスタムボタンより先に「最新のコードをコピー」を落とす', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() })
    await $.turn.complete({ answer: ANSWER, durationMs: 10, isAborted: false, turnId: 't1', reason: 'answer' } as never)
    const ui = await $.ui.mount({ ...band(30), surface: 'terminal' })
    expect(await ui.find({ type: 'Button', key: 'tako-code-latest' })).toBeUndefined()
    expect(await ui.find({ type: 'Button', key: 'tako-button-compact' })).toBeDefined()
    await ui.unmount()
  })
})

describe('ツールの要約（#1964）', () => {
  test('ToolGroup を展開し、呼び出しを名前・中身・状態・「詳細」の 1 行に描く（両 surface）', async ($, on) => {
    const { w } = await started(on, $, { exitCode: 0, view: view() })
    let ui = await $.ui.mount({ ...toolGroup([{ tool: 'Bash', input: { command: 'ls' } }]), surface: 'terminal' })
    expect(w.groups.at(-1)).toBe(true)
    await ui.unmount()
    for (const surface of SURFACES) {
      ui = await $.ui.mount({ ...toolUse('Bash', { command: 'touch a.txt && echo made\nsecond line' }), surface })
      expect(await ui.find({ type: 'Text', text: 'Bash' })).toBeDefined()
      expect(await ui.find({ type: 'Text', text: 'touch a.txt && echo made' })).toBeDefined()
      expect(await ui.find({ type: 'Text', text: '完了' })).toBeDefined()
      expect((await ui.find({ type: 'Button', key: 'tako-tool-toolu_1' }))?.props.label).toBe('詳細')
      await ui.unmount()
    }
  })

  test('状態（実行中・エラー・中断）と、ツールごとの中身（パス・パターン・MCP の名前）', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() })
    const cases: [string, Record<string, unknown>, Record<string, boolean>, string, string][] = [
      ['Bash', { command: 'sleep 9' }, { isRunning: true }, 'sleep 9', '実行中'],
      ['Read', { file_path: '/tmp/a.rs' }, { isErrored: true }, '/tmp/a.rs', 'エラー'],
      ['Grep', { pattern: 'fn main' }, { isInterrupted: true }, 'fn main', '中断'],
      ['mcp__tako__tako_list_panes', { pane: 3, filter: 'claude' }, {}, 'claude', '完了'],
    ]
    for (const [tool, input, state, subject, word] of cases) {
      const ui = await $.ui.mount({ ...toolUse(tool, input, state), surface: 'terminal' })
      expect(await ui.find({ type: 'Text', text: subject })).toBeDefined()
      expect(await ui.find({ type: 'Text', text: word })).toBeDefined()
      await ui.unmount()
    }
    const ui = await $.ui.mount({ ...toolUse('mcp__tako__tako_list_panes', {}), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: 'tako.tako_list_panes' })).toBeDefined()
    await ui.unmount()
  })

  test('「詳細」でエンジンの行を要約の下に戻し、「閉じる」で畳む', async ($, on) => {
    await started(on, $, { exitCode: 0, view: view() })
    let ui = await $.ui.mount({ ...toolUse('Bash', { command: 'ls' }), surface: 'terminal' })
    await ui.press({ key: 'tako-tool-toolu_1' })
    await ui.unmount()
    ui = await $.ui.mount({ ...toolUse('Bash', { command: 'ls' }), surface: 'terminal' })
    const tree = (await ui.drawn()) as { type: string; children?: { type: string }[] }
    expect(tree.type).toBe('Box')
    expect(JSON.stringify(tree)).toContain('"type":"engine"')
    expect((await ui.find({ type: 'Button', key: 'tako-tool-toolu_1' }))?.props.label).toBe('閉じる')
    await ui.press({ key: 'tako-tool-toolu_1' })
    await ui.unmount()
    ui = await $.ui.mount({ ...toolUse('Bash', { command: 'ls' }), surface: 'terminal' })
    expect(JSON.stringify(await ui.drawn())).not.toContain('"type":"engine"')
    await ui.unmount()
  })

  test('他の mod が描いたツールの行は上に残して要約を足す・tool_summary 偽はグループも行もそのまま', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view() }
    const { w, clock } = await started(on, $, reply, 'ok', 'other-mod tool row')
    let ui = await $.ui.mount({ ...toolUse('Bash', { command: 'ls' }), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: 'other-mod tool row' })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: 'Bash' })).toBeDefined()
    await ui.unmount()
    reply.view = view({ chat: TERMINAL_CHAT })
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...toolGroup([{ tool: 'Bash', input: { command: 'ls' } }]), surface: 'terminal' })
    expect(w.groups.at(-1)).toBe(false)
    await ui.unmount()
    ui = await $.ui.mount({ ...toolUse('Bash', { command: 'ls' }), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: 'other-mod tool row' })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: 'Bash' })).toBeUndefined()
    await ui.unmount()
  })
})

describe('報告（#1964）', () => {
  test('renders.chat は tako が頼んだ組そのまま・view.chat が無ければ欠ける', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view() }
    const { w, clock } = await started(on, $, reply)
    await clock.advance(1_000)
    expect(rendersOf(w).chat).toEqual({ bubble: true, code_copy: true, tool_summary: true, notify: false })
    reply.view = view({ chat: undefined })
    await clock.advance(15_000)
    await clock.advance(1_000)
    expect(rendersOf(w).chat).toBeUndefined()
    // A/B の S3 の描き方では renders ごと送らない
    reply.view = view({ band_style: 's3' })
    await clock.advance(15_000)
    await clock.advance(1_000)
    expect(w.sent.at(-1)?.report.renders).toBeUndefined()
  })

  test('tako が応答しなくなったら（45 秒）チャットを描かず renders.chat を引っ込める', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view() }
    const { w, clock } = await started(on, $, reply)
    reply.exitCode = 1
    await clock.advance(46_000)
    const ui = await $.ui.mount({ ...userMessage('ビルドして'), surface: 'terminal' })
    expect(((await ui.drawn()) as { type: string }).type).toBe('engine')
    await ui.unmount()
    await clock.advance(1_000)
    expect(rendersOf(w).chat).toBeUndefined()
  })

  test('本筋の会話の行の数を chat_seq に載せ、notify のときだけすぐ報告する（サブエージェント・添付は数えない）', async ($, on) => {
    const reply: Reply = { exitCode: 0, view: view({ chat: { ...TERMINAL_CHAT, notify: true } }) }
    const { w, clock } = await started(on, $, reply)
    await clock.advance(15_000)
    const before = w.sent.length
    const seq0 = w.sent.at(-1)?.report.chat_seq as number
    await $.session.append({ message: { type: 'user', role: 'user', content: [{ type: 'text', text: 'x' }] }, door: 'prompt', origin: { kind: 'composer' }, uuid: 'u1' } as never)
    await $.session.append({ message: { type: 'assistant', role: 'assistant', content: [{ type: 'text', text: 'y' }] }, door: 'response', origin: { kind: 'model' }, uuid: 'u2' } as never)
    await $.session.append({ message: { type: 'attachment', role: 'user', content: [{ type: 'text', text: 'z' }] }, door: 'attachment', origin: { kind: 'engine' }, uuid: 'u3' } as never)
    await $.session.append({ message: { type: 'assistant', role: 'assistant', content: [{ type: 'text', text: 'y' }] }, door: 'response', origin: { kind: 'model' }, uuid: 'u4', agentId: 'sub' } as never)
    await clock.advance(1_000)
    expect(w.sent.length).toBe(before + 1)
    expect(w.sent.at(-1)?.report.chat_seq).toBe(seq0 + 2)
    expect(JSON.stringify(w.sent.at(-1)?.report)).not.toContain('u2')
    // notify 偽（ターミナル表示・mod がチャットを持つ）は数えるだけで報告を増やさない
    reply.view = view({ chat: TERMINAL_CHAT })
    await clock.advance(15_000)
    // 組が変わった応答を受けた直後の報告（renders.chat を返す 1 回）を流しきってから数える
    await clock.advance(2_000)
    const quiet = w.sent.length
    await $.session.append({ message: { type: 'user', role: 'user', content: [{ type: 'text', text: 'x' }] }, door: 'prompt', origin: { kind: 'composer' }, uuid: 'u5' } as never)
    await clock.advance(1_000)
    expect(w.sent.length).toBe(quiet)
    await clock.advance(15_000)
    expect(w.sent.at(-1)?.report.chat_seq).toBe(seq0 + 3)
  })
})
