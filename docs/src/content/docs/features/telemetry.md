---
title: Error Telemetry
description: Automatic error reporting for improving tako
---

# Error Telemetry

tako has an opt-in error telemetry feature that automatically sends crash reports (panic / critical errors) to help improve the software. **This feature is disabled by default** and requires explicit opt-in.

## What is collected

When telemetry is enabled, the following information is sent on panic or critical error:

| Field | Description | Example |
|---|---|---|
| `version` | tako version | `0.5.5` |
| `os_version` | OS version (macOS only) | `macOS 26.0 (Darwin 25.2.0)` |
| `error_kind` | Error category | `panic` / `critical` / `invariant_violation` |
| `message` | Error message (paths masked) | `index out of bounds at ~/src/main.rs:42` |
| `backtrace` | Stack trace (paths masked) | `~/src/...` |

## What is NOT collected

- Screen content, terminal output, or input text
- Current working directory
- User name, host name, or email
- File contents or command history
- Any personally identifiable information (PII)

All file paths in error messages and stack traces are masked:
- `/Users/<name>/...` → `~/...` (or `/Users/<user>/...`)
- `/home/<name>/...` → `/home/<user>/...`
- `/var/folders/<id>/<id>/...` → `/var/folders/<tmp>/...`

## How to enable/disable

### CLI

```bash
# Check status
tako telemetry status

# Enable
tako telemetry on

# Disable
tako telemetry off
```

### MCP

```json
{ "action": "on" }   // Enable
{ "action": "off" }  // Disable
{ "action": "status" } // Check (default)
```

Tool name: `tako_telemetry`

### Setup

The `tako setup` wizard asks whether to enable telemetry. You can change it at any time with the CLI/MCP commands above.

## Transparency

All sent reports are logged locally at `<data_dir>/telemetry.log`. You can review exactly what was (or would be) sent. The `status` command shows the log file path and count.

## Data handling

| Item | Detail |
|---|---|
| Storage | Cloudflare Workers KV |
| Retention | 90 days (auto-deleted) |
| Access | Only the project owner can read reports |
| Write endpoint | Rate-limited (10 req/min/IP), no authentication required |
| Read endpoint | Requires admin token (not included in the binary) |

## Deletion request

To request deletion of your reports or to ask questions about the telemetry data, please contact the project maintainer via GitHub Issues or email.

- GitHub: https://github.com/takushio2525/tako/issues
- Email: shiozawataku2525@gmail.com

## Source code

- Worker (collection endpoint): [`web/tako-error-collector/`](https://github.com/takushio2525/tako/tree/main/web/tako-error-collector)
- Rust client: [`crates/tako-control/src/telemetry.rs`](https://github.com/takushio2525/tako/tree/main/crates/tako-control/src/telemetry.rs)
