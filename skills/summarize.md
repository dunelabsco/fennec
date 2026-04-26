---
name: summarize
description: Condense a URL, file, or transcript into a shorter form. Use when the user asks to summarise, or when a source is too long for further tool chains.
always: false
---

# summarize

"Summarize" is not a standalone tool — it is a pattern using Fennec's existing fetch/read tools plus your own language ability.

## The flow

1. **Fetch the source** using the most specific tool available:
   - `web_fetch` for http(s) URLs (HTML → markdown).
   - `read_file` for local files.
   - `pdf_read` for PDFs.
   - For YouTube, fetch a transcript URL via `web_fetch` — the player page itself contains no usable transcript.
2. **Read the result.** Note the structure: headings, argument boundaries, the thesis.
3. **Pick a summary shape** that matches the request (see below).
4. **Write the summary in your own words.** Never paste source text verbatim as if it were your summary.
5. **Cite location** — URL or file path — so the user can go read the original.

## Summary shapes

Match the shape to the request. If the user didn't specify, pick from the source type; if uncertain, ask.

| Shape | When |
|---|---|
| Bullet TL;DR (3–7 bullets) | Article, blog post, meeting notes |
| One paragraph | Chat-length ask, quick context |
| Problem / Approach / Result | Research paper, postmortem, technical writeup |
| Chronological outline | Transcript, call recording, long conversation |
| Diff summary | Two versions of the same document |

## Rules

- State the author's CLAIM, not just the topic. "This post argues X" beats "This post is about X".
- Preserve numbers, dates, names, direct quotes. Those are the parts the user cannot reconstruct from your prose.
- Flag uncertainty. If the source contradicts itself or is unclear, say so — don't paper over it.
- Use quotation marks for direct quotation. Reworded passages are your prose.
- Target 5–15% of source length, capped around 400 words unless the user asked for more.

## When the tool output is partial

- `web_fetch` may truncate very large pages. If you see a truncation marker, fetch a narrower URL fragment or search the page for the relevant section.
- `pdf_read` may miss scanned/image pages (no OCR). Say "summary omits pages N–M (image-only)."
- If you only saw half the document, the summary must say so.

## Anti-patterns

- Summarising something you skimmed. If you're not sure, re-read the relevant section.
- Adding your opinion without marking it. Summary first, commentary separately.
- Listing section titles as a summary — that is a table of contents.
- Inventing detail the source doesn't support. When uncertain, omit.
