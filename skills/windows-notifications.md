---
name: windows-notifications
description: Send toast notifications on Windows via the BurntToast PowerShell module. Use when the user wants a desktop alert for a long-running task, a reminder, or a status ping. Windows only.
always: false
requirements:
  - powershell
---

# windows-notifications

Windows 10 / 11 toast notifications from scripts are easiest through the **BurntToast** PowerShell module. It's a thin wrapper over the native `Windows.UI.Notifications` APIs — without it, raw PowerShell calls into those APIs are verbose and error-prone.

`requirements: [powershell]` keeps this skill hidden on non-Windows hosts (macOS and Linux don't ship `powershell` under that name — `pwsh` yes, `powershell` no).

## First-time setup

Install BurntToast as the user (no admin needed):

```powershell
Install-Module -Name BurntToast -Scope CurrentUser -Force
```

If the user's ExecutionPolicy blocks module scripts:
```powershell
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
```

Verify:
```powershell
Import-Module BurntToast
Get-Module BurntToast
```

## Basic notification

```powershell
New-BurntToastNotification -Text "Title line", "Body line"
```

`-Text` accepts 1–3 strings: the first is the title, subsequent ones are body lines.

From the shell tool (cmd.exe or Windows Terminal):
```
powershell -NoProfile -Command "New-BurntToastNotification -Text 'Backup done', 'Took 12 minutes'"
```

## With a custom icon / hero image

```powershell
New-BurntToastNotification -Text "Report ready" -AppLogo "C:\path\to\icon.png"
New-BurntToastNotification -Text "New photo" -HeroImage "C:\path\to\photo.jpg"
```

- `AppLogo` — small (square) icon shown in the corner.
- `HeroImage` — large banner image at the top of the toast.

Use absolute paths. Relative paths silently break.

## Actions (buttons)

```powershell
$button = New-BTButton -Content "Open logs" -Arguments "file:///C:/logs/build.log"
New-BurntToastNotification -Text "Build failed", "See logs for details" -Button $button
```

`-Arguments` is whatever URL or protocol you want macOS-style invoked — `file:///`, `https://`, or a custom registered scheme.

## Snooze / dismiss buttons

```powershell
$snooze = New-BTButton -Snooze
$dismiss = New-BTButton -Dismiss
New-BurntToastNotification -Text "Reminder", "Check the oven" -Button $snooze, $dismiss
```

## Sound

```powershell
New-BurntToastNotification -Text "Done" -Sound 'Alarm'
New-BurntToastNotification -Text "Quiet notify" -Silent
```

Built-in sound names: `Default`, `IM`, `Mail`, `Reminder`, `SMS`, `Alarm`, `Alarm2`–`Alarm10`, `Call`, `Call2`–`Call10`. `-Silent` suppresses sound entirely.

## Scheduled (deliver later)

```powershell
New-BurntToastNotification -Text "Meeting in 5 min" -Trigger (New-BTNotificationTrigger -At '2026-04-22 14:55:00')
```

For recurring reminders, prefer Fennec's `cron` tool to fire the notification instead of BurntToast's scheduled triggers — cron integrates with other skills.

## Multi-line body with formatting

Toast notifications support a limited subset of XML for richer content. BurntToast exposes this via `New-BTText`:

```powershell
$title = New-BTText -Content "Header" -MaxLines 1
$body = New-BTText -Content "Long message body that will wrap if needed" -MaxLines 3
$binding = New-BTBinding -Children $title, $body
$visual = New-BTVisual -BindingGeneric $binding
$content = New-BTContent -Visual $visual
Submit-BTNotification -Content $content
```

For most cases the simple `-Text "a","b"` form is fine.

## Header (grouping related toasts)

```powershell
$header = New-BTHeader -Id 'backup' -Title 'Backup'
New-BurntToastNotification -Text "Step 1 done" -Header $header
New-BurntToastNotification -Text "Step 2 done" -Header $header
```

Notifications with the same header `Id` stack together in Action Center.

## Rules

- BurntToast is a user-scope module. Don't install `-Scope AllUsers` unless the user explicitly asked — it requires admin and bleeds across accounts.
- Toasts appear at the **bottom-right of the user's current login session**. They do NOT deliver if the user is locked out, logged off, or on another account. For truly reliable delivery, use Fennec's send-message channels (Telegram, email) instead.
- Focus Assist rules can silently suppress toasts. If the user complains they didn't see the notification, check **Settings → System → Notifications → Focus**.
- Images (`-AppLogo`, `-HeroImage`) must be on the local filesystem. Network paths or http URLs don't work.
- For scripts run as SYSTEM (service account), toasts don't reach the logged-in user — that's a Windows-level constraint, not BurntToast's.

## Failure modes

- `The term 'New-BurntToastNotification' is not recognized` → module isn't installed for this user. Run `Install-Module -Name BurntToast -Scope CurrentUser`.
- `Cannot be loaded because running scripts is disabled` → ExecutionPolicy. `Set-ExecutionPolicy RemoteSigned -Scope CurrentUser`.
- Toast fires but no image shows → absolute path is wrong or the file isn't readable by the user. Test with `Test-Path`.
- Toast fires and disappears instantly → Focus Assist is on, or the notification landed in Action Center directly. Check the Action Center icon.
- Nothing visible at all but no error → the Windows Notifications service is disabled. Enable via **Settings → System → Notifications**.

## Related

- The `powershell` skill covers the broader PowerShell usage; this one is scoped to the notification UX.
- For scheduled / recurring notifications, combine with Fennec's `cron` tool rather than BurntToast's own trigger scheduling.
