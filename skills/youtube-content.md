---
name: youtube-content
description: Download YouTube transcripts / subtitles and basic video metadata via yt-dlp. Use when the user asks to summarise, quote, or analyse a YouTube video. Requires the yt-dlp CLI.
always: false
requirements:
  - yt-dlp
---

# youtube-content

No API key needed. Runs through the `yt-dlp` CLI via the shell tool. `requirements: [yt-dlp]` auto-hides this skill when the binary isn't installed.

## One-time install (if `yt-dlp` is missing)

```
# macOS
brew install yt-dlp

# Linux with pipx
pipx install yt-dlp

# Or the standalone binary
curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp \
    -o ~/.local/bin/yt-dlp && chmod +x ~/.local/bin/yt-dlp
```

## Transcripts

**Manual (human-written) subtitles first, fall back to auto-generated:**
```
yt-dlp --write-sub --write-auto-sub --sub-lang en --skip-download \
       -o "%(id)s.%(ext)s" "<URL>"
```

- Output: a `.vtt` file named after the video ID (e.g. `dQw4w9WgXcQ.en.vtt`).
- `--sub-lang en` — English; change for other languages (`de`, `es`, `ja`, etc.).
- `--sub-format vtt` — VTT is default and easiest to read.
- `--skip-download` — we want only subtitles, not the video bytes.

**Auto-generated only** (for videos without manual captions):
```
yt-dlp --write-auto-sub --sub-lang en --skip-download -o "%(id)s.%(ext)s" "<URL>"
```

**Reading the .vtt**

`.vtt` files start with a `WEBVTT` header and contain cue timings. For plain text, strip those with awk / sed, or read the file and filter the non-timestamp lines in code.

## Video metadata (no download)

```
yt-dlp --dump-json --skip-download "<URL>"
```

Returns a single JSON blob with `title`, `uploader`, `duration`, `description`, `view_count`, `upload_date`, `thumbnail`, and much more. Parse with `jq`.

Lightweight alternative when you only want the title:
```
yt-dlp --get-title "<URL>"
```

## Playlists

```
yt-dlp --flat-playlist --dump-json "<PLAYLIST_URL>" | jq '{id, title}'
```

One JSON object per video. Don't mass-download transcripts without asking the user — each video is a separate subtitle fetch and counts against YouTube's rate limit.

## Channel recent videos

```
yt-dlp --flat-playlist --dump-single-json --playlist-end 10 \
       "<CHANNEL_URL>/videos" | jq '.entries[] | {id, title}'
```

## Rules

- Always `--skip-download` when you only want text or metadata. Downloading the video is slow and wastes bandwidth.
- Clean up `.vtt` files after use unless the user asked to keep them.
- Quote the URL — YouTube URLs contain `?` and `&` that the shell will otherwise interpret.
- Rate-limit yourself: YouTube throttles IPs that fetch rapidly. Serialise batch operations; add a small sleep between videos.

## Failure modes

- `ERROR: ... Video unavailable` → video is private, age-restricted, deleted, or geo-blocked. Confirm with the user.
- `WARNING: There are no automatic captions for requested language` → try another `--sub-lang`, or combine `--write-auto-sub` with `--convert-subs srt` and `--sub-lang en` to trigger YouTube's auto-translation (works for many but not all videos).
- `HTTP Error 429 Too Many Requests` → throttled. Back off for a few minutes; don't retry immediately.
- Empty `.vtt` file → subtitles exist but contain only music markers or chapter labels. There's no text to work with.
