// fake-responses-api.mjs — 実物の codex を本物の API・認証なしで動かす偽の Responses API（#2418）
//
// 何のためか: codex の worker の MCP の絞り込み（`-c mcp_servers.<名>.enabled=false`）が実行時に効くか =
// 止めた server の子プロセスが立たないかを、実物の codex で確かめたい。`codex exec` / TUI は認証が無いと
// MCP を起こす前に止まるので、一時の CODEX_HOME の config.toml で `model_provider` をこの偽 API へ向ける
// （`[model_providers.<名>] base_url = "http://127.0.0.1:<port>/v1"`・`wire_api = "responses"`・
// `requires_openai_auth = false`）。本物の codex の認証・課金は使わない。
//
// 答え: 最後の発話に「Show this card: {JSON}」があれば、要求の tools にある `…tako_show_command` を
// その JSON で呼ぶ function_call を返し、その結果（function_call_output）が来たら「CARD-DONE」（呼べなかった =
// `unsupported call` / 承認で断られたなら「CARD-FAILED」）と答える
// （worker から tako の MCP が使えることの確認）。「Reply with exactly one word: X」なら X、ほかは「ok」。
// FAKE_DELAY_MS を渡すと答える前にその時間だけ待つ（`codex exec` を生かしたまま子孫を数えるため）。
// 受けた要求は 1 行ずつ FAKE_LOG へ（経路と、要求の tools の名前だけ。本文は書かない）。
//
// 使い方: node scripts/lib/fake-responses-api.mjs <port>   （127.0.0.1 だけで待つ。0 なら空きの番号）
//         起動したら標準出力へ `FAKE_API_PORT=<番号>` を 1 行出す
import http from 'node:http'
import fs from 'node:fs'

const port = Number(process.argv[2] ?? 0)
const log = process.env.FAKE_LOG
const delay = Number(process.env.FAKE_DELAY_MS ?? 0)

function note(line) {
  if (log) fs.appendFileSync(log, line + '\n')
}

// 発話の本文（input の要素は文字列 / { type: 'message', content: [...] } / function_call_output など）
function textOf(item) {
  if (typeof item === 'string') return item
  const c = item?.content
  if (typeof c === 'string') return c
  if (!Array.isArray(c)) return ''
  return c.map(part => (typeof part?.text === 'string' ? part.text : '')).join('\n')
}

// 要求の tools の名前（function はそのまま・namespace は中の名前を `<namespace>__<name>` でも引ける形で）
function toolNames(tools) {
  const out = []
  for (const t of Array.isArray(tools) ? tools : []) {
    if (typeof t?.name === 'string') out.push(t.name)
    for (const inner of Array.isArray(t?.tools) ? t.tools : []) {
      if (typeof inner?.name === 'string') out.push(`${t.name ?? ''}__${inner.name}`)
    }
  }
  return out
}

function sse(res, events) {
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' })
  for (const ev of events) {
    res.write(`event: ${ev.type}\ndata: ${JSON.stringify(ev)}\n\n`)
  }
  res.end()
}

let seq = 0
function reply(res, item) {
  seq += 1
  const id = `resp_fake_${seq}`
  const usage = {
    input_tokens: 100,
    input_tokens_details: { cached_tokens: 0 },
    output_tokens: 5,
    output_tokens_details: { reasoning_tokens: 0 },
    total_tokens: 105,
  }
  sse(res, [
    { type: 'response.created', response: { id } },
    { type: 'response.output_item.done', output_index: 0, item },
    { type: 'response.completed', response: { id, usage } },
  ])
}

function message(text) {
  seq += 1
  return {
    type: 'message',
    role: 'assistant',
    id: `msg_fake_${seq}`,
    status: 'completed',
    content: [{ type: 'output_text', text, annotations: [] }],
  }
}

// `…tako_show_command` の呼び先。codex 0.154.0 は MCP のツールを `{ type: 'namespace', name: 'mcp__<server>',
// tools: [{ type: 'function', name: '<tool>' }] }` で渡し、呼び出しは `name: '<tool>'` + `namespace: 'mcp__<server>'`
// の形でだけ受ける（`mcp__<server>__<tool>` の 1 語は `unsupported call` = 実測）
function takoShowCommand(tools) {
  for (const t of Array.isArray(tools) ? tools : []) {
    if (typeof t?.name === 'string' && /tako_show_command$/.test(t.name)) return { name: t.name }
    for (const inner of Array.isArray(t?.tools) ? t.tools : []) {
      if (typeof inner?.name === 'string' && /tako_show_command$/.test(inner.name)) {
        return { name: inner.name, namespace: t.name }
      }
    }
  }
  return null
}

function answer(body) {
  const input = Array.isArray(body?.input) ? body.input : []
  const last = input[input.length - 1]
  // tako の MCP を呼んだ結果が返ってきた番。呼べなかった（知らない呼び出し・承認待ちで断られた）なら CARD-FAILED
  if (last?.type === 'function_call_output' || last?.type === 'custom_tool_call_output') {
    const out = JSON.stringify(last.output ?? '')
    return message(/unsupported call|requires approval/.test(out) ? 'CARD-FAILED' : 'CARD-DONE')
  }
  // 最後のユーザーの発話
  const users = input.filter(i => i?.role === 'user')
  const text = users.length ? textOf(users[users.length - 1]) : ''
  const at = text.indexOf('Show this card: ')
  if (at >= 0) {
    const tool = takoShowCommand(body?.tools)
    if (tool !== null) {
      let args = text.slice(at + 'Show this card: '.length).trim()
      const nl = args.indexOf('\n')
      if (nl >= 0) args = args.slice(0, nl)
      seq += 1
      return {
        type: 'function_call',
        id: `fc_fake_${seq}`,
        call_id: `call_fake_${seq}`,
        ...tool,
        arguments: args,
        status: 'completed',
      }
    }
    return message('NO-TAKO-TOOL')
  }
  const m = /Reply with exactly one word: (\S+)/.exec(text)
  return message(m ? m[1] : 'ok')
}

const server = http.createServer((req, res) => {
  let raw = ''
  req.on('data', d => {
    raw += d
  })
  req.on('end', () => {
    let body = null
    try {
      body = raw ? JSON.parse(raw) : null
    } catch {
      body = null
    }
    const path = (req.url ?? '').split('?')[0]
    if (req.method === 'POST' && /\/responses$/.test(path)) {
      const input = Array.isArray(body?.input) ? body.input : []
      const last = input[input.length - 1]
      const takoTools = (Array.isArray(body?.tools) ? body.tools : [])
        .filter(t => /tako/.test(t?.name ?? ''))
        .map(t => `${t.type}:${t.name}`)
      note(
        `POST ${path} last=${last?.type ?? last?.role ?? '-'} tako=${takoTools.join(',')} ` +
          `tools=${toolNames(body?.tools).filter(n => /mcp|__/.test(n)).join(',')}`,
      )
      const send = () => reply(res, answer(body))
      if (delay > 0) setTimeout(send, delay)
      else send()
      return
    }
    note(`${req.method} ${path}`)
    // モデル一覧などの付随の要求には空で答える
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ object: 'list', data: [], models: [] }))
  })
})

server.listen(port, '127.0.0.1', () => {
  process.stdout.write(`FAKE_API_PORT=${server.address().port}\n`)
})
