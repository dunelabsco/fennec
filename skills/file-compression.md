---
name: file-compression
description: Compress and decompress files and directories via zip / tar / gzip / 7z through the shell tool. Use when the user wants to bundle files for sharing, extract an archive, or pick a format.
always: false
---

# file-compression

Covers the common archive formats on Unix-like systems. All via the shell tool. No new tool to install for the basics (`tar`, `zip`, `gzip`, `bzip2`, `xz` ship with most distros); 7-Zip needs a package.

## Picking the format

| Format | Strengths | Weaknesses | Install |
|---|---|---|---|
| `zip` | Cross-platform (Windows double-click), per-file compression | Weaker ratio than modern codecs | Built-in on macOS/Linux |
| `tar.gz` / `.tgz` | Standard on Unix, preserves permissions | No encryption, not Windows-friendly | Built-in |
| `tar.xz` | Best ratio for text / source code | Slower to compress | Built-in on most systems |
| `tar.bz2` | Better ratio than gzip, older than xz | Slower than gzip, worse than xz | Built-in |
| `7z` | Best compression, encryption, cross-platform | Needs `p7zip` or `7z` binary | `brew install p7zip` / `apt install p7zip-full` |

General rule: **tar.gz** for Unix-to-Unix sharing, **zip** for sharing with Windows users, **7z** for best ratio or encryption.

## Create archives

**zip**
```
zip -r output.zip file_or_dir [more files...]
zip -r output.zip dir -x '*.log' '*.tmp'           # exclude patterns
zip -er output.zip dir                             # encrypt with a password prompt
zip -r -9 output.zip dir                           # max compression
```

**tar.gz**
```
tar -czf output.tar.gz file_or_dir
tar -czf output.tar.gz --exclude='*.log' dir
```

**tar.xz** (best ratio for text / code):
```
tar -cJf output.tar.xz dir
```

**tar.bz2**:
```
tar -cjf output.tar.bz2 dir
```

**7z** (best ratio overall, supports AES-256 encryption):
```
7z a output.7z file_or_dir
7z a -mx=9 output.7z dir                           # max compression
7z a -p output.7z dir                              # password-protected (prompt)
7z a -p'password' -mhe=on output.7z dir            # encrypt filenames too
```

## Extract archives

```
unzip input.zip -d target/

tar -xf input.tar         # auto-detects gzip/xz/bzip2 on modern GNU/BSD tar
tar -xzf input.tar.gz -C target/
tar -xJf input.tar.xz -C target/
tar -xjf input.tar.bz2 -C target/

7z x input.7z -otarget/
```

`-C <dir>` / `-o<dir>` picks the destination. Always specify — don't extract into cwd.

## Inspect without extracting

```
unzip -l input.zip                # list zip contents
tar -tzf input.tar.gz             # list tar.gz contents
tar -tJf input.tar.xz             # list tar.xz contents
7z l input.7z                     # list 7z contents
```

## Single-file compression (not an archive, just one file)

```
gzip file                         # produces file.gz, removes file
gzip -k file                      # keep original too
gunzip file.gz                    # decompress
xz file                           # file.xz
bzip2 file                        # file.bz2
```

For a one-file-off compression, these are simpler than tar. For multiple files, tar into a single archive first.

## Encryption

- **zip** `-e` / `-er` — uses legacy ZipCrypto by default (weak). Not safe for sensitive data. Some modern tools support AES via `--encrypt`.
- **7z** `-p` with `-mhe=on` — AES-256, also encrypts filenames. Strong.
- **tar** — no built-in encryption. Pipe through `gpg`:
  ```
  tar -cJf - dir | gpg -c -o dir.tar.xz.gpg
  # decrypt:
  gpg -d dir.tar.xz.gpg | tar -xJf -
  ```

Use 7z or gpg-over-tar for anything confidential. Legacy zip encryption is broken.

## Rules

- Always specify `-C <dir>` (tar) / `-d <dir>` (unzip) / `-o<dir>` (7z) when extracting. Extracting into cwd silently dumps files where they shouldn't go.
- For unknown archives, inspect first (`unzip -l`, `tar -tzf`, `7z l`) before extracting.
- Sensitive data → 7z with `-mhe=on` or gpg. Not plain `zip -e`.
- Archive bombs (e.g. huge `.zip` that expands to TB) are a real risk. Check listed size before extracting files from unknown sources:
  ```
  unzip -l archive.zip | tail -1          # shows total uncompressed size
  ```
- Don't delete the original until you've verified the archive extracts cleanly: `tar -tzf archive.tar.gz >/dev/null && rm -rf dir`.

## Failure modes

- `tar: Cannot open: No such file or directory` → path wrong, often due to `cd` changing state.
- `End-of-central-directory signature not found` (unzip) → file is truncated or not actually a zip.
- `7z: command not found` → install `p7zip-full` (Linux) or `p7zip` (macOS).
- `bzip2: data integrity error when decompressing` → file is corrupt. Re-download.
- Weirdly tiny extracted output → archive contained a symlink / sparse file you didn't expect. Use `tar -tvf` to inspect.
