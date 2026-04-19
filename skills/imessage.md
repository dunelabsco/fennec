---
name: imessage
description: Send iMessage / SMS messages from the macOS Messages app via AppleScript. Use when the user wants to send a quick text from the Mac. macOS only.
always: false
---

# imessage

Messages.app on macOS can send iMessage (to Apple users) and SMS (via iPhone SMS relay, if the user has it enabled). Driven via AppleScript. First use prompts for Automation permission.

## Send a message

By phone number (SMS or iMessage — iMessage preferred when the recipient is an Apple user):
```bash
osascript <<'APPLESCRIPT'
tell application "Messages"
  set targetService to 1st service whose service type = iMessage
  set buddy to buddy "+15551234567" of targetService
  send "Message body" to buddy
end tell
APPLESCRIPT
```

By email (iMessage-only):
```bash
osascript -e 'tell application "Messages" to send "Hello" to buddy "friend@icloud.com" of (1st service whose service type = iMessage)'
```

To a group chat (by chat ID — see "Finding the target"):
```bash
osascript -e 'tell application "Messages" to send "Hello all" to chat "iMessage;+;chat123456789"'
```

## Finding the target

The tricky part: resolving "John" to a phone number or chat ID.

**Phone numbers** — pass them in E.164 format (`+15551234567`). Ask the user if unsure.

**Chat IDs** — AppleScript exposes these, but they're opaque strings. Enumerate:
```bash
osascript <<'APPLESCRIPT'
tell application "Messages"
  set out to {}
  repeat with c in chats
    set end of out to (id of c) & " — " & (name of c)
  end repeat
  return out
end tell
APPLESCRIPT
```

## Read recent messages (limited)

Messages.app's AppleScript read API is weak — most reliable approach is reading `~/Library/Messages/chat.db` directly with `sqlite3` (Full Disk Access required):
```bash
sqlite3 ~/Library/Messages/chat.db \
  "SELECT datetime(date/1000000000 + strftime('%s','2001-01-01'),'unixepoch','localtime') AS time, is_from_me, text \
   FROM message ORDER BY date DESC LIMIT 20;"
```

Requires Full Disk Access granted to the running shell / terminal in System Settings.

## Rules

- **Confirm before sending.** Messages go to real phones — a mis-sent text is much more disruptive than a mis-sent email. Show the user the exact recipient and text, wait for approval.
- `send` returns immediately; actual delivery is async. Check Messages.app UI to confirm.
- SMS relay to non-Apple numbers requires an iPhone paired to the same Apple ID with "Text Message Forwarding" enabled. If it's not set up, the send silently fails or falls back to iMessage-only.
- Don't send images, attachments, or rich content via this path — AppleScript support is unreliable. Use the UI for those.

## Failure modes

- `-1743` → automation permission missing.
- `Can't get buddy` → number not in E.164 format, or recipient isn't on iMessage and SMS relay isn't configured.
- Message appears green (SMS) instead of blue (iMessage) → recipient isn't signed into iMessage on any of their devices.
- `chat.db` query fails → Full Disk Access not granted, or macOS upgrade moved / schema-changed the DB.
- Send appears to succeed but no reply ever arrives → the number has blocked the user's iMessage address. No way to detect this programmatically.
