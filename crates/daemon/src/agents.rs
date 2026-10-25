//! What an agent's hooks tell the daemon, and the attention made of them (REQ-129 to REQ-133).
//! Claude runs `jwn hook <event>` at its hooks; that sends `agent_event` here.

use contract::{AgentEvent, AttentionKind, ErrorCode, Id};

use crate::commands::{Done, err};
use crate::core::Core;
use crate::model::Own;
use crate::server::Shared;

/// The attentions an agent raises and ends itself.
pub const KINDS: [AttentionKind; 4] = [
    AttentionKind::Permission,
    AttentionKind::Question,
    AttentionKind::Plan,
    AttentionKind::AgentDone,
];

/// What one hook does: the attention it raises, and whether it first ends what the agent had
/// raised (the agent moving on means the person has answered).
struct Change {
    clear: bool,
    raise: Option<AttentionKind>,
}

fn change(event: AgentEvent, notification_type: Option<&str>, tool: Option<&str>) -> Change {
    match event {
        AgentEvent::Notification => Change {
            clear: false,
            raise: match notification_type {
                Some("permission_prompt") => Some(AttentionKind::Permission),
                Some("elicitation_dialog" | "agent_needs_input") => Some(AttentionKind::Question),
                _ => None,
            },
        },
        AgentEvent::PreToolUse => Change {
            clear: true,
            raise: match tool {
                Some("AskUserQuestion") => Some(AttentionKind::Question),
                Some("ExitPlanMode") => Some(AttentionKind::Plan),
                _ => None,
            },
        },
        AgentEvent::PostToolUse | AgentEvent::UserPromptSubmit | AgentEvent::SessionEnd => Change {
            clear: true,
            raise: None,
        },
        AgentEvent::Stop => Change {
            clear: true,
            raise: Some(AttentionKind::AgentDone),
        },
    }
}

/// Ends what an agent had raised on `pane` and raises `raise`, which is how a loop ends its turn
/// (REQ-909, REQ-910).
pub(crate) fn settle(core: &mut Core, pane: &str, raise: Option<AttentionKind>) {
    let own = core
        .model
        .workspaces
        .iter_mut()
        .flat_map(|w| w.panes.iter_mut())
        .find(|p| p.id == pane);
    if let Some(p) = own {
        p.attention.retain(|o| !KINDS.contains(&o.kind));
        if let Some(kind) = raise {
            p.attention.push(Own {
                kind,
                exit_code: None,
                rev: None,
            });
        }
    }
}

pub fn event(
    shared: &Shared,
    pane: &Id,
    event: AgentEvent,
    notification_type: Option<&str>,
    tool: Option<&str>,
) -> Done {
    // A loop that is on for this pane decides what the end of its turn is (REQ-905).
    if event == AgentEvent::Stop
        && let Some(done) = crate::tasklists::on_stop(shared, pane)
    {
        return done;
    }
    let mut core = shared.lock();
    // A conversation's attention is the engine's own events' (REQ-913): its hooks only answer a loop.
    if core.convs.contains_key(pane) {
        return Ok(None);
    }
    let p = core
        .model
        .workspaces
        .iter_mut()
        .flat_map(|w| w.panes.iter_mut())
        .find(|p| p.id == *pane)
        .ok_or_else(|| err(ErrorCode::NotFound, format!("no pane {pane}")))?;
    let c = change(event, notification_type, tool);
    if c.clear {
        p.attention.retain(|o| !KINDS.contains(&o.kind));
    }
    if let Some(kind) = c.raise {
        let has = |k: AttentionKind| p.attention.iter().any(|o| o.kind == k);
        // A plan or a question is also asked for permission; the more specific one says it.
        let covered = kind == AttentionKind::Permission
            && (has(AttentionKind::Plan) || has(AttentionKind::Question));
        if !covered && !has(kind) {
            p.attention.push(Own {
                kind,
                exit_code: None,
                rev: None,
            });
        }
    }
    core.task_look_pane(pane);
    core.refresh();
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_hook_raises_or_ends_what_it_should() {
        let raises = |e, n, t| change(e, n, t).raise;
        assert_eq!(
            raises(AgentEvent::Notification, Some("permission_prompt"), None),
            Some(AttentionKind::Permission)
        );
        assert_eq!(
            raises(AgentEvent::Notification, Some("elicitation_dialog"), None),
            Some(AttentionKind::Question)
        );
        assert_eq!(
            raises(AgentEvent::Notification, Some("idle_prompt"), None),
            None,
            "an agent idle for a minute is not news after its Stop"
        );
        assert_eq!(
            raises(AgentEvent::PreToolUse, None, Some("ExitPlanMode")),
            Some(AttentionKind::Plan)
        );
        assert_eq!(
            raises(AgentEvent::PreToolUse, None, Some("AskUserQuestion")),
            Some(AttentionKind::Question)
        );
        assert_eq!(raises(AgentEvent::PreToolUse, None, Some("Bash")), None);
        assert_eq!(
            raises(AgentEvent::Stop, None, None),
            Some(AttentionKind::AgentDone)
        );
        assert!(!change(AgentEvent::Notification, None, None).clear);
        for e in [
            AgentEvent::PreToolUse,
            AgentEvent::PostToolUse,
            AgentEvent::UserPromptSubmit,
            AgentEvent::Stop,
            AgentEvent::SessionEnd,
        ] {
            assert!(
                change(e, None, None).clear,
                "{e:?} means the agent moved on"
            );
        }
    }
}
