use super::{
    BTreeMap, BlockingClient, BufRead, BufReader, Client, CreateRequest, CreateResponse,
    DebugChatResponse, Deserialize, DownloadCancellation, DownloadPhase, DownloadProgress,
    GenerationCompletion, GenerationEvent, HashMap, Instant, MAX_CONTROL_ATTEMPTS,
    MAX_RESPONSE_BYTES, MAX_STREAM_LINE_BYTES, ModelManifest, ModelOutputDecoder, Mutex,
    OllamaChatRequest, OllamaError, OllamaGenerateChunk, OllamaGenerateRequest, OllamaProvider,
    OllamaProviderConfig, OllamaStreamChunk, Operation, PULL_DEADLINE, Path, PullRequest,
    PullResponse, Serialize, StatusCode, TagsResponse, VerifiedOllamaModel, VersionResponse,
    classify_status, complete_bundle_bytes, emit_decoder_events, fs, normalize_digest,
    version_at_least,
};

impl OllamaProvider {
    /// Startup policy retained across supervised provider restarts.
    #[must_use]
    pub const fn qwen_tuning_policy(&self) -> super::QwenTuningPolicy {
        self.config.tuning_policy
    }

    /// Builds the bounded clients for the explicit loopback configuration.
    ///
    /// # Errors
    ///
    /// Returns a sanitized availability error if HTTP clients cannot be built.
    pub fn new(config: OllamaProviderConfig) -> Result<Self, OllamaError> {
        let control_client = BlockingClient::build(
            Client::builder()
                .connect_timeout(config.connect_timeout)
                .timeout(config.control_timeout),
        )
        .map_err(|()| OllamaError::Unavailable)?;
        let pull_client = BlockingClient::build(
            Client::builder()
                .connect_timeout(config.connect_timeout)
                .timeout(config.pull_timeout),
        )
        .map_err(|()| OllamaError::Unavailable)?;
        Ok(Self {
            config,
            control_client,
            pull_client,
            operation: Mutex::new(()),
        })
    }

    /// Checks whether the configured loopback provider answers its bounded
    /// control endpoint. This does not inspect, pull, or mutate any model.
    ///
    /// # Errors
    ///
    /// Returns a sanitized availability or response error.
    pub fn check_available(&self) -> Result<(), OllamaError> {
        let _: VersionResponse = self.get_json(Operation::Version)?;
        Ok(())
    }

    /// Checks compatibility and exact tag digest. This method never pulls or mutates.
    ///
    /// # Errors
    ///
    /// Returns a sanitized compatibility, availability, or model identity error.
    pub fn verify_model(&self, model: &ModelManifest) -> Result<VerifiedOllamaModel, OllamaError> {
        if !model.supports_current_runtime() {
            return Err(OllamaError::InvalidConfiguration);
        }
        let distribution = model
            .ollama_distribution()
            .ok_or(OllamaError::InvalidConfiguration)?;
        let version: VersionResponse = self.get_json(Operation::Version)?;
        if !version_at_least(&version.version, model.backend_compatibility_version())
            || (model.is_mlx() && version.version != crate::PINNED_OLLAMA_VERSION)
        {
            return Err(OllamaError::IncompatibleServer);
        }
        let tags: TagsResponse = self.get_json(Operation::Tags)?;
        let expected_digest = format!("sha256:{}", distribution.manifest_sha256());
        let matched = tags.models.iter().any(|candidate| {
            candidate.name == distribution.tag()
                && normalize_digest(&candidate.digest) == normalize_digest(&expected_digest)
        });
        if !matched {
            return Err(OllamaError::ModelUnavailable);
        }
        Ok(VerifiedOllamaModel {
            tag: distribution.tag().to_owned(),
            manifest_sha256: distribution.manifest_sha256().to_owned(),
            provider_version: version.version,
            tuning_policy: self.config.tuning_policy,
        })
    }

