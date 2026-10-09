// tako mod の帯とサイドバー（#1881 / S3）を `claude plugin test` で確かめる。
// tako 本体は呼ばない: `process.run` をテストの hook で受け、応答（tako 側の材料 = view）を返す。
// 描画は terminal と desktop の両 surface で同じ本体を回す（受け入れ条件 3）
import type { On } from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'

type Sent = { argv: readonly string[]; report: Record<string, unknown> }
type Reply = { exitCode: number; view?: unknown }

const SURFACES = ['terminal', 'desktop'] as const

// 80 / 144 / 300 桁の端末で帯の本文に使える桁（右端の 5 桁は Claude Code の `[-]`）
const WIDTHS = [75, 139, 295] as const

const USAGE = {
  startedAt: 0,
  context: { tokens: 850000, window: 1000000, percent: 85 },
  rateLimits: [{ kind: 'five_hour', percentUsed: 92, resetsAt: '2026-10-08T14:30:00.000Z' }],
  cost: { usd: 0.007 },
}

// 長いペイン名・タブ名・要注意 1・閾値超えの ctx と使用制限 2 つ = 帯の区切りが全部出る材料
function fullView(extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    pane: 7,
    pane_title: 'オーケストレーターの master ペイン（長い名前の切り詰めを見る）',
    tab_title: 'tako-wt-1881 の作業タブ（これも長い）',
    lang: 'ja',
    worker_count: 3,
    attention: 1,
    workers: [
      { pane: 9, name: 'fix-1881', state: 'waiting', attention: 'permission' },
      { pane: 8, name: 'docs', state: 'busy' },
      { pane: 10, name: 'tests', state: 'idle' },
    ],
    warnings: [
      { kind: 'ctx', percent: 85 },
      { kind: 'five_hour', percent: 92, resets_at: 1791477000 },
      { kind: 'seven_day', percent: 81, resets_at: 1791651600 },
    ],
    ctx: { tokens: 850000, window: 1000000, percent: 85 },
    rate_limits: [
      { kind: 'five_hour', percent: 92, resets_at: 1791477000 },
      { kind: 'seven_day', percent: 81, resets_at: 1791651600 },
    ],
    thresholds: { ctx_percent: 80, limit_percent: 80 },
    ...extra,
  }
}

