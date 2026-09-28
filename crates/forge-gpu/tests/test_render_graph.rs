// timed-gates: exempt(the graph compile cost is printed for the record, not asserted)
//! `test_render_graph` — pass ordering, culling, transient aliasing and barriers (Ch.9, M1-1).
//!
//! * Order is computed from versioned hazards (RAW, WAR, WAW), not declaration order; a pair
//!   of passes that each need the other's input intact is a cycle error. WAR and WAW are
//!   ordered against the next *live* writer, so a culled writer in between hides nothing.
//! * Dead passes are culled; roots (side effects, writers of imports) never are.
//! * Transients with disjoint lifetimes share one physical allocation; the barrier plan
//!   records every transition, including the aliasing hand-over.
//! * Every compiled plan passes the independent verifier ([`verify_plan`]).
//! * On a real device: the aliased frame renders correct pixels, a steady-state frame
//!   allocates nothing, and a pass touching an undeclared resource is refused.
//!
//! Positive control (W2): `positive_control_the_verifier_catches_every_broken_plan` breaks
//! a correct plan seven ways (read-after-write order, write-after-read order,
//! write-after-write order, culled root, missing barrier, overlapping aliases, short slot
//! usage) and asserts each is caught;
//! `positive_control_the_verifier_sees_hazards_through_a_culled_writer` breaks WAR and WAW
//! order across a culled writer and asserts both are caught.
//! `a_waw_hazard_through_a_culled_writer_is_a_cycle_naming_only_its_component` guards the
//! compile side of the same rule (the WAW edge from the previous *live* writer): with the
//! next-version rule compile returns [Z, C, A, W], which the verifier rejects (checked by
//! reverting that one line in `plan()`); the cycle error names only the strongly connected
//! component, not passes stuck behind it.

use std::time::Instant;

use forge_gpu::wgpu;
use forge_gpu::{
    Access, AdapterPool, BufferDesc, CompileOptions, GpuError, GraphPlan, Imports, PoolOptions,
    RenderGraph, ResourceId, TextureDesc, TransientPool, transfer, verify_plan,
};

fn tex(w: u32, h: u32) -> TextureDesc {
    TextureDesc::d2(w, h, wgpu::TextureFormat::Rgba8Unorm)
}

/// A, B, C, D, E, F declared in that order; C reads the version B overwrote.
fn hazard_graph() -> RenderGraph {
    let mut g = RenderGraph::new();
    let out = g.import_texture("out", tex(8, 8));
    let t = g.create_texture("t", tex(8, 8));
    let junk = g.create_texture("junk", tex(8, 8));
    let t1 = g.add_pass("A").write(t, Access::ColorAttachment);
    let t2 = g.add_pass("B").modify(t1, Access::ColorAttachment);
    let out1 = {
        let mut p = g.add_pass("C");
        p.read(t1, Access::CopySrc);
        p.write(out, Access::CopyDst)
    };
    {
        let mut p = g.add_pass("D");
        p.read(t2, Access::CopySrc);
        p.modify(out1, Access::CopyDst);
    }
    g.add_pass("E").write(junk, Access::ColorAttachment);
    g.add_pass("F").side_effect();
    g
}

#[test]
fn order_follows_hazards_not_declaration() {
    let plan = hazard_graph().compile().expect("compiles").plan().clone();
    assert_eq!(plan.order_names(), ["A", "C", "B", "D", "F"]);
    let e = plan.pass_index("E").expect("E");
    assert!(plan.passes[e].culled, "E's output is never read");
    assert!(
        !plan.passes[plan.pass_index("F").expect("F")].culled,
        "side effect kept"
    );
    assert_eq!(verify_plan(&plan), Vec::<String>::new());
    // Without culling E runs, in declaration order among the free passes.
    let all = hazard_graph()
        .compile_with(CompileOptions {
            cull: false,
            ..CompileOptions::default()
        })
        .expect("compiles");
    assert_eq!(all.plan().order_names(), ["A", "C", "B", "D", "E", "F"]);
    assert_eq!(verify_plan(all.plan()), Vec::<String>::new());
}

/// A pure-writes import `out` (v1) and overwrites import `r`; B pure-writes `out` again
/// (v2, from A's handle); C, declared last, reads `r` at version 0. WAR delays A until after
/// C, and only the WAW edge keeps B after A, so `out` ends up holding B's contents.
fn waw_graph() -> RenderGraph {
    let mut g = RenderGraph::new();
    let out = g.import_texture("out", tex(4, 4));
    let r = g.import_texture("r", tex(4, 4));
    let out1 = {
        let mut p = g.add_pass("A");
        p.write(r, Access::CopyDst);
        p.write(out, Access::CopyDst)
    };
    g.add_pass("B").write(out1, Access::ColorAttachment);
    g.add_pass("C").side_effect().read(r, Access::Sampled);
    g
}

