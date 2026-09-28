//! [`SceneError`] — the `SCENE-*` codes (`docs/error-codes.md`).
//!
//! A scene command or the deriver refuses with one of these, carried to the bus as the
//! `why` of a `CmdError` (`CMD-0011` for a command's bad arguments, `CMD-0014` for an edit
//! scene composition refuses), so the message a person or an automation session reads leads with
//! both codes and names what to do instead.

use std::fmt;

use forge_cmd::CmdError;
use forge_core::{CodedError, ErrorCode, error_code};

/// Every way scene composition refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SceneError {
    /// `SCENE-0001`: no scene in the project has this id.
    UnknownScene(String),
    /// `SCENE-0002`: instancing (or moving an instance) here would make a scene contain
    /// itself. `chain` names the scenes around the loop, first to last (`Forest`, `Grove`,
    /// `Forest`).
    Cycle {
        /// The loop, as scene names.
        chain: Vec<String>,
    },
    /// `SCENE-0003`: the node comes from a base scene, so its place in the hierarchy is the
    /// base scene's: it cannot be deleted or moved in an instance.
    FromBase {
        /// The node's name.
        node: String,
        /// The scene it comes from.
        scene: String,
        /// What was refused (`delete`, `move`).
        what: &'static str,
    },
    /// `SCENE-0004`: the scene still has instances or derived scenes, so it cannot be
    /// deleted or moved out of the scene library.
    InUse {
        /// The scene's name.
        scene: String,
        /// How many instances and derived scenes use it.
        users: usize,
        /// Some of their names.
        names: Vec<String>,
    },
    /// `SCENE-0005`: the entity is not linked to a base scene (revert and Make Local need an
    /// instanced or inherited node).
    NotLinked(String),
    /// `SCENE-0006`: `scene.*` properties are the composition's own bookkeeping; only the
    /// scene commands (`forge.scene.*`) write them.
    Reserved {
        /// The property path.
        path: String,
        /// The command that tried.
        by: String,
    },
    /// `SCENE-0007`: a scene id is malformed (`[a-z0-9_]`, 1–64 characters) or already used.
    BadId(String),
    /// `SCENE-0008`: Make Local is for an instance's root; this node is inside an instance.
    NotInstanceRoot(String),
}

impl SceneError {
    /// The stable error code (`docs/error-codes.md`).
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::UnknownScene(_) => error_code!("SCENE-0001"),
            Self::Cycle { .. } => error_code!("SCENE-0002"),
            Self::FromBase { .. } => error_code!("SCENE-0003"),
            Self::InUse { .. } => error_code!("SCENE-0004"),
            Self::NotLinked(_) => error_code!("SCENE-0005"),
            Self::Reserved { .. } => error_code!("SCENE-0006"),
            Self::BadId(_) => error_code!("SCENE-0007"),
            Self::NotInstanceRoot(_) => error_code!("SCENE-0008"),
        }
    }

    /// As a command's bad arguments (`CMD-0011`), for a scene command's own refusal.
    #[must_use]
    pub fn bad_args(self, target: &str) -> CmdError {
        CmdError::BadArgs {
            target: target.to_string(),
            why: self.to_string(),
        }
    }

    /// As a policy refusal (`CMD-0014`), for an edit the deriver refuses whichever command
    /// carried it.
    #[must_use]
    pub fn refused(self) -> CmdError {
        CmdError::PolicyRefused {
            what: self.to_string(),
        }
    }

    /// One representative of every variant — for the allocator-registration test.
    #[doc(hidden)]
    #[must_use]
    pub fn all_variants_for_tests() -> Vec<SceneError> {
        vec![
            Self::UnknownScene("x".into()),
            Self::Cycle {
                chain: vec!["a".into(), "a".into()],
            },
            Self::FromBase {
                node: "n".into(),
                scene: "s".into(),
                what: "delete",
            },
            Self::InUse {
                scene: "s".into(),
                users: 1,
                names: vec!["i".into()],
            },
            Self::NotLinked("n".into()),
            Self::Reserved {
                path: "scene.base".into(),
                by: "SetProperty".into(),
            },
            Self::BadId("A B".into()),
            Self::NotInstanceRoot("n".into()),
        ]
    }
}

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::UnknownScene(id) => write!(f, "no scene has the id {id:?}"),
            Self::Cycle { chain } => write!(
                f,
                "this would make a scene contain itself: {} (a scene cannot be instanced inside itself or inside a scene that contains it)",
                chain.join(" \u{2192} ")
            ),
            Self::FromBase { node, scene, what } => write!(
                f,
                "{node:?} comes from the scene {scene:?}; {what} it there (Open base scene), or Make Local on the instance first"
            ),
            Self::InUse {
                scene,
                users,
                names,
            } => write!(
                f,
                "the scene {scene:?} is used by {users} instance(s) or derived scene(s) ({}); delete them or Make Local on them first",
                names.join(", ")
            ),
            Self::NotLinked(n) => {
                write!(f, "{n:?} is not an instance of a scene or part of one")
            }
            Self::Reserved { path, by } => write!(
                f,
                "{path} is scene-composition bookkeeping; {by} may not change it (use the forge.scene.* commands)"
            ),
            Self::BadId(id) => write!(
                f,
                "{id:?} is not a free scene id (1-64 characters of a-z, 0-9 and _, not used by another scene)"
            ),
            Self::NotInstanceRoot(n) => write!(
                f,
                "{n:?} is inside an instance; Make Local works on the instance's root"
            ),
        }
    }
}

impl std::error::Error for SceneError {}

impl CodedError for SceneError {
    fn error_code(&self) -> ErrorCode {
        self.code()
    }
}