// エンジンの下の世界。reply.view を変えると tako の応答を差し替えられる。registered に `/tako` の説明文が溜まる
function world(on: On, sent: Sent[], reply: Reply, registered: string[] = []): void {
  on('process.run', ($, e) => {
    sent.push({ argv: e.argv, report: JSON.parse(e.init?.stdin ?? '{}') as Record<string, unknown> })
    const body = reply.view === undefined ? { accepted: 'stored', tako: {} } : { accepted: 'stored', tako: { view: reply.view } }
    const stdout = reply.exitCode === 0 ? JSON.stringify(body) : ''
    const stderr = reply.exitCode === 0 ? '' : 'tako に繋がらない'
    return { value: { exitCode: reply.exitCode, stdout, stderr, isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('session.usage', () => ({ value: USAGE }))
  on('session.model', () => ({ value: 'claude-haiku-5-5' }))
  on('session.id', () => ({ value: 'session-1' }))
  on('session.version', () => ({ value: { version: '2.1.294' } }))
  on('ui.log', () => ({ value: undefined }))
  on('command.register', ($, e) => {
    registered.push(e.description)
    return { value: { command: e.name } }
  })
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('session.end', ($, e) => ({ sessionId: e.sessionId }))
  on('turn.start', ($, e) => ({ turnId: e.turnId }))
  on('turn.complete', () => ({ text: '' }))
  on('classic.PermissionRequest', () => ({}))
  // エンジン自身の描画（mod が next(e) へ譲ったときに描かれるもの）
  on('ui.render', ($, e) => $.ui.resolve(e).Box({ key: 'engine-own' }))
}

const START = { cwd: '/tmp', surface: 'terminal', isInteractive: true } as const
const IN_TAKO = { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' }

function band(bodyColumns: number, hasSurvey = false) {
  return {
    plugin: 'tako',
    component: 'AbovePrompt' as const,
    props: {
      hasSurvey,
      isWorking: false,
      maxRows: 10,
      bodyColumns,
      scroll: { offset: 0, bodyRows: 10 },
      view: {},
    },
  }
}

const SIDEBAR = {
  plugin: 'tako',
  component: 'Pane' as const,
  requestId: 'tako',
  props: { title: 'tako', isFocused: false, bodyColumns: 60, placement: 'dock' as const, scroll: { offset: 0, bodyRows: 30 }, view: {} },
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

describe('tako mod の帯（AbovePrompt）', () => {
  test('80 / 144 / 300 桁で 1 行に収まり、狭いほど優先度の低い区切りから落ちる', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView() })
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    for (const surface of SURFACES) {
      const seen: Record<number, string[]> = {}
      for (const columns of WIDTHS) {
        const ui = await $.ui.mount({ ...band(columns), surface })
        const line = (await ui.find({ type: 'Text' }))?.text ?? ''
        expect(line).not.toContain('\n')
        expect(cells(line)).toBeLessThanOrEqual(columns)
        expect(line).toStartWith('tako | ')
        await clock.advance(1_000)
        const reported = sent.at(-1)?.report.band as { shown: boolean; columns: number; segments: string[] }
        expect(reported).toEqual(expect.objectContaining({ shown: true, columns }))
        seen[columns] = reported.segments
        await ui.unmount()
      }
      // 300 桁は全部、80 桁はタブ名と worker 数を落として要注意・ctx・使用制限を残す
      expect(seen[295]).toEqual(['tako', 'pane', 'tab', 'workers', 'attention', 'ctx', 'five_hour', 'seven_day'])
      expect(seen[75]).toEqual(['tako', 'pane', 'attention', 'ctx', 'five_hour', 'seven_day'])
      // 144 桁: 300 桁より少なく 80 桁より多いか同じ（並びは保つ）
      expect(seen[139]?.length ?? 0).toBeGreaterThanOrEqual(seen[75]?.length ?? 0)
    }
  })

  test('とても狭い幅でも 1 行（末尾を切る）', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView() })
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(20), surface })
      const found = await ui.find({ type: 'Text' })
      expect(found?.props.wrap).toBe('truncate-end')
      expect(found?.text ?? '').not.toContain('\n')
      await ui.unmount()
    }
  })

  test('閾値未満では ctx / 使用制限を帯に出さない（サイドバーには出す）', async ($, on) => {
    const sent: Sent[] = []
    // tako は閾値を超えたものだけを warnings に載せる。ctx 50% / 5h 50% なら空
    const below = fullView({
      warnings: [],
      ctx: { tokens: 500000, window: 1000000, percent: 50 },
      rate_limits: [{ kind: 'five_hour', percent: 50, resets_at: 1791477000 }],
    })
    world(on, sent, { exitCode: 0, view: below })
    const clock = mock.clock(on, { now: 1791470000000 })
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(295), surface })
      const line = (await ui.find({ type: 'Text' }))?.text ?? ''
      expect(line).not.toContain('ctx')
      expect(line).not.toContain('5h')
      expect(line).toContain('worker 3')
      expect(line).toContain('要注意 1')
      await ui.unmount()
      const pane = await $.ui.mount({ ...SIDEBAR, surface })
      expect(await pane.find({ type: 'Text', text: /ctx 50%/ })).toBeDefined()
      expect(await pane.find({ type: 'Text', text: /5h 50%/ })).toBeDefined()
      await pane.unmount()
    }
  })

  test('tako の外・応答に view が無い（A/B）・調査票の表示中は描かない', async ($, on) => {
    // tako の外（TAKO_PANE_ID が無い）
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView() })
    const clock = mock.clock(on)
    mock.env(on, { TAKO_CLI: '/opt/tako/tako' })
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(139), surface })
      expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeUndefined()
      await ui.unmount()
    }
    expect(sent.length).toBe(0)
  })

  test('応答に view が無い tako（TAKO_1877_S3_LEGACY / S3 前の tako）では描かない', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0 })
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(139), surface })
      expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeUndefined()
      await ui.unmount()
    }
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.band).toEqual(expect.objectContaining({ shown: false }))
  })

  test('調査票（hasSurvey）が帯を使っている間は譲る', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView() })
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(139, true), surface })
      expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeUndefined()
      await ui.unmount()
    }
  })

  test('tako の応答が 45 秒途切れたら帯を消し、戻れば描く', async ($, on) => {
    const sent: Sent[] = []
    const reply: Reply = { exitCode: 0, view: fullView() }
    world(on, sent, reply)
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    reply.exitCode = 1
    await clock.advance(30_000)
    let ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeDefined()
    await ui.unmount()
    await clock.advance(16_000)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeUndefined()
    await ui.unmount()
    reply.exitCode = 0
    await clock.advance(15_000)
    ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeDefined()
    await ui.unmount()
  })

  test('権限ダイアログ・質問をはさんでも帯の材料は変わらず、閉じた後も描ける', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView() })
    let release: () => void = () => {}
    const held = new Promise<void>(resolve => {
      release = resolve
    })
    on('tool.call', async () => {
      await held
      return { result: { answers: {} } }
    })
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    await $.turn.start({ text: '', turnId: 't1' })
    await $.classic.PermissionRequest({ tool_name: 'Bash', tool_input: {} })
    const call = $.tool.call({ tool: 'AskUserQuestion', questions: [] } as never)
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.turn).toBe('question')
    release()
    await call
    await $.turn.complete({ answer: '', durationMs: 5, isAborted: false, turnId: 't1', reason: 'answer' })
    await clock.advance(1_000)
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(139), surface })
      const line = (await ui.find({ type: 'Text' }))?.text ?? ''
      expect(line).toStartWith('tako | ')
      expect(line).not.toContain('承認待ち')
      await ui.unmount()
    }
  })
})

