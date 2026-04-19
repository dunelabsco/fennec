---
name: wsl
description: Run Linux commands from Windows via `wsl.exe`. Use when the user is on Windows and wants to run bash, a Linux CLI, or a Linux-only tool without switching out of their Windows shell.
always: false
requirements:
  - wsl
---

# wsl

Windows Subsystem for Linux lets Windows users run a real Linux userspace (Ubuntu, Debian, Alpine, etc.) from `cmd`, `PowerShell`, or Windows Terminal. `wsl.exe` is the launcher / orchestrator. `requirements: [wsl]` auto-hides the skill on macOS / Linux and on Windows without WSL installed.

## Quick check

```
wsl -l -v
```

Lists installed distros with state (`Running` / `Stopped`) and WSL version (1 or 2). Empty list means the user hasn't installed a distro yet.

## Install WSL (if missing)

Windows 10 build 19041+ / Windows 11:
```
wsl --install                              # installs WSL + default Ubuntu distro
wsl --install -d Debian                    # specific distro
wsl --list --online                         # show available distros before picking
```

After install, reboot. First launch of the distro prompts the user to set up a Linux user.

## Run a command (one-shot)

```
wsl -- ls -la ~/Documents                  # runs in the default distro
wsl -d Ubuntu -- uname -a                  # specific distro
wsl -d Ubuntu -u root -- apt update        # specific distro + user
```

`--` separates WSL args from the Linux command. Anything after `--` goes to Linux as-is.

## Run a shell interactively

```
wsl                                         # default distro, default user, interactive
wsl -d Ubuntu                               # named distro
wsl --cd "C:\Users\me\project"              # set starting directory (Windows path)
```

## Pipe between Windows and Linux

Paths translate across the `/mnt/` mount:

- Windows `C:\Users\me\file.txt` → Linux `/mnt/c/Users/me/file.txt`
- Linux `/home/me/file.txt` → Windows `\\wsl$\Ubuntu\home\me\file.txt` (use Explorer) or `wsl.localhost` on Win11

Pipe a command:
```
Get-ChildItem C:\logs\*.log | wsl -- xargs -I{} sh -c 'wc -l "{}"'
wsl -- grep -r "TODO" /mnt/c/projects/myrepo
```

## Run a Linux tool from PowerShell

Most Linux CLI tools work transparently:
```
wsl -- jq . C:\Users\me\data.json
wsl -- awk '{print $1}' /mnt/c/input.csv
wsl -- curl https://example.com
```

For long-running tools, wrap in tmux (see the `tmux` skill) inside the distro.

## Manage distros

```
wsl --list --online                        # what's available to install
wsl --install -d <name>                    # install one
wsl --set-default <name>                   # which distro wsl.exe uses by default
wsl --terminate <name>                     # stop a running distro (doesn't uninstall)
wsl --unregister <name>                    # DELETE a distro and its files (irreversible)
wsl --shutdown                             # stop the WSL kernel entirely
```

`--shutdown` is the "turn it off and on again" — useful after updating the kernel or when a distro misbehaves.

## WSL 1 vs WSL 2

- **WSL 2** (default on Win 11): full Linux kernel in a lightweight VM. Better Linux-native performance (e.g. Docker, compile), worse cross-filesystem performance (reading `/mnt/c/...` is slow).
- **WSL 1**: translation layer. Faster on `/mnt/c/...`, but not a full kernel (no systemd, no containers).

Convert:
```
wsl --set-version <distro> 2
```

## Export / import (backup a distro)

```
wsl --export Ubuntu C:\backups\ubuntu-2026-04-19.tar
wsl --import UbuntuCopy C:\WSL\UbuntuCopy C:\backups\ubuntu-2026-04-19.tar
```

Useful for moving a distro between drives, or snapshotting before a risky upgrade.

## Rules

- **`wsl --unregister <name>` is irreversible.** It deletes the distro's entire filesystem. Confirm with the user before running.
- `/mnt/c/...` paths work but are slow on WSL 2. For dev workflows, clone repos into the Linux filesystem (e.g. `~/code/`), not under `/mnt/c/`.
- Environment variables don't cross the boundary automatically. Use `WSLENV` to selectively share (e.g. `WSLENV=GITHUB_TOKEN:PATH/l`).
- Windows line endings (`\r\n`) in scripts break bash. If the user edits scripts in Notepad / a Windows editor, run `dos2unix` inside WSL first.
- File permissions on `/mnt/c/` don't reflect Linux semantics — don't `chmod` Windows files and expect it to stick.

## Failure modes

- `The Windows Subsystem for Linux instance has terminated.` → distro crashed or was shut down. `wsl --shutdown` then retry.
- `wsl: command not found` → WSL isn't installed or the user is on a non-Windows host (skill should be auto-hidden by `requirements:`).
- Sluggish cross-filesystem I/O → you're reading `/mnt/c/...` from WSL 2. Move the work into the Linux filesystem.
- Different results running the same command interactively vs via `wsl --` → interactive mode sources `~/.bashrc`; non-interactive doesn't. Export vars explicitly or use `wsl -- bash -lc '<cmd>'` to force a login shell.
- Systemd-dependent tools fail → older WSL versions don't have systemd. Enable via `/etc/wsl.conf` → `[boot]` → `systemd=true` then `wsl --shutdown`.
