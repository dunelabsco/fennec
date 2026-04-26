---
name: raycast-extensions
description: Invoke Raycast commands and extensions from outside the Raycast UI via the `raycast://` URL scheme. Use when the user has Raycast installed and wants to fire an extension / script command / AI command from Fennec. macOS only.
always: false
---

# raycast-extensions

Raycast is the launcher / productivity app popular with Mac power users. Every command it exposes (built-in, extension, script-command, AI-command) has a deeplink URL. Fennec invokes these with the macOS `open` command — no HTTP, no API key.

Raycast itself has to be running for the deeplink to take effect (it launches automatically when the deeplink hits if it's installed and configured). macOS only.

## Invocation

```
open "raycast://<path>"
```

URL-encode special characters in arguments (`%20` for spaces, `%2F` for `/`, etc.).

## Extension commands

The most common case — invoking a command from an installed extension:

```
raycast://extensions/<author>/<extension-slug>/<command-slug>
```

Example — open the built-in "Quicklinks" extension's "Create Quicklink" command:
```
open "raycast://extensions/raycast/quicklinks/create-quicklink"
```

Example — Linear extension's "My Issues":
```
open "raycast://extensions/linear/linear/my-issues"
```

### Finding the deeplink

The easiest path: open Raycast, find the command, press `⌘+K` (Actions) → **Copy Deeplink**. That gives the exact URL.

## Script commands

User-installed script commands (bash/python/etc. snippets run by Raycast):

```
raycast://script-commands/<slugified-file-name-without-extension>
```

Pass arguments via the `arguments` query parameter (repeatable for multi-arg commands):
```
open "raycast://script-commands/color-conversion?arguments=%23FF0000&arguments=rgb"
```

## AI commands

User-defined AI command definitions:

```
raycast://ai-commands/<slugified-command-name>
```

## Fallback text / pre-filled input

Many commands accept a `fallbackText` parameter to pre-fill the first input field or to hand the command a starting string:

```
open "raycast://extensions/raycast/file-search/search-files?fallbackText=~/Library/Application%20Support/"
```

`fallbackText` is a Raycast-wide convention; not every command honours it but most do.

## Built-in / system commands (utilities)

Raycast ships a few universal deeplinks:

```
open "raycast://confetti"                            # celebratory animation (demo / notification)
open "raycast://extensions/raycast/window-management/center"
open "raycast://extensions/raycast/clipboard-history/clipboard-history"
```

## Discovery

Without the in-app "Copy Deeplink" action, finding `<author>/<extension>/<command>` slugs is manual — browse the Raycast Store page for the extension (URL has the slug pattern) and check the extension's `package.json` (`commands[].name` is the command slug).

## When this skill fits

- The user has Raycast installed and a specific command they use often.
- Fennec can trigger it as part of a larger workflow — e.g. "at 9am every weekday, run the Raycast command that clears my inbox."
- The task is easier to solve as a Raycast command than as a direct integration (Raycast extensions exist for a lot of services).

When the task has no matching Raycast command, skip this skill — don't try to fake it.

## Rules

- Raycast prompts for confirmation the first time an external deeplink is used for a given command (security measure). Subsequent runs work without prompting.
- Deeplinks work only on the host machine (no remote invocation). Don't try to run these from a server or over SSH.
- `arguments` is repeatable; each `arguments=<value>` is one positional arg in order.
- For commands that take no arguments, a plain deeplink with no query string is enough.
- Extension slugs and command slugs are case-sensitive.

## Failure modes

- `open: ... raycast://...: No such file or directory` → macOS doesn't know the `raycast://` scheme. Raycast isn't installed or hasn't been run once to register the scheme.
- Deeplink fires but nothing happens → the command slug is wrong, or the extension isn't installed for this user. Check the slug by using "Copy Deeplink" in Raycast.
- User confirmation prompt appears every time → security setting. Nothing to do; the first run is always confirmed.
- Arguments don't populate → the command's `argument` definitions in its `package.json` don't match what you passed, or the value wasn't URL-encoded.

## Related

- `mac-shortcuts` skill: Apple's native equivalent. Covers the same use case if the user runs Shortcuts instead of Raycast.
