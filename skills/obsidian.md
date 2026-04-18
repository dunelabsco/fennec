---
name: obsidian
description: Read and write notes in the user's Obsidian vault. Use when the user wants to capture, retrieve, or update notes without opening the Obsidian app.
always: false
---

# obsidian

An Obsidian vault is just a directory of markdown files on disk. Fennec reads and writes them with `read_file`, `write_file`, and `list_dir`, and uses the `shell` tool with `find` / `grep` for larger sweeps. No Obsidian process needed. Backlinks, graph view, and plugins are handled by the Obsidian app itself when the user next opens the vault.

## Getting the vault path

Vault location is per-user. Ask once, remember it via `memory_store`:

> "Where is your Obsidian vault? Full path, please."

Common defaults: `~/Documents/Obsidian`, `~/vault`, `~/notes`. Do not guess; ask.

After confirming, save it to memory with a stable key like `obsidian_vault_path` so you don't ask again next session.

## Vault conventions

Many vaults use top-level folders by note type:

```
<vault>/
  Daily/       YYYY-MM-DD.md per day
  Projects/    one subfolder per project
  Areas/       ongoing responsibilities (Finance/, Health/)
  Resources/   reference notes, web clips
  People/      one file per person
  _Inbox/      unprocessed capture
  .obsidian/   app config — DO NOT TOUCH
```

Not every vault follows this. Before assuming, `list_dir` on the vault root or ask the user.

## Links between notes

Obsidian uses **wikilinks**, not standard markdown links:

```
[[Some Note]]                  # link by filename (no .md extension)
[[Some Note|display text]]     # link with alias
[[Some Note#Heading]]          # link to a heading
[[Some Note#^block-id]]        # link to a block (block id is a caret + slug)
![[Some Note]]                 # embed the target note's content inline
![[image.png]]                 # embed an image
```

Backlinks are automatic — Obsidian builds them when the vault is next opened. You do not write backlinks manually.

## Frontmatter

Most users keep YAML frontmatter on every note:

```
---
title: Some Note
date: 2026-04-19
tags: [agent, automation]
status: draft
---
```

Tags can live in frontmatter (`tags: [foo, bar]`) or inline in the body (`#foo`). Respect what the vault already uses — don't mix styles if the user is consistent.

## Common operations

**Append to today's daily note**

```
today=$(date +%Y-%m-%d)
f="<vault>/Daily/$today.md"
# create if missing with minimal frontmatter, then append the new content
```

Inspect a few existing daily notes first to match the user's format. Some vaults use `YYYY-MM-DD`, others `DD-MM-YYYY`, others a different folder entirely.

**Create a new concept note**

1. Pick a filename — human-readable, no extension in the link later. `camelCase`, `kebab-case`, `Title Case` are all common; match the vault.
2. Write YAML frontmatter + body.
3. If relevant, link to it from the appropriate daily note or project page.

**Search by tag**

```
grep -rl '^tags:.*project-x' <vault>     # frontmatter tags
grep -rl '#project-x' <vault>            # inline hashtags
```

Or `find <vault> -name '*.md' -newer /tmp/since` piped into a loop that `read_file`s each match — useful when the filter is more complex than a regex.

## Rules

- **Never touch `.obsidian/`.** That's app settings, hotkeys, and plugin state. Corrupting it breaks the user's setup.
- **Don't rename files casually.** Renames break links pointing at the old filename (Obsidian only auto-updates renames done inside the app). Ask before renaming.
- **Preserve existing frontmatter.** When editing a note, read the current frontmatter, add fields, don't overwrite the block.
- **Don't auto-maintain a `## Backlinks` section.** Obsidian handles backlinks itself. A hand-written list will go stale.

## Anti-patterns

- Creating notes scattered at vault root when the vault uses folders.
- Adding `[[wikilinks]]` to notes that do not exist yet without telling the user — they will see unresolved-link warnings.
- Writing daily-note content in the wrong format for the vault (`2026-04-19.md` vs `19-04-2026.md`) — check existing files first.
- Using standard markdown links `[text](file.md)` inside a vault. They technically work but break the graph view and feel wrong to Obsidian users.
