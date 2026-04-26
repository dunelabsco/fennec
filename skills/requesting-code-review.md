---
name: requesting-code-review
description: Make a pull request easy to review quickly. Use when opening a PR or asking a human for a read.
always: false
---

# requesting-code-review

Reviewers decide in the first 30 seconds whether to read closely or skim. Make the first paragraph work for them.

## The five-line template

```
Summary: one sentence, what the change does.
Why: the motivating bug / feature / constraint. Link the ticket if tracked.
Scope: files or components touched; what is explicitly NOT touched.
Risk: what could go wrong; the parts you are least sure about.
Test plan: what you ran; what you did not.
```

Five lines, not five paragraphs. If a line doesn't apply, say so — e.g. "Scope: single-file docs fix, no code paths changed."

## What to ask for

- **Fresh eyes on X** — a specific concern to examine.
- **Architecture sanity check** — when the change introduces a new pattern worth validating before expanding it.
- **Shippable, or blocker?** — clarifies the reviewer's decision.
- **I am unsure about Y** — reviewers love being pointed at the hard part.

"Review" in the abstract invites vague "looks good" responses. Ask for something specific.

## What to include in the PR body

- Brief reproduction if the PR fixes a bug (command, expected vs actual output).
- Screenshots or before/after for UI and formatting changes.
- Migration notes for anything touching deployment, config, or data.
- Call out anything that LOOKS wrong but is intentional — saves a review round.

## Responding to review comments

- Address every thread, even if the answer is "intentional — here is why".
- Distinguish `Done.` (fixed in a follow-up push) from `Will do in a separate PR because X.` from `Disagree because Y.`
- When a reviewer is wrong, explain the reasoning instead of closing the thread. They might be wrong on this line but right on the pattern.
- Push back politely when a comment would balloon the PR's scope. Review can go back and forth; a 2000-line PR cannot.

## Anti-patterns

- Squashing meaningful commits at PR-open time — reviewers lose the story of how you got here.
- "Small PR: 47 files changed." If it's large, say so and guide the reviewer.
- Pinging reviewers in chat before they have had a chance to read the description.
- Marking threads resolved on behalf of the reviewer.