#[test]
fn a_later_writer_of_the_same_resource_runs_later() {
    let plan = waw_graph().compile().expect("compiles").plan().clone();
    assert_eq!(plan.order_names(), ["C", "A", "B"]);
    assert_eq!(verify_plan(&plan), Vec::<String>::new());
}

/// A writes transient `t` (v1); Y overwrites it (v2) but nothing reads v2, so Y is culled;
/// Z overwrites it again (v3); side-effect W reads v3; side-effect X, declared last, reads
/// v1. The hazard that keeps X before Z runs *through* the culled Y (X reads v1, Y writes
/// v2), so ordering must look for the next writer that is live, not the next version's.
fn culled_middle_writer_graph() -> RenderGraph {
    let mut g = RenderGraph::new();
    let t = g.create_texture("t", tex(4, 4));
    let t1 = g.add_pass("A").write(t, Access::ColorAttachment);
    let t2 = g.add_pass("Y").write(t1, Access::ColorAttachment);
    let t3 = g.add_pass("Z").write(t2, Access::ColorAttachment);
    g.add_pass("W").side_effect().read(t3, Access::Sampled);
    g.add_pass("X").side_effect().read(t1, Access::Sampled);
    g
}

#[test]
fn a_reader_runs_before_the_next_live_writer_when_the_middle_writer_is_culled() {
    let plan = culled_middle_writer_graph()
        .compile()
        .expect("compiles")
        .plan()
        .clone();
    let y = plan.pass_index("Y").expect("Y");
    assert!(plan.passes[y].culled, "nothing reads Y's version");
    // X must sample A's contents, so it runs before Z overwrites them.
    assert_eq!(plan.order_names(), ["A", "X", "Z", "W"]);
    assert_eq!(verify_plan(&plan), Vec::<String>::new());
    // Without culling Y runs, and X still precedes every later writer.
    let all = culled_middle_writer_graph()
        .compile_with(CompileOptions {
            cull: false,
            ..CompileOptions::default()
        })
        .expect("compiles");
    assert_eq!(all.plan().order_names(), ["A", "X", "Y", "Z", "W"]);
    assert_eq!(verify_plan(all.plan()), Vec::<String>::new());
}

#[test]
fn positive_control_the_verifier_sees_hazards_through_a_culled_writer() {
    let good = culled_middle_writer_graph()
        .compile()
        .expect("compiles")
        .plan()
        .clone();
    assert!(verify_plan(&good).is_empty());
    let idx = |p: &GraphPlan, n: &str| {
        p.order
            .iter()
            .position(|&i| p.passes[i].name == n)
            .expect("in order")
    };

    // X moved after Z: it would sample Z's contents instead of A's (WAR across culled Y).
    let mut p = good.clone();
    let (x, z) = (idx(&p, "X"), idx(&p, "Z"));
    p.order.swap(x, z);
    assert_eq!(p.order_names(), ["A", "Z", "X", "W"]);
    assert!(
        verify_plan(&p)
            .iter()
            .any(|e| e.starts_with("WAR: `X` reads t@1 after `Z`")),
        "{:?}",
        verify_plan(&p)
    );

    // A moved after Z: two live writers of `t` out of order (WAW across culled Y).
    let mut p = good;
    let (a, z) = (idx(&p, "A"), idx(&p, "Z"));
    p.order.swap(a, z);
    assert!(
        verify_plan(&p)
            .iter()
            .any(|e| e.starts_with("WAW: `A` and `Z`")),
        "{:?}",
        verify_plan(&p)
    );
}

