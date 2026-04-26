# Fennec skills

This directory contains markdown-authored skills that extend what Fennec knows how to do. Skills are LLM instructions, not executable code — they teach the agent when and how to use Fennec's built-in tools (shell, files, web, cron, memory, etc.).

## Format

One file per skill, flat layout:

```
skills/<name>.md
```

Each file is YAML frontmatter followed by markdown:

```
---
name: my-skill
description: What the skill does and when to use it.
always: false            # true = always in system prompt; false = loaded on demand
requirements:            # optional; skill hides if any command is missing from PATH
  - gh
---

# Skill body...
```

The loader lives in `src/skills/loader.rs`. It reads `*.md` files directly from the skills directory (flat, non-recursive), parses frontmatter, and hides any skill whose `requirements` are not available on the host.

## Installing

Fennec loads skills from `~/.fennec/skills/` at runtime. To activate the skills in this directory:

```
cp skills/*.md ~/.fennec/skills/
```

Restart any running interactive agent or gateway to pick up new skills.

## Contents

| Skill | Mode | Requires | Purpose |
|---|---|---|---|
| `writing-plans.md` | always-on | — | Plan compound work before executing |
| `systematic-debugging.md` | always-on | — | Debug from root cause, not symptom |
| `skill-creator.md` | on-demand | — | Author a new Fennec skill |
| `github.md` | on-demand | `gh` | Use the gh CLI for GitHub workflows |

Always-on skills are injected into the system prompt every turn, so they stay short. On-demand skills are listed to the agent with their description; the agent loads the body via the `load_skill` tool when relevant.

## Authoring a new skill

Load the `skill-creator` skill and follow it. In short: one file per skill, frontmatter with `name` and `description`, body focused on *when* and *how* rather than *what*.
