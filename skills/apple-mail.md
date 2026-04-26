---
name: apple-mail
description: Read, search, and send mail via the macOS Mail.app using AppleScript. Use when the user wants to compose / send a quick email from macOS without opening the app UI, or search their local inbox. macOS only.
always: false
---

# apple-mail

Mail.app on macOS is AppleScript-scriptable. Works with every account the user has configured (iCloud, Gmail, IMAP, Exchange). First use prompts for Automation permission.

For the cloud API path (Gmail specifically), use the `google-workspace` skill instead — it sends via SMTP over OAuth, doesn't require Mail.app to be running, and works on any platform. This skill is for when the user wants to drive their existing Mail.app.

## List accounts / mailboxes

```bash
osascript -e 'tell application "Mail" to return name of accounts'
osascript -e 'tell application "Mail" to return name of mailboxes of account "iCloud"'
```

## Search recent messages

```bash
osascript <<'APPLESCRIPT'
tell application "Mail"
  set out to {}
  set msgs to messages of inbox whose read status is false
  repeat with m in msgs
    set end of out to (subject of m) & " — " & (sender of m)
    if (count of out) ≥ 20 then exit repeat
  end repeat
  return out
end tell
APPLESCRIPT
```

Filter by sender:
```bash
osascript -e 'tell application "Mail" to return subject of (messages of inbox whose sender contains "sam@example.com")'
```

## Read a specific message

```bash
osascript <<'APPLESCRIPT'
tell application "Mail"
  set m to first message of inbox whose subject is "Quarterly report"
  return "From: " & (sender of m) & return & "Subject: " & (subject of m) & return & return & (content of m)
end tell
APPLESCRIPT
```

## Compose and send

Send immediately:
```bash
osascript <<'APPLESCRIPT'
tell application "Mail"
  set newMsg to make new outgoing message with properties {subject:"Subject here", content:"Body text.", visible:false}
  tell newMsg
    make new to recipient with properties {address:"sam@example.com"}
    make new cc recipient with properties {address:"copy@example.com"}
  end tell
  send newMsg
end tell
APPLESCRIPT
```

Draft only (leave in Drafts, let the user review):
```bash
osascript <<'APPLESCRIPT'
tell application "Mail"
  set newMsg to make new outgoing message with properties {subject:"Draft", content:"...", visible:true}
  tell newMsg
    make new to recipient with properties {address:"sam@example.com"}
  end tell
  -- don't call `send`
end tell
APPLESCRIPT
```

## Attachments

```applescript
tell newMsg
  make new attachment with properties {file name:POSIX file "/absolute/path/to/file.pdf"} at after last paragraph
end tell
```

## Rules

- **Never send mail silently.** Show the user the exact `subject`, `to`, `cc`, and `content` you're about to send, and wait for explicit approval. This is one of the most regrettable classes of bugs.
- Mail.app must be running and logged into the relevant account for Send to succeed.
- AppleScript's `content` is plain text by default. For HTML, create the message with `"html content"` instead of `"content"`, and provide HTML.
- Attachments must be absolute POSIX paths. Tilde `~` won't expand in AppleScript — resolve first.

## Failure modes

- `-1743` → automation permission missing.
- Silent failure on `send` → account offline, credentials expired, or Mail.app is quit. Check the app.
- `Attachment not found` → path wrong or file permissions deny Mail.app. Check with `ls -l`.
- Gmail accounts configured via IMAP in Mail.app send fine, but DKIM signing is done by Gmail on send — no extra work needed.