    /// Verifies an application-created local tag and returns its manifest identity.
    ///
    /// # Errors
    ///
    /// Returns a sanitized request, provider-availability, response, or missing-model error.
    pub fn verify_registered_tag(&self, tag: &str) -> Result<VerifiedOllamaModel, OllamaError> {
        if tag.is_empty() || tag.len() > 200 {
            return Err(OllamaError::InvalidRequest);
        }
        let version: VersionResponse = self.get_json(Operation::Version)?;
        let tags: TagsResponse = self.get_json(Operation::Tags)?;
        let candidate = tags
            .models
            .iter()
            .find(|candidate| candidate.name == tag)
            .ok_or(OllamaError::ModelUnavailable)?;
        let digest = normalize_digest(&candidate.digest);
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(OllamaError::InvalidResponse);
        }
        Ok(VerifiedOllamaModel {
            tag: tag.to_owned(),
            manifest_sha256: digest.to_owned(),
            provider_version: version.version,
            tuning_policy: self.config.tuning_policy,
        })
    }

    /// Creates an Ollama manifest from digest-addressed GGUF blobs already in
    /// the private provider store.
    ///
    /// # Errors
    ///
    /// Returns a sanitized request, provider-availability, response, or verification error.
    pub fn create_from_blobs(
        &self,
        tag: &str,
        files: BTreeMap<String, String>,
    ) -> Result<VerifiedOllamaModel, OllamaError> {
        if tag.is_empty()
            || tag.len() > 200
            || files.is_empty()
            || files.values().any(|digest| {
                let digest = normalize_digest(digest);
                digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(OllamaError::InvalidRequest);
        }
        let request = CreateRequest {
            model: tag,
            stream: false,
            files,
            template: "{{ .Prompt }}",
            parser: "glimmer",
        };
        let response = self
            .pull_client
            .post(self.url(Operation::Create))
            .json(&request)
            .send()
            .map_err(|_| OllamaError::Unavailable)?;
        if !response.status().is_success() {
            return Err(classify_status(response.status()));
        }
        let bytes = response.bytes().map_err(|_| OllamaError::InvalidResponse)?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(OllamaError::InvalidResponse);
        }
        let created: CreateResponse =
            serde_json::from_slice(&bytes).map_err(|_| OllamaError::InvalidResponse)?;
        if created.status != "success" {
            return Err(OllamaError::InvalidResponse);
        }
        self.verify_registered_tag(tag)
    }

    /// Publishes one already digest-verified file to Ollama's content-addressed
    /// store when the blob is not present. Bytes stream from disk and are never
    /// buffered as a complete model in memory.
    ///
    /// # Errors
    ///
    /// Returns a sanitized request, filesystem, provider, or response error.
    pub fn ensure_blob(&self, path: &Path, sha256: &str) -> Result<(), OllamaError> {
        let digest = normalize_digest(sha256);
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(OllamaError::InvalidRequest);
        }
        let metadata = fs::symlink_metadata(path).map_err(|_| OllamaError::InvalidRequest)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(OllamaError::InvalidRequest);
        }
        let url = format!("http://{}/api/blobs/sha256:{digest}", self.config.address);
        let head = self
            .control_client
            .head(&url)
            .send()
            .map_err(|_| OllamaError::Unavailable)?;
        if head.status().is_success() {
            return Ok(());
        }
        if head.status() != StatusCode::NOT_FOUND {
            return Err(classify_status(head.status()));
        }
        let file = fs::File::open(path).map_err(|_| OllamaError::InvalidRequest)?;
        let response = self
            .pull_client
            .post(url)
            .body(reqwest::blocking::Body::new(file))
            .send()
            .map_err(|_| OllamaError::Unavailable)?;
        if !response.status().is_success() {
            return Err(classify_status(response.status()));
        }
        Ok(())
    }

    /// User-initiated pull followed by exact compatibility and digest verification.
    ///
    /// # Errors
    ///
    /// Returns a sanitized cancellation, deadline, transport, or verification error.
    pub fn pull_and_verify(
        &self,
        model: &ModelManifest,
        cancellation: &DownloadCancellation,
        mut progress: impl FnMut(DownloadProgress),
    ) -> Result<VerifiedOllamaModel, OllamaError> {
        if !model.supports_current_runtime() {
            return Err(OllamaError::InvalidConfiguration);
        }
        if cancellation.is_cancelled() {
            return Err(OllamaError::Cancelled);
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| OllamaError::Unavailable)?;
        let distribution = model
            .ollama_distribution()
            .ok_or(OllamaError::InvalidConfiguration)?;
        let total = complete_bundle_bytes(model)?;
        let expected_layers = if let Some(blobs) = distribution.tensor_blobs() {
            blobs
                .iter()
                .map(|blob| (blob.sha256().to_owned(), blob.bytes()))
                .collect::<HashMap<_, _>>()
        } else {
            std::iter::once(model.artifact())
                .chain(
                    model
                        .companions()
                        .iter()
                        .map(crate::CompanionArtifact::artifact),
                )
                .map(|artifact| (artifact.sha256().to_owned(), artifact.installed_bytes()))
                .collect::<HashMap<_, _>>()
        };
        let mut layer_progress = HashMap::<String, u64>::new();
        progress(DownloadProgress::provider(
            0,
            total,
            DownloadPhase::Downloading,
        ));
        let response = self
            .pull_client
            .post(self.url(Operation::Pull))
            .json(&PullRequest {
                model: distribution.tag(),
                stream: true,
                insecure: false,
            })
            .send()
            .map_err(|_| OllamaError::Unavailable)?;
        if !response.status().is_success() {
            return Err(classify_status(response.status()));
        }
        let deadline = Instant::now() + PULL_DEADLINE;
        let mut reader = BufReader::new(response);
        let mut line = Vec::new();
        loop {
            if cancellation.is_cancelled() {
                return Err(OllamaError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(OllamaError::DeadlineExceeded);
            }
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if line.len() > MAX_STREAM_LINE_BYTES {
                        return Err(OllamaError::InvalidResponse);
                    }
                    let event: PullResponse =
                        serde_json::from_slice(&line).map_err(|_| OllamaError::InvalidResponse)?;
                    if let (Some(digest), Some(completed)) = (event.digest, event.completed)
                        && let Some(expected) = expected_layers.get(normalize_digest(&digest))
                    {
                        layer_progress.insert(
                            normalize_digest(&digest).to_owned(),
                            completed.min(*expected),
                        );
                        let aggregate = layer_progress.values().copied().sum::<u64>().min(total);
                        progress(DownloadProgress::provider(
                            aggregate,
                            total,
                            DownloadPhase::Downloading,
                        ));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => return Err(OllamaError::Unavailable),
            }
        }
        progress(DownloadProgress::provider(
            total,
            total,
            DownloadPhase::Verifying,
        ));
        let verified = self.verify_model(model)?;
        progress(DownloadProgress::provider(
            total,
            total,
            DownloadPhase::Installing,
        ));
        Ok(verified)
    }

    /// Streams typed NDJSON chunks with cancellation and one total deadline.
    ///
    /// # Errors
    ///
    /// Returns a sanitized cancellation, deadline, transport, or response error.
    pub fn stream_chat(
        &self,
        request: &OllamaChatRequest,
        cancellation: &DownloadCancellation,
        mut on_chunk: impl FnMut(OllamaStreamChunk),
    ) -> Result<(), OllamaError> {
        let mut invalid_response = false;
        let result = self.stream_chat_bytes(request, cancellation, |line| {
            let Ok(chunk) = serde_json::from_slice(line) else {
                invalid_response = true;
                return false;
            };
            on_chunk(chunk);
            true
        });
        if invalid_response {
            Err(OllamaError::InvalidResponse)
        } else {
            result
        }
    }

    /// Returns Ollama's exact native rendered prompt without running inference.
    ///
    /// # Errors
    ///
    /// Returns a sanitized provider, status, size, or response-shape error.
    pub fn render_chat_prompt(&self, request: &OllamaChatRequest) -> Result<String, OllamaError> {
        self.render_chat_prompt_with_cancellation(request, || false)
    }

    /// Renders without inference, interrupting pending HTTP I/O when cancelled.
    ///
    /// # Errors
    /// Returns cancellation, deadline, transport or bounded response errors.
    pub fn render_chat_prompt_with_cancellation(
        &self,
        request: &OllamaChatRequest,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<String, OllamaError> {
        let request = request.render_only();
        let mut bytes = Vec::new();
        super::stream::receive(
            self.config,
            self.url(Operation::Chat),
            serde_json::to_vec(&request).map_err(|_| OllamaError::InvalidRequest)?,
            is_cancelled,
            |chunk| {
                if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                    return Err(OllamaError::InvalidResponse);
                }
                bytes.extend_from_slice(chunk);
                Ok(true)
            },
        )?;
        let rendered: DebugChatResponse =
            serde_json::from_slice(&bytes).map_err(|_| OllamaError::InvalidResponse)?;
        if rendered.debug_info.rendered_template.is_empty() {
            return Err(OllamaError::InvalidResponse);
        }
        Ok(rendered.debug_info.rendered_template)
    }

    /// Streams bounded raw NDJSON records without interpreting model output.
    ///
    /// Returning `false` from `on_bytes` stops the response after the current
    /// record. Each record includes its transport newline when one was sent.
    ///
    /// # Errors
    ///
    /// Returns a sanitized cancellation, deadline, transport, or size error.
    pub fn stream_chat_bytes(
        &self,
        request: &OllamaChatRequest,
        cancellation: &DownloadCancellation,
        on_bytes: impl FnMut(&[u8]) -> bool,
    ) -> Result<(), OllamaError> {
        self.stream_bytes(Operation::Chat, request, cancellation, on_bytes)
    }

    /// Maps raw Ollama records and transport termination into one decoder lifecycle.
    ///
    /// # Errors
    ///
    /// Returns the sanitized provider error after first emitting the decoder's
    /// corresponding failed or cancelled terminal event.
    pub fn stream_chat_events(
        &self,
        request: &OllamaChatRequest,
        cancellation: &DownloadCancellation,
        decoder: &mut impl ModelOutputDecoder,
        mut on_event: impl FnMut(GenerationEvent),
    ) -> Result<(), OllamaError> {
        let mut output_started = false;
        let result = self.stream_chat_bytes(request, cancellation, |bytes| {
            if !output_started {
                output_started = true;
                on_event(GenerationEvent::OutputStarted);
            }
            emit_decoder_events(decoder.push(bytes), &mut on_event)
        });
        let terminal_events = match result {
            Ok(()) => decoder.finish(GenerationCompletion::Stop),
            Err(OllamaError::Cancelled) => decoder.cancel(),
            Err(_) => decoder.fail_upstream(),
        };
        emit_decoder_events(terminal_events, &mut on_event);
        result
    }

    /// Streams raw generation response fragments through an adapter-owned decoder.
    ///
    /// # Errors
    ///
    /// Returns a sanitized cancellation, deadline, transport, or response error
    /// after completing the decoder lifecycle.
    pub fn stream_generate_events(
        &self,
        request: &OllamaGenerateRequest,
        cancellation: &DownloadCancellation,
        decoder: &mut impl ModelOutputDecoder,
        mut on_event: impl FnMut(GenerationEvent),
    ) -> Result<(), OllamaError> {
        let mut completion = GenerationCompletion::Stop;
        let mut invalid_response = false;
        let mut output_started = false;
        let result = self.stream_bytes(Operation::Generate, request, cancellation, |bytes| {
            if !output_started {
                output_started = true;
                on_event(GenerationEvent::OutputStarted);
            }
            let Ok(chunk) = serde_json::from_slice::<OllamaGenerateChunk>(bytes) else {
                invalid_response = true;
                return false;
            };
            if chunk.done && chunk.done_reason == "length" {
                completion = GenerationCompletion::Length;
            }
            emit_decoder_events(decoder.push(chunk.response.as_bytes()), &mut on_event)
        });
        let result = if invalid_response {
            Err(OllamaError::InvalidResponse)
        } else {
            result
        };
        let terminal_events = match result {
            Ok(()) => decoder.finish(completion),
            Err(OllamaError::Cancelled) => decoder.cancel(),
            Err(_) => decoder.fail_upstream(),
        };
        emit_decoder_events(terminal_events, &mut on_event);
        result
    }

    fn stream_bytes<T: Serialize>(
        &self,
        operation: Operation,
        request: &T,
        cancellation: &DownloadCancellation,
        mut on_bytes: impl FnMut(&[u8]) -> bool,
    ) -> Result<(), OllamaError> {
        let mut line = Vec::new();
        let mut stopped = false;
        super::stream::receive(
            self.config,
            self.url(operation),
            serde_json::to_vec(request).map_err(|_| OllamaError::InvalidRequest)?,
            || cancellation.is_cancelled(),
            |chunk| {
                for piece in chunk.split_inclusive(|byte| *byte == b'\n') {
                    if line.len().saturating_add(piece.len()) > MAX_STREAM_LINE_BYTES {
                        return Err(OllamaError::InvalidResponse);
                    }
                    line.extend_from_slice(piece);
                    if piece.last() == Some(&b'\n') {
                        if !on_bytes(&line) {
                            stopped = true;
                            return Ok(false);
                        }
                        line.clear();
                    }
                }
                Ok(true)
            },
        )?;
        if !stopped && !line.is_empty() {
            on_bytes(&line);
        }
        Ok(())
    }

    fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        operation: Operation,
    ) -> Result<T, OllamaError> {
        for attempt in 0..MAX_CONTROL_ATTEMPTS {
            let result = self.control_client.get(self.url(operation)).send();
            match result {
                Ok(response) => {
                    if !response.status().is_success() {
                        return Err(classify_status(response.status()));
                    }
                    let bytes = response.bytes().map_err(|_| OllamaError::InvalidResponse)?;
                    if bytes.len() > MAX_RESPONSE_BYTES {
                        return Err(OllamaError::InvalidResponse);
                    }
                    return serde_json::from_slice(&bytes)
                        .map_err(|_| OllamaError::InvalidResponse);
                }
                Err(_) if attempt + 1 < MAX_CONTROL_ATTEMPTS => {}
                Err(_) => return Err(OllamaError::Unavailable),
            }
        }
        Err(OllamaError::Unavailable)
    }

    fn url(&self, operation: Operation) -> String {
        format!("http://{}{}", self.config.address, operation.path())
    }
}
