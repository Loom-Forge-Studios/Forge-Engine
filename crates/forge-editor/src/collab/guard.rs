//! [`CollabGuard`]: **authorisation per command, not per session** (Ch.37 §37.8), installed on
//! the core's bus as its `forge_cmd::CommandGuard` once collaboration backends are attached.
//!
//! For a project bound to a team (its `team.id` setting) it reads the issuer's role from the
//! identity database **at every command**, so a role reduced mid-session is felt by the very
//! next command, and:
//!
//! * refuses team management beyond the issuer's rank (`may_assign`: nobody raises a role
//!   above their own), publishing and claiming for roles without those rights, approval for
//!   roles that do not review, and every collaboration command when the licence does not
//!   allow collaboration (E-57: only these; nothing else is ever locked);
//! * refuses an **edit by a Reviewer or a Viewer** (they have no sandbox to change);
//! * refuses an edit **inside a subtree another teammate claimed** — whoever sends it, by
//!   whatever path (a panel, an automation session, a batch, an undo or a redo): the claim is
//!   enforced by the core, the UI does not just hide it (Ch.37 §37.4).
//!
//! The core's own operations (`forge.project.load`, `forge.team.bind`, `forge.collab.sync`:
//! the baseline arriving) are not edits by anyone and pass.

use std::collections::BTreeMap;
use std::sync::Arc;

use forge_cmd::{Change, CmdError, CommandGuard, EditorCommand, EntityKey, Guarded, Project};
use forge_project::collab::{CollabView, IdentityView, Role, Team, may_assign};
use serde_json::Value as Json;

use super::licence::{EntitlementSource, evaluate};
use super::*;

/// See the module docs.
pub struct CollabGuard {
    pub identity: Arc<dyn IdentityView>,
    pub server: Arc<dyn CollabView>,
    pub licence: Arc<dyn EntitlementSource>,
    /// Whose editor this is (automation sessions, scripts and tests act as them).
    pub owner: String,
    /// W2 positive control only (`test_lapse_degrades_never_locks`): a lapsed licence
    /// refuses every edit — the lock-out E-57 forbids. Never set outside that control.
    #[doc(hidden)]
    pub lapse_locks: bool,
}

fn refuse(what: impl Into<String>) -> CmdError {
    CmdError::PolicyRefused { what: what.into() }
}

impl CollabGuard {
    fn licence_ok(&self, team: Option<&str>) -> Result<(), CmdError> {
        let s = evaluate(
            self.licence.entitlement().as_ref(),
            self.licence.now_ms(),
            forge_project::format::ENGINE_VERSION,
            team,
        );
        if s.collaboration {
            Ok(())
        } else {
            Err(refuse(s.why_not.unwrap_or_else(|| {
                "collaboration is not available with this licence".into()
            })))
        }
    }

    fn team(&self, id: &str) -> Result<Team, CmdError> {
        self.identity.team(id).ok_or_else(|| {
            refuse(format!(
                "this project belongs to team {id}, which the team server does not know"
            ))
        })
    }

