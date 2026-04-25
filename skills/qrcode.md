---
name: qrcode
description: Generate QR codes from text / URLs via the `qrencode` CLI. Use when the user wants to produce a scannable QR code for sharing URLs, Wi-Fi credentials, contact info, or short text.
always: false
requirements:
  - qrencode
---

# qrcode

`qrencode` is a widely-packaged CLI tool for generating QR codes. Simple, fast, no network. `requirements: [qrencode]` auto-hides this skill when the binary isn't installed.

## Install (if missing)

```
# macOS
brew install qrencode

# Debian / Ubuntu
apt install qrencode

# Fedora / RHEL
dnf install qrencode
```

## Basic usage

```
qrencode -o output.png "Text or URL to encode"
```

Default: PNG, 256×256-ish, medium error correction.

## Common options

| Flag | What it does |
|---|---|
| `-o <file>` | Output path (use `-` for stdout). `.png` is default; `-t SVG` picks SVG. |
| `-t <TYPE>` | Output type: `PNG`, `SVG`, `EPS`, `ANSI`, `ANSI256`, `UTF8`, `ASCII`, `ASCIIi`. |
| `-s <size>` | Module pixel size (PNG): 1 for tiny, 10 for a 450×450 PNG. Default 3. |
| `-l <L/M/Q/H>` | Error correction: `L`=7%, `M`=15% (default), `Q`=25%, `H`=30%. Higher = bigger QR, more damage-tolerant. |
| `-m <N>` | Margin / quiet zone in modules. Default 4. Some scanners want 2 for tighter layouts. |
| `-v <N>` | Fixed version (1–40). Usually omit and let qrencode pick. |
| `-d <DPI>` | DPI for PNG output. Default 72. |

## Terminal output (no file)

Print a scannable QR code in the terminal:
```
qrencode -t ANSIUTF8 "https://example.com"          # UTF-8 half-blocks + ANSI colour (densest, most scannable)
qrencode -t UTF8 "https://example.com"              # plain UTF-8 block characters, no colour (larger)
qrencode -t ANSI "https://example.com"              # ANSI colour with space characters (no UTF-8 needed)
qrencode -t ANSI256UTF8 "https://example.com"       # 256-colour variant of ANSIUTF8 (qrencode ≥ 4.1)
```

`ANSIUTF8` is usually the smallest readable option in a modern terminal. `UTF8` is the most compatible when colour isn't available.

> **Version note:** `ANSI256UTF8` requires qrencode ≥ 4.1. Older distros (Debian stable, Ubuntu LTS releases shipping qrencode 4.0.x) reject it with `Invalid output type`. Run `qrencode --version` to check; if older than 4.1, fall back to `ANSIUTF8`.

## Common content patterns

**URL**
```
qrencode -o url.png "https://example.com/path"
```

**Wi-Fi credentials** (scanner auto-joins on most phones):
```
qrencode -o wifi.png "WIFI:T:WPA;S:NetworkName;P:password;;"
```

Fields: `T:` (WPA | WPA2 | WEP | nopass), `S:` SSID, `P:` password, `H:true;` if hidden.

**vCard** (business card):
```
qrencode -o vcard.png "BEGIN:VCARD
VERSION:3.0
FN:Sam Morgan
EMAIL:sam@example.com
TEL:+15551234567
END:VCARD"
```

**Geo location**:
```
qrencode -o geo.png "geo:37.7749,-122.4194"
```

**Calendar event**:
```
qrencode -o event.png "BEGIN:VEVENT
SUMMARY:Meeting
DTSTART:20260424T150000Z
DTEND:20260424T160000Z
END:VEVENT"
```

## Size + error correction trade-off

Higher `-l` level = more visible modules = physically bigger QR at the same pixel size, but more tolerance for dirt / damage. Common picks:
- `-l L` — digital only, smallest QR.
- `-l M` — default; fine for most on-screen use.
- `-l Q` — print, sticker, outdoor.
- `-l H` — decorative QRs with logo overlays (logo obscures 20-25%, so high ECC is mandatory).

## Rules

- QR codes have a data limit (~4,296 alphanumeric chars, less for binary). If the content is huge, shorten the URL first (see `shortened-url` skill) rather than trying to cram everything.
- Default output path — always specify `-o`; never assume cwd.
- Verify by scanning! Use `zbarimg` or a phone camera before giving the QR to the user.

## Failure modes

- `qrencode: command not found` → install (see above). This skill's `requirements:` auto-hides it if that's the case.
- `Failed to encode the input data: Input data too large` → content exceeds QR capacity. Shorten or split.
- Generated QR won't scan → check the output file isn't truncated (`-s` too small, losing detail), or bump `-l` up a level.
- Transparent-background need? Pass `--background=FFFFFF00` — or use SVG output and style downstream.
