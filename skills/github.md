---
name: github
description: Use the gh CLI for GitHub work — PRs, issues, workflow runs, releases, raw API. Invoke through the shell tool.
always: false
requirements:
  - gh
---

# github

For anything that touches github.com — pull requests, issues, Actions, releases, raw API queries — prefer the `gh` CLI over scripting `git` + `curl` by hand. Run it through the shell tool.

## Authentication

Before any command, confirm the user is logged in:

```
gh auth status
```

If they are not logged in, ask them to run `gh auth login` themselves. `gh auth login` needs interactive browser or token input and must not be automated silently.

## Scope

`gh` is for GitHub-specific operations. Use plain `git` for: cloning, committing, pushing, diffing locally. Do not reach for `gh` where `git` already does the job — it is slower and authenticates unnecessarily.

## Pull requests

Open a PR from the current branch:

```
gh pr create --title "..." --body "..."
```

Always pass `--title` and `--body` explicitly. `gh pr create` without them drops into an interactive editor and blocks the agent.

`--base` defaults to the repo's default branch. Pass `--base <branch>` only when targeting a non-default branch.

Inspect a PR:

```
gh pr view <number> --json state,title,author,mergeable,statusCheckRollup
```

Check CI:

```
gh pr checks <number>
```

Review or merge:

```
gh pr review <number> --approve --body "..."
gh pr merge <number> --squash --delete-branch
```

## Issues

```
gh issue list --limit 20 --state open
gh issue view <number>
gh issue create --title "..." --body "..."
gh issue comment <number> --body "..."
```

## Workflow runs (Actions)

```
gh run list --limit 10
gh run view <run-id> --log-failed
gh run rerun <run-id>
```

## Raw API

For endpoints not covered by a subcommand:

```
gh api repos/OWNER/REPO/commits/HEAD/check-runs
```

Prefer this over cobbling together `curl` with a token — `gh` handles auth and rate limits for you.

## Which repo `gh` targets

Inside a clone, `gh` uses the clone's `origin` remote. If multiple remotes are configured it may prompt to pick one. Outside a clone, every command needs `--repo owner/name` or it errors. To avoid ambiguity, pass `--repo owner/name` explicitly when the target is not the current clone.

## Rate limits

```
gh api rate_limit
```

shows remaining budget. Bulk queries should paginate (`--paginate`) and cache results, not re-query per item.

## Common failure modes

- `gh: not authenticated` → user runs `gh auth login`.
- `Resource not accessible by integration` → token scope is too narrow; user extends scopes via `gh auth refresh -s <scope>`.
- `HTTP 409 merge conflict` → do not force anything; report the conflict and let the user resolve.
- `gh: command not found` → the skill should not have loaded; if it did, the binary is missing from PATH.
