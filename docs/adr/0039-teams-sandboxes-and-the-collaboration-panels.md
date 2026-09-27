# ADR 0039 — Teams and sandboxes on the in-memory team server, per-command role checks, and the collaboration panels

- **Status:** accepted
- **Date:** 2026-09-23

## Context

WP-U10 builds the team and collaboration UI: Create Team → Add Member in three clicks, sandbox
status, a per-user Live/Pull toggle, the publish/review queue, presence in the hierarchy,
inspector and viewport, scoped ownership claims, a three-way conflict panel on the reflect
tree, and licence status with lapse → degrade. `forge-identity`, `forge-collab`,
`forge-server` and `forge-licence` do not exist (M5-15, M5-18, M6-10, M5-25). The Git store
and revision history do (WP-16, ADR 0035). Lane A builds the split editor's transport
concurrently, so changes to `forge-cmd` and the editor core had to be additive.

## Decision

1. **The server side is two trait pairs with labelled in-memory stand-ins (D-4)**, in
   `forge_project::collab`: `IdentityView`/`IdentityBackend` (`MemoryIdentity`: local
   accounts, teams, members, roles, per-path publish scopes, pending invites by email or a
   minted join code) and `CollabView`/`CollabBackend` (`MemoryCollab`). The `*View` half reads
   and writes only session state (presence, a person's own Live/Pull policy and sandbox
   visibility); panels get it. The `*Backend` half changes team or baseline state and **only
   the editor core names it** — the I7 guard now lists both write traits as project-state
   writers, with reasoned allow-list entries for the core's two items.
2. **The team baseline is a real `ProjectStore`.** `MemoryCollab` wraps whatever store it is
   given — memory, `LocalFs`, or the **Git** store (tested on a `git:` folder: a real Git
   repository whose commits carry the command log). **Publishing is `ProjectStore::commit`**:
   the sandbox's document as the working files, committed with the envelopes that made it, a
   fast-forward from the sandbox's base (`PROJECT-0016` when the baseline moved: pull first).
3. **A core is one person's sandbox** (`project_view = baseline ⊕ sandbox_deltas`, I19): it
   records the envelopes its clients apply as deltas, and the server keeps a row per sandbox
   (base, unpublished transactions, summary, visibility, policy). `Private` rows are withheld
   from everyone but their owner and Owners.
4. **Pulling is a three-way merge on the reflect tree** (`forge_project::merge::merge3`):
   base = the baseline at the sandbox's base, theirs = the revision pulled (read back from the
   store), mine = the project. Per subject (a setting, a name, a parent, one property,
   existence): one side changed → that side; both to the same value → it; both differently →
   a **conflict with all three values**. "Changed" is the failed precondition of Ch.37 §37.4.
   Deleted-vs-edited, orphans and hierarchy loops are conflicts too; the same fresh key
   allocated on both sides re-keys mine (references follow) instead of conflicting. A clean
   merge applies as one **key-preserving patch** (`forge.collab.sync`, planned with the new
   additive `DiffBuilder::spawn_at`) whose every step carries the value it replaces and is
   refused (`CMD-0008`) when the project no longer holds it; conflicts stop the pull, nothing
   applied, until each has a side (`forge.collab.resolve`). Keys are shared across sandboxes,
   so a presence badge, a claim and a merge all name an entity the same way.
5. **Live and Pull are one mechanism** (I20): Live pulls the head on the next pump of any
   client after the server moves (the server's observer only wakes clients — it never takes
   a core lock, so one core publishing never deadlocks against another); Pull when asked, up
   to a chosen revision. The policy is session state, one click either way.
6. **Authorisation per command** (Ch.37 §37.8): a new additive `forge_cmd::CommandGuard`
   (`Bus::set_guard`) is asked, with the issuer and the planned diff, in `apply`, a dry run,
   a batch preview, undo and redo.
7. **Team management, claims, publish and review are commands the core performs** (E-36's
   shape): the planner checks arguments, the core checks its own state before the bus
   applies (a refusal is a `Rejection`), the guard checks the role, then the core performs
   on the backend and writes the audit book (origin `team`, sandbox-tagged). Team commands,
   publish and review are human-only (roles are grants, §21.18); they are performed
   operations with an explicit inverse command, so nothing undoes or redoes them. Roles,
   members and invites live in the identity database, not the project: cloning a repository
   never hands out the member list or a join code.
