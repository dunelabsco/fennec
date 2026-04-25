---
name: image-ops
description: Resize, crop, rotate, convert format, composite, and inspect images via ImageMagick. Use when the user wants to manipulate image files — thumbnail generation, format conversion, batch resize, strip EXIF, etc.
always: false
requirements:
  - magick
---

# image-ops

ImageMagick is the standard CLI for image manipulation. In ImageMagick 7+ the canonical binary is `magick`; legacy names like `convert`, `identify`, `mogrify`, `composite`, `montage`, `compare` still work but the project treats `magick` as the front door.

`requirements: [magick]` auto-hides the skill when ImageMagick 7 isn't installed. On an older IM6 install, the `magick` binary isn't present — the user should upgrade.

## Install (if missing)

```
# macOS
brew install imagemagick

# Debian / Ubuntu
apt install imagemagick

# Fedora / RHEL
dnf install ImageMagick
```

Verify: `magick -version` should print "Version: ImageMagick 7.x.x".

## Inspect an image (metadata, dimensions)

```
magick identify input.jpg                         # one-line summary
magick identify -verbose input.jpg                # full EXIF + ICC + layers
magick identify -format "%wx%h %m %Q" input.jpg   # just width x height + format + quality
```

## Format conversion

```
magick input.png output.jpg                       # PNG → JPEG
magick input.jpg -quality 85 output.jpg           # re-encode at quality 85
magick input.jpg output.webp                      # JPEG → WebP
magick input.heic output.jpg                      # iPhone HEIC → JPEG (needs libheif)
magick input.tiff -alpha off output.jpg           # drop alpha channel for formats that don't support it
```

## Resize

```
magick input.jpg -resize 800x600 output.jpg        # "fit inside" 800x600, keep aspect
magick input.jpg -resize 800x600^ output.jpg       # "fill" 800x600, keep aspect (may overflow one axis)
magick input.jpg -resize 800x600! output.jpg       # stretch to exactly 800x600 (distorts)
magick input.jpg -resize 50% output.jpg            # half size
magick input.jpg -resize 800x output.jpg           # width 800, height auto
magick input.jpg -thumbnail 200x200^ output.jpg    # resize + strip metadata (smallest output)
```

## Crop

```
magick input.jpg -crop 400x300+100+50 output.jpg   # 400x300 rectangle, offset +100,+50 from top-left
magick input.jpg -gravity center -crop 400x400+0+0 output.jpg   # centered 400x400 square
```

`+repage` right after a crop discards the original canvas metadata; usually what you want.

## Rotate, flip

```
magick input.jpg -rotate 90 output.jpg             # rotate 90° clockwise
magick input.jpg -rotate -90 output.jpg            # 90° counter-clockwise
magick input.jpg -flip output.jpg                  # top-bottom mirror
magick input.jpg -flop output.jpg                  # left-right mirror
magick input.jpg -auto-orient output.jpg           # respect EXIF orientation tag
```

`-auto-orient` is worth running on anything from a phone — most cameras write orientation as EXIF metadata rather than rotating the pixels.

## Strip metadata (privacy)

```
magick input.jpg -strip output.jpg                 # removes EXIF, ICC, thumbnail, all profiles
```

Photos from phones carry location + device data; `-strip` is the standard cleanup before sharing publicly.

> **Warning:** `-strip` also drops the **ICC colour profile**. Wide-gamut sources (iPhone DisplayP3, Adobe RGB DSLRs) viewed as untagged sRGB look noticeably desaturated or shifted. To drop EXIF/GPS but keep colour, exclude the ICC profile from the strip:
> ```
> magick input.jpg -strip +profile '!icc,*' output.jpg   # strip everything except ICC
> ```
> See "Preserve colour profiles" under Rules.

## Composite / watermark

```
magick input.jpg watermark.png -gravity southeast -composite output.jpg
magick input.jpg logo.png -geometry +20+20 -composite output.jpg
```

`-gravity` picks the alignment (`center`, `north`, `southeast`, etc.); `-geometry +X+Y` gives pixel offset.

## Batch (multi-file) via mogrify

`magick` writes one output at a time. For batches, `mogrify` modifies files in place:

```
mogrify -resize 1600x -quality 85 *.jpg
```

**Danger:** `mogrify` overwrites originals. Copy to a working dir first, or pass `-path dest/` so outputs land elsewhere.

Or loop with `magick`:
```
for f in *.jpg; do magick "$f" -resize 1600x "out/$f"; done
```

## Rules

- **Always write to an explicit output path.** ImageMagick lets you overwrite silently.
- For very large images (e.g. 100 MP scans), use `-limit memory 1GiB -limit map 2GiB` to avoid OOM.
- Preserve colour profiles for anything destined for print: don't strip ICC unless you're sure.
- For thumbnails used as OG images or social previews, `-thumbnail` (not `-resize`) is the right op — it strips unnecessary chunks and produces smaller files.

## Failure modes

- `convert: attempt to perform an operation not allowed by the security policy 'PDF'` → ImageMagick ships with a restrictive `policy.xml` that blocks PDF / MVG / PS by default (CVE mitigations). Edit `/etc/ImageMagick-7/policy.xml` to re-enable only if you trust the input.
- `delegate failed` on `.heic` or `.webp` → missing codec in your IM build. Install `libheif` / `libwebp` and rebuild ImageMagick or use `heif-convert` separately.
- Weird colour shift on JPEG → missing or wrong ICC profile. `magick input.jpg -strip -profile sRGB.icc output.jpg` forces sRGB.
- Dimensions wrong after rotate → the image had EXIF orientation; run `-auto-orient` before the rotate.
