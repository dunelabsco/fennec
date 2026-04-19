---
name: contacts
description: Look up people in the macOS Contacts app via AppleScript — find phone numbers, emails, addresses by name. Use when the user refers to someone by name and you need their contact details. macOS only.
always: false
---

# contacts

Contacts.app on macOS exposes the system address book via AppleScript. Syncs with iCloud + other CardDAV accounts. First use prompts for Automation permission.

## Search by name

```bash
osascript <<'APPLESCRIPT'
tell application "Contacts"
  set people to (every person whose name contains "Sam")
  set out to {}
  repeat with p in people
    set n to name of p
    set phones to value of phones of p
    set emails to value of emails of p
    set end of out to n & " | " & (phones as string) & " | " & (emails as string)
  end repeat
  return out
end tell
APPLESCRIPT
```

## Get a specific person

```bash
osascript <<'APPLESCRIPT'
tell application "Contacts"
  set p to first person whose first name is "Sam" and last name is "Morgan"
  set ph to value of first phone of p
  set em to value of first email of p
  return "Phone: " & ph & return & "Email: " & em
end tell
APPLESCRIPT
```

## List all phones / emails for a person

A contact often has multiple. Iterate:
```bash
osascript <<'APPLESCRIPT'
tell application "Contacts"
  set p to first person whose name is "Sam Morgan"
  set out to {}
  repeat with ph in phones of p
    set end of out to (label of ph) & ": " & (value of ph)
  end repeat
  return out
end tell
APPLESCRIPT
```

Labels: `"_$!<Mobile>!$_"`, `"_$!<Home>!$_"`, `"_$!<Work>!$_"` — Contacts uses internal label codes. Strip them or map to plain English:
```applescript
if label of ph contains "Mobile" then "Mobile"
else if label of ph contains "Work" then "Work"
...
```

## Create a contact

```bash
osascript <<'APPLESCRIPT'
tell application "Contacts"
  set p to make new person with properties {first name:"Sam", last name:"Morgan"}
  tell p
    make new phone at end of phones with properties {label:"mobile", value:"+15551234567"}
    make new email at end of emails with properties {label:"work", value:"sam@example.com"}
  end tell
  save
end tell
APPLESCRIPT
```

**Important:** `save` is required or the new record stays in memory only.

## Alternative: `contacts` CLI (`contacts` binary, Homebrew)

For simple reads, a one-liner beats AppleScript:
```bash
contacts -f "%n %p" "Sam"         # name + phone
contacts -H "Sam"                  # multi-line human output
```

Install: `brew install contacts`. Read-only, no writes.

## Rules

- Don't edit or delete contacts silently. These records often represent people the user cares about — confirm every write.
- iCloud sync is eventually-consistent. A newly created contact may take seconds to propagate to the user's phone.
- For disambiguation (multiple "Sam"s), return all matches and ask the user which one.
- Phone numbers in Contacts are often stored with formatting (parentheses, dashes, spaces). Normalise with a regex when comparing or using with other skills.

## Failure modes

- `-1743` → automation permission missing.
- `Can't get person` → name mismatch. Use `contains` rather than exact match for robustness.
- Empty phones/emails list → the contact exists but has no data in that field. Handle gracefully.
- Label codes look like `"_$!<...>!$_"` — strip or translate them before showing the user.
