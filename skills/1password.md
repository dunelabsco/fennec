---
name: 1password
description: Read items and secrets from the user's 1Password vault via the `op` CLI. Use when the user needs to pull a password, API key, SSH key, or note without leaving the terminal.
always: false
requirements:
  - op
---

# 1password

The 1Password CLI (`op`) lets scripts read items from the user's vault after a one-time authentication. No server-side API needed; everything happens locally.

`requirements: [op]` auto-hides this skill when the binary isn't on PATH.

## First-time setup

1. Install: https://developer.1password.com/docs/cli/get-started.
2. Sign in: `op signin` (first time prompts for account URL + master password; later `eval $(op signin)` refreshes the session in a shell).
3. Integrate with the desktop app (optional but nice) so biometrics unlock the CLI — enable it in 1Password desktop app settings → Developer → "Connect with 1Password CLI".

Once signed in, the CLI reads items the user is entitled to. Nothing fennec-side; all local.

## Core commands

**Check signed-in status**
```
op whoami
```

**List vaults the user can see**
```
op vault list
```

**List items in a vault**
```
op item list --vault <VaultName>
```

**Get one field from an item (the safest read path)**
```
op read "op://<Vault>/<Item>/<field>"
```

Example:
```
op read "op://Personal/GitHub/password"
op read "op://Personal/GitHub/credential"     # API tokens often live in this field
```

Output goes to stdout — a single line, the raw value. Never log it.

**Get full item as JSON**
```
op item get "<Item name>" --vault "<Vault>" --format json
```

**Get specific fields**
```
op item get "<Item>" --fields label=username,label=password --format json
```

## Load secrets into a one-shot command

```
op run --env-file=".env.template" -- my-command
```

`.env.template` contains placeholders:
```
GITHUB_TOKEN=op://Personal/GitHub/credential
```

`op run` replaces the references with real values, runs the command, and tears down the env on exit. Never writes plaintext to disk.

## Inject into a file (with care)

```
op inject -i template.yaml -o real.yaml
```

Resolves `op://...` references in the template. Output file contains plaintext secrets — treat accordingly (chmod 600, don't commit).

## Rules

- Never echo the raw secret into chat logs or memory. Use it in the command and move on.
- Prefer `op run` over `op read` when you can — the secret never leaves the subprocess.
- When writing the secret to a file is unavoidable, `chmod 600` and clean up.
- Don't cache `op` output in memory or on disk; re-read on demand. The user's session can be revoked at any time.
- Multi-account setups exist (`op signin` with account selector). If `op whoami` returns the wrong account, run `op signin --account <shorthand>`.

## Failure modes

- `[ERROR] 401: Authentication required` → session expired. `eval $(op signin)` or unlock via desktop app.
- `[ERROR] "<item>" isn't an item` → name mismatch. `op item list` to see what the user actually has.
- `[ERROR] more than one item matches` → multiple items share the name. Specify `--vault`, or use the item ID from `op item list`.
- Biometric prompts blocking the CLI → ensure the desktop app is running and "Connect with 1Password CLI" is enabled.
