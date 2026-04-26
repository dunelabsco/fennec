---
name: nano-pdf
description: Read and generate PDFs. Use for extracting text from a PDF, or producing one from markdown / HTML when the user asks for a file they can send.
always: false
---

# nano-pdf

Two jobs this skill covers: reading PDFs with Fennec's built-in tool, and generating PDFs via common CLIs through the shell tool.

## Reading PDFs

Fennec ships a `pdf_read` tool:

```
pdf_read(source: "/path/to/paper.pdf")
pdf_read(source: "https://arxiv.org/pdf/2401.12345.pdf")
pdf_read(source: "/path/to/paper.pdf", max_chars: 20000)
```

- `source` accepts a local file path OR an http(s) URL. No need to download first.
- `max_chars` truncates the output. Default is 100,000; lower it when you only need a snippet.
- Output is extracted text. Images and complex layouts may arrive degraded — PDFs are not a clean format.
- Password-protected PDFs fail. If the user has a password, ask them to decrypt first: `qpdf --password=... --decrypt in.pdf out.pdf`.
- Scanned PDFs with no OCR layer return empty or gibberish text. Say so explicitly; do not guess content.

## Generating PDFs

There is no built-in PDF-generation tool. Route through the shell tool. Pick the path based on what is installed.

### Markdown → PDF via pandoc

Most flexible; requires a PDF engine.

```
pandoc input.md -o output.pdf
```

Default engine is `pdflatex` (can't handle some Unicode characters). For modern documents:

```
pandoc input.md -o output.pdf --pdf-engine=xelatex
pandoc input.md -o output.pdf --pdf-engine=lualatex
```

Lightweight path when LaTeX is not installed:

```
pandoc input.md -o output.pdf --pdf-engine=wkhtmltopdf
```

Pandoc options worth knowing:

- `--toc` — table of contents.
- `-V geometry:margin=1in` — margins.
- `-V mainfont="Georgia"` — font (requires xelatex/lualatex).
- `--highlight-style=pygments` — code block syntax colouring.

Check what is available before invoking:

```
command -v pandoc
command -v xelatex
command -v wkhtmltopdf
```

### HTML → PDF via wkhtmltopdf

Direct path when the source is already HTML:

```
wkhtmltopdf input.html output.pdf
wkhtmltopdf --page-size A4 --margin-top 15 input.html output.pdf
```

`wkhtmltopdf` is deprecated upstream but still widely installed and adequate for print-to-PDF of self-contained pages.

### Headless Chrome alternative

```
chromium --headless --disable-gpu --print-to-pdf=output.pdf file://$(pwd)/input.html
# or: google-chrome --headless --disable-gpu --print-to-pdf=output.pdf file://...
```

More reliable than wkhtmltopdf for modern CSS and web fonts, slower and heavier.

## Picking the path

| Input | Preferred path |
|---|---|
| Markdown → PDF | pandoc (+ xelatex for Unicode; + wkhtmltopdf if no LaTeX) |
| HTML → PDF | wkhtmltopdf, or headless chrome for modern CSS |
| LaTeX → PDF | `xelatex input.tex` or `pdflatex input.tex` directly |
| docx / epub → PDF | pandoc (it converts the input side too) |

If nothing is installed, say so. Do not fake a PDF by renaming a text file.

## Rules

- Always write to an explicit output path. Don't rely on cwd guesswork.
- Verify the output exists and is non-empty: `[ -s output.pdf ] && echo ok`.
- Show the user the final file path when you are done.
- For long-running PDF builds (big docs with LaTeX), run inside tmux (see the `tmux` skill) so the shell tool doesn't time out.

## Failure modes

- `pdflatex: command not found` → install a LaTeX distribution (`texlive-full` on Debian, MacTeX on macOS, MiKTeX on Windows), or fall back to `--pdf-engine=wkhtmltopdf`.
- `wkhtmltopdf: command not found` → many systems don't have it. Try headless chrome, or ask the user to install.
- `Undefined control sequence` from LaTeX → source has unescaped special characters. Either escape manually (`$`, `&`, `%`, `#`, `_`, `{`, `}`, `~`, `^`, `\`) or switch to `--pdf-engine=xelatex`, which is more forgiving.
- Output file exists but is tiny → the engine failed silently. Check stderr, not just the exit code.
