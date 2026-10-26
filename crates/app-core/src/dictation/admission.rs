use std::sync::{Arc, Mutex};

use crate::AppCoreError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivityKind {
    Chat,
    Dictation,
}

struct ActiveActivity {
    generation: u64,
    kind: ActivityKind,
}

pub(crate) struct ForegroundActivityGate {
    state: Mutex<Option<ActiveActivity>>,
    next_generation: Mutex<u64>,
}

impl ForegroundActivityGate {
    pub(crate) const fn new() -> Self {
        Self {
            state: Mutex::new(None),
            next_generation: Mutex::new(0),
        }
    }

    fn begin(self: &Arc<Self>, kind: ActivityKind) -> Result<ActivityLease, AppCoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| AppCoreError::DictationUnavailable)?;
        if state.is_some() {
            return Err(AppCoreError::ForegroundActivityBusy);
        }
        let mut next = self
            .next_generation
            .lock()
            .map_err(|_| AppCoreError::DictationUnavailable)?;
        *next = next.saturating_add(1);
        let generation = *next;
        *state = Some(ActiveActivity { generation, kind });
        Ok(ActivityLease {
            gate: Arc::clone(self),
            generation,
            kind,
        })
    }

    pub(super) fn begin_dictation(
        self: &Arc<Self>,
    ) -> Result<DictationActivityLease, AppCoreError> {
        self.begin(ActivityKind::Dictation)
            .map(|lease| DictationActivityLease { _lease: lease })
    }

    pub(crate) fn begin_chat(self: &Arc<Self>) -> Result<ChatActivityLease, AppCoreError> {
        self.begin(ActivityKind::Chat)
            .map(|lease| ChatActivityLease { _lease: lease })
    }
}

struct ActivityLease {
    gate: Arc<ForegroundActivityGate>,
    generation: u64,
    kind: ActivityKind,
}

impl Drop for ActivityLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.gate.state.lock()
            && state.as_ref().is_some_and(|active| {
                active.generation == self.generation && active.kind == self.kind
            })
        {
            *state = None;
        }
    }
}

/// Exclusive admission lease held for the complete chat-generation lifetime.
pub struct ChatActivityLease {
    _lease: ActivityLease,
}

pub(super) struct DictationActivityLease {
    _lease: ActivityLease,
}
