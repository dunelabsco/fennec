---
name: apple-calendar
description: Read events, create new events, and query calendars in the macOS Calendar app via AppleScript. Use for "what's on my calendar" questions and scheduling requests. macOS only.
always: false
---

# apple-calendar

macOS Calendar.app is AppleScript-scriptable. Covers any calendar the user has connected (iCloud, Google, Exchange, subscribed) — they all show up in Calendar.app. First use prompts for Automation permission.

## List calendars

```bash
osascript -e 'tell application "Calendar" to return name of calendars'
```

## Events in a date range

```bash
osascript <<'APPLESCRIPT'
tell application "Calendar"
  set startD to current date
  set endD to startD + 7 * days
  set out to {}
  repeat with c in calendars
    set evs to (events of c whose start date ≥ startD and start date ≤ endD)
    repeat with e in evs
      set end of out to (summary of e) & " @ " & (start date of e as string)
    end repeat
  end repeat
  return out
end tell
APPLESCRIPT
```

For single-calendar queries, replace the outer `repeat with c` loop with direct access, e.g. `calendar "Work"`.

## Create an event

```bash
osascript <<'APPLESCRIPT'
tell application "Calendar"
  set startD to current date
  set year of startD to 2026
  set month of startD to 4
  set day of startD to 24
  set hours of startD to 14
  set minutes of startD to 0
  set seconds of startD to 0
  set endD to startD + 1 * hours
  tell calendar "Work"
    make new event with properties {summary:"Meeting with Sam", start date:startD, end date:endD, location:"Room 3"}
  end tell
end tell
APPLESCRIPT
```

With attendees (adds their names, does NOT send invites from AppleScript):
```bash
osascript -e 'tell application "Calendar" to tell calendar "Work" to make new event with properties {summary:"Review", start date:(current date), end date:(current date) + 1 * hours, attendees:{make new attendee with properties {email:"sam@example.com"}}}'
```

## Quick alternative: `icalBuddy` CLI

For read-only "what's next" queries, `icalBuddy` (Homebrew: `brew install ical-buddy`) is much simpler:
```bash
icalBuddy eventsToday
icalBuddy -ic "Work" -eed -iep "datetime,title,location" eventsToday+7
```

Does not need AppleScript or permission prompts once installed. Can't create events.

## Rules

- Calendar.app only syncs events it has permission for. Google Calendar needs the user to have added their account in System Settings → Internet Accounts.
- Creating events via AppleScript does NOT send invites to attendees, even if `attendees` are set. For true meeting invites, use the `google-workspace` skill with Calendar API, or ask the user to create via the UI.
- Event times are local by default. For UTC-aware scheduling, convert carefully or use the REST-based `google-workspace` Calendar flow.

## Failure modes

- `-1743` → automation permission missing.
- `Can't get calendar "..."` → name mismatch. List calendars first. Names can contain special characters (em-dashes etc.) — double-check.
- Subscribed (read-only) calendars reject `make new event` with an error. Confirm writability first.
- `icalBuddy` not found → install via `brew install ical-buddy`, or use AppleScript.
