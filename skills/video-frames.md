---
name: video-frames
description: Extract frames, thumbnails, or clips from video files via ffmpeg. Use when the user wants a still image, a preview, or a segment of a video.
always: false
requirements:
  - ffmpeg
---

# video-frames

ffmpeg is the standard tool for video manipulation. This skill covers frame extraction, thumbnail generation, trimming, and format conversion — the common agent-useful operations. `requirements: [ffmpeg]` auto-hides the skill when ffmpeg isn't on PATH.

## Install (if missing)

```
# macOS
brew install ffmpeg

# Debian / Ubuntu
apt install ffmpeg

# Fedora / RHEL
dnf install ffmpeg   # via RPM Fusion
```

## Frame extraction

**One frame per second** (common for video summarisation):
```
ffmpeg -i input.mp4 -vf "fps=1" frame-%04d.jpg
```

**One frame every N seconds**:
```
ffmpeg -i input.mp4 -vf "fps=1/5" frame-%04d.jpg     # every 5 seconds
```

**Scene-change detection** (pull a frame only when the scene cuts):
```
ffmpeg -i input.mp4 -vf "select='gt(scene,0.3)',showinfo" -vsync vfr scene-%04d.jpg
```

Scene threshold 0.1–0.5; lower = more frames.

**First frame only**:
```
ffmpeg -i input.mp4 -vf "select=eq(n\,0)" -vframes 1 first.jpg
```

## Thumbnail at a specific time

```
ffmpeg -ss 00:00:30 -i input.mp4 -vframes 1 -q:v 2 thumb.jpg
```

`-ss` before `-i` is fast (seeks without decoding). `-q:v 2` is high-quality JPEG (1 = best, 31 = worst).

For exact-frame accuracy (slower but precise), put `-ss` after `-i`:
```
ffmpeg -i input.mp4 -ss 00:00:30 -vframes 1 thumb.jpg
```

## Clip / trim

**Trim without re-encoding** (fast, stream copy):
```
ffmpeg -ss 00:01:00 -to 00:02:30 -i input.mp4 -c copy clip.mp4
```

**Trim with re-encoding** (precise boundaries, slower):
```
ffmpeg -i input.mp4 -ss 00:01:00 -to 00:02:30 -c:v libx264 -c:a aac clip.mp4
```

## Metadata probe (no extraction)

```
ffprobe -v error -print_format json -show_format -show_streams input.mp4
```

Returns duration, bitrate, codecs, resolution, frame rate. Useful before expensive operations.

Quick one-liner for just duration:
```
ffprobe -v error -show_entries format=duration -of csv=p=0 input.mp4
```

## Format conversion

**To GIF** (small, low frame rate):
```
ffmpeg -i input.mp4 -vf "fps=10,scale=480:-1:flags=lanczos" -loop 0 out.gif
```

**To WebP animated** (smaller file than GIF at same quality):
```
ffmpeg -i input.mp4 -vf "fps=15,scale=480:-1" -loop 0 out.webp
```

**Re-encode to smaller MP4**:
```
ffmpeg -i input.mp4 -c:v libx264 -crf 28 -preset slow -c:a aac -b:a 96k small.mp4
```

`-crf 23` is "visually lossless", `28` is noticeably compressed, `32` is very compressed.

## Rules

- Always write to an explicit output path. Don't let ffmpeg guess.
- ffmpeg refuses to overwrite by default — pass `-y` to force, or delete the target first. Don't add `-y` blindly if the user might care about the old file.
- For long videos, wrap the command in tmux (see the `tmux` skill) so the shell tool doesn't time out.
- Huge frame dumps (1 fps × 1-hour video = 3600 frames) eat disk and context. Ask the user before extracting more than ~60 frames at once.

## Failure modes

- `Unknown encoder 'libx264'` → ffmpeg build lacks that codec. Use the default `h264` encoder or install a full ffmpeg build.
- `No such filter: 'fps'` → very old ffmpeg. Upgrade.
- `Invalid data found when processing input` → input file is corrupt, truncated, or not actually a video.
- Zero-byte outputs → the filter chain produced no output (e.g. scene threshold too high). Loosen the filter.
- Garbled output on some players → `-pix_fmt yuv420p` after `-c:v libx264` fixes most compatibility issues.