/// The compile-side WAW edge across a culled writer (WP-08 verifier leftover).
///
/// A writes the import `out` (a root) and transient `t` (v1, never read); Y overwrites `t`
/// (v2, never read — culled); Z overwrites it again (v3); side-effect C, declared later,
/// reads `t@3` and `out@0`; side-effect W reads `t@3`.
///
/// Hazards among live passes: C reads `out@0`, which A overwrites → C before A (WAR); Z
/// reads nothing but must follow A as the next *live* writer of `t` → A before Z (WAW,
/// across culled Y); C reads Z's `t@3` → Z before C (RAW). That is a cycle A → Z → C → A.
/// With the next-*version* WAW rule the A → Z edge goes through culled Y and vanishes, and
/// compile returns the order [Z, C, A, W] — A overwrites `t` after Z, so W samples A's
/// contents instead of Z's. W is stuck downstream of the cycle but is not on it.
fn culled_waw_cycle_graph() -> RenderGraph {
    let mut g = RenderGraph::new();
    let out = g.import_texture("out", tex(4, 4));
    let t = g.create_texture("t", tex(4, 4));
    let t1 = {
        let mut p = g.add_pass("A");
        p.write(out, Access::ColorAttachment);
        p.write(t, Access::ColorAttachment)
    };
    let t2 = g.add_pass("Y").write(t1, Access::ColorAttachment);
    let t3 = g.add_pass("Z").write(t2, Access::ColorAttachment);
    g.add_pass("C")
        .side_effect()
        .read(t3, Access::Sampled)
        .read(out, Access::CopySrc);
    g.add_pass("W").side_effect().read(t3, Access::Sampled);
    g
}

/// Compile must either report the cycle (`GPU-0006`, naming exactly `on_cycle`) or produce
/// a plan the independent verifier accepts. Returns whether it was a cycle.
fn cycle_or_verified(g: RenderGraph, opts: CompileOptions, on_cycle: &[&str]) -> bool {
    match g.compile_with(opts) {
        Ok(c) => {
            assert_eq!(
                verify_plan(c.plan()),
                Vec::<String>::new(),
                "compile returned an order the verifier rejects: {:?}",
                c.plan().order_names()
            );
            false
        }
        Err(e) => {
            let s = e.to_string();
            assert!(s.starts_with("GPU-0006") && s.contains("cycle"), "{s}");
            let expected = format!("{on_cycle:?}");
            assert!(
                s.contains(&expected),
                "the cycle must name exactly {expected}: {s}"
            );
            true
        }
    }
}

#[test]
fn a_waw_hazard_through_a_culled_writer_is_a_cycle_naming_only_its_component() {
    // Culling on: Y is dead, the cycle is A → Z → C; W is stuck behind it but not named.
    assert!(cycle_or_verified(
        culled_waw_cycle_graph(),
        CompileOptions::default(),
        &["A", "Z", "C"],
    ));
    let s = culled_waw_cycle_graph()
        .compile()
        .err()
        .expect("cycle")
        .to_string();
    assert!(!s.contains("\"W\"") && !s.contains("\"Y\""), "{s}");
    // Culling off: Y runs and sits on the cycle too.
    assert!(cycle_or_verified(
        culled_waw_cycle_graph(),
        CompileOptions {
            cull: false,
            ..CompileOptions::default()
        },
        &["A", "Y", "Z", "C"],
    ));
    // Without C's stale read of `out` there is no cycle, and the order verifies: A before Z.
    let mut g = RenderGraph::new();
    let out = g.import_texture("out", tex(4, 4));
    let t = g.create_texture("t", tex(4, 4));
    let t1 = {
        let mut p = g.add_pass("A");
        p.write(out, Access::ColorAttachment);
        p.write(t, Access::ColorAttachment)
    };
    let t2 = g.add_pass("Y").write(t1, Access::ColorAttachment);
    let t3 = g.add_pass("Z").write(t2, Access::ColorAttachment);
    g.add_pass("W").side_effect().read(t3, Access::Sampled);
    let c = g.compile().expect("acyclic");
    assert_eq!(c.plan().order_names(), ["A", "Z", "W"]);
    assert_eq!(verify_plan(c.plan()), Vec::<String>::new());
}

#[test]
fn a_mutual_overwrite_is_a_cycle() {
    let mut g = RenderGraph::new();
    let u = g.import_texture("u", tex(4, 4));
    let t = g.create_texture("t", tex(4, 4));
    let t1 = g.add_pass("A").write(t, Access::ColorAttachment);
    let t2 = {
        let mut p = g.add_pass("B");
        p.read(u, Access::Sampled);
        p.modify(t1, Access::ColorAttachment)
    };
    {
        let mut p = g.add_pass("C");
        p.read(t1, Access::Sampled);
        p.write(u, Access::ColorAttachment);
    }
    g.add_pass("D").side_effect().read(t2, Access::Sampled);
    let err = g.compile().err().expect("a cycle");
    let s = err.to_string();
    assert!(s.starts_with("GPU-0006"), "{s}");
    assert!(s.contains("cycle") && s.contains("[\"B\", \"C\"]"), "{s}");
    // D waits on B's output, so it is stuck too — but it is not on the cycle.
    assert!(!s.contains("\"D\""), "{s}");
}

