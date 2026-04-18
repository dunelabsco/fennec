---
name: research
description: Do multi-source research on a topic and report back verified findings. Use when a single lookup isn't enough — the answer needs multiple sources, dates, or cross-checks.
always: false
---

# research

Research is what you do when a single tool call doesn't answer the question. The goal: reach a conclusion that would survive a hostile reviewer.

## When to research vs answer from memory

Research when:

- The question has a right answer but you aren't sure what it is.
- The answer depends on recent events (anything after your training cutoff).
- The question concerns a specific library or API where version drift matters.
- The user explicitly asks to "look into" or "check" something.

Answer from memory when:

- The fact is stable and well-established.
- You have already researched the same thing in this session or recent memory and cited it.

If uncertain, research. Being confidently wrong is worse than being slow.

## Source hierarchy

Prefer sources in this order:

1. **Official docs, specs, or source code.** The vendor's manual, RFC, reference implementation. Authoritative.
2. **Reputable third-party references.** MDN, cppreference, language books, well-maintained community wikis.
3. **Community answers** (Stack Overflow, blog posts). Useful for "how do I...?" — verify against #1 or #2 before trusting.
4. **LLM intuition (yours).** Last resort. Label it explicitly when you use it.

Do not invert this order because it is faster. Speed comes from knowing when to stop, not from skipping verification.

## Multi-source verification

- Two sources agreeing only counts if they are independent. Two tutorials that copy-pasted each other are one source.
- If two sources disagree, say so. Report both, name the conflict, and say which you trust and why.
- Pin versions when version matters. "In FastAPI 0.116, …" beats "In FastAPI, …".

## Working pattern

1. **Refine the question.** A vague ask ("how do I deploy this?") becomes specific ("what's the recommended way to deploy a FastAPI app behind Cloudflare Tunnel as of today?") before you start.
2. **Start broad, narrow fast.** A `web_search` for orientation, then `web_fetch` on the most promising result.
3. **Parallelise independent lookups.** Multiple `delegate` calls are fine for disjoint subtopics when that tool is available.
4. **Record findings into memory** when the answer will matter again next week.
5. **Cite in the reply.** URLs or file paths the user can verify themselves. Short direct quotes if the source phrasing is important.

## Anti-patterns

- Stopping at the first plausible answer.
- Paraphrasing a single source and presenting it as consensus.
- Burying uncertainty. "I think" or "it's not clear, but" belongs in the reply, not absent from it.
- Researching what you were supposed to just do.
- Fetching twenty sources when two would have been enough. Research is bounded, not exhaustive.