    /// Check a collaboration command.
    fn command(
        &self,
        target: &str,
        args: &str,
        user: &str,
        project: &Project,
    ) -> Result<(), CmdError> {
        if CORE_ONLY.contains(&target) {
            return Ok(());
        }
        let a: Json = serde_json::from_str(args).unwrap_or(Json::Null);
        let bound = team_of(project.setting(TEAM_ID_SETTING));
        self.licence_ok(bound.as_deref())?;
        match target {
            TEAM_CREATE => {
                if let Some(t) = bound {
                    return Err(refuse(format!("this project already belongs to team {t}")));
                }
                return Ok(());
            }
            TEAM_ACCEPT => {
                return match bound {
                    Some(t) if self.team(&t)?.member(user).is_some() => Err(refuse(format!(
                        "{user} is already a member of this project's team"
                    ))),
                    _ => Ok(()),
                };
            }
            _ => {}
        }
        let Some(id) = bound else {
            return Err(refuse(
                "this project does not belong to a team yet: create one, or join one with a code",
            ));
        };
        let team = self.team(&id)?;
        let role = team.role_of(user);
        let Some(r) = role else {
            return Err(refuse(format!("{user} is not a member of {}", team.name)));
        };
        let target_role = |a: &Json| -> Result<Role, CmdError> {
            let who = text(target, a, "user")?;
            team.role_of(who)
                .ok_or_else(|| refuse(format!("{who} is not a member of {}", team.name)))
        };
        match target {
            TEAM_INVITE => may_assign(role, None, role_arg(target, &a)?).map_err(refuse),
            TEAM_REVOKE_INVITE => {
                if r.manages_members() {
                    Ok(())
                } else {
                    Err(refuse(format!(
                        "a {} cannot revoke invites (Owners and Maintainers can)",
                        r.label()
                    )))
                }
            }
            TEAM_SET_ROLE => {
                let current = target_role(&a)?;
                may_assign(role, Some(current), role_arg(target, &a)?).map_err(refuse)
            }
            TEAM_SET_PATHS => {
                let current = target_role(&a)?;
                may_assign(role, Some(current), current).map_err(refuse)
            }
            TEAM_REMOVE => {
                let current = target_role(&a)?;
                may_assign(role, Some(current), Role::Viewer).map_err(refuse)
            }
            CLAIM | PUBLISH => {
                if r.edits() {
                    Ok(())
                } else {
                    Err(refuse(format!(
                        "a {} cannot {} (Developers, Maintainers and Owners can)",
                        r.label(),
                        if target == CLAIM { "claim" } else { "publish" }
                    )))
                }
            }
            RELEASE => {
                let entity = number(target, &a, "entity")?;
                let holder = self
                    .server
                    .claims()
                    .into_iter()
                    .find(|c| c.entity == entity)
                    .map(|c| c.holder);
                match holder {
                    Some(h) if h != user && !r.maintains() => Err(refuse(format!(
                        "that claim is {h}'s: only they (or a Maintainer) release it"
                    ))),
                    _ => Ok(()),
                }
            }
            APPROVE | REJECT => {
                if !r.approves() {
                    return Err(refuse(format!(
                        "a {} cannot review publish requests (Owners, Maintainers and Reviewers can)",
                        r.label()
                    )));
                }
                let id = number(target, &a, "request")?;
                let author = self
                    .server
                    .requests()
                    .into_iter()
                    .find(|q| q.id == id)
                    .map(|q| q.author);
                if author.as_deref() == Some(user) {
                    return Err(refuse("someone else reviews your own publish request"));
                }
                Ok(())
            }
            REVIEW_POLICY => {
                if r.maintains() {
                    Ok(())
                } else {
                    Err(refuse(format!(
                        "a {} cannot change the review rules (Owners and Maintainers can)",
                        r.label()
                    )))
                }
            }
            // Pull and resolve: any member (a Viewer's view follows the baseline too).
            _ => Ok(()),
        }
    }

    /// Check an ordinary edit (any command's diff, an undo, a redo).
    fn edit(&self, user: &str, diff: &[Change], project: &Project) -> Result<(), CmdError> {
        if diff.is_empty() {
            return Ok(());
        }
        let Some(id) = team_of(project.setting(TEAM_ID_SETTING)) else {
            return Ok(());
        };
        // A lapsed licence locks nothing (E-57): the collaboration rules pause with it.
        if let Err(e) = self.licence_ok(Some(&id)) {
            return if self.lapse_locks { Err(e) } else { Ok(()) };
        }
        let Some(team) = self.identity.team(&id) else {
            return Ok(());
        };
        // Not a member: a detached copy of a team project, never published (publishing
        // checks membership); the team's rules are not this sandbox's.
        let Some(role) = team.role_of(user) else {
            return Ok(());
        };
        if !role.edits() {
            return Err(refuse(format!(
                "a {} of {} reads the project but does not change it",
                role.label(),
                team.name
            )));
        }
        let claims: BTreeMap<u64, (String, String)> = self
            .server
            .claims()
            .into_iter()
            .filter(|c| c.holder != user)
            .map(|c| (c.entity, (c.holder, c.label)))
            .collect();
        if claims.is_empty() {
            return Ok(());
        }
        let covered = |start: Option<EntityKey>| -> Option<&(String, String)> {
            let mut cur = start;
            let mut steps = 0usize;
            while let Some(k) = cur {
                if let Some(c) = claims.get(&k.0) {
                    return Some(c);
                }
                steps += 1;
                if steps > project.len() + 1 {
                    return None;
                }
                cur = project.entity(k).and_then(|e| e.parent());
            }
            None
        };
        for c in diff {
            let hit = match c {
                Change::Created { parent, .. } => covered(*parent),
                Change::Reparented { entity, after, .. } => {
                    covered(Some(*entity)).or_else(|| covered(*after))
                }
                other => other.entity().and_then(|e| covered(Some(e))),
            };
            if let Some((holder, label)) = hit {
                return Err(refuse(format!(
                    "\u{201c}{label}\u{201d} is claimed by {holder}: it is read-only for you until they release it"
                )));
            }
        }
        Ok(())
    }
}

impl CommandGuard for CollabGuard {
    fn check(&self, g: &Guarded<'_>) -> Result<(), CmdError> {
        let user = member_of(g.issuer, &self.owner);
        match g.cmd {
            Some(EditorCommand::Invoke { target, .. })
                if target == forge_project::format::LOAD_CMD
                    || CORE_ONLY.contains(&target.as_str()) =>
            {
                Ok(())
            }
            Some(EditorCommand::Invoke { target, args }) if is_target(target) => {
                self.command(target, args, &user, g.project)
            }
            _ => self.edit(&user, &g.diff.changes, g.project),
        }
    }
}
