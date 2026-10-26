//! Verified model loading and bounded local inference lifecycle control.

mod client;
mod error;
mod memory;
mod types;
pub(crate) use types::validate_normalized_image_bytes;
mod verify;

#[cfg(test)]
mod tests;

pub use error::InferenceError;
pub use types::{
    ActiveInferenceProfile, CompanionLoadConfig, GLIMMER_IMAGE_TOKENS, GenerationRequest,
    InferenceRuntimeStatus, MAX_GENERATION_IMAGES, MemoryPressureLevel, ModelLoadConfig,
    PINNED_LLAMA_CPP_VERSION,
};

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU8, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use client::{HealthStatus, ServerClient};
use verify::{VerifiedLaunch, verify};

use crate::{
    GenerationBinding, GenerationEvent, InferenceProvider, LlamaSidecar, LlamaSidecarConfig,
    ModelOutputDecoder, SidecarEndpoint, SidecarStatus, generation::ActiveModelDecoder,
};

const MAX_PROMPT_BYTES: usize = 4 * 1024 * 1024;
const LOAD_TIMEOUT: Duration = Duration::from_mins(3);
const HEALTH_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const MEMORY_POLL_INTERVAL: Duration = Duration::from_secs(2);
const RESTART_WINDOW: Duration = Duration::from_mins(1);
const MAX_RESTARTS: usize = 3;

#[derive(Clone)]
struct LoadedModel {
    config: ModelLoadConfig,
    endpoint: SidecarEndpoint,
    context_tokens: u32,
    output_tokens: u32,
    image_tokens: Option<u32>,
    binding: GenerationBinding,
}

struct ActiveGeneration {
    id: String,
    cancellation: Arc<AtomicU8>,
}

#[derive(Default)]
struct RuntimeState {
    loaded: Option<LoadedModel>,
    active: Option<ActiveGeneration>,
    failed: bool,
    restart_attempts: VecDeque<Instant>,
    pressure: Option<MemoryPressureLevel>,
}

/// Single-model, single-generation controller for local `llama-server` inference.
pub struct LocalInferenceRuntime {
    sidecar: LlamaSidecar,
    client: ServerClient,
    state: Mutex<RuntimeState>,
    lifecycle: Mutex<()>,
}

impl LocalInferenceRuntime {
    /// Creates an unloaded runtime.
    ///
    /// # Errors
    ///
    /// Returns an error if the bounded loopback HTTP clients cannot be created.
    pub fn new() -> Result<Self, InferenceError> {
        Ok(Self {
            sidecar: LlamaSidecar::new(),
            client: ServerClient::new()?,
            state: Mutex::new(RuntimeState::default()),
            lifecycle: Mutex::new(()),
        })
    }

    /// Fully hashes, inspects and loads one managed model.
    ///
    /// Existing model state is unloaded first. The model is not marked loaded
    /// until its digest, GGUF compatibility, limits and helper health all pass.
    ///
    /// # Errors
    ///
    /// Returns a typed error for integrity, compatibility, pressure, launch or
    /// readiness failures.
    pub fn load_model(&self, config: ModelLoadConfig) -> Result<(), InferenceError> {
        let _lifecycle = self.lock_lifecycle()?;
        if self.lock_state()?.active.is_some() {
            return Err(InferenceError::GenerationAlreadyActive);
        }
        self.unload_locked()?;
        if memory::system_pressure() == MemoryPressureLevel::Critical {
            return Err(InferenceError::MemoryPressure);
        }
        let verified = verify(&config)?;
        let endpoint = self.start_verified(&verified)?;
        let multimodal = verified
            .companions
            .iter()
            .any(|companion| companion.kind == crate::CompanionKind::VisionProjector);
        let execution_profile = config.execution_profile.clone();
        let capabilities = execution_profile.as_ref().map_or_else(
            || crate::ModelCapabilities::glimmer_runtime(verified.context_tokens, multimodal),
            |profile| profile.capabilities.clone(),
        );
        let binding = GenerationBinding::glimmer_llama_cpp(
            execution_profile.as_ref().map_or_else(
                || format!("imported:{}", config.expected_sha256),
                |profile| profile.model_id.clone(),
            ),
            execution_profile.as_ref().map_or_else(
                || config.expected_sha256.clone(),
                |profile| profile.model_version.clone(),
            ),
            config.expected_sha256.clone(),
            types::PINNED_LLAMA_CPP_VERSION,
            capabilities,
        );
        let mut state = self.lock_state()?;
        state.loaded = Some(LoadedModel {
            config,
            endpoint,
            context_tokens: verified.context_tokens,
            output_tokens: verified.output_tokens,
            image_tokens: multimodal.then_some(GLIMMER_IMAGE_TOKENS),
            binding,
        });
        state.failed = false;
        state.restart_attempts.clear();
        state.pressure = Some(MemoryPressureLevel::Normal);
        Ok(())
    }

