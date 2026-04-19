---
name: youtube-audio
description: Download audio-only tracks from YouTube (or any yt-dlp-supported site) via yt-dlp + ffmpeg. Use when the user wants an mp3 / m4a for a podcast, music, lecture, or interview. Does NOT bypass copyright — only for content the user has rights to use.
always: false
requirements:
  - yt-dlp
  - ffmpeg
---

# youtube-audio

Sibling skill to `youtube-content` (which pulls transcripts + metadata). This one extracts the actual audio stream to a local file. Runs via the shell tool using `yt-dlp` + `ffmpeg`.

`requirements: [yt-dlp, ffmpeg]` — the skill auto-hides if either is missing. Both are commonly installed together.

## Install (if either is missing)

```
# macOS
brew install yt-dlp ffmpeg

# Linux
pipx install yt-dlp                 # or standalone yt-dlp binary
apt install ffmpeg                   # Debian / Ubuntu
```

## Basic extraction

Audio-only to MP3 (re-encoded by ffmpeg):
```
yt-dlp -x --audio-format mp3 -o "%(title)s.%(ext)s" "<URL>"
```

- `-x` / `--extract-audio` — pull audio, discard video.
- `--audio-format <fmt>` — `mp3`, `m4a`, `wav`, `flac`, `opus`, `aac`, `vorbis`. Picks the best available stream and re-encodes.
- `-o <template>` — output filename. See "Templates" below.

Audio to native format (no re-encode, fast, original codec):
```
yt-dlp -f bestaudio -o "%(title)s.%(ext)s" "<URL>"
```

Skip the re-encode; you get whatever YouTube delivers (usually m4a / webm). Faster and preserves quality, but may be a codec the user's player doesn't love.

## Quality control

```
yt-dlp -x --audio-format mp3 --audio-quality 0 "<URL>"    # best (VBR ~245 kbps)
yt-dlp -x --audio-format mp3 --audio-quality 5 "<URL>"    # medium
yt-dlp -x --audio-format mp3 --audio-quality 9 "<URL>"    # worst (small file)
```

`--audio-quality` is 0 (best) to 9 (worst) for MP3. For other codecs the scale varies; check `yt-dlp -h | grep audio-quality`.

## Output templates (`-o`)

yt-dlp expands fields:

| Template | Result |
|---|---|
| `"%(title)s.%(ext)s"` | `My Song.mp3` |
| `"%(uploader)s - %(title)s.%(ext)s"` | `Artist - My Song.mp3` |
| `"%(playlist_index)02d - %(title)s.%(ext)s"` | `01 - First.mp3`, `02 - Second.mp3` |
| `"%(upload_date)s_%(title)s.%(ext)s"` | `20260418_Song.mp3` |

Escape the quotes in shell; stick with single quotes around the `-o` template.

## Playlists

```
yt-dlp -x --audio-format mp3 \
       -o "%(playlist_index)02d - %(title)s.%(ext)s" \
       "<PLAYLIST_URL>"
```

Limit a playlist to N items:
```
--playlist-items 1-10
--playlist-items 1,3,5-8
```

Be polite — YouTube's rate limits kick in. Space bulk downloads: `--sleep-interval 2`.

## Embed metadata + cover art

```
yt-dlp -x --audio-format mp3 \
       --embed-metadata \
       --embed-thumbnail \
       --add-metadata \
       "<URL>"
```

- `--embed-metadata` — ID3 tags (title, uploader, etc.).
- `--embed-thumbnail` — cover art from the video thumbnail.

## Trim / split by chapters

YouTube videos often have chapter markers (podcast episodes, lectures). Extract as one file per chapter:
```
yt-dlp -x --audio-format mp3 --split-chapters \
       -o "%(chapter_number)02d - %(chapter)s.%(ext)s" \
       "<URL>"
```

Or extract a specific time range (requires ffmpeg):
```
yt-dlp -x --audio-format mp3 \
       --download-sections "*00:05:00-00:15:00" \
       "<URL>"
```

## Rules

- **Only download content the user has rights to use.** Personal archival of content the user uploaded or content under a permissive license is fine. Mass downloading of copyrighted material isn't.
- Quote URLs — `&`, `?` will be interpreted by the shell otherwise.
- For playlists or long videos, run under `tmux` (see the `tmux` skill) so the shell tool doesn't time out.
- Don't hammer YouTube. Add `--sleep-interval 2` between items in batch runs.
- Huge downloads (podcasts, multi-hour lectures) can be hundreds of MB. Confirm with the user before starting.

## Failure modes

- `ERROR: ... This video is unavailable` → private, age-restricted, deleted, region-blocked. Confirm with the user.
- `ERROR: ffprobe and ffmpeg not found` → `ffmpeg` isn't on PATH. Install it.
- `HTTP Error 429` → YouTube throttled the IP. Back off 10+ minutes; add `--sleep-interval 5` on retries.
- Silent `.m4a` instead of `.mp3` even with `--audio-format mp3` → re-encoding failed somewhere; check ffmpeg output.
- DRM-protected content → yt-dlp will refuse. That's a hard stop.

## Related

- `youtube-content`: transcripts and metadata (no audio files).
- `video-frames`: still frames from videos.
- `openai-whisper`: transcribe the downloaded audio with whisper if the video has no captions.
