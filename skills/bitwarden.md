---
name: bitwarden
description: Read items and secrets from the user's Bitwarden vault via the `bw` CLI. Use when the user needs to pull a password, API key, SSH key, or note without leaving the terminal.
always: false
requirements:
  - bw
---

# bitwarden

The Bitwarden CLI (`bw`) reads items from the user's vault (open-source password manager, cloud or self-hosted). Sibling skill to `1password` — pick whichever the user actually uses; users typically have one or the other.

`requirements: [bw]` auto-hides this skill when the binary isn't on PATH.

## Install (if missing)

```
# macOS
brew install bitwarden-cli

# Linux
npm install -g @bitwarden/cli           # or grab the standalone binary from github.com/bitwarden/clients

# Windows
winget install Bitwarden.CLI
```

## Configure server (if self-hosted)

Default points at `https://vault.bitwarden.com`. For self-hosted (Vaultwarden or official):
```
bw config server https://vault.yourdomain.com
```
Run once; persists.

## Login + unlock (one-time per session)

```
bw login                    # interactive: email + master password [+ 2FA if enabled]
# OR (for headless / automation):
bw login --apikey           # uses BW_CLIENTID + BW_CLIENTSECRET env vars
```

After login, the vault is **locked**. Unlock to start a session:

```
bw unlock                   # prints the session key to stdout
```

Export the session to the environment so subsequent `bw` commands can use it:
```
export BW_SESSION="$(bw unlock --raw)"
```

`--raw` gives you just the session key with no surrounding text — ready to assign to a variable. The session stays valid for the shell's lifetime; close the shell to lock again.

## Check status

```
bw status
```

Reports `unauthenticated` / `locked` / `unlocked` plus server URL and last-sync time.

## List items

```
bw list items                              # all items, returns JSON array
bw list items --search <term>              # filter by search
bw list items --folderid <folder_id>       # filter by folder
bw list items --collectionid <col_id>      # filter by collection (org vaults)
bw list folders
bw list collections
```

Output is JSON. Pipe to `jq` for extraction:
```
bw list items --search github | jq '.[] | {name, login: .login.username}'
```

## Read a specific field

**Password for an item** (the most common case):
```
bw get password "GitHub"                   # search term; errors if multiple match
bw get password <item_id>                  # exact id (safer)
```

**Other field types**:
```
bw get username <item_id>
bw get uri <item_id>
bw get totp <item_id>                      # 2FA code from stored TOTP seed
bw get notes <item_id>
```

**Full item as JSON**:
```
bw get item <item_id>                      # all fields, JSON
```

**Custom fields** (labels the user defined):
```
bw get item <item_id> | jq '.fields[] | select(.name == "API Token") | .value'
```

## Find an item's ID

```
bw list items --search "GitHub" | jq '.[] | {id, name}'
```

IDs are UUIDs. Once you have the ID, use it everywhere — it's stable and doesn't collide like names do.

## Sync with the server

The CLI caches locally. Pull latest changes from the server:
```
bw sync
```

Run this after the user says they added/changed something in the app.

## Rules

- **Never echo the raw secret into chat, logs, or memory.** Use it in a subprocess and move on.
- Prefer `$(bw get password <id>)` inline in a command so the secret never hits disk.
- `BW_SESSION` is a decryption key — don't log it, don't write it to disk. Scope it to the shell that needs it.
- When writing a secret to a file is unavoidable, `chmod 600` and clean up.
- Session expires when the shell exits or after the server-configured vault timeout; handle the re-unlock case by reading `bw status` first.
- Don't cache `bw` output across sessions; always re-read on demand. The user may have rotated the secret since.

## Failure modes

- `You are not logged in.` → `bw login` first.
- `Vault is locked.` → `bw unlock` and export `BW_SESSION`.
- `Session key is invalid.` → `BW_SESSION` is stale; unlock again.
- `More than one result was found.` on `bw get password "X"` → search term is ambiguous. List first, then use the UUID.
- `Not found.` on a specific id → the item was deleted / moved to trash. Run `bw sync`; if still missing, the user actually removed it.
- CLI won't start on macOS (unsigned binary) → install via Homebrew instead of npm, or allow in System Settings → Privacy & Security.

## Related

- `1password`: same pattern for users on 1Password. Users typically have one or the other.
- Both CLIs are design-equivalent: `signin`/`login` → unlock session → `read`/`get` field.