8. **Review rules are the baseline's** (O-16): a per-project switch plus per-path overrides
   (the most specific matching glob wins), read from the baseline at publish time; Owners and
   Maintainers publish directly. Publish scopes are member globs over content paths
   (`scene/<names…>`, `settings/<key>`), checked against what the publish changes.
9. **Licence** (Ch.38 §38.2): `EntitlementSource` with the labelled `MemoryEntitlement`,
   evaluated locally with no network call in the status path; the four rules, a 14-day grace
   period until O-27 settles it. On lapse only the collaboration commands are refused (with
   the reason) and the panels grey; edits, saves, opens and builds are untouched (E-57).
   Renew is a person's button and the only thing that may reach out.
10. **The panels** (`plugins/forge-panels-collab`, I16) follow the backends through live
    feeds at ≤ 10 Hz, only while visible (§21.11); the hierarchy, inspector and viewport draw
    presence and claims the same way (`RowItem::note` in `forge-ui`), and the inspector turns
    read-only under another person's claim. The editor binary attaches the in-memory server
    and a Team stand-in entitlement; every panel names the stand-ins.

## Why — the owner's two rules

1. **Better for the user**: three clicks to a team; a join code or an email, revocable until
   used; one click between Live and Pull, with nothing lost either way; a teammate's change
   arrives as exactly that change (keys kept: selection, claims and presence survive a pull);
   two edits of one property stop and show both sides rather than overwrite; claims are
   enforced, not decorative; a lapsed licence never costs a project, a build or a ship.
2. **Faster / more efficient**: a sandbox is a row; an idle team editor with every
   collaboration panel open draws and wakes nothing; presence is coalesced to 10 Hz; a pull
   costs a merge over the documents and a patch of only what changed (release, dev box:
   10,000 entities merge in 13 ms and patch in 4 ms; 100,000 in 145 ms and 48 ms), never a
   reload of the project; the per-command check is a map lookup plus an ancestor walk only
   when claims exist.

## Alternatives rejected

- **Team membership as project settings** — the member list and join codes would ship in every
  clone, and a new member could not join a baseline they cannot yet read.
- **Rebasing by reloading the merged document** (`forge.project.load`) — every entity gets a
  new key in every pull; selection, claims and presence break (`test_live_pull`'s control).
- **Rewriting the local store's history to rebase** — the `ProjectStore` trait has no reset;
  sandbox deltas are commands, not store revisions, so rebasing them needs none.
- **Role checks in each planner** — planners do not see the issuer; a guard on the bus sees
  every path (undo and batch included) in one place.

## Consequences

- Gate rows (`Bound`): `C-team-roles-per-command`, `C-ownership-claims-enforced`,
  `C-sandbox-live-pull-in-memory`, `C-conflicts-surfaced`, `C-lapse-degrades-never-locks`,
  `C-presence-live`, `C-collab-panels-idle`. `Unbuilt` (D-4): `C-identity-backend`
  (`forge-identity`, M5-15), `C-collab-backend` (`forge-collab` / `forge-server`, M5-18,
  M6-10), `C-licence-backend` (`forge-licence`, M5-25). I19 and I20 stay `Unbuilt` for the
  real server; their shape is exercised on the stand-in by the rows above.

## Amendment (verification fixes)

- **A join code is a credential for its role.** Whoever types it joins as that role, so a
  member who could read an Owner code could make themselves an Owner from a second account.
  `IdentityView::team` withholds every code; `IdentityView::team_as(id, viewer)` shows a
  code's text only where `viewer` could have minted that invite (`may_assign`: an Owner all,
  a Maintainer those below Maintainer); the core checks a code against
  `IdentityBackend::team_record`. The Team panel lists pending invites only to those who
  manage members, and the audited invite outcome says "a join code", never the code.
  Chosen by rule 1 (nobody gains a role they were not given). Gate row
  `C-join-codes-withheld`.
- **`team.*` and `collab.*` are reserved** to their commands (and the core's load and sync),
  guarded by `C-team-settings-reserved` with a control core that lacks the reservation.
- **Sandbox deltas cost nothing per frame** (rule 2): gesture frames fold (a write to a key
  an earlier frame of the same transaction wrote, with only other pure writes between,
  replaces it); the unpublished count and the summary (`forge_project::summary::Summary`,
  add/remove) are kept as deltas arrive and on undo/redo/cancel; the sandbox row is sent per
  transaction, undo or redo, not per frame. Gate row `C-sandbox-deltas-bounded`.
- The hierarchy compares notes before touching its tree (`C-hierarchy-notes-no-damage`).
