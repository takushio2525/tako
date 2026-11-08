#Requires -Version 7.0
# pwsh 7 専用（#2329）。Windows PowerShell 5.1 は BOM の無い UTF-8 の日本語を ANSI として読み構文が崩れるので、1 行目の #Requires で理由を出して止める

<#
.SYNOPSIS
    Claude Code を認証なしで「1 ターン最後まで」走らせるための偽の Messages API（Issue #2433）。

.DESCRIPTION
    test-claude-git-bash-2433.ps1 が別プロセスで起こす。127.0.0.1 の空いたポートで待ち、届いた要求を
    LogDir へ残して、次の形で答える（Messages API の形。ストリーミングの要求には SSE で答える）:

      - tools にシェルのツール（Bash / PowerShell）があり、まだ tool_result が無い
          → そのツールの tool_use を 1 つ返す（Bash を優先。走ったシェルを名乗るコマンド）
      - tool_result が届いた → 中身を記録して end_turn のテキストで締める
      - それ以外（tools の無い副次の要求など） → 短いテキストで締める
      - /v1/messages/count_tokens → 固定の件数

    本物の認証情報は使わない（呼び手が ANTHROPIC_API_KEY にダミーを入れ、ANTHROPIC_BASE_URL をここへ向ける）。
    要求の本文は LogDir の req-NNN.json、要約は requests.log、tool_result は tool-results.log に残る。

.PARAMETER Port
    待つポート（呼び手が空いたものを選ぶ）。

.PARAMETER LogDir
    要求と要約を書く置き場（呼び手の砂場の中）。

.PARAMETER ReadyFile
    待ち受けを始めたら作るファイル（呼び手はこれを見てから claude を起こす）。
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][int]$Port,
    [Parameter(Mandatory = $true)][string]$LogDir,
    [Parameter(Mandatory = $true)][string]$ReadyFile
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Utf8 = [System.Text.UTF8Encoding]::new($false)
New-Item -ItemType Directory -Path $LogDir -Force | Out-Null
$RequestLog = Join-Path $LogDir 'requests.log'
$ToolResultLog = Join-Path $LogDir 'tool-results.log'

# 走ったシェルを名乗らせるコマンド（Bash は $0 と OSTYPE、PowerShell は版と系統）
$Probe = @{
    Bash       = 'echo "TAKO2433_SHELL bash zero=$0 ostype=$OSTYPE"'
    PowerShell = 'Write-Output "TAKO2433_SHELL powershell edition=$($PSVersionTable.PSEdition) version=$($PSVersionTable.PSVersion)"'
}

function Write-Log([string]$Path, [string]$Line) {
    [System.IO.File]::AppendAllText($Path, $Line + "`n", $Utf8)
}

function ConvertTo-CompactJson($Value) {
    $Value | ConvertTo-Json -Depth 32 -Compress
}

# 応答の本文（content ブロックの並び）を、要求に合わせて SSE か 1 つの JSON で返す
function Send-Message($Context, $Request, [object[]]$Blocks, [string]$StopReason) {
    $model = if ($Request -and $Request.PSObject.Properties['model']) { [string]$Request.model } else { 'claude-tako2433' }
    $stream = $Request -and $Request.PSObject.Properties['stream'] -and $Request.stream -eq $true
    $response = $Context.Response
    $response.StatusCode = 200
    $message = [ordered]@{
        id            = 'msg_tako2433'
        type          = 'message'
        role          = 'assistant'
        model         = $model
        content       = @()
        stop_reason   = $null
        stop_sequence = $null
        usage         = [ordered]@{ input_tokens = 1; output_tokens = 1 }
    }
    if (-not $stream) {
        $message.content = $Blocks
        $message.stop_reason = $StopReason
        $response.ContentType = 'application/json'
        $bytes = $Utf8.GetBytes((ConvertTo-CompactJson $message))
    } else {
        $sb = [System.Text.StringBuilder]::new()
        $emit = {
            param([string]$EventName, $Data)
            [void]$sb.Append("event: $EventName`n")
            [void]$sb.Append('data: ' + (ConvertTo-CompactJson $Data) + "`n`n")
        }
        & $emit 'message_start' ([ordered]@{ type = 'message_start'; message = $message })
        for ($i = 0; $i -lt $Blocks.Count; $i++) {
            $b = $Blocks[$i]
            if ($b.type -eq 'text') {
                & $emit 'content_block_start' ([ordered]@{ type = 'content_block_start'; index = $i; content_block = [ordered]@{ type = 'text'; text = '' } })
                & $emit 'content_block_delta' ([ordered]@{ type = 'content_block_delta'; index = $i; delta = [ordered]@{ type = 'text_delta'; text = $b.text } })
            } else {
                & $emit 'content_block_start' ([ordered]@{ type = 'content_block_start'; index = $i; content_block = [ordered]@{ type = 'tool_use'; id = $b.id; name = $b.name; input = @{} } })
                & $emit 'content_block_delta' ([ordered]@{ type = 'content_block_delta'; index = $i; delta = [ordered]@{ type = 'input_json_delta'; partial_json = (ConvertTo-CompactJson $b.input) } })
            }
            & $emit 'content_block_stop' ([ordered]@{ type = 'content_block_stop'; index = $i })
        }
        & $emit 'message_delta' ([ordered]@{ type = 'message_delta'; delta = [ordered]@{ stop_reason = $StopReason; stop_sequence = $null }; usage = [ordered]@{ output_tokens = 1 } })
        & $emit 'message_stop' ([ordered]@{ type = 'message_stop' })
        $response.ContentType = 'text/event-stream'
        $bytes = $Utf8.GetBytes($sb.ToString())
    }
    $response.ContentLength64 = $bytes.Length
    $response.OutputStream.Write($bytes, 0, $bytes.Length)
    $response.OutputStream.Close()
}

