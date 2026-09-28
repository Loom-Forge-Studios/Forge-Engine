//! A small rig over a bus with scene composition installed.
#![allow(dead_code)]

use forge_cmd::{
    Applied, Bus, Change, CmdError, CommandSink, EditorCommand, EntityKey, Issuer, Value,
};
use forge_scene::{SceneFaults, model};

pub struct Rig {
    pub bus: Bus,
}

impl Rig {
    pub fn new() -> Self {
        let mut bus = Bus::new();
        forge_scene::install(&mut bus, &[]);
        Self { bus }
    }

    pub fn with_faults(faults: SceneFaults) -> Self {
        let mut bus = Bus::new();
        forge_scene::install_with_faults(&mut bus, &[], faults);
        Self { bus }
    }

    pub fn try_run(&mut self, cmd: EditorCommand) -> Result<std::sync::Arc<Applied>, CmdError> {
        let e = self.bus.envelope(Issuer::Test, cmd);
        self.bus.apply(e).map_err(|r| r.error)
    }

    pub fn run(&mut self, cmd: EditorCommand) -> std::sync::Arc<Applied> {
        let label = format!("{cmd:?}");
        self.try_run(cmd)
            .unwrap_or_else(|e| panic!("{label} was refused: {e}"))
    }

    pub fn invoke(
        &mut self,
        target: &str,
        args: serde_json::Value,
    ) -> Result<std::sync::Arc<Applied>, CmdError> {
        self.try_run(EditorCommand::Invoke {
            target: target.into(),
            args: args.to_string(),
        })
    }

    /// A new scene; returns its root.
    pub fn scene(&mut self, name: &str, id: &str) -> EntityKey {
        self.invoke(
            "forge.scene.new",
            serde_json::json!({"name": name, "id": id}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        self.root(id)
    }

    pub fn root(&self, id: &str) -> EntityKey {
        model::scene_root(self.bus.project(), id).unwrap_or_else(|| panic!("no scene {id}"))
    }

    /// A derived scene of `base`; returns its root.
    pub fn inherit(&mut self, base: &str, name: &str, id: &str) -> EntityKey {
        self.invoke(
            "forge.scene.new_inherited",
            serde_json::json!({"base": base, "name": name, "id": id}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        self.root(id)
    }

    /// An instance of `scene` under `parent`; returns its root.
    pub fn instance(&mut self, scene: &str, parent: Option<EntityKey>, name: &str) -> EntityKey {
        let a = self
            .try_instance(scene, parent, name)
            .unwrap_or_else(|e| panic!("{e}"));
        first_created(&a)
    }

    pub fn try_instance(
        &mut self,
        scene: &str,
        parent: Option<EntityKey>,
        name: &str,
    ) -> Result<std::sync::Arc<Applied>, CmdError> {
        self.invoke(
            "forge.scene.instance",
            serde_json::json!({"scene": scene, "parent": parent.map(|p| p.0), "name": name}),
        )
    }

    pub fn spawn(&mut self, name: &str, parent: Option<EntityKey>) -> EntityKey {
        let a = self.run(EditorCommand::Spawn {
            name: name.into(),
            parent,
        });
        first_created(&a)
    }

    pub fn set(&mut self, e: EntityKey, path: &str, v: Value) {
        self.run(EditorCommand::SetProperty {
            entity: e,
            path: path.into(),
            value: v,
        });
    }

    pub fn get(&self, e: EntityKey, path: &str) -> Option<Value> {
        self.bus
            .project()
            .entity(e)
            .and_then(|d| d.property(path))
            .cloned()
    }

    pub fn name(&self, e: EntityKey) -> String {
        self.bus
            .project()
            .entity(e)
            .map(|d| d.name().to_string())
            .unwrap_or_default()
    }

    /// The child of `parent` named `name`.
    pub fn child(&self, parent: EntityKey, name: &str) -> EntityKey {
        self.try_child(parent, name)
            .unwrap_or_else(|| panic!("{} has no child {name:?}", self.name(parent)))
    }

    pub fn try_child(&self, parent: EntityKey, name: &str) -> Option<EntityKey> {
        let p = self.bus.project();
        p.entity(parent)?
            .children()
            .find(|c| p.entity(*c).is_some_and(|e| e.name() == name))
    }

    /// Follow a path of names below `from`.
    pub fn at(&self, from: EntityKey, path: &[&str]) -> EntityKey {
        path.iter().fold(from, |k, n| self.child(k, n))
    }
}

pub fn first_created(a: &Applied) -> EntityKey {
    a.diff
        .changes
        .iter()
        .find_map(|c| match c {
            Change::Created { entity, .. } => Some(*entity),
            _ => None,
        })
        .unwrap_or_else(|| panic!("nothing created: {:?}", a.diff))
}

pub fn f(x: f64) -> Value {
    Value::Float(x)
}

pub fn t(s: &str) -> Value {
    Value::Text(s.into())
}

/// The `Tree` scene: Tree { Trunk, Leaves } with a few properties.
pub fn tree(rig: &mut Rig) -> EntityKey {
    let root = rig.scene("Tree", "tree");
    rig.set(root, "height", f(4.0));
    rig.set(root, "transform.position", Value::Vec3([0.0, 0.0, 0.0]));
    let trunk = rig.spawn("Trunk", Some(root));
    rig.set(trunk, "mesh", t("trunk.mesh"));
    rig.set(trunk, "radius", f(0.3));
    let leaves = rig.spawn("Leaves", Some(root));
    rig.set(leaves, "mesh", t("leaves.mesh"));
    rig.set(leaves, "color", t("green"));
    root
}