#[test]
fn misuse_is_reported_by_compile() {
    let expect = |g: RenderGraph, needle: &str| {
        let s = g.compile().err().map(|e| e.to_string()).unwrap_or_default();
        assert!(s.contains(needle), "wanted {needle:?} in {s:?}");
    };
    let mut g = RenderGraph::new();
    let t = g.create_texture("t", tex(4, 4));
    g.add_pass("P").side_effect().read(t, Access::Sampled);
    expect(g, "reads `t` before any pass writes it");

    let mut g = RenderGraph::new();
    let t = g.create_texture("t", tex(4, 4));
    let t1 = g.add_pass("A").write(t, Access::ColorAttachment);
    let _t2 = g.add_pass("B").modify(t1, Access::ColorAttachment);
    g.add_pass("C").write(t1, Access::ColorAttachment);
    expect(g, "would fork the resource");

    let mut g = RenderGraph::new();
    let t = g.create_texture("t", tex(4, 4));
    let t1 = g.add_pass("A").write(t, Access::ColorAttachment);
    {
        let mut p = g.add_pass("B");
        p.read(t1, Access::Sampled);
        p.modify(t1, Access::ColorAttachment);
    }
    expect(g, "uses `t` twice");

    let mut g = RenderGraph::new();
    let b = g.create_buffer("b", BufferDesc { size: 16 });
    g.add_pass("A").write(b, Access::ColorAttachment);
    expect(g, "not a valid access for buffer `b`");

    let mut g = RenderGraph::new();
    let t = g.import_texture("t", tex(4, 4));
    g.add_pass("A").read(t, Access::ColorAttachment);
    expect(g, "which writes");

    let mut other = RenderGraph::new();
    let _ = other.create_texture("x", tex(1, 1));
    let foreign = other.create_texture("y", tex(1, 1));
    let mut g = RenderGraph::new();
    g.add_pass("A").read(foreign, Access::Sampled);
    expect(g, "from another graph");
}

/// a [P0,P1] and c [P2,P3] share a description; b and d overlap everything else.
fn alias_graph() -> RenderGraph {
    let mut g = RenderGraph::new();
    let out = g.import_texture("out", tex(16, 16));
    let a = g.create_texture("a", tex(16, 16));
    let b = g.create_texture("b", tex(16, 16));
    let c = g.create_texture("c", tex(16, 16));
    let d = g.create_texture("d", tex(8, 8));
    let a1 = g.add_pass("P0").write(a, Access::ColorAttachment);
    let (b1, d1) = {
        let mut p = g.add_pass("P1");
        p.read(a1, Access::Sampled);
        (
            p.write(b, Access::ColorAttachment),
            p.write(d, Access::StorageWrite),
        )
    };
    let c1 = {
        let mut p = g.add_pass("P2");
        p.read(b1, Access::Sampled);
        p.write(c, Access::ColorAttachment)
    };
    {
        let mut p = g.add_pass("P3");
        p.read(c1, Access::CopySrc);
        p.read(d1, Access::Sampled);
        p.write(out, Access::CopyDst);
    }
    g
}

fn slot_of(plan: &GraphPlan, name: &str) -> usize {
    plan.resources
        .iter()
        .find(|r| r.name == name)
        .and_then(|r| r.slot)
        .unwrap_or_else(|| panic!("{name} has no slot"))
}

