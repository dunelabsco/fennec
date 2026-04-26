---
name: skill-creator
description: Create a new Fennec skill. Use when the user asks to add a skill, or when a repeated procedure is worth capturing as a reusable skill file.
always: false
---

# skill-creator

Fennec skills are single markdown files in `~/.fennec/skills/`. Each file teaches Fennec a specific procedure or body of knowledge. The loader parses YAML frontmatter and injects the content into context either always (`always: true`) or on demand.

## When to create a skill

- The user explicitly asks: "make a skill for X".
- A multi-step procedure has come up more than once and is worth codifying.
- A tool has non-obvious usage patterns that would benefit every future invocation.

Do not create a skill for a one-shot task. Memory entries are a better fit for short-lived context.

## File format

Every skill is one `.md` file whose name matches the `name` field. Frontmatter keys the loader recognizes:

| Key | Required | What it does |
|---|---|---|
| `name` | yes | Machine-readable identifier; matches filename stem |
| `description` | yes | One sentence. Starts with a verb. Mentions the trigger ("Use when..."). This is what the agent reads to decide whether to load the skill |
| `always` | no | `true` = full body injected every turn. `false` (default) = only the description is surfaced; body loads on demand via the `load_skill` tool |
| `requirements` | no | YAML list of CLI commands that must exist in PATH, or the skill is silently hidden |

Example frontmatter (hypothetical skill; illustration only):

```
---
name: ip-lookup
description: Resolve an IP address to geolocation and ASN details via a public API.
always: false
requirements:
  - curl
  - jq
---
```

## Writing guidance

- **Length.** Always-on skills stay short (roughly under 80 lines) — every turn pays the token cost. On-demand can go longer but prefer compact.
- **Description style.** "Do X when Y." Avoid "This skill does...". The agent reads descriptions as triggers; they must answer "should I load this?" in one sentence.
- **No duplication.** If Fennec already has a tool for the job (shell, files, cron, memory, web, browser, etc.), the skill's job is *when and how* to invoke it — not reimplementing it.
- **Examples beat prose.** Short code blocks demonstrating real commands are more useful than paragraphs of explanation.
- **Be specific about failures.** List the common error messages and what to do about each.

## Installing the skill

Write the file to `~/.fennec/skills/<name>.md`. Fennec loads skills at agent start; restart the interactive agent or gateway to pick up a new skill. Invalid skills are logged and skipped — they do not abort startup.

## Validation checklist before finishing

1. Filename stem equals the `name` field (e.g. `weather.md` → `name: weather`).
2. Frontmatter fenced by `---` on its own line above and below. No extra text before the opening fence.
3. Description names the trigger, not the implementation.
4. If `requirements` listed, each command is one that actually resolves via PATH on the user's system.
5. Skill body is original prose — do not copy from other agent projects.
