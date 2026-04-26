---
name: powershell
description: Run PowerShell commands, scripts, and cmdlets on Windows (or macOS/Linux via pwsh). Use for Windows-native tasks like registry reads, service control, file ACLs, and any cmdlet the user's team relies on.
always: false
---

# powershell

PowerShell is the default automation shell on Windows. The modern cross-platform PowerShell Core (`pwsh`) also runs on macOS and Linux but is less commonly installed there. Invoke via the shell tool.

## Which binary?

- **Windows**: `powershell` (Windows PowerShell 5.1) OR `pwsh` (PowerShell 7+, if installed). Prefer `pwsh` when available.
- **macOS / Linux**: only `pwsh` (install with `brew install powershell` or the user's package manager).

Check availability:
```bash
command -v pwsh || command -v powershell
```

## One-shot command

```bash
pwsh -NoProfile -Command "Get-Process | Where-Object { $_.CPU -gt 10 } | Select-Object -First 5 Name, CPU"
```

or on Windows with legacy:
```bash
powershell -NoProfile -Command "Get-Service bits | Select Status, StartType"
```

Flags worth knowing:
- `-NoProfile` — skip the user's profile script, faster and more predictable.
- `-Command "..."` — run inline.
- `-File script.ps1` — run a script file.
- `-OutputFormat XML` — structured output (machine-parseable) instead of formatted text.

## Common cmdlets (Windows)

**Services**
```
Get-Service
Get-Service -Name 'WinRM'
Start-Service -Name '...'
Stop-Service -Name '...'
```

**Processes**
```
Get-Process
Get-Process chrome | Stop-Process
```

**Registry**
```
Get-ItemProperty -Path 'HKLM:\Software\Microsoft\Windows\CurrentVersion'
Set-ItemProperty -Path '...' -Name '...' -Value '...'
```

**Event log**
```
Get-WinEvent -LogName System -MaxEvents 20
```

**Files & ACLs**
```
Get-ChildItem -Path C:\Users -Recurse -File | Select -First 10
Get-Acl 'C:\path\to\file'
```

**Network**
```
Test-Connection -ComputerName example.com -Count 2
Test-NetConnection -ComputerName example.com -Port 443
Resolve-DnsName example.com
```

## Scheduled tasks (Windows)

```
Get-ScheduledTask | Select TaskName, State
Register-ScheduledTask -TaskName 'MyTask' -Action (New-ScheduledTaskAction -Execute 'notepad.exe') -Trigger (New-ScheduledTaskTrigger -Daily -At '3am')
```

Fennec's own `cron` tool is usually a better fit for scheduling than Task Scheduler, unless the user specifically wants it in the Windows scheduler UI.

## Quoting

PowerShell quoting is awkward across the shell boundary. Prefer single quotes around the `-Command` argument and double quotes inside:
```bash
pwsh -NoProfile -Command 'Get-Process | Where-Object { $_.Name -eq "code" }'
```

For complex scripts, write to a `.ps1` file and run with `-File`:
```bash
cat > /tmp/script.ps1 <<'PS'
$services = Get-Service
$services | Where-Object { $_.Status -eq 'Running' } | Select -First 10
PS
pwsh -NoProfile -File /tmp/script.ps1
```

## Rules

- `-NoProfile` unless the user explicitly wants profile-configured functions.
- Test-Connection and Test-NetConnection are PowerShell's ping / port-check; don't assume `ping` alone.
- PowerShell returns objects, not text. When piping to `jq` or shell tools, use `ConvertTo-Json -Depth 3` first.
- Long-running cmdlets (`Get-EventLog` with big log, `Get-ChildItem -Recurse` on system drive) should run under `tmux` (see the `tmux` skill) to avoid shell timeouts.

## Failure modes

- `The term 'pwsh' is not recognized` → fall back to `powershell` on Windows, or install `pwsh` on macOS/Linux.
- `Execution of scripts is disabled on this system` → Windows ExecutionPolicy is Restricted. Run with `-ExecutionPolicy Bypass` (single invocation) or ask the user to set `Set-ExecutionPolicy RemoteSigned -Scope CurrentUser`.
- `Access is denied` on registry / service → need elevation. Tell the user to run the agent / shell as admin.
- Output is weirdly formatted → pipe through `| Format-Table -AutoSize` or `| ConvertTo-Json` for consistent shapes.
