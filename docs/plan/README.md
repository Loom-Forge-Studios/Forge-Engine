# Engineering plan

Read in this order.

| Document | What it is | Read when |
|---|---|---|
| [`master-plan.md`](master-plan.md) | The plan. 20 binding invariants, system contracts, 37 chapters, the risk register. | First. Always. |
| [`decisions.md`](decisions.md) | Ratified decisions, rejected proposals and the reasoning, open questions. | Before proposing anything that sounds clever. |
| [`milestones.md`](milestones.md) | M0–M8 with definitions of done and gate rows. | Planning a work block. |

## How to use it

1. Read the master plan's *Reality check* and *Working agreements* in full. They are binding.
2. Read the decisions. Rejected items are rejected; re-proposing one requires new information.
3. Find your milestone, then find your chapter.
4. If your chapter is marked **BRIEF**, the first job is to expand it into a full chapter — not to write code.
5. Every definition-of-done item that gets settled gets a gate row, and a gate row is a test file, not a claim.

## A note on the numbers

Chapter numbers, DoD item ids and invariant ids are **identities, not positions**.
Chapters 31–37 were added after 1–30 and are numbered by allocation rather than by where
they read; the master plan's document map gives the reading order. Renumbering anything
here would break every gate row, handoff and cross-reference that cites it.