#[test]
fn disjoint_transients_share_one_allocation() {
    let compiled = alias_graph().compile().expect("compiles");
    let plan = compiled.plan();
    assert_eq!(
        slot_of(plan, "a"),
        slot_of(plan, "c"),
        "a [0,1] and c [2,3] alias"
    );
    assert_ne!(
        slot_of(plan, "a"),
        slot_of(plan, "b"),
        "b [1,2] overlaps both"
    );
    assert_ne!(
        slot_of(plan, "d"),
        slot_of(plan, "a"),
        "different description"
    );
    assert_eq!(plan.slots.len(), 3);
    let s = &plan.slots[slot_of(plan, "a")];
    assert_eq!(
        s.texture_usage,
        wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        "the slot carries every resident's usages"
    );
    let (virt, phys) = plan.transient_bytes();
    println!("transients: {virt} bytes declared, {phys} bytes allocated");
    assert!(phys < virt);
    assert_eq!(verify_plan(plan), Vec::<String>::new());

    let unaliased = alias_graph()
        .compile_with(CompileOptions {
            aliasing: false,
            ..CompileOptions::default()
        })
        .expect("compiles");
    assert_eq!(unaliased.plan().slots.len(), 4);
    assert_eq!(verify_plan(unaliased.plan()), Vec::<String>::new());

    // Buffers of different sizes share a slot sized for the larger.
    let mut g = RenderGraph::new();
    let out = g.import_buffer("out", BufferDesc { size: 1024 });
    let x = g.create_buffer("x", BufferDesc { size: 256 });
    let y = g.create_buffer("y", BufferDesc { size: 1024 });
    let x1 = g.add_pass("X").write(x, Access::StorageWrite);
    let out1 = {
        let mut p = g.add_pass("X2");
        p.read(x1, Access::StorageRead);
        p.write(out, Access::StorageWrite)
    };
    let y1 = g.add_pass("Y").write(y, Access::StorageWrite);
    {
        let mut p = g.add_pass("Y2");
        p.read(y1, Access::CopySrc);
        p.modify(out1, Access::CopyDst);
    }
    let c = g.compile().expect("compiles");
    let p = c.plan();
    assert_eq!(p.slots.len(), 1, "{:?}", p.slots);
    assert_eq!(
        p.slots[0].kind,
        forge_gpu::ResourceKind::Buffer(BufferDesc { size: 1024 })
    );
    assert_eq!(verify_plan(p), Vec::<String>::new());
}

#[test]
fn the_barrier_plan_records_every_transition() {
    let compiled = alias_graph().compile().expect("compiles");
    let plan = compiled.plan();
    let id = |n: &str| {
        ResourceId(
            plan.resources
                .iter()
                .position(|r| r.name == n)
                .expect("res") as u32,
        )
    };
    let pass = |n: &str| plan.pass_index(n).expect("pass");
    let got: Vec<_> = plan
        .barriers
        .iter()
        .map(|b| (b.before, b.res, b.from, b.to, b.aliased_from))
        .collect();
    use Access::*;
    let want = vec![
        (pass("P0"), id("a"), None, ColorAttachment, None),
        (pass("P1"), id("a"), Some(ColorAttachment), Sampled, None),
        (pass("P1"), id("b"), None, ColorAttachment, None),
        (pass("P1"), id("d"), None, StorageWrite, None),
        (pass("P2"), id("b"), Some(ColorAttachment), Sampled, None),
        (pass("P2"), id("c"), None, ColorAttachment, Some(id("a"))),
        (pass("P3"), id("c"), Some(ColorAttachment), CopySrc, None),
        (pass("P3"), id("d"), Some(StorageWrite), Sampled, None),
        (pass("P3"), id("out"), None, CopyDst, None),
    ];
    assert_eq!(got, want);

    // Storage write after storage write needs a barrier even though the state is the same;
    // two reads in the same state need none.
    let mut g = RenderGraph::new();
    let out = g.import_buffer("out", BufferDesc { size: 64 });
    let s = g.create_buffer("s", BufferDesc { size: 64 });
    let s1 = g.add_pass("W1").write(s, Access::StorageWrite);
    let s2 = g.add_pass("W2").modify(s1, Access::StorageWrite);
    g.add_pass("R1").side_effect().read(s2, Access::StorageRead);
    {
        let mut p = g.add_pass("R2");
        p.read(s2, Access::StorageRead);
        p.write(out, Access::StorageWrite);
    }
    let c = g.compile().expect("compiles");
    let p = c.plan();
    let on_s: Vec<_> = p
        .barriers
        .iter()
        .filter(|b| b.res == s2.resource())
        .map(|b| (p.passes[b.before].name.as_str(), b.from, b.to))
        .collect();
    assert_eq!(
        on_s,
        vec![
            ("W1", None, StorageWrite),
            ("W2", Some(StorageWrite), StorageWrite),
            ("R1", Some(StorageWrite), StorageRead),
        ]
    );
    assert_eq!(verify_plan(p), Vec::<String>::new());
}

