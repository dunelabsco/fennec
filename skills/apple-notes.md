---
name: apple-notes
description: Read, create, and update notes in Apple Notes via AppleScript. Use when the user wants to capture a note, retrieve one, or append to existing notes. macOS only.
always: false
---

# apple-notes

Apple Notes is scriptable via AppleScript. Run through the shell tool with `osascript`. macOS only. No API key, but first use triggers a TCC prompt: the user must approve Automation access in System Settings → Privacy & Security → Automation.

## List notes

```bash
osascript <<'APPLESCRIPT'
tell application "Notes"
  set out to {}
  repeat with n in notes of default account
    set end of out to (name of n)
  end repeat
  return out
end tell
APPLESCRIPT
```

## Read a note by name

```bash
osascript -e 'tell application "Notes" to return body of (first note whose name is "Grocery List") of default account'
```

Returns HTML — Notes stores rich text as HTML.

## Create a note

```bash
osascript <<'APPLESCRIPT'
tell application "Notes"
  tell default account
    set n to make new note with properties {name:"Title", body:"<p>Body text</p>"}
    return id of n
  end tell
end tell
APPLESCRIPT
```

Body accepts HTML: `<h1>`, `<p>`, `<b>`, `<br>`, `<ul><li>`, etc. Plain-text newlines don't render — use `<br>` or `<p>` tags.

## Append to a note

```bash
osascript <<'APPLESCRIPT'
tell application "Notes"
  tell default account
    set n to first note whose name is "Grocery List"
    set body of n to (body of n) & "<br>• New item"
  end tell
end tell
APPLESCRIPT
```

## Work with folders

```bash
# list folders
osascript -e 'tell application "Notes" to return name of folders of default account'

# create inside a folder
osascript -e 'tell application "Notes" to make new note at folder "Work" of default account with properties {name:"...", body:"..."}'
```

## Rules

- First script run prompts for Automation permission. If the user declines, every call fails silently with `-1743`. Guide them to System Settings → Privacy & Security → Automation → Terminal (or the running shell) → enable "Notes".
- Body is HTML. Escape user-supplied characters: `<` → `&lt;`, `>` → `&gt;`, `&` → `&amp;`.
- Multi-line AppleScript is easier to maintain in a here-doc than a long `-e` string.
- Don't bulk-edit notes silently. The user's Notes are personal; always confirm destructive changes.

## Failure modes

- `-1743 errAEEventNotPermitted` → automation permission missing.
- `Can't get note "..."` → name doesn't match. List notes first.
- iCloud sync lag → edits on another device may take seconds to appear. Re-read the body before writing if the note is hot.
- HTML with unescaped `&` → AppleScript fails or writes garbage. Escape first.