describe('tako mod のトグルとサイドバー', () => {
  test('/tako band off|on で帯を隠し / 戻し、$.store へ保存して報告にも載せる', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView() })
    const clock = mock.clock(on, { now: 5_000 })
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    const off = await $.command.run({ command: 'tako', args: 'band off' })
    expect(off.text).toContain('帯を隠した')
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(139), surface })
      expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeUndefined()
      await ui.unmount()
    }
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.band).toEqual(expect.objectContaining({ hidden: true, toggled_at: 6_000 }))
    const back = await $.command.run({ command: 'tako', args: 'band on' })
    expect(back.text).toContain('帯を出した')
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeDefined()
    await ui.unmount()
  })

  test('$.store に保存したトグルは次のセッション（再起動）でも保たれる', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView() })
    const clock = mock.clock(on, { now: 9_000 })
    mock.env(on, IN_TAKO)
    mock.store(on, { band: { hidden: true, at: 5_000 } })
    await $.session.start(START)
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.band).toEqual(expect.objectContaining({ hidden: true, toggled_at: 5_000 }))
    for (const surface of SURFACES) {
      const ui = await $.ui.mount({ ...band(139), surface })
      expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeUndefined()
      await ui.unmount()
    }
  })

  test('tako からの中継（tako mod band on|off）は $.store より新しいときだけ従う', async ($, on) => {
    const sent: Sent[] = []
    const reply: Reply = { exitCode: 0, view: fullView({ band_request: { hidden: false, at: 4_000 } }) }
    world(on, sent, reply)
    const clock = mock.clock(on, { now: 9_000 })
    mock.env(on, IN_TAKO)
    mock.store(on, { band: { hidden: true, at: 5_000 } })
    await $.session.start(START)
    await clock.advance(1_000)
    // 古い中継（4,000 < 5,000）は無視 = 隠したまま
    await clock.advance(15_000)
    expect(sent.at(-1)?.report.band).toEqual(expect.objectContaining({ hidden: true, toggled_at: 5_000 }))
    // 新しい中継で出す
    reply.view = fullView({ band_request: { hidden: false, at: 20_000 } })
    await clock.advance(15_000)
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.band).toEqual(expect.objectContaining({ hidden: false, toggled_at: 20_000 }))
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeDefined()
    await ui.unmount()
  })

  test('/tako でサイドバーを開き、worker と要注意・ボタンでトグルできる', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView() })
    const opened: string[] = []
    on('ui.open', ($, e) => {
      opened.push(e.id)
      return { value: { isPlaced: true } }
    })
    const clock = mock.clock(on, { now: 1791470000000 })
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    const run = await $.command.run({ command: 'tako' })
    expect(run.text).toBeUndefined()
    expect(opened).toEqual(['tako'])
    for (const surface of SURFACES) {
      const pane = await $.ui.mount({ ...SIDEBAR, surface })
      expect(await pane.find({ type: 'Text', text: /オーケストレーター/ })).toBeDefined()
      expect(await pane.find({ type: 'Text', text: /! fix-1881 +承認待ち/ })).toBeDefined()
      expect(await pane.find({ type: 'Text', text: /docs +作業中/ })).toBeDefined()
      expect(await pane.find({ type: 'Text', text: /worker 3 +要注意 1/ })).toBeDefined()
      expect(await pane.find({ type: 'Text', text: /5h 92% +リセットまで 1h57m/ })).toBeDefined()
      expect((await pane.find({ key: 'band-toggle' }))?.props.label).toBe('帯を隠す')
      await pane.unmount()
    }
    const pane = await $.ui.mount({ ...SIDEBAR, surface: 'terminal' })
    await pane.press({ key: 'band-toggle' })
    await pane.unmount()
    const ui = await $.ui.mount({ ...band(139), surface: 'terminal' })
    expect(await ui.find({ type: 'Text', text: /^tako \| / })).toBeUndefined()
    await ui.unmount()
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.band).toEqual(expect.objectContaining({ hidden: true }))
  })

  test('サイドバーを置けないときは理由を返す・知らない引数は使い方を返す', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, { exitCode: 0, view: fullView({ lang: 'en' }) })
    on('ui.open', () => ({ value: { isPlaced: false, reason: 'narrow' } }))
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    expect((await $.command.run({ command: 'tako' })).text).toContain('narrow')
    expect((await $.command.run({ command: 'tako', args: 'what' })).text).toContain('usage')
    // 知らない値（on / off 以外）でトグルを動かさない
    expect((await $.command.run({ command: 'tako', args: 'band maybe' })).text).toContain('usage')
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.band).toEqual(expect.objectContaining({ hidden: false }))
  })

  test('/tako の説明文は tako の表示言語で登録し直す', async ($, on) => {
    const sent: Sent[] = []
    const registered: string[] = []
    world(on, sent, { exitCode: 0, view: fullView() }, registered)
    const clock = mock.clock(on)
    mock.env(on, IN_TAKO)
    mock.store(on)
    await $.session.start(START)
    await clock.advance(1_000)
    // 起動時は既定（en）で登録し、応答で ja と分かったら日本語で登録し直す。説明文の頭に tako を付けない
    expect(registered.length).toBe(2)
    expect(registered[1]).toContain('サイドバー')
    for (const d of registered) expect(d).not.toStartWith('tako')
  })
})