#[test]
fn positive_control_the_verifier_catches_every_broken_plan() {
    let good = hazard_graph().compile().expect("compiles").plan().clone();
    assert!(verify_plan(&good).is_empty());
    let idx = |p: &GraphPlan, n: &str| {
        p.order
            .iter()
            .position(|&i| p.passes[i].name == n)
            .expect("in order")
    };

    // 1. A reader before its writer (RAW).
    let mut p = good.clone();
    let (a, c) = (idx(&p, "A"), idx(&p, "C"));
    p.order.swap(a, c);
    assert!(
        verify_plan(&p).iter().any(|e| e.starts_with("RAW")),
        "{:?}",
        verify_plan(&p)
    );

    // 2. A reader after the pass that overwrote its version (WAR).
    let mut p = good.clone();
    let (c, b) = (idx(&p, "C"), idx(&p, "B"));
    p.order.swap(c, b);
    assert!(
        verify_plan(&p).iter().any(|e| e.starts_with("WAR")),
        "{:?}",
        verify_plan(&p)
    );

    // 3. Two writers of one resource out of version order (WAW).
    let waw = waw_graph().compile().expect("compiles").plan().clone();
    assert!(verify_plan(&waw).is_empty());
    let mut p = waw;
    let (a, b) = (idx(&p, "A"), idx(&p, "B"));
    p.order.swap(a, b);
    assert!(
        verify_plan(&p).iter().any(|e| e.starts_with("WAW")),
        "{:?}",
        verify_plan(&p)
    );

    // 4. A root culled.
    let mut p = good.clone();
    let f = p.pass_index("F").expect("F");
    p.passes[f].culled = true;
    p.order.retain(|&i| i != f);
    assert!(
        verify_plan(&p)
            .iter()
            .any(|e| e.contains("root pass `F` was culled"))
    );

    // 5. A missing barrier.
    let mut p = good.clone();
    p.barriers.remove(1);
    assert!(
        verify_plan(&p)
            .iter()
            .any(|e| e.starts_with("missing barrier"))
    );

    // 6. Overlapping residents forced into one slot.
    let aliased = alias_graph().compile().expect("compiles").plan().clone();
    let mut p = aliased.clone();
    let b = p.resources.iter().position(|r| r.name == "b").expect("b");
    let a_slot = slot_of(&p, "a");
    let b_slot = slot_of(&p, "b");
    p.slots[b_slot].residents.retain(|r| r.0 as usize != b);
    p.slots[a_slot].residents.insert(1, ResourceId(b as u32));
    p.resources[b].slot = Some(a_slot);
    assert!(
        verify_plan(&p).iter().any(|e| e.contains("overlap")),
        "{:?}",
        verify_plan(&p)
    );

    // 7. A slot created without a usage one resident needs.
    let mut p = aliased;
    let s = slot_of(&p, "a");
    p.slots[s]
        .texture_usage
        .remove(wgpu::TextureUsages::COPY_SRC);
    assert!(verify_plan(&p).iter().any(|e| e.contains("usage lacks")));
}

#[test]
fn compile_cost_is_small() {
    // A 256-pass chain with a transient per pass (worst case for aliasing search).
    let t0 = Instant::now();
    let runs = 20;
    for _ in 0..runs {
        let mut g = RenderGraph::new();
        let out = g.import_texture("out", tex(64, 64));
        let mut prev = None;
        for i in 0..256 {
            let t = g.create_texture(&format!("t{i}"), tex(64, 64));
            let mut p = g.add_pass(&format!("p{i}"));
            if let Some(h) = prev {
                p.read(h, Access::Sampled);
            }
            prev = Some(p.write(t, Access::ColorAttachment));
        }
        let mut p = g.add_pass("final");
        if let Some(h) = prev {
            p.read(h, Access::CopySrc);
        }
        p.write(out, Access::CopyDst);
        drop(p);
        let c = g.compile().expect("compiles");
        assert_eq!(
            c.plan().slots.len(),
            2,
            "a chain ping-pongs between two allocations"
        );
    }
    let per = t0.elapsed() / runs;
    println!("build + compile, 257 passes / 257 resources: {per:?} per graph");
}

// ---- on a real device ------------------------------------------------------------------

mod common;

fn pool() -> Option<AdapterPool> {
    common::pool(&PoolOptions::default())
}

fn clear_pass(
    ctx: &mut forge_gpu::PassContext<'_>,
    h: forge_gpu::Handle,
    color: wgpu::Color,
) -> Result<(), GpuError> {
    let load = ctx.color_load(h, color)?;
    let view = ctx.view(h)?.clone();
    let _rp = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    });
    Ok(())
}

fn copy_pass(
    ctx: &mut forge_gpu::PassContext<'_>,
    src: forge_gpu::Handle,
    dst: forge_gpu::Handle,
) -> Result<(), GpuError> {
    let (s, d) = (ctx.texture(src)?.clone(), ctx.texture(dst)?.clone());
    ctx.encoder
        .copy_texture_to_texture(s.as_image_copy(), d.as_image_copy(), s.size());
    Ok(())
}

