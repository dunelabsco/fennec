---
name: openai-whisper
description: Transcribe audio to text — via the OpenAI Whisper API (cloud, paid) or the open-source whisper CLI (local, free). Use for voice memos, meeting recordings, podcasts, interviews.
always: false
---

# openai-whisper

Two paths. Detect availability and prefer local when present (free, offline, private).

## Path 1 — Local `whisper` CLI (free)

One-time install:
```
pipx install openai-whisper
# or: pip install -U openai-whisper
```

Also needs `ffmpeg` for anything that isn't raw WAV:
```
# macOS
brew install ffmpeg
# Debian/Ubuntu
apt install ffmpeg
```

Run:
```
whisper input.mp3 --model base --output_format txt --language en
```

Models (pick by speed / accuracy trade-off):
- `tiny` — fastest, least accurate.
- `base` — good default.
- `small` / `medium` — higher accuracy, slower.
- `large` — best accuracy, slowest.
- `turbo` — current large-v3-derived fast variant.

Options:
- `--output_format` — `txt`, `vtt`, `srt`, `json`, `tsv`.
- `--language` — ISO-639-1 (omit for auto-detect).
- `--task translate` — transcribe AND translate into English.

Output files appear next to the input, same basename: `input.mp3 --output_format txt` → `input.txt`.

## Path 2 — OpenAI Whisper API (paid, roughly $0.006/min — verify current pricing before long runs)

Needs `OPENAI_API_KEY`.

```
curl https://api.openai.com/v1/audio/transcriptions \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H "Content-Type: multipart/form-data" \
  -F file="@./input.mp3" \
  -F model="whisper-1"
```

Models:
- `whisper-1` — stable, cheapest.
- `gpt-4o-transcribe` — higher accuracy, pricier.
- `gpt-4o-mini-transcribe` — cost-tuned.
- `gpt-4o-transcribe-diarize` — speaker labels.

Accepted formats: `flac`, `mp3`, `mp4`, `mpeg`, `mpga`, `m4a`, `ogg`, `wav`, `webm`. Maximum file size is 25 MB — split larger files with `ffmpeg` before upload.

Optional params:
- `response_format` — `json` (default), `text`, `srt`, `verbose_json`, `vtt`.
- `timestamp_granularities[]` — `segment`, `word`. Requires `response_format=verbose_json`.
- `language` — ISO-639-1 code; omit for auto-detect.
- `prompt` — bias the transcription toward names / domain terms.

## Picking the path at runtime

```
if command -v whisper      -> local
elif $OPENAI_API_KEY set   -> API
else                       -> ask the user to install whisper OR set OPENAI_API_KEY
```

Always tell the user which path you used — the transcripts can differ, and the user may prefer one over the other.

## Long audio

- **Local:** whisper handles multi-hour files but is slow on CPU. Run under `tmux` (see the tmux skill) so the shell tool doesn't time out; poll the output file.
- **API:** 25 MB cap. Split first:
  ```
  ffmpeg -i long.mp3 -f segment -segment_time 600 -c copy chunk-%03d.mp3
  ```
  Transcribe each chunk, concatenate in order.

## Rules

- Confirm with the user before running long audio through the paid API — bill depends on duration.
- Save transcripts to disk alongside the audio, not only to the terminal. Long transcripts scroll out of view.
- For sensitive content (private meetings, medical, legal) default to local whisper. Ask before sending to the API.

## Failure modes

- `whisper: command not found` → install the CLI (see Path 1).
- `ffmpeg: not found` (local path) → install ffmpeg.
- `HTTP 401` (API path) → bad `OPENAI_API_KEY`.
- `HTTP 413 Payload Too Large` → file > 25 MB; split it.
- Gibberish transcript → wrong `--language` hint, or the audio is too degraded. Try `--model large` if you were on `tiny`.
