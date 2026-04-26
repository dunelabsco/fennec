---
name: audio-edit
description: Trim, merge, convert, normalize, and extract from audio files via ffmpeg (and sox for specific needs). Use when the user wants to edit an audio track without opening a DAW.
always: false
requirements:
  - ffmpeg
---

# audio-edit

ffmpeg handles almost every common audio edit. `sox` is a specialized alternative for effects-heavy work (de-noise, reverb, silence detection). This skill focuses on ffmpeg since it's the one you've almost certainly already installed (via `video-frames` or `youtube-audio`).

## Format conversion

```
ffmpeg -i input.wav output.mp3                     # lossy, default VBR
ffmpeg -i input.mp3 -c:a libmp3lame -b:a 192k output.mp3   # fixed 192 kbps
ffmpeg -i input.wav -c:a libopus -b:a 96k output.opus     # opus (smaller, modern)
ffmpeg -i input.mp3 output.flac                    # lossy → lossless (no quality gain, just size)
ffmpeg -i input.m4a output.wav                     # m4a → uncompressed WAV
```

`-c:a copy` re-packages the audio stream without re-encoding — fast, preserves quality, but only if source and destination containers accept the same codec.

## Trim (cut a segment)

**Stream copy (fast, re-encoding avoided):**
```
ffmpeg -ss 00:01:00 -to 00:02:30 -i input.mp3 -c copy clip.mp3
```
`-ss` before `-i` seeks fast (keyframe-aligned). May be slightly imprecise on boundaries.

**Precise boundaries (re-encode):**
```
ffmpeg -i input.mp3 -ss 00:01:00 -to 00:02:30 -c:a libmp3lame -b:a 192k clip.mp3
```
`-ss` after `-i` is sample-accurate but slower.

Use `-t <duration>` instead of `-to` if you prefer duration-based trimming. `-t` accepts raw seconds or `HH:MM:SS`:
```
ffmpeg -ss 00:01:00 -i input.mp3 -t 90 -c copy clip.mp3           # 90 seconds from 1:00
ffmpeg -ss 00:01:00 -i input.mp3 -t 00:01:30 -c copy clip.mp3     # equivalent
```

Each `HH:MM:SS` component should stay in its normal range (seconds 0–59); use raw seconds for anything longer than 60s when in doubt.

## Concatenate (merge)

For **same-codec files** (all mp3, all wav, etc.) — use the concat demuxer:

```
# Create a list file:
cat > list.txt <<EOF
file 'part1.mp3'
file 'part2.mp3'
file 'part3.mp3'
EOF

ffmpeg -f concat -safe 0 -i list.txt -c copy merged.mp3
```

For **mixed-codec files**, re-encode through the concat filter:
```
ffmpeg -i a.wav -i b.mp3 -i c.m4a -filter_complex '[0:a][1:a][2:a]concat=n=3:v=0:a=1[out]' -map '[out]' merged.mp3
```

## Normalize volume

**EBU R128 loudness normalization** (broadcast-standard, what podcasts use):
```
ffmpeg -i input.mp3 -af 'loudnorm=I=-16:TP=-1.5:LRA=11' normalized.mp3
```
Targets -16 LUFS (podcast norm). Use `-I=-23` for EBU broadcast, `-I=-14` for Spotify.

**Peak normalization** (simple, louder but less consistent):
```
ffmpeg -i input.mp3 -af 'dynaudnorm' normalized.mp3
```

## Extract audio from video

```
ffmpeg -i input.mp4 -vn -c:a copy audio.m4a        # stream copy (fastest, original codec)
ffmpeg -i input.mp4 -vn -c:a libmp3lame -b:a 192k audio.mp3
```

`-vn` means "no video" — drops the video stream.

## Speed / pitch

**Change tempo without pitch shift:**
```
ffmpeg -i input.mp3 -filter:a 'atempo=1.25' faster.mp3      # 25% faster
ffmpeg -i input.mp3 -filter:a 'atempo=0.85' slower.mp3      # 15% slower
```
`atempo` accepts 0.5–100.0; chain filters for larger ratios: `atempo=2.0,atempo=2.0` = 4x.

**Change pitch AND tempo** (the "chipmunk" effect):
```
ffmpeg -i input.mp3 -filter:a 'asetrate=44100*1.25' chipmunk.mp3
```

## Silence trim

Remove leading / trailing silence:
```
ffmpeg -i input.mp3 -af 'silenceremove=start_periods=1:start_duration=0.3:start_threshold=-40dB:stop_periods=1:stop_duration=0.3:stop_threshold=-40dB' trimmed.mp3
```

## Split by silence (one file per track)

```
ffmpeg -i long.mp3 -af 'silencedetect=n=-40dB:d=1' -f null - 2>&1 | grep 'silence_end'
```
Shows silence positions; use them as manual trim points.

## Probe metadata (no extraction)

```
ffprobe -v error -print_format json -show_format -show_streams input.mp3
```

Quick duration:
```
ffprobe -v error -show_entries format=duration -of csv=p=0 input.mp3
```

## sox alternative for specific effects

Install: `brew install sox` / `apt install sox`. Good at:
- Noise reduction (`noiseprof` + `noisered`)
- Reverb (`reverb`)
- Silence detection with better knobs than ffmpeg
- Sample-rate conversion with higher-quality resamplers

```
sox input.wav output.mp3                              # format conversion
sox input.wav output.wav reverb 50                    # add reverb
sox input.wav cleaned.wav noiseprof noise.prof && \
  sox input.wav cleaned.wav noisered noise.prof 0.3   # de-noise
```

sox can't read MP3 out of the box on some distros — convert via ffmpeg to WAV first.

## Rules

- Long operations under `tmux` (see the `tmux` skill) so the shell tool doesn't time out.
- Always write to an explicit output path. ffmpeg refuses to overwrite without `-y`; don't pass `-y` blindly.
- Loudness normalization is two-pass for best results — ffmpeg's `loudnorm` runs single-pass by default. For release-grade audio, do a measurement pass first and feed the targets into a second pass.
- Lossy → lossy re-encodes degrade quality. If source is MP3 and destination is also MP3, use `-c:a copy` when possible.

## Failure modes

- `Unknown encoder 'libmp3lame'` → ffmpeg was built without LAME. Install the full build (Homebrew ships everything; distro packages may be minimal). Try `-c:a aac` or `-c:a libopus` as alternatives.
- Distorted output after `atempo` > 2.0 → chain multiple filters (`atempo=2.0,atempo=2.0`) instead of one big jump.
- Clipping after `loudnorm` → lower the `TP` (true-peak) ceiling further (try `-1.0`).
- Empty output file → no audio stream in source, or the filter chain produced no output. Check stderr.