fn frame(out1: &wgpu::Texture, out2: &wgpu::Texture) -> (forge_gpu::CompiledGraph, Imports) {
    let mut g = RenderGraph::new();
    let o1 = g.import_texture("out1", tex(4, 4));
    let o2 = g.import_texture("out2", tex(4, 4));
    let t1 = g.create_texture("t1", tex(4, 4));
    let t2 = g.create_texture("t2", tex(4, 4));
    let t1w = {
        let mut p = g.add_pass("red");
        let h = p.write(t1, Access::ColorAttachment);
        p.run(move |ctx| clear_pass(ctx, h, wgpu::Color::RED));
        h
    };
    {
        let mut p = g.add_pass("copy1");
        p.read(t1w, Access::CopySrc);
        let o = p.write(o1, Access::CopyDst);
        p.run(move |ctx| copy_pass(ctx, t1w, o));
    }
    let t2w = {
        let mut p = g.add_pass("green");
        let h = p.write(t2, Access::ColorAttachment);
        p.run(move |ctx| clear_pass(ctx, h, wgpu::Color::GREEN));
        h
    };
    {
        let mut p = g.add_pass("copy2");
        p.read(t2w, Access::CopySrc);
        let o = p.write(o2, Access::CopyDst);
        p.run(move |ctx| copy_pass(ctx, t2w, o));
    }
    let mut imports = Imports::new();
    imports.texture(o1, out1).texture(o2, out2);
    (g.compile().expect("compiles"), imports)
}

#[test]
fn an_aliased_frame_renders_correctly_and_steady_state_allocates_nothing() {
    let Some(pool) = pool() else { return };
    let dev = pool.primary();
    let mk = || {
        dev.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("out"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let (out1, out2) = (mk(), mk());
    let mut transients = TransientPool::new();
    let (g, imports) = frame(&out1, &out2);
    assert_eq!(g.plan().slots.len(), 1, "t1 and t2 alias");
    let s1 = g.execute(dev, &mut transients, &imports).expect("frame 1");
    assert_eq!(
        (
            s1.passes_run,
            s1.virtual_resources,
            s1.physical_resources,
            s1.allocations
        ),
        (4, 2, 1, 1)
    );
    let red = transfer::read_texture(dev, &out1).expect("read");
    let green = transfer::read_texture(dev, &out2).expect("read");
    assert!(red.chunks(4).all(|p| p == [255, 0, 0, 255]), "{red:?}");
    assert!(green.chunks(4).all(|p| p == [0, 255, 0, 255]), "{green:?}");
    for _ in 0..3 {
        let (g, imports) = frame(&out1, &out2);
        let s = g.execute(dev, &mut transients, &imports).expect("frame n");
        assert_eq!(s.allocations, 0, "steady state reuses the pool");
    }
    assert_eq!(transients.held(), (1, 0));
    assert!(dev.take_uncaptured_errors().is_empty());
}

#[test]
fn a_compute_pass_and_a_buffer_import_run_through_the_graph() {
    let Some(pool) = pool() else { return };
    let dev = pool.primary();
    let module = forge_gpu::shader::compile_wgsl(
        &dev.device,
        "fill.wgsl",
        "@group(0) @binding(0) var<storage, read_write> o: array<u32>;\n\
         @compute @workgroup_size(64) fn main(@builtin(global_invocation_id) i: vec3<u32>) { o[i.x] = i.x * 3u + 7u; }",
    )
    .expect("valid");
    let pipeline = dev
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fill"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let out = transfer::upload_buffer(dev, "out", &[0u8; 1024], wgpu::BufferUsages::empty());
    let mut g = RenderGraph::new();
    let o = g.import_buffer("out", BufferDesc { size: 1024 });
    let s = g.create_buffer("s", BufferDesc { size: 1024 });
    let s1 = {
        let mut p = g.add_pass("fill");
        let h = p.write(s, Access::StorageWrite);
        p.run(move |ctx| {
            let bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ctx.buffer(h)?.as_entire_binding(),
                }],
            });
            let mut cp = ctx
                .encoder
                .begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            cp.set_pipeline(&pipeline);
            cp.set_bind_group(0, &bg, &[]);
            cp.dispatch_workgroups(4, 1, 1);
            Ok(())
        });
        h
    };
    {
        let mut p = g.add_pass("copy");
        p.read(s1, Access::CopySrc);
        let d = p.write(o, Access::CopyDst);
        p.run(move |ctx| {
            let (a, b) = (ctx.buffer(s1)?.clone(), ctx.buffer(d)?.clone());
            ctx.encoder.copy_buffer_to_buffer(&a, 0, &b, 0, 1024);
            Ok(())
        });
    }
    let mut imports = Imports::new();
    imports.buffer(o, &out);
    g.compile()
        .expect("compiles")
        .execute(dev, &mut TransientPool::new(), &imports)
        .expect("runs");
    let bytes = transfer::read_buffer(dev, &out, 0, 1024).expect("read");
    for (i, w) in bytes.chunks(4).enumerate() {
        let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        assert_eq!(v, i as u32 * 3 + 7);
    }
    assert!(dev.take_uncaptured_errors().is_empty());
}

