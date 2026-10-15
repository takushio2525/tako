// tako mod の送信時の確認（#2050 = 鍵らしき文字の 2 段目）を `claude plugin test` で確かめる。
// tako 本体は呼ばない: `process.run` をテストの hook で受け、tako の判定の答え（verdict）を返す。
// 確認ダイアログ（$.ui.ask = AskUserQuestion）を通る分岐は実物の claude で確かめる
// （scripts/test-paste-guard-2050.sh）
import type { On } from 'claude-code'
import { describe, expect, mock, test } from 'claude-code/testing'

type Call = { argv: readonly string[]; stdin: string }

// 偽の鍵（前置詞と本体を分けて書く = 本物の鍵の形をソースに置かない）
const SECRET = 'ghp_' + 'EXAMPLE0123456789abcdefghijklmnopqrst'
const FOUND = { rule: 'github_token', line: 1, masked: 'ghp_…qrst' }

// 判定の答え（`tako paste-guard check` の出力）と終了コードを決めて、呼ばれた引数・標準入力を記録する
function world(on: On, calls: Call[], toasts: string[], logs: string[], verdict: string, exitCode = 0): void {
  on('process.run', ($, e) => {
    const argv = e.argv
    if (argv[1] === 'paste-guard') {
      calls.push({ argv, stdin: e.init?.stdin ?? '' })
      const findings = verdict === 'pass' ? [] : [FOUND]
      const stdout = exitCode === 0 ? JSON.stringify({ verdict, findings }) : ''
      return { value: { exitCode, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
    }
    return { value: { exitCode: 0, stdout: '{}', stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  on('ui.toast', ($, e) => {
    toasts.push(e.text)
    return { value: undefined }
  })
  on('ui.log', ($, e) => {
    logs.push(e.text)
    return { value: undefined }
  })
  on('session.usage', () => ({ value: { startedAt: 0, context: { window: 1000000 }, rateLimits: [], cost: { usd: 0 } } }))
  on('session.model', () => ({ value: 'claude-haiku-5-5' }))
  on('session.id', () => ({ value: 'session-1' }))
  on('session.version', () => ({ value: { version: '2.1.294' } }))
  on('session.start', ($, e) => ({ cwd: e.cwd }))
  // エンジンの下（送信が入った = 打たれた文字がそのまま返る）
  on('prompt.submit', ($, e) => ({ text: e.text }))
}

const START = { cwd: '/tmp', surface: 'terminal', isInteractive: true } as const
const ENV = { TAKO_PANE_ID: '7', TAKO_CLI: '/opt/tako/tako' }

describe('tako mod の送信時の確認（#2050）', () => {
  test('人の入力は送る前に tako へ判定させ、本文は標準入力で渡す（argv に載せない）', async ($, on) => {
    const calls: Call[] = []
    world(on, calls, [], [], 'pass')
    mock.clock(on)
    mock.env(on, ENV)
    await $.session.start(START)
    const text = `これで試して ${SECRET}`
    const entered = await $.prompt.submit({ text, origin: { kind: 'composer' }, wait: false })
    expect(entered).toEqual(expect.objectContaining({ text }))
    expect(calls.length).toBe(1)
    expect(calls[0]?.argv).toEqual(['/opt/tako/tako', 'paste-guard', 'check', '--pane', '7'])
    expect(calls[0]?.stdin).toBe(text)
    expect(JSON.stringify(calls[0]?.argv)).not.toContain('EXAMPLE')
  })

  test('AI・仕組みの送信（peer / plugin / 通知）は判定しない', async ($, on) => {
    const calls: Call[] = []
    world(on, calls, [], [], 'confirm')
    mock.clock(on)
    mock.env(on, ENV)
    await $.session.start(START)
    for (const origin of [
      { kind: 'peer' as const },
      { kind: 'task-notification' as const },
      { kind: 'plugin' as const, name: 'other' },
    ]) {
      const entered = await $.prompt.submit({ text: SECRET, origin, wait: false })
      expect(entered).toEqual(expect.objectContaining({ text: SECRET }))
    }
    expect(calls.length).toBe(0)
  })

  test('警告だけの段: 送信は止めずにトーストを出す（伏せ字だけ）', async ($, on) => {
    const calls: Call[] = []
    const toasts: string[] = []
    world(on, calls, toasts, [], 'warn')
    mock.clock(on)
    mock.env(on, ENV)
    await $.session.start(START)
    const entered = await $.prompt.submit({ text: SECRET, origin: { kind: 'composer' }, wait: false })
    expect(entered).toEqual(expect.objectContaining({ text: SECRET }))
    expect(toasts.length).toBe(1)
    expect(toasts[0]).toContain('ghp_…qrst')
    expect(toasts[0]).toContain('github_token')
    expect(toasts[0]).not.toContain('EXAMPLE')
  })

  test('tako が答えない（CLI の失敗）なら送信を通し、デバッグログに本文を書かない', async ($, on) => {
    const calls: Call[] = []
    const logs: string[] = []
    world(on, calls, [], logs, 'confirm', 1)
    mock.clock(on)
    mock.env(on, ENV)
    await $.session.start(START)
    const entered = await $.prompt.submit({ text: SECRET, origin: { kind: 'composer' }, wait: false })
    expect(entered).toEqual(expect.objectContaining({ text: SECRET }))
    expect(calls.length).toBe(1)
    for (const line of logs) expect(line).not.toContain('EXAMPLE')
    expect(logs.some(l => l.includes('paste-guard exit 1'))).toBe(true)
  })

  test('tako の外（TAKO_PANE_ID が無い）では判定しない', async ($, on) => {
    const calls: Call[] = []
    world(on, calls, [], [], 'confirm')
    mock.clock(on)
    mock.env(on, { TAKO_CLI: '/opt/tako/tako' })
    await $.session.start(START)
    const entered = await $.prompt.submit({ text: SECRET, origin: { kind: 'composer' }, wait: false })
    expect(entered).toEqual(expect.objectContaining({ text: SECRET }))
    expect(calls.length).toBe(0)
  })
})
