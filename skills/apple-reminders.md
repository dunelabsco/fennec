---
name: apple-reminders
description: Read, create, and complete reminders in Apple Reminders via AppleScript. Use when the user wants to add a task with a due date, check what's coming up, or mark a reminder done. macOS only.
always: false
---

# apple-reminders

macOS Reminders is AppleScript-scriptable. iCloud sync works transparently. First use prompts for Automation permission.

## List lists

```bash
osascript -e 'tell application "Reminders" to return name of lists'
```

## List open reminders in a list

```bash
osascript <<'APPLESCRIPT'
tell application "Reminders"
  set out to {}
  repeat with r in reminders of list "Inbox"
    if completed of r is false then
      set end of out to (name of r) & " (due " & (due date of r as string) & ")"
    end if
  end repeat
  return out
end tell
APPLESCRIPT
```

## Create a reminder

Simple (no date):
```bash
osascript -e 'tell application "Reminders" to tell list "Inbox" to make new reminder with properties {name:"Call dentist"}'
```

With a specific date (build numerically to avoid locale parsing quirks):
```bash
osascript <<'APPLESCRIPT'
tell application "Reminders"
  set d to current date
  set year of d to 2026
  set month of d to 4
  set day of d to 24
  set hours of d to 15
  set minutes of d to 0
  set seconds of d to 0
  tell list "Inbox"
    make new reminder with properties {name:"Call dentist", due date:d}
  end tell
end tell
APPLESCRIPT
```

With a body/note:
```bash
osascript -e 'tell application "Reminders" to tell list "Inbox" to make new reminder with properties {name:"Call dentist", body:"Ask about follow-up x-rays"}'
```

## Mark complete

```bash
osascript <<'APPLESCRIPT'
tell application "Reminders"
  set r to first reminder of list "Inbox" whose name is "Call dentist" and completed is false
  set completed of r to true
end tell
APPLESCRIPT
```

## Rules

- Default list name is typically `"Reminders"` on older macOS and `"Inbox"` on newer — list names first and don't assume.
- Use numeric date construction, not string parsing. String-parsed dates break across locales.
- Time defaults to 9 AM if only the day is set.
- iCloud sync may take a few seconds. Don't assume a just-created reminder is visible on other devices immediately.

## Failure modes

- `-1743` → automation permission missing. System Settings → Privacy & Security → Automation.
- `Can't get reminder` → name mismatch or already completed. Use the "whose completed is false" filter.
- Missing list → `"Can't get list ..."`. List lists first; case-sensitive.
