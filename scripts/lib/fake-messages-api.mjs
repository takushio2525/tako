// fake-messages-api.mjs — 実物の Claude Code を本物の API・認証なしで動かす偽の Messages API（#1962）
//
// 何のためか: tako mod の実経路テストは実物の claude（2.1.294 以上）に描かせて画面で確かめたいが、
// 本物の認証（キーチェーン・設定 dir の資格情報）を一時の設定 dir へ写す道は使えない（写してもいけない）。
// そこで一時の CLAUDE_CONFIG_DIR + ANTHROPIC_BASE_URL=この偽 API + ダミーの ANTHROPIC_API_KEY で動かす
// （設計書 §9.1 の試作と同じ構成。API キー経由なので $.session.usage().rateLimits は空）。
//
// 答え: 最後の発話に「Reply with exactly one word: X」があれば X、「Reply with N numbered lines」なら
// LINE-1〜LINE-N の N 行（#1961: 会話を画面より長くして遡れるようにする。N は 1〜500）、
// 要約の依頼（/compact）には要約の文、ほかは「ok」。usage の input_tokens は FAKE_INPUT_TOKENS（既定 60000 = ctx が数 % 出る）。
// #1963: 最後の発話に「Show this card: {JSON}」があれば、要求の tools にある `…tako_show_command` を
// その JSON（commands / label）で呼ぶ tool_use を返し、その結果（tool_result）が来たら「CARD-DONE」と答える。
// 受けた要求は 1 行ずつ FAKE_LOG へ（経路・stream・要約の依頼か・ツールを呼んだかだけ。本文は書かない）。
// #2050: FAKE_MARK を渡すと、最後の発話がその文字を含んだかの真偽（marked）も書く（鍵らしき文字の
// 確認で「やめた送信が API へ届いていない」を本文を残さずに確かめる口）。
// #1964: 「Reply with code blocks」なら 2 つのコードブロック（157 字・改行なしの長いコマンドと Python）を
// 含む Markdown、「Run this command: <cmd>」なら要求の tools にある Bash をその command で呼び、結果が
// 来たら「RUN-DONE」と答える（チャット風の描画 = 吹き出し・コードのコピー・ツールの要約を実物で見る材料）。
// #2578: 「Echo verbatim: <本文>」なら <本文> をそのまま答える（claude が長い URL / パスを自分で
// 折り返して字下げする形を、実物の描画で作る材料。` | ` は改行に読み替える = 1 行の send で複数行を返す）。
//
// 使い方: node scripts/lib/fake-messages-api.mjs <port>   （127.0.0.1 だけで待つ）
import http from 'node:http'
import fs from 'node:fs'

const port = Number(process.argv[2] ?? 0)
const inputTokens = Number(process.env.FAKE_INPUT_TOKENS ?? 60000)
const log = process.env.FAKE_LOG
const mark = process.env.FAKE_MARK

function textOf(content) {
  if (typeof content === 'string') return content
  if (!Array.isArray(content)) return ''
  return content.map(part => (typeof part?.text === 'string' ? part.text : '')).join('\n')
}

// 最後の発話が tool_result（ツールの結果を返した番）か
function isToolResult(content) {
  return Array.isArray(content) && content.some(part => part?.type === 'tool_result')
}

// 「Show this card: {JSON}」の JSON と、呼ぶツールの名前（要求の tools から探す = MCP の登録名に依らない）
function cardCall(body, text) {
  const at = text.indexOf('Show this card: ')
  if (at < 0) return null
  let input
  try {
    input = JSON.parse(text.slice(at + 'Show this card: '.length).trim())
  } catch {
    return null
  }
  const tools = Array.isArray(body?.tools) ? body.tools : []
  const tool = tools.find(t => typeof t?.name === 'string' && /tako_show_command$/.test(t.name))
  return tool === undefined ? null : { name: tool.name, input }
}

// 「Run this command: <cmd>」の Bash 呼び出し（要求の tools に Bash があるときだけ）
function bashCall(body, text) {
  const m = /Run this command: (.+)$/m.exec(text)
  if (m === null) return null
  const tools = Array.isArray(body?.tools) ? body.tools : []
  if (!tools.some(t => t?.name === 'Bash')) return null
  return { name: 'Bash', input: { command: m[1].trim(), description: 'Run the check command' } }
}

