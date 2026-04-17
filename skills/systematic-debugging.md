---
name: systematic-debugging
description: Debug from root cause, never from the first plausible patch. Applies whenever code, configuration, or observed behaviour is broken.
always: true
---

# systematic-debugging

When something is broken, do not guess at fixes. Follow the sequence below.

## The order

1. **Reproduce.** Get the bug to happen on command. If you cannot reproduce, you are fixing the wrong thing.
2. **Read the full error.** Entire stack trace, not the top frame. Error messages describe where the symptom surfaced, not always the cause. Copy the exact text into your working notes.
3. **Narrow to the smallest failing input.** Strip away anything that does not change the bug. Keep stripping until the remaining code is the bug.
4. **Hypothesis, then test.** Write down what you think is wrong in one sentence. Design one change that would confirm or deny it. Run it. If the test cannot distinguish hypothesis from reality, the test is wrong.
5. **Fix the cause, not the symptom.** A fix that suppresses the error without explaining it is not a fix. It is a trap for the next person.
6. **Add a regression guard.** A failing test that would have caught this bug, committed alongside the fix.

## Hard rules

- After three failed fixes on the same hypothesis, stop. The hypothesis is wrong. Re-read the error from step 2.
- Never say "it works now" without re-running the exact reproduction from step 1.
- Silent `try/except`, `catch`, `unwrap_or`, or `|| true` around the error path is almost always wrong while debugging. Let failures surface.
- Reverting is a valid answer. If the bug arrived with a recent commit and nothing urgent depends on that commit, revert and fix on a branch.

## Signals you are off-track

- You are reading code you have not connected to the bug.
- You are editing configuration files hoping something changes.
- You are running the same command with minor variations waiting for it to pass.
- You are asking the user to retry without explaining what you changed.

When any of these happen, stop and return to step 1.
