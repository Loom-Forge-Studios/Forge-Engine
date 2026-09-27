# ADR 0058 — Scene composition: nesting and inheritance as one mechanism, derived in the bus, stored as references

- **Status:** accepted
- **Date:** 2026-09-27
- **Plan references:** Ch.28 §28.1–28.8 (FULL since this WP), Ch.7 (amended additively: the
  bus's deriver and the project's reference index), Ch.6 (reflected property diffs), Ch.3
  (deterministic trig for placement), Ch.33 (project files), I7, I8, W1/W2. WP-U20, DoD M5-9.

## Context

M5-9 asks for Godot's model: scenes nest (an instance of a scene inside another) and inherit
(a scene derived from a base stores only its overrides), with base changes reaching every
instance except overridden properties, cycles refused readably, every change a command, and
instances stored as references. The engine already had one project document owned by the bus
(entities with reflect-path properties, a hierarchy), a mirror per client, flat transforms
(an entity's `Transform` is a frame and an offset, not relative to its parent), and project
files that list every entity.

Open choices, decided by the owner's two rules (better for the user; faster and leaner):

1. **Where scenes live.** Separate files per scene (Godot's `.tscn`) would need a second
   document model, scene tabs, and a load path per scene. The library is filtered out of the
   world (viewport, play core); a scene is edited in isolation ("Open base scene", session
   state).
2. **How overrides are recorded.** Explicit override flags would be bookkeeping that can
   drift from the values. **Overrides are the reflected property diff** against what the
   node is expected to hold (Ch.6 reflect paths, `OverrideRecord`), so an edit on an instance
   *is* the override and Revert is setting the value back. Cost: a value set equal to the
   base is not an override (as in Godot).
3. **How a base edit reaches instances.** The rule "update a mirror only where it still
   holds what it was expected to hold before" makes overrides survive without flags.
4. **Finding a base node's instances fast.** A scan per edit is O(project). The project keeps
   a **reference index** of entity-valued properties, maintained by the only mutation path
   (not content: not hashed, not on the wire), so a lookup is O(log n + k) and a command that
   touches no scene pays an index lookup per change.
5. **Placement.** With flat transforms, instances of a scene would all stand where the scene
   was authored. A scene's layout is read **relative to its root**; an instance's root pose
   against the scene root's is a rigid placement applied to its nodes' transforms (forge-num's
   bit-reproducible trig, so every platform computes the same bits). Moving an instance is
   not an override of its nodes.
6. **Store form.** Instance roots, override records only where a node carries something of
   its own, and `scene.keys` (`uid:id` per unstored mirror) so decoding restores every id —
   lossless, so a `ProjectDoc` in memory and every merge stay as before. Measured: 100
   instances of a 21-node scene store in 59,164 B vs 1,109,920 B as copies (18.8x).
7. **Premium boundary.** Composition is a base feature. No base crate gained a premium
   dependency.

## Decision

As above; specified in Ch.28 §28.1–28.8. New crate `crates/forge-scene` (below the editor,
the project files and the play core; on forge-cmd, forge-core, forge-num, forge-trace).
Error prefix `SCENE` (`SCENE-0001..0008`).

## Consequences

- Instances are materialised entities in memory (fast to read and draw; memory grows with
  instance size); files hold references (18.8x smaller in the measured case).
- Deleting a node from an instance, or a scene in use, is refused with the way forward named;
  Make Local breaks a link explicitly and keeps nested scenes linked.
- Follow-ups: scenes shared across projects/packages; "editable children"; showing the
  expansion warnings of an opened file in the console.