function Send-Json($Context, [int]$Status, $Body) {
    $bytes = $Utf8.GetBytes((ConvertTo-CompactJson $Body))
    $Context.Response.StatusCode = $Status
    $Context.Response.ContentType = 'application/json'
    $Context.Response.ContentLength64 = $bytes.Length
    $Context.Response.OutputStream.Write($bytes, 0, $bytes.Length)
    $Context.Response.OutputStream.Close()
}

# tool_result の中身（文字列か、text ブロックの並び）を 1 行にする
function Get-ToolResultText($Block) {
    $c = $Block.content
    if ($c -is [string]) { return $c }
    (@($c) | Where-Object { $_.type -eq 'text' } | ForEach-Object { $_.text }) -join ' '
}

$listener = [System.Net.HttpListener]::new()
$listener.Prefixes.Add("http://127.0.0.1:$Port/")
$listener.Start()
[System.IO.File]::WriteAllText($ReadyFile, 'ready', $Utf8)

$n = 0
while ($listener.IsListening) {
    $ctx = $listener.GetContext()
    $n++
    $path = $ctx.Request.Url.AbsolutePath
    $reader = [System.IO.StreamReader]::new($ctx.Request.InputStream, $Utf8)
    $body = $reader.ReadToEnd()
    $reader.Dispose()
    [System.IO.File]::WriteAllText((Join-Path $LogDir ('req-{0:D3}.json' -f $n)), $body, $Utf8)
    try {
        if ($path -like '*/count_tokens') {
            Write-Log $RequestLog "$n $($ctx.Request.HttpMethod) $path -> count_tokens"
            Send-Json $ctx 200 @{ input_tokens = 10 }
            continue
        }
        if ($path -notlike '*/v1/messages') {
            Write-Log $RequestLog "$n $($ctx.Request.HttpMethod) $path -> 404"
            Send-Json $ctx 404 @{ type = 'error'; error = @{ type = 'not_found_error'; message = 'tako2433 fake' } }
            continue
        }
        $req = $body | ConvertFrom-Json -Depth 100
        $tools = @()
        if ($req.PSObject.Properties['tools'] -and $req.tools) {
            $tools = @($req.tools | ForEach-Object { if ($_.PSObject.Properties['name']) { [string]$_.name } })
        }
        $results = @()
        foreach ($m in @($req.messages)) {
            if ($m.content -is [string]) { continue }
            foreach ($b in @($m.content)) {
                if ($b.PSObject.Properties['type'] -and $b.type -eq 'tool_result') { $results += $b }
            }
        }
        $shellTools = @($tools | Where-Object { $_ -in @('Bash', 'PowerShell') })
        Write-Log $RequestLog ("{0} POST {1} model={2} stream={3} tools={4} shell_tools=[{5}] tool_results={6}" -f `
                $n, $path, $req.model, ($req.PSObject.Properties['stream'] -and $req.stream), $tools.Count, ($shellTools -join ','), $results.Count)
        if ($results.Count -gt 0) {
            foreach ($r in $results) {
                $text = (Get-ToolResultText $r) -replace '\s+', ' '
                $isError = $r.PSObject.Properties['is_error'] -and $r.is_error
                Write-Log $ToolResultLog "is_error=$isError | $text"
            }
            Send-Message $ctx $req @([ordered]@{ type = 'text'; text = 'TAKO2433_DONE' }) 'end_turn'
        } elseif ($shellTools.Count -gt 0) {
            $name = if ('Bash' -in $shellTools) { 'Bash' } else { 'PowerShell' }
            $tool = @($req.tools | Where-Object { $_.PSObject.Properties['name'] -and $_.name -eq $name })[0]
            $toolInput = [ordered]@{ command = $Probe[$name] }
            $props = $tool.input_schema.properties
            if ($props -and $props.PSObject.Properties['description']) { $toolInput["description"] = "tako2433 probe" }
            Write-Log $RequestLog "$n -> tool_use $name"
            Send-Message $ctx $req @([ordered]@{ type = 'tool_use'; id = 'toolu_tako2433'; name = $name; input = $toolInput }) "tool_use"
        } else {
            Send-Message $ctx $req @([ordered]@{ type = 'text'; text = 'TAKO2433_TEXT' }) 'end_turn'
        }
    } catch {
        Write-Log $RequestLog "$n ERROR $($_.Exception.Message)"
        try { Send-Json $ctx 500 @{ type = 'error'; error = @{ type = 'api_error'; message = 'tako2433 fake error' } } } catch { }
    }
}