// 直前の assistant の発話が呼んだツールの名前（tool_result への答えを呼んだツールで分ける）
function lastToolName(messages) {
  const prev = [...messages].reverse().find(m => m?.role === 'assistant')
  const parts = Array.isArray(prev?.content) ? prev.content : []
  return parts.find(part => part?.type === 'tool_use')?.name ?? null
}

// #1964: コードブロックを 2 つ含む Markdown（1 つ目は 157 字・改行なし = コピーで改行が混ざらないかを見る）
const CODE_REPLY = [
  'Here is the build command:',
  '',
  '```bash',
  'cargo build -p tako-cli && TAKO_ISOLATED=1 TAKO_DATA_DIR=/tmp/tako-s7-very-long-data-dir-name ./out/tako-cli/tako mod --json | jq ".panes[] | {pane, reason}"',
  '```',
  '',
  'Then check it from Python:',
  '',
  '```python',
  'print("hello from tako")',
  '```',
].join('\n')

function answer(body) {
  const messages = Array.isArray(body?.messages) ? body.messages : []
  const last = [...messages].reverse().find(m => m?.role === 'user')
  const text = textOf(last?.content)
  const all = messages.map(m => textOf(m?.content)).join('\n') + textOf(body?.system)
  const summarize = /summar/i.test(text) && /conversation/i.test(all)
  const word = /Reply with exactly one word: ([A-Za-z0-9_-]+)/.exec(text)
  const lines = /Reply with (\d+) numbered lines/.exec(text)
  const count = lines ? Math.min(500, Math.max(1, Number(lines[1]))) : 0
  const echo = /Echo verbatim: (.+)$/m.exec(text)
  const call = summarize || isToolResult(last?.content) ? null : (cardCall(body, text) ?? bashCall(body, text))
  const reply = summarize
    ? 'SUMMARY-1962: the conversation so far was a short check.'
    : isToolResult(last?.content)
      ? lastToolName(messages) === 'Bash'
        ? 'RUN-DONE'
        : 'CARD-DONE'
      : echo
        ? echo[1].trim().split(' | ').join('\n')
      : word
        ? word[1]
        : count > 0
          ? Array.from({ length: count }, (_, i) => `LINE-${i + 1}`).join('\n')
          : /Reply with code blocks/.test(text)
            ? CODE_REPLY
            : 'ok'
  const marked = mark ? text.includes(mark) : null
  return { reply, summarize, call, marked }
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
    const { reply, summarize, call, marked } = answer(body)
    if (log) fs.appendFileSync(log, `${JSON.stringify({ path, stream: body.stream === true, summarize, tool: call?.name ?? null, marked })}\n`)
    const model = typeof body.model === 'string' ? body.model : 'claude-haiku-fake'
    const usage = { input_tokens: inputTokens, output_tokens: 5, cache_creation_input_tokens: 0, cache_read_input_tokens: 0 }
    const message = { id: `msg_${Date.now()}`, type: 'message', role: 'assistant', model, stop_sequence: null }
    const toolUse = call === null ? null : { type: 'tool_use', id: `toolu_fake_${Date.now()}`, name: call.name, input: call.input }
    if (body.stream !== true) {
      res.writeHead(200, { 'content-type': 'application/json' })
      const content = toolUse === null ? [{ type: 'text', text: reply }] : [toolUse]
      res.end(JSON.stringify({ ...message, content, stop_reason: toolUse === null ? 'end_turn' : 'tool_use', usage }))
      return
    }
    res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' })
    const send = (event, data) => res.write(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`)
    send('message_start', { type: 'message_start', message: { ...message, content: [], stop_reason: null, usage: { ...usage, output_tokens: 1 } } })
    if (toolUse === null) {
      send('content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } })
      send('content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: reply } })
    } else {
      send('content_block_start', { type: 'content_block_start', index: 0, content_block: { ...toolUse, input: {} } })
      send('content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'input_json_delta', partial_json: JSON.stringify(toolUse.input) } })
    }
    send('content_block_stop', { type: 'content_block_stop', index: 0 })
    send('message_delta', { type: 'message_delta', delta: { stop_reason: toolUse === null ? 'end_turn' : 'tool_use', stop_sequence: null }, usage: { output_tokens: 5 } })
    send('message_stop', { type: 'message_stop' })
    res.end()
  })
})

server.listen(port, '127.0.0.1', () => {
  const addr = server.address()
  process.stdout.write(`FAKE_API_PORT=${typeof addr === 'object' && addr !== null ? addr.port : port}\n`)
})
