// tako mod の報告（#1879）を `claude plugin test` で確かめる。
// tako 本体は呼ばない: `process.run` をテストの hook で受けて、送られた報告を記録するだけ。
import type { On } from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'

type Sent = { argv: readonly string[]; report: Record<string, unknown> }

const USAGE = {
  startedAt: 0,
  context: { tokens: 59426, window: 1000000, percent: 6 },
  rateLimits: [{ kind: 'five_hour', percentUsed: 3, resetsAt: '2026-10-08T14:30:00.000Z' }],
  cost: { usd: 0.007 },
}

// 報告に載せてはならないキー（本文・プロンプト・ツールの引数）
const BODY_KEYS = ['text', 'answer', 'prompt', 'message', 'messages', 'content', 'tool_input', 'command']

// エンジンの下の世界を用意する。exitCode を変えると「tako が落ちている」を作れる。
// usage を渡すと $.session.usage() の答えを途中で変えられる（使用制限の観測時刻の検査。#1903）
function world(on: On, sent: Sent[], exitCode = 0, usage: () => unknown = () => USAGE): void {
  // `$` の呼び出しは { value } で答える
  on('process.run', ($, e) => {
    sent.push({ argv: e.argv, report: JSON.parse(e.init?.stdin ?? '{}') as Record<string, unknown> })
    const stderr = exitCode === 0 ? '' : 'tako の外'
    return { value: { exitCode, stdout: '{}', stderr, isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('session.usage', () => ({ value: usage() }))
  on('session.model', () => ({ value: 'claude-haiku-5-5' }))
  on('session.id', () => ({ value: 'session-1' }))
  on('session.version', () => ({ value: { version: '2.1.294' } }))
  on('ui.log', () => ({ value: undefined }))
  // エンジンの呼び出し元が答える分（テストでは下に誰もいない）
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  on('session.end', ($, e) => ({ sessionId: e.sessionId }))
  on('turn.start', ($, e) => ({ turnId: e.turnId }))
  on('turn.step', async function* ($, e) {
    yield { kind: 'text', index: 0, text: 'ok' }
    return { turnId: e.turnId, index: e.index, answer: 'ok', toolUses: [], stopReason: 'end_turn', usage: null }
  })
  on('turn.complete', () => ({ text: '' }))
  on('classic.PermissionRequest', () => ({}))
  on('classic.Stop', () => ({}))
}

const START = { cwd: '/tmp', surface: 'terminal', isInteractive: true } as const

describe('tako mod の報告', () => {
  test('tako の外（TAKO_PANE_ID が無い）では何も呼ばない', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await $.turn.start({ text: 'hello', turnId: 't1' })
    await clock.advance(20_000)
    expect(sent.length).toBe(0)
  })

  test('TAKO_CLI が無ければ何も呼ばない（tako mod off / 注入されていない）', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7' })
    await $.session.start(START)
    await clock.advance(20_000)
    expect(sent.length).toBe(0)
  })

  test('ターンの状態と使用量を 1 秒以内に報告し、本文は載せない', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on, { now: 1_000_000 })
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako', CLAUDE_CONFIG_DIR: '/cfg' })
    await $.session.start(START)
    await clock.advance(1_000)
    expect(sent.length).toBe(1)
    const first = sent[0]
    expect(first?.argv).toEqual(['/opt/tako/tako', 'mod', 'report'])
    expect(first?.report).toEqual(
      expect.objectContaining({
        schema: 1,
        claude_version: '2.1.294',
        session_id: 'session-1',
        model: 'claude-haiku-5-5',
        turn: 'idle',
        ended: false,
        config_dir: '/cfg',
        context: { tokens: 59426, window: 1000000, percent: 6 },
        rate_limits: [
          { kind: 'five_hour', percent_used: 3, resets_at: '2026-10-08T14:30:00.000Z', observed_at: 1_001_000 },
        ],
      }),
    )

    await $.turn.start({ text: 'secret prompt', turnId: 't1' })
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.turn).toBe('busy')

    await $.classic.PermissionRequest({ tool_name: 'Bash', tool_input: { command: 'rm -rf secret' } })
    await clock.advance(1_000)
    expect(sent.at(-1)?.report).toEqual(expect.objectContaining({ turn: 'permission', pending_tool: 'Bash' }))

    await $.classic.PermissionRequest({ tool_name: 'AskUserQuestion', tool_input: { questions: [] } })
    await clock.advance(1_000)
    expect(sent.at(-1)?.report).toEqual(expect.objectContaining({ turn: 'question', pending_tool: 'AskUserQuestion' }))

    await $.turn.complete({ answer: 'secret answer', durationMs: 1193, isAborted: false, turnId: 't1', reason: 'answer' })
    await clock.advance(1_000)
    expect(sent.at(-1)?.report).toEqual(
      expect.objectContaining({ turn: 'idle', last_turn: { duration_ms: 1193, reason: 'answer' } }),
    )
    expect(sent.at(-1)?.report.pending_tool).toBeUndefined()

    for (const { report } of sent) {
      for (const key of BODY_KEYS) expect(Object.keys(report)).not.toContain(key)
      expect(JSON.stringify(report)).not.toContain('secret')
    }
  })

  test('変化が無ければ 15 秒に 1 回の heartbeat だけ送る', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await clock.advance(1_000)
    expect(sent.length).toBe(1)
    await clock.advance(13_000)
    expect(sent.length).toBe(1)
    await clock.advance(2_000)
    expect(sent.length).toBe(2)
  })

  test('利用者が /plugin で止めていれば理由を 1 回だけ報告して休眠する（env で読まれていても。#1959）', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    on('settings.read', () => ({ value: { enabledPlugins: { 'tako@skills-dir': false } } }))
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await clock.advance(60_000)
    expect(sent.length).toBe(1)
    expect(sent[0]?.report).toEqual(expect.objectContaining({ dormant: 'user_disabled', ended: false }))
    // 休眠中はターンが動いても送らない
    await $.turn.start({ text: 'hello', turnId: 't1' })
    await clock.advance(20_000)
    expect(sent.length).toBe(1)
  })

  test('走っている間に止められたら次の見直しで休眠する（#1959）', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    let disabled = false
    on('settings.read', () => ({ value: { enabledPlugins: disabled ? { 'tako@skills-dir': false } : {} } }))
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await clock.advance(1_000)
    expect(sent.length).toBe(1)
    expect(sent[0]?.report.dormant).toBeUndefined()
    disabled = true
    await clock.advance(15_000)
    expect(sent.at(-1)?.report.dormant).toBe('user_disabled')
    const count = sent.length
    await clock.advance(60_000)
    expect(sent.length).toBe(count)
  })

  test('使用制限の observed_at は値が変わったときだけ打ち直す（heartbeat では前の時刻のまま。#1903）', async ($, on) => {
    const sent: Sent[] = []
    const FIVE = { kind: 'five_hour', percentUsed: 3, resetsAt: '2026-10-08T14:30:00.000Z' }
    const WEEK = { kind: 'seven_day', percentUsed: 2, resetsAt: '2026-10-10T13:00:00.000Z' }
    let limits: unknown[] = [FIVE]
    world(on, sent, 0, () => ({ ...USAGE, rateLimits: limits }))
    const clock = mock.clock(on, { now: 1_000_000 })
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    const observed = (): [string, number][] =>
      (sent.at(-1)?.report.rate_limits as { kind: string; observed_at: number }[]).map(l => [l.kind, l.observed_at])
    await $.session.start(START)
    await clock.advance(1_000)
    expect(observed()).toEqual([['five_hour', 1_001_000]])
    // 変化が無いまま heartbeat が 2 回: 報告は届く（at は進む）が観測時刻は動かない
    await clock.advance(30_000)
    expect(sent.length).toBe(3)
    expect(sent.at(-1)?.report.at).toBe(1_031_000)
    expect(observed()).toEqual([['five_hour', 1_001_000]])
    // % が動いた窓は打ち直し、新しく現れた窓は今の時刻
    limits = [{ ...FIVE, percentUsed: 4 }, WEEK]
    await clock.advance(15_000)
    expect(observed()).toEqual([
      ['five_hour', 1_046_000],
      ['seven_day', 1_046_000],
    ])
    // resetsAt だけが変わっても打ち直す（新しい窓）。変わらない窓は前の時刻のまま
    limits = [{ ...FIVE, percentUsed: 4, resetsAt: '2026-10-08T19:30:00.000Z' }, WEEK]
    await clock.advance(15_000)
    expect(observed()).toEqual([
      ['five_hour', 1_061_000],
      ['seven_day', 1_046_000],
    ])
    // session.start（ホットリロードでも来る）をまたいでも観測時刻は保つ（$.state に置いている）
    await $.session.start(START)
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.at).toBe(1_062_000)
    expect(observed()).toEqual([
      ['five_hour', 1_061_000],
      ['seven_day', 1_046_000],
    ])
  })

  test('$.state を読めなくても報告は止めず、今の時刻を打つ（#1903）', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    // 下の層が読み取りを断る = $.state.get が失敗する（throw は下の層が握りつぶして素通しになる）
    on('state.get', () => ({ deny: 'state unavailable' }) as never)
    const clock = mock.clock(on, { now: 1_000_000 })
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await clock.advance(1_000)
    expect(sent.length).toBe(1)
    expect(sent.at(-1)?.report).toEqual(
      expect.objectContaining({
        turn: 'idle',
        rate_limits: [
          { kind: 'five_hour', percent_used: 3, resets_at: '2026-10-08T14:30:00.000Z', observed_at: 1_001_000 },
        ],
      }),
    )
  })

  test('サブエージェントの turn.complete ではターンを終えない', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await $.turn.start({ text: '', turnId: 't1' })
    await $.turn.complete({ answer: '', durationMs: 5, isAborted: false, turnId: 't2', reason: 'answer', agentId: 'sub-1' })
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.turn).toBe('busy')
  })

  test('session.end で ended: true を送り、その後は送らない', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await clock.advance(1_000)
    await $.session.end({ reason: 'prompt_input_exit', sessionId: 'session-1', resume: { sessionId: 'session-1' } } as never)
    expect(sent.at(-1)?.report.ended).toBe(true)
    const count = sent.length
    await clock.advance(30_000)
    expect(sent.length).toBe(count)
  })

  test('/clear では報告を止めない', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await clock.advance(1_000)
    await $.session.end({ reason: 'clear', sessionId: 'session-1', resume: { sessionId: 'session-1' } } as never)
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.ended).toBe(false)
  })

  test('AskUserQuestion は呼び出しの開始で question になり、返ると busy へ戻る', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    let release: () => void = () => {}
    const held = new Promise<void>(resolve => {
      release = resolve
    })
    on('tool.call', async () => {
      await held
      return { result: { answers: {} } }
    })
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await $.turn.start({ text: '', turnId: 't1' })
    const call = $.tool.call({ tool: 'AskUserQuestion', questions: [] } as never)
    await clock.advance(1_000)
    expect(sent.at(-1)?.report).toEqual(expect.objectContaining({ turn: 'question', pending_tool: 'AskUserQuestion' }))
    release()
    await call
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.turn).toBe('busy')
  })

  test('classic 系が届かないときは ask ルールに当たった判定だけ permission にする', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const verdicts: Record<string, { decision: 'ask'; rule?: string }> = {
      Read: { decision: 'ask' },
      Bash: { decision: 'ask', rule: 'Bash' },
    }
    on('tool.check', ($, e) => verdicts[e.tool] ?? { decision: 'allow' })
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await $.turn.start({ text: '', turnId: 't1' })
    // ルールの無い ask は auto モードの分類器が黙って通しうるので拾わない
    await $.tool.check({ tool: 'Read', input: {} })
    await clock.advance(1_000)
    expect(sent.at(-1)?.report).toEqual(expect.objectContaining({ turn: 'busy', classic_events: false }))
    await $.tool.check({ tool: 'Bash', input: { command: 'rm -rf secret' } })
    await clock.advance(1_000)
    expect(sent.at(-1)?.report).toEqual(expect.objectContaining({ turn: 'permission', pending_tool: 'Bash' }))
    expect(JSON.stringify(sent.at(-1)?.report)).not.toContain('secret')
  })

  test('classic 系が届くときは PermissionRequest を正とし classic_events を立てる', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await $.classic.PermissionRequest({ tool_name: 'Bash', tool_input: {} })
    await clock.advance(1_000)
    expect(sent.at(-1)?.report).toEqual(expect.objectContaining({ turn: 'permission', classic_events: true }))
  })

  test('effort は turn.step から拾う（classic 系が届かない環境でも欠けない。#1880）', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await $.turn.start({ text: '', turnId: 't1' })
    // turn.step はストリーム = 読み切ってはじめてフックが最後まで走る
    const stream = $.turn.step({ turnId: 't1', index: 0, model: 'claude-haiku-5-5', effort: 'medium', messageCount: 3 } as never)
    let step = await stream.next()
    while (step.done !== true) step = await stream.next()
    expect(step.value.answer).toBe('ok')
    await clock.advance(1_000)
    expect(sent.at(-1)?.report).toEqual(expect.objectContaining({ effort: 'medium', classic_events: false }))
    // サブエージェントのステップは親の effort を書き換えない
    const sub = $.turn.step({ turnId: 't2', index: 0, model: 'x', effort: 'low', messageCount: 1, agentId: 'sub-1' } as never)
    let subStep = await sub.next()
    while (subStep.done !== true) subStep = await sub.next()
    await clock.advance(1_000)
    expect(sent.at(-1)?.report.effort).toBe('medium')
  })

  test('tako が落ちていてもターンは普通に流れる', async ($, on) => {
    const sent: Sent[] = []
    world(on, sent, 1)
    const clock = mock.clock(on)
    mock.env(on, { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    await clock.advance(1_000)
    await $.turn.start({ text: '', turnId: 't1' })
    await $.classic.PermissionRequest({ tool_name: 'Bash', tool_input: {} })
    await $.turn.complete({ answer: '', durationMs: 5, isAborted: false, turnId: 't1', reason: 'answer' })
    await clock.advance(1_000)
    expect(sent.length).toBe(2)
  })
})
