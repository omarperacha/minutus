//! Deduplicate request/round model-pass snapshots at their persistence boundary.
use ipc_types::{Conversation, MessagePart, TaskActivity, TaskActivityKind};
use std::collections::BTreeSet;

pub(super) fn unpersisted(
    conversation: &Conversation,
    model_passes: &[TaskActivity],
) -> Vec<MessagePart> {
    let persisted = conversation
        .messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter_map(|part| match part {
            MessagePart::Activity { activity } if activity.kind == TaskActivityKind::ModelPass => {
                Some(activity.id.as_str())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    model_passes
        .iter()
        .filter(|activity| !persisted.contains(activity.id.as_str()))
        .cloned()
        .map(|activity| MessagePart::Activity { activity })
        .collect()
}