    /// Streams one generation into model-neutral events.
    ///
    /// Prompt tokenization is performed by the authenticated local helper
    /// before generation, and Rust rejects any request exceeding the loaded
    /// context or output limits.
    ///
    /// # Errors
    ///
    /// Returns an error if the request cannot start or if its upstream stream
    /// fails. Once accepted, exactly one terminal parser event is emitted even
    /// when the returned result is an error.
    pub fn start_generation(
        &self,
        request: &GenerationRequest,
        mut emit: impl FnMut(GenerationEvent),
    ) -> Result<(), InferenceError> {
        let (mut parser, loaded, cancellation) = self.prepare_generation(request)?;

        let mut parser_terminal = false;
        let mut parser_failed = false;
        let mut output_started = false;
        let mut last_memory_poll = Instant::now();
        let result = self.client.stream(
            &loaded.endpoint,
            request,
            || {
                if cancel_reason(&cancellation) != CancelReason::None {
                    return Err(InferenceError::GenerationInterrupted);
                }
                if last_memory_poll.elapsed() >= MEMORY_POLL_INTERVAL {
                    last_memory_poll = Instant::now();
                    let pressure = memory::system_pressure();
                    if pressure != MemoryPressureLevel::Normal {
                        self.handle_memory_pressure(pressure)?;
                        return Err(InferenceError::GenerationInterrupted);
                    }
                }
                Ok(())
            },
            |bytes| {
                if !output_started {
                    output_started = true;
                    emit(GenerationEvent::OutputStarted);
                }
                for event in parser.push(bytes) {
                    parser_terminal |= is_terminal(&event);
                    parser_failed |= matches!(event, GenerationEvent::Failed { .. });
                    emit(event);
                }
                if parser_terminal {
                    Err(InferenceError::MalformedModelOutput)
                } else {
                    Ok(())
                }
            },
        );

        if let Ok(completed) = &result
            && let Some(metrics) = completed.metrics
        {
            emit(GenerationEvent::Metrics { metrics });
        }

        let cancellation_reason = cancel_reason(&cancellation);
        if !parser_terminal {
            let terminal_events = match (&result, cancellation_reason) {
                (_, CancelReason::User | CancelReason::Unload) => parser.cancel(),
                (_, CancelReason::Memory) => parser.fail_memory_pressure(),
                (Ok(completed), CancelReason::None) => parser.finish(completed.completion),
                (Err(_), CancelReason::None) => parser.fail_upstream(),
            };
            for event in terminal_events {
                parser_terminal |= is_terminal(&event);
                parser_failed |= matches!(event, GenerationEvent::Failed { .. });
                emit(event);
            }
        }
        self.clear_generation(&request.id)?;

        match cancellation_reason {
            CancelReason::User | CancelReason::Unload => Err(InferenceError::Cancelled),
            CancelReason::Memory => Err(InferenceError::MemoryPressure),
            CancelReason::None => match result {
                Ok(_) if parser_terminal && !parser_failed => Ok(()),
                Ok(_) => Err(InferenceError::MalformedModelOutput),
                Err(error) => {
                    self.refresh_failure_state()?;
                    Err(error)
                }
            },
        }
    }

