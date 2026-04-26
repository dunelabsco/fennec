---
name: tmux
description: Run long-lived or interactive commands inside a detached tmux session. Use when a command exceeds the shell tool's timeout, or you need to send input across multiple turns.
always: false
requirements:
  - tmux
---

# tmux

The shell tool runs one command and returns when it finishes. Anything long-running, interactive, or requiring multiple inputs over time needs tmux. Fennec drives tmux through the shell tool; tmux holds state between calls.

## Session lifecycle

Create a detached session and run something inside it:

```
tmux new-session -d -s <name> '<command>'
```

- `-d` keeps it detached. Never omit this from a Fennec-driven session — attached sessions block the shell tool.
- `-s <name>` gives the session a stable id so later commands can reach it.
- Single-quote the command; escape internal quotes with care.

Confirm it exists:

```
tmux has-session -t <name> 2>/dev/null && echo running || echo gone
```

List all sessions:

```
tmux list-sessions
```

Kill when done:

```
tmux kill-session -t <name>
```

## Sending input

```
tmux send-keys -t <name> '<text>' Enter
```

- Pass `Enter` as a literal argument (not inside the quoted text) to send a return key. Other keys: `Up`, `Down`, `C-c`, `C-d`.
- Multiple `send-keys` calls are cumulative; the session's current prompt receives them in order.

## Reading output

Capture the pane contents without attaching:

```
tmux capture-pane -t <name> -p
```

- `-p` prints to stdout instead of saving to buffer.
- `-S -N` includes N lines of scrollback: `tmux capture-pane -t <name> -p -S -100`.

## Common patterns

**Run a build, poll later**

```
tmux new-session -d -s build 'cargo build --release 2>&1 | tee /tmp/build.log'
# later in another turn:
tmux capture-pane -t build -p -S -50
tmux kill-session -t build
```

**Interactive REPL the agent steers**

```
tmux new-session -d -s py 'python3 -i'
tmux send-keys -t py 'import numpy as np' Enter
tmux send-keys -t py 'np.arange(5)' Enter
tmux capture-pane -t py -p -S -10
```

## Rules

- Always `-d` on create. Attached sessions block the shell tool.
- Always give a deterministic `-s <name>`. Random names make cleanup impossible.
- Kill sessions when done. Orphaned tmux sessions consume memory and produce confusing results in future turns.
- For commands that finish in under a minute, plain shell is fine. Tmux is for things that don't.

## Failure modes

- `can't find session` → wrong `-t <name>`, or it was killed. Run `tmux list-sessions`.
- `duplicate session` on create → it already exists. Kill it or pick another name.
- Empty `capture-pane` output → command produced no stdout, or is writing to a file. Check the command.
- `tmux: server not found` on older setups → run a trivial `tmux new-session -d -s init 'true' && tmux kill-session -t init` once to start the server.
