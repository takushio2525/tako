# tako shell integration (PowerShell) -- emits OSC 7 (cwd) / OSC 133 (prompt marks). FR-2.4.1 / #525
#
# Dot-sourced from the user's $PROFILE by the marker block that `tako setup` writes.
# It does nothing outside a tako pane, so leaving it in the profile is harmless.
#
# ASCII ONLY. Windows PowerShell 5.1 reads a BOM-less .ps1 as the ANSI code page
# (CP932 on a Japanese Windows), so any non-ASCII byte here would be mangled once this
# text lives next to a profile that has no BOM. Non-ASCII data (a script path with
# Japanese characters) is escaped as [char]0xNNNN by the writer, never embedded raw.
#
# Works on Windows PowerShell 5.1 and PowerShell 7+.

if ($env:TAKO_PANE_ID -and -not $global:__takoShellIntegration) {
    $global:__takoShellIntegration = $true

    $global:__takoEsc = [char] 27
    $global:__takoBel = [char] 7
    # True while the shell sits at a prompt waiting for input. Only the fallback hook (used
    # when PSReadLine is absent) reads it: there the first command lookup after a prompt is
    # the user's command, which is where OSC 133;C belongs.
    $global:__takoAtPrompt = $false
    # True once a command actually ran, so the next prompt owes an OSC 133;D.
    $global:__takoRan = $false

    # Inside tako's persistence container (psmux / tmux on a socket named tako*) the OSC has
    # to be wrapped in a DCS passthrough to reach the outer terminal. Detected the same way
    # as the zsh / bash scripts: the first field of $TMUX is the socket path.
    #
    # The container's own variables are then dropped, so that running psmux / tmux inside a
    # tako pane is not treated as nesting (tako's container is meant to be invisible plumbing).
    # PSMUX_SESSION is psmux's nesting guard -- leaving it set makes `psmux` refuse to start
    # with "sessions should be nested with care" (measured).
    # Where the OSC goes when the container swallows it (#766). psmux parses OSC 7 / 133
    # into its own screen model and re-renders for the client, so nothing that is not part of
    # that model can reach the outer terminal -- there is no byte-level passthrough for ANY
    # sequence, and `allow-passthrough` is an accepted-but-unread option upstream. tako sets
    # TAKO_OSC_SINK for such containers and reads the file instead of the PTY. The bytes are
    # the SAME ones that would have gone to the console, so tako's parser and state machine
    # are unchanged.
    $global:__takoSink = $env:TAKO_OSC_SINK
    $global:__takoSinkBuf = ''
    # No BOM: tako reads raw bytes and the payload is ASCII by construction (paths are
    # percent-encoded). A BOM would land in front of the first ESC and break the scan.
    $global:__takoSinkEnc = New-Object System.Text.UTF8Encoding($false)

    $global:__takoPassthrough = $false
    if ($env:TMUX) {
        $__takoSock = ($env:TMUX -split ',')[0]
        $__takoLeaf = $__takoSock.Substring($__takoSock.LastIndexOfAny([char[]] ('\', '/')) + 1)
        if ($__takoLeaf.StartsWith('tako')) {
            $global:__takoPassthrough = $true
            Remove-Item Env:TMUX -ErrorAction SilentlyContinue
            Remove-Item Env:TMUX_PANE -ErrorAction SilentlyContinue
            Remove-Item Env:PSMUX_SESSION -ErrorAction SilentlyContinue
        }
    }

    # Wrap a raw escape sequence for the transport in use. Inside a passthrough every ESC of
    # the payload must be doubled (multiplexer rule).
    function global:__takoWrap([string] $seq) {
        # The side channel carries the sequence unwrapped: it never passes through the
        # container, so the DCS wrapping (and its ESC doubling) would only be noise.
        if ($global:__takoSink) {
            $global:__takoSinkBuf += $seq
            return ''
        }
        if ($global:__takoPassthrough) {
            $esc = [string] $global:__takoEsc
            return $esc + 'Ptmux;' + $seq.Replace($esc, $esc + $esc) + $esc + '\'
        }
        return $seq
    }

    # Percent-encode a path for a file:// URI, over its UTF-8 bytes. ':' and '/' stay literal
    # so the drive letter survives readably; everything outside the RFC 3986 unreserved set is
    # escaped so a '%' in a path cannot corrupt the decode on the tako side.
    function global:__takoUriPath([string] $path) {
        $out = New-Object System.Text.StringBuilder
        foreach ($b in [System.Text.Encoding]::UTF8.GetBytes($path)) {
            if (($b -ge 0x61 -and $b -le 0x7A) -or ($b -ge 0x41 -and $b -le 0x5A) -or
                ($b -ge 0x30 -and $b -le 0x39) -or
                $b -eq 0x2D -or $b -eq 0x2E -or $b -eq 0x5F -or $b -eq 0x7E -or
                $b -eq 0x2F -or $b -eq 0x3A) {
                [void] $out.Append([char] $b)
            } else {
                [void] $out.AppendFormat('%{0:X2}', $b)
            }
        }
        return $out.ToString()
    }

    # Write the sequences collected since the last flush as ONE file, replacing whatever was
    # there. A prompt owes up to three of them (133;D, 133;A, OSC 7) and they have to land
    # together -- writing each one separately would leave only the last, losing the exit code.
    #
    # `.new` + rename so the reader never sees a half-written burst (rename over an existing
    # file is atomic on NTFS). Failures are swallowed: a pane whose sink went away must keep
    # working as a terminal.
    function global:__takoSinkFlush {
        if (-not $global:__takoSink) { return }
        $buf = $global:__takoSinkBuf
        $global:__takoSinkBuf = ''
        if (-not $buf) { return }
        try {
            $tmp = $global:__takoSink + '.new'
            [System.IO.File]::WriteAllText($tmp, $buf, $global:__takoSinkEnc)
            Move-Item -LiteralPath $tmp -Destination $global:__takoSink -Force -ErrorAction Stop
        } catch { }
    }

    function global:__takoMark([string] $body) {
        return (__takoWrap ([string] $global:__takoEsc + ']133;' + $body + [string] $global:__takoBel))
    }

    # Defence in depth for #970. A verbatim path (\\?\C:\dir) becomes //?/C:/dir once the
    # separators are replaced, and file:/// + that is a path nothing can open: the pane cwd
    # ends up as ///?/C:/dir and every git operation in that tab fails. tako strips the prefix
    # where it resolves paths (tako_core::platform::path, boundary B26), but a shell can still
    # INHERIT a verbatim working directory: Set-Location rejects those (the FileSystem provider
    # answers 'Cannot find path' - measured), so the only way in is the cwd handed to
    # CreateProcess - tako itself started from one, or a pane reopened from a layout that a
    # pre-#970 build wrote. Only the emitter sees that. Stripping here is unconditional: unlike
    # the Rust side, which keeps the prefix when removing it would change meaning, there is no
    # usable URI form that keeps it.
    function global:__takoStripVerbatim([string] $path) {
        $ic = [System.StringComparison]::OrdinalIgnoreCase
        if ($path.StartsWith('\\?\UNC\', $ic)) { return '\\' + $path.Substring(8) }
        if ($path.StartsWith('\\?\', $ic)) { return $path.Substring(4) }
        return $path
    }

    # OSC 7. Only the FileSystem provider has a path the outer terminal can use
    # (Cert:\ / HKLM:\ would report a directory that does not exist).
    function global:__takoCwdSequence {
        $loc = $ExecutionContext.SessionState.Path.CurrentLocation
        if ($loc.Provider.Name -ne 'FileSystem') { return '' }
        $native = __takoStripVerbatim $loc.ProviderPath
        $uri = __takoUriPath ($native.Replace('\', '/'))
        return (__takoWrap ([string] $global:__takoEsc + ']7;file:///' + $uri + [string] $global:__takoBel))
    }

    # OSC 133;C (command started). Written straight to the console because it happens between
    # the prompt and the command output, not while a prompt string is being built.
    function global:__takoEmitExecuted {
        try { [Console]::Write((__takoMark 'C')) } catch { }
        __takoSinkFlush
    }

    # Exit code of the command that just finished, from the state captured at prompt entry.
    #
    # $? tells success/failure but carries no number; $LASTEXITCODE carries a number but ONLY
    # native executables ever set it. Taking $LASTEXITCODE whenever $? is false is wrong: a
    # cmdlet failure leaves the previous native command's code in place, so
    # `cmd /c exit 7` followed by a failing cmdlet would report 7 (measured).
    # So the failure is attributed first: if the newest error belongs to the history entry that
    # just ran, it is a PowerShell-level error and 1 is the right answer.
    function global:__takoExitCode($ok, $last, $err) {
        if ($ok) { return 0 }
        try {
            $hist = Get-History -Count 1
            if ($hist -and $err -and $err.InvocationInfo -and
                $err.InvocationInfo.HistoryId -eq $hist.Id) {
                return 1
            }
        } catch { }
        if ($null -ne $last -and $last -ne 0) { return $last }
        return 1
    }

    # Where OSC 133;C comes from: the console host calls PSConsoleHostReadLine (defined by
    # PSReadLine) to read one line, and it returns exactly when the user submits. Wrapping it
    # marks the real submit -- Enter, Ctrl+Enter, a pasted newline, any key binding.
    #
    # Measured in a real pane: the function already exists when this script is sourced, and the
    # host looks up 'PSConsoleHostReadLine' and 'Set-StrictMode' *before* the user types
    # anything -- which is why a command-lookup hook cannot be the primary mechanism here
    # (it would report Running while the pane sits idle).
    $global:__takoOriginalReadLine = $function:PSConsoleHostReadLine

    if ($global:__takoOriginalReadLine) {
        function global:PSConsoleHostReadLine {
            $line = & $global:__takoOriginalReadLine
            # A bare Enter runs nothing, so it must not open a command (there would be no
            # matching OSC 133;D and the pane would stay Running forever).
            if ($line -and $line.Trim().Length -gt 0) {
                $global:__takoRan = $true
                __takoEmitExecuted
            }
            return $line
        }
    } else {
        # PSReadLine is absent (it can be removed in a profile, and Windows PowerShell can run
        # without it). Fall back to the command-lookup hook -- the DEBUG-trap equivalent, and the
        # same shape the bash integration uses. The noisy lookups above all come from PSReadLine,
        # so in exactly this configuration they do not happen.
        $global:__takoPrevLookup = $ExecutionContext.SessionState.InvokeCommand.PreCommandLookupAction
        $ExecutionContext.SessionState.InvokeCommand.PreCommandLookupAction = {
            param($CommandName, $CommandLookupEventArgs)
            try {
                if ($global:__takoAtPrompt -and $CommandName -ne 'prompt' -and $CommandName -ne 'out-default') {
                    $global:__takoAtPrompt = $false
                    $global:__takoRan = $true
                    __takoEmitExecuted
                }
            } catch { }
            # Keep whatever the user (or another integration) had registered.
            if ($global:__takoPrevLookup) {
                & $global:__takoPrevLookup $CommandName $CommandLookupEventArgs
            }
        }
    }

    $global:__takoOriginalPrompt = $function:prompt

    function global:prompt {
        # MUST be the first three statements: anything else clobbers $?.
        $__takoOk = $global:?
        $__takoLast = $global:LASTEXITCODE
        $__takoErr = $global:Error[0]

        $out = ''
        if ($global:__takoRan) {
            $global:__takoRan = $false
            $out += (__takoMark ('D;' + (__takoExitCode $__takoOk $__takoLast $__takoErr)))
        }
        $out += (__takoMark 'A')
        $out += (__takoCwdSequence)

        # Hand the user's prompt the same $? / $LASTEXITCODE it would have seen without tako
        # (many prompts render success or failure themselves). -ErrorAction Ignore keeps the
        # synthetic failure out of $Error, and it has to be the last statement before the call.
        $global:LASTEXITCODE = $__takoLast
        if (-not $__takoOk) { Write-Error 'tako' -ErrorAction Ignore }

        if ($global:__takoOriginalPrompt) {
            $out += (& $global:__takoOriginalPrompt)
        } else {
            $out += 'PS ' + $ExecutionContext.SessionState.Path.CurrentLocation + '> '
        }
        $out += (__takoMark 'B')

        # Hand the whole burst (133;D with the exit code, 133;A, OSC 7) to the side channel in
        # one write. With a sink the marks contributed '' to $out, so the visible prompt is
        # exactly what the user's own prompt function produced.
        __takoSinkFlush

        # Arm the fallback hook last: everything above runs commands of its own.
        $global:__takoAtPrompt = $true
        return $out
    }
}
