---
name: mac-shortcuts
description: Invoke user-defined macOS Shortcuts (from the Shortcuts app) via the `shortcuts` CLI. Use when the user has set up automations in Shortcuts and wants them run from Fennec. macOS only.
always: false
requirements:
  - shortcuts
---

# mac-shortcuts

Apple's Shortcuts app (macOS 12 Monterey+) ships a `shortcuts` CLI that can list, run, and share shortcuts the user has created. No API key; no AppleScript required. `requirements: [shortcuts]` auto-hides the skill on older macOS / non-Mac hosts.

## List the user's shortcuts

```bash
shortcuts list
```

Returns one shortcut name per line.

## Run a shortcut

Without input:
```bash
shortcuts run "Send arrived text"
```

With text input on stdin:
```bash
echo "The meeting is moved to 3pm" | shortcuts run "Send message to team"
```

With a file as input:
```bash
shortcuts run "Resize image" --input-path photo.jpg --output-path resized.jpg
```

Output (if the shortcut returns something):
```bash
result=$(shortcuts run "Get battery percentage")
echo "$result"
```

## View a shortcut

Open in the Shortcuts UI:
```bash
shortcuts view "Send arrived text"
```

Useful when the user wants to inspect or edit what a shortcut does.

## Sign and share

```bash
shortcuts sign --mode anyone -i input.shortcut -o signed.shortcut
```

Produces a signed `.shortcut` file the user can send to someone else.

## When this fits

- The user has already built a useful Shortcut (e.g. "Enable focus mode", "Start my morning routine", "Toggle VPN") and wants to fire it from chat.
- The task is macOS-app-specific and already automated in Shortcuts — don't rebuild what the user has built.

Skip this skill for tasks you can handle natively (files, shell, web) — Shortcuts adds latency and friction compared to direct tools.

## Rules

- Enumerate before invoking. "Run the X shortcut" should start with `shortcuts list | grep -i x` to confirm the name.
- Shortcut names are case-sensitive on `run`. Quote them — they often contain spaces.
- Shortcuts that require user interaction (choose from menu, accept permissions) will hang the shell tool. Prefer fully-automated shortcuts.
- Some shortcuts open UI windows; they'll complete the background action but leave the Shortcut app in the foreground.

## Failure modes

- `shortcuts: command not found` → macOS 11 or older. Upgrade or use AppleScript.
- `No shortcut named "..."` → typo or case mismatch. Run `shortcuts list` to verify.
- Shortcut hangs → interactive prompt waiting for user input. Kill and rewrite the shortcut to avoid prompts.
- `Error running shortcut` with no detail → the shortcut itself errored (bad action, permission denied). Open in the UI (`shortcuts view`) to debug.
