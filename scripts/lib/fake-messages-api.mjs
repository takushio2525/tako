// fake-messages-api.mjs — 実物の Claude Code を本物の API・認証なしで動かす偽の Messages API（#1962）
//
// 何のためか: tako mod の実経路テストは実物の claude（2.1.294 以上）に描かせて画面で確かめたいが、
// 本物の認証（キーチェーン・設定 dir の資格情報）を一時の設定 dir へ写す道は使えない（写してもいけない）。
// そこで一時の CLAUDE_CONFIG_DIR + ANTHROPIC_BASE_URL=この偽 API + ダミーの ANTHROPIC_API_KEY で動かす
// （設計書 §9.1 の試作と同じ構成。API キー経由なので $.session.usage().rateLimits は空）。
//
// 答え: 最後の発話に「Reply with exactly one word: X」があれば X、要約の依頼（/compact）には
// 要約の文、ほかは「ok」。usage の input_tokens は FAKE_INPUT_TOKENS（既定 60000 = ctx が数 % 出る）。
// 受けた要求は 1 行ずつ FAKE_LOG へ（経路・stream・要約の依頼かだけ。本文は書かない）。
//
// 使い方: node scripts/lib/fake-messages-api.mjs <port>   （127.0.0.1 だけで待つ）
import http from 'node:http'
import fs from 'node:fs'

const port = Number(process.argv[2] ?? 0)
const inputTokens = Number(process.env.FAKE_INPUT_TOKENS ?? 60000)
const log = process.env.FAKE_LOG

function textOf(content) {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content.map(part => (typeof part?.text === 'string' ? part.text : '')).join('\n')
}

function answer(body) {
  const messages = Array.isArray(body?.messages) ? body.messages : []
  const last = [...messages].reverse().find(m => m?.role === 'user')
  const text = textOf(last?.content)
  const all = messages.map(m => textOf(m?.content)).join('\n') + textOf(body?.system)
  const summarize = /summar/i.test(text) && /conversation/i.test(all)
  const word = /Reply with exactly one word: ([A-Za-z0-9_-]+)/.exec(text)
  const reply = summarize ? 'SUMMARY-1962: the conversation so far was a short check.' : word ? word[1] : 'ok'
  return { reply, summarize }
}

const server = http.createServer((req, res) => {
  let raw = ''
  req.on('data', chunk => {
    raw += chunk
  })
  req.on('end', () => {
    const path = (req.url ?? '').split('?')[0]
    let body = {}
    try {
      body = raw === '' ? {} : JSON.parse(raw)
    } catch {
      body = {}
    }
    if (req.method === 'POST' && path.endsWith('/v1/messages/count_tokens')) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ input_tokens: inputTokens }))
      return
    }
    if (!(req.method === 'POST' && path.endsWith('/v1/messages'))) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end('{}')
      return
    }
    const { reply, summarize } = answer(body)
    if (log) fs.appendFileSync(log, `${JSON.stringify({ path, stream: body.stream === true, summarize })}\n`)
    const model = typeof body.model === 'string' ? body.model : 'claude-haiku-fake'
    const usage = { input_tokens: inputTokens, output_tokens: 5, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 }
    const message = { id: `msg_${Date.now()}`, type: 'message', role: 'assistant', model, stop_sequence: null }
    if (body.stream !== true) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ ...message, content: [{ type: 'text', text: reply }], stop_reason: 'end_turn', usage }))
      return
    }
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' })
    const send = (event, data) => res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`)
    send('message_start', { type: 'message_start', message: { ...message, content: [], stop_reason: null, usage: { ...usage, output_tokens: 1 } } })
    send('content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } })
    send('content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: reply } })
    send('content_block_stop', { type: 'content_block_stop', index: 0 })
    send('message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 5 } })
    send('message_stop', { type: 'message_stop' })
    res.end()
  })
})

server.listen(port, '127.0.0.1', () => {
  const addr = server.address()
  process.stdout.write(`FAKE_API_PORT=${typeof addr === 'object' && addr !== null ? addr.port : port}\n`)
})