#[test]
fn positive_control_an_undeclared_access_and_an_unbound_import_are_refused() {
    let Some(pool) = pool() else { return };
    let dev = pool.primary();
    let mut g = RenderGraph::new();
    let a = g.create_texture("a", tex(4, 4));
    let b = g.create_texture("b", tex(4, 4));
    g.add_pass("keep-b")
        .side_effect()
        .write(b, Access::ColorAttachment);
    {
        let mut p = g.add_pass("sneaky");
        p.side_effect();
        let h = p.write(a, Access::ColorAttachment);
        let _ = h;
        // Declares `a` but touches `b`.
        p.run(move |ctx| ctx.texture(b).map(|_| ()));
    }
    let e = g
        .compile()
        .expect("compiles")
        .execute(dev, &mut TransientPool::new(), &Imports::new())
        .expect_err("refused");
    assert!(
        e.to_string().contains("touched `b` without declaring it"),
        "{e}"
    );

    let mut g = RenderGraph::new();
    let o = g.import_texture("o", tex(4, 4));
    g.add_pass("w").write(o, Access::ColorAttachment);
    let e = g
        .compile()
        .expect("compiles")
        .execute(dev, &mut TransientPool::new(), &Imports::new())
        .expect_err("refused");
    assert!(e.to_string().contains("import `o` is not bound"), "{e}");
    assert!(dev.take_uncaptured_errors().is_empty());
}

/// A retained graph (every body `run_retained`) executes again and again without being
/// rebuilt; its bodies read per-frame data from shared state. Positive control: a graph
/// with a single-use body refuses a second execution instead of silently skipping the pass.
#[test]
fn a_retained_graph_executes_every_frame_and_a_single_use_body_refuses_to_rerun() {
    use std::cell::Cell;
    use std::rc::Rc;
    let Some(pool) = pool() else { return };
    let dev = pool.primary();
    let out = dev.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("out"),
        size: wgpu::Extent3d {
            width: 4,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let green = Rc::new(Cell::new(0.0));
    let runs = Rc::new(Cell::new(0u32));
    let build = |retained: bool| {
        let mut g = RenderGraph::new();
        let o = g.import_texture("out", tex(4, 4));
        let mut p = g.add_pass("fill");
        let h = p.write(o, Access::ColorAttachment);
        let (green, runs) = (green.clone(), runs.clone());
        let body = move |ctx: &mut forge_gpu::PassContext<'_>| {
            runs.set(runs.get() + 1);
            let c = wgpu::Color {
                r: 0.0,
                g: green.get(),
                b: 0.0,
                a: 1.0,
            };
            clear_pass(ctx, h, c)
        };
        if retained {
            p.run_retained(body);
        } else {
            p.run(body);
        }
        (g.compile().expect("compiles"), o)
    };
    let mut transients = TransientPool::new();
    let (mut g, o) = build(true);
    let mut imports = Imports::new();
    imports.texture(o, &out);
    for (frame, level) in [(1u32, 0.0), (2, 1.0), (3, 0.0)] {
        green.set(level);
        g.execute_retained(dev, &mut transients, &imports, None)
            .expect("retained frame");
        assert_eq!(runs.get(), frame);
        let px = transfer::read_texture(dev, &out).expect("read");
        let want = if level > 0.5 { 255 } else { 0 };
        assert_eq!(px[1], want, "frame {frame} sees that frame's data");
    }
    // Control: the single-use body runs once, then the graph refuses.
    let (mut once, o2) = build(false);
    let mut imports2 = Imports::new();
    imports2.texture(o2, &out);
    once.execute_retained(dev, &mut transients, &imports2, None)
        .expect("first run");
    let err = once
        .execute_retained(dev, &mut transients, &imports2, None)
        .expect_err("a spent single-use body must not rerun silently");
    assert!(err.to_string().contains("single-use"), "{err}");
    assert!(dev.take_uncaptured_errors().is_empty());
}