    /// Returns exact verified identity and limits for the idle loaded model.
    ///
    /// # Errors
    ///
    /// Returns an error when no model is loaded, generation is active or runtime
    /// state is unavailable.
    pub fn active_profile(&self) -> Result<ActiveInferenceProfile, InferenceError> {
        self.refresh_failure_state()?;
        let state = self.lock_state()?;
        if state.active.is_some() {
            return Err(InferenceError::GenerationAlreadyActive);
        }
        let loaded = state
            .loaded
            .as_ref()
            .ok_or(InferenceError::ModelNotLoaded)?;
        Ok(ActiveInferenceProfile {
            binding: loaded.binding.clone(),
            context_tokens: loaded.context_tokens,
            output_tokens: loaded.output_tokens,
            image_tokens: loaded.image_tokens,
        })
    }

    /// Tokenizes one complete rendered prompt with the authenticated active helper.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid input, unavailable model state, active
    /// generation or helper failure.
    pub fn tokenize_prompt(
        &self,
        binding: &GenerationBinding,
        prompt: &str,
    ) -> Result<u64, InferenceError> {
        if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
            return Err(InferenceError::InvalidPrompt);
        }
        self.refresh_failure_state()?;
        let endpoint = {
            let state = self.lock_state()?;
            if state.active.is_some() {
                return Err(InferenceError::GenerationAlreadyActive);
            }
            let loaded = state
                .loaded
                .as_ref()
                .ok_or(InferenceError::ModelNotLoaded)?;
            if &loaded.binding != binding {
                return Err(InferenceError::ProfileBindingMismatch);
            }
            loaded.endpoint.clone()
        };
        u64::try_from(self.client.tokenize(&endpoint, prompt)?)
            .map_err(|_| InferenceError::ContextLimitExceeded)
    }

    fn prepare_generation(
        &self,
        request: &GenerationRequest,
    ) -> Result<(ActiveModelDecoder, LoadedModel, Arc<AtomicU8>), InferenceError> {
        if request.prompt.is_empty() || request.prompt.len() > MAX_PROMPT_BYTES {
            return Err(InferenceError::InvalidPrompt);
        }
        types::validate_images(&request.prompt, &request.images)?;
        let system_pressure = memory::system_pressure();
        if system_pressure != MemoryPressureLevel::Normal {
            self.handle_memory_pressure(system_pressure)?;
            return Err(InferenceError::MemoryPressure);
        }
        self.refresh_failure_state()?;
        let loaded = {
            let state = self.lock_state()?;
            if state.failed {
                return Err(InferenceError::RestartRequired);
            }
            if state.pressure.unwrap_or(MemoryPressureLevel::Normal) != MemoryPressureLevel::Normal
            {
                return Err(InferenceError::MemoryPressure);
            }
            if state.active.is_some() {
                return Err(InferenceError::GenerationAlreadyActive);
            }
            state.loaded.clone().ok_or(InferenceError::ModelNotLoaded)?
        };
        if request.binding != loaded.binding {
            return Err(InferenceError::ProfileBindingMismatch);
        }
        let parser = ActiveModelDecoder::new(loaded.binding.adapter(), &request.id)
            .map_err(InferenceError::Adapter)?;
        if request.output_tokens == 0 || request.output_tokens > loaded.output_tokens {
            return Err(InferenceError::OutputLimitExceeded);
        }
        let prompt_tokens = self.client.tokenize(&loaded.endpoint, &request.prompt)?;
        if !request.images.is_empty() && loaded.image_tokens.is_none() {
            return Err(InferenceError::MultimodalNotLoaded);
        }
        let image_tokens = usize::try_from(loaded.image_tokens.unwrap_or(0))
            .map_err(|_| InferenceError::ContextLimitExceeded)?
            .checked_mul(request.images.len())
            .ok_or(InferenceError::ContextLimitExceeded)?;
        let requested_output = usize::try_from(request.output_tokens)
            .map_err(|_| InferenceError::OutputLimitExceeded)?;
        let context_tokens = usize::try_from(loaded.context_tokens)
            .map_err(|_| InferenceError::ContextLimitExceeded)?;
        if prompt_tokens
            .checked_add(image_tokens)
            .ok_or(InferenceError::ContextLimitExceeded)?
            .checked_add(requested_output)
            .is_none_or(|total| total > context_tokens)
        {
            return Err(InferenceError::ContextLimitExceeded);
        }
        let cancellation = Arc::new(AtomicU8::new(CancelReason::None as u8));
        let mut state = self.lock_state()?;
        if state.active.is_some() {
            return Err(InferenceError::GenerationAlreadyActive);
        }
        state.active = Some(ActiveGeneration {
            id: request.id.clone(),
            cancellation: Arc::clone(&cancellation),
        });
        drop(state);
        Ok((parser, loaded, cancellation))
    }

    /// Actively stops the matching server-side generation session.
    ///
    /// # Errors
    ///
    /// Returns an error if controller state is unavailable or cancellation
    /// cannot reach the helper. A failed cancellation force-stops the helper.
    pub fn stop_generation(&self, generation_id: &str) -> Result<bool, InferenceError> {
        let target = self.mark_cancelled(generation_id, CancelReason::User)?;
        let Some((endpoint, id)) = target else {
            return Ok(false);
        };
        if let Err(error) = self.client.cancel(&endpoint, &id) {
            let _ = self.sidecar.stop();
            if let Ok(mut state) = self.state.lock() {
                state.failed = true;
            }
            return Err(error);
        }
        Ok(true)
    }

    /// Cancels active work and terminates the helper to release model memory.
    ///
    /// # Errors
    ///
    /// Returns an error if lifecycle state or helper shutdown is unavailable.
    pub fn unload_model(&self) -> Result<(), InferenceError> {
        let _lifecycle = self.lock_lifecycle()?;
        self.unload_locked()
    }

    /// Restarts an unexpectedly failed helper, bounded to three attempts/minute.
    ///
    /// The managed artifact is fully reverified before every restart.
    ///
    /// # Errors
    ///
    /// Returns an error when no failed load exists, the restart bound is
    /// exhausted, or reverification/relaunch fails.
    pub fn restart_after_failure(&self) -> Result<(), InferenceError> {
        let _lifecycle = self.lock_lifecycle()?;
        self.refresh_failure_state()?;
        let (config, binding) = {
            let mut state = self.lock_state()?;
            if !state.failed || state.active.is_some() {
                return Err(InferenceError::RestartNotAvailable);
            }
            reserve_restart(&mut state.restart_attempts, Instant::now())?;
            state
                .loaded
                .as_ref()
                .map(|loaded| (loaded.config.clone(), loaded.binding.clone()))
                .ok_or(InferenceError::RestartNotAvailable)?
        };
        self.sidecar.stop().map_err(InferenceError::Sidecar)?;
        let verified = verify(&config)?;
        let endpoint = self.start_verified(&verified)?;
        let mut state = self.lock_state()?;
        state.loaded = Some(LoadedModel {
            config,
            endpoint,
            context_tokens: verified.context_tokens,
            output_tokens: verified.output_tokens,
            image_tokens: verified
                .companions
                .iter()
                .any(|companion| companion.kind == crate::CompanionKind::VisionProjector)
                .then_some(GLIMMER_IMAGE_TOKENS),
            binding,
        });
        state.failed = false;
        Ok(())
    }

    /// Applies an OS/native memory-pressure transition.
    ///
    /// Warning cancels generation but retains the model. Critical pressure
    /// additionally terminates the helper and clears loaded state.
    ///
    /// # Errors
    ///
    /// Returns an error if controller or helper state cannot be changed safely.
    pub fn handle_memory_pressure(&self, level: MemoryPressureLevel) -> Result<(), InferenceError> {
        let target = {
            let mut state = self.lock_state()?;
            state.pressure = Some(level);
            if level == MemoryPressureLevel::Normal {
                return Ok(());
            }
            state.active.as_ref().and_then(|active| {
                active
                    .cancellation
                    .compare_exchange(
                        CancelReason::None as u8,
                        CancelReason::Memory as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .ok()
                    .and_then(|_| {
                        state
                            .loaded
                            .as_ref()
                            .map(|loaded| (loaded.endpoint.clone(), active.id.clone()))
                    })
            })
        };
        if let Some((endpoint, id)) = target {
            let _ = self.client.cancel(&endpoint, &id);
        }
        if level == MemoryPressureLevel::Critical {
            self.sidecar.stop().map_err(InferenceError::Sidecar)?;
            let mut state = self.lock_state()?;
            state.loaded = None;
            state.failed = false;
        }
        Ok(())
    }

    /// Returns current sanitized lifecycle state and detects helper exits.
    ///
    /// # Errors
    ///
    /// Returns an error when process or controller state cannot be inspected.
    pub fn status(&self) -> Result<InferenceRuntimeStatus, InferenceError> {
        self.refresh_failure_state()?;
        let state = self.lock_state()?;
        if state.failed {
            return Ok(InferenceRuntimeStatus::Failed {
                restart_attempts_remaining: MAX_RESTARTS
                    .saturating_sub(state.restart_attempts.len()),
            });
        }
        if let Some(active) = &state.active {
            return Ok(InferenceRuntimeStatus::Generating {
                generation_id: active.id.clone(),
            });
        }
        Ok(state
            .loaded
            .as_ref()
            .map_or(InferenceRuntimeStatus::Unloaded, |loaded| {
                InferenceRuntimeStatus::Loaded {
                    context_tokens: loaded.context_tokens,
                    output_tokens: loaded.output_tokens,
                    multimodal: loaded.image_tokens.is_some(),
                }
            }))
    }

    fn start_verified(&self, verified: &VerifiedLaunch) -> Result<SidecarEndpoint, InferenceError> {
        let mut config = LlamaSidecarConfig::new(&verified.executable, &verified.model)
            .with_context_tokens(verified.context_tokens)
            .map_err(InferenceError::Sidecar)?;
        if let Some(projector) = verified
            .companions
            .iter()
            .find(|companion| companion.kind == crate::CompanionKind::VisionProjector)
        {
            config = config
                .with_multimodal_projector(&projector.path, GLIMMER_IMAGE_TOKENS)
                .map_err(InferenceError::Sidecar)?;
        }
        let endpoint = self
            .sidecar
            .start(&config)
            .map_err(InferenceError::Sidecar)?;
        let deadline = Instant::now() + LOAD_TIMEOUT;
        loop {
            if self.sidecar.status().map_err(InferenceError::Sidecar)? != SidecarStatus::Running {
                return Err(InferenceError::SidecarExited);
            }
            match self.client.health(&endpoint) {
                Ok(HealthStatus::Ready) => return Ok(endpoint),
                Ok(HealthStatus::Loading) | Err(InferenceError::Server(_)) => {}
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                let _ = self.sidecar.stop();
                return Err(InferenceError::LoadTimedOut);
            }
            thread::sleep(HEALTH_RETRY_INTERVAL);
        }
    }

    fn unload_locked(&self) -> Result<(), InferenceError> {
        let target = {
            let state = self.lock_state()?;
            state.active.as_ref().and_then(|active| {
                active
                    .cancellation
                    .compare_exchange(
                        CancelReason::None as u8,
                        CancelReason::Unload as u8,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .ok()
                    .and_then(|_| {
                        state
                            .loaded
                            .as_ref()
                            .map(|loaded| (loaded.endpoint.clone(), active.id.clone()))
                    })
            })
        };
        if let Some((endpoint, id)) = target {
            let _ = self.client.cancel(&endpoint, &id);
        }
        self.sidecar.stop().map_err(InferenceError::Sidecar)?;
        let mut state = self.lock_state()?;
        state.loaded = None;
        state.failed = false;
        state.pressure = Some(MemoryPressureLevel::Normal);
        Ok(())
    }

    fn mark_cancelled(
        &self,
        generation_id: &str,
        reason: CancelReason,
    ) -> Result<Option<(SidecarEndpoint, String)>, InferenceError> {
        let state = self.lock_state()?;
        let Some(active) = state
            .active
            .as_ref()
            .filter(|active| active.id == generation_id)
        else {
            return Ok(None);
        };
        active
            .cancellation
            .compare_exchange(
                CancelReason::None as u8,
                reason as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok();
        Ok(state
            .loaded
            .as_ref()
            .map(|loaded| (loaded.endpoint.clone(), active.id.clone())))
    }

    fn clear_generation(&self, generation_id: &str) -> Result<(), InferenceError> {
        let mut state = self.lock_state()?;
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.id == generation_id)
        {
            state.active = None;
        }
        Ok(())
    }

    fn refresh_failure_state(&self) -> Result<(), InferenceError> {
        if let SidecarStatus::Exited { .. } =
            self.sidecar.status().map_err(InferenceError::Sidecar)?
        {
            let mut state = self.lock_state()?;
            if state.loaded.is_some() {
                state.failed = true;
            }
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, RuntimeState>, InferenceError> {
        self.state
            .lock()
            .map_err(|_| InferenceError::RuntimeUnavailable)
    }

    fn lock_lifecycle(&self) -> Result<MutexGuard<'_, ()>, InferenceError> {
        self.lifecycle
            .lock()
            .map_err(|_| InferenceError::RuntimeUnavailable)
    }
}

impl Drop for LocalInferenceRuntime {
    fn drop(&mut self) {
        let _ = self.sidecar.stop();
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Eq, PartialEq)]
enum CancelReason {
    None = 0,
    User = 1,
    Memory = 2,
    Unload = 3,
}

fn cancel_reason(value: &AtomicU8) -> CancelReason {
    match value.load(Ordering::Acquire) {
        1 => CancelReason::User,
        2 => CancelReason::Memory,
        3 => CancelReason::Unload,
        _ => CancelReason::None,
    }
}

fn reserve_restart(attempts: &mut VecDeque<Instant>, now: Instant) -> Result<(), InferenceError> {
    while attempts
        .front()
        .is_some_and(|attempt| now.duration_since(*attempt) > RESTART_WINDOW)
    {
        attempts.pop_front();
    }
    if attempts.len() >= MAX_RESTARTS {
        return Err(InferenceError::RestartLimitExceeded);
    }
    attempts.push_back(now);
    Ok(())
}

const fn is_terminal(event: &GenerationEvent) -> bool {
    matches!(
        event,
        GenerationEvent::Completed { .. }
            | GenerationEvent::Cancelled
            | GenerationEvent::Failed { .. }
    )
}

impl InferenceProvider for LocalInferenceRuntime {
    fn active_profile(&self) -> Result<ActiveInferenceProfile, InferenceError> {
        Self::active_profile(self)
    }

    fn tokenize_prompt(
        &self,
        binding: &GenerationBinding,
        prompt: &str,
    ) -> Result<u64, InferenceError> {
        Self::tokenize_prompt(self, binding, prompt)
    }

    fn start_generation(
        &self,
        request: &GenerationRequest,
        emit: &mut dyn FnMut(GenerationEvent),
    ) -> Result<(), InferenceError> {
        Self::start_generation(self, request, emit)
    }
}
