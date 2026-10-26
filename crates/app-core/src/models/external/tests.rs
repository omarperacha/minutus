//! Lifecycle tests use tiny pinned artifacts, never real weights or inference.
use crate::{AppCore, AppCoreError};
use ipc_types::ModelRegistrationState;
use model_runtime::{ExternalModelError, ModelKey};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

struct Fixture {
    directory: tempfile::TempDir,
    core: AppCore,
    key: ModelKey,
    paths: Vec<PathBuf>,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_required_projector(false)
    }
    fn with_required_projector(required: bool) -> Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let mut core = AppCore::initialize(directory.path().join("app"))?;
        core.ollama_models_directory = Some(directory.path().canonicalize()?.join("runtime-store"));
        // Install an idle managed supervisor; no process or model is launched.
        let service = crate::models::ollama_service::OllamaService::bundled(
            model_runtime::OllamaProvider::new(model_runtime::OllamaProviderConfig::loopback(
                "127.0.0.1:11434".parse()?,
            )?)?,
            model_runtime::OllamaSidecar::new(),
            model_runtime::OllamaSidecarConfig::new(
                directory.path().join("unused-runtime"),
                core.ollama_models_directory
                    .as_ref()
                    .ok_or("missing runtime store")?,
            ),
        );
        core.ollama_shutdown = service.shutdown_handle();
        core.ollama_service = std::sync::Mutex::new(service);

        let mut document: serde_json::Value =
            serde_json::from_str(include_str!("../../../../model-runtime/catalogue/v4.json"))?;
        let models = document["models"].as_array_mut().ok_or("models missing")?;
        models.truncate(1);
        let model = &mut models[0];
        model["companions"][0]["required"] = required.into();
        let mut paths = Vec::new();
        let primary = b"GGUF tiny primary fixture";
        let path = directory.path().join("already-owned.gguf");
        std::fs::write(&path, primary)?;
        pin_artifact(&mut model["artifact"], primary);
        paths.push(path);
        for (index, companion) in model["companions"]
            .as_array_mut()
            .ok_or("companions missing")?
            .iter_mut()
            .enumerate()
        {
            let bytes = format!("GGUF tiny companion fixture {index}");
            let path = directory.path().join(format!("companion-{index}.gguf"));
            std::fs::write(&path, bytes.as_bytes())?;
            pin_artifact(companion, bytes.as_bytes());
            paths.push(path);
        }
        core.model_catalogue =
            model_runtime::ModelCatalogue::parse_json(&serde_json::to_string(&document)?)?;
        let key = core.model_catalogue.models()[0].key().clone();
        Ok(Self {
            directory,
            core,
            key,
            paths,
        })
    }
    fn state(&self) -> Result<ModelRegistrationState, AppCoreError> {
        self.core.catalogue_registration_state(&self.key)
    }
    fn register(&self) -> Result<(), AppCoreError> {
        self.core.register_external_model(
            &self.key,
            &self.paths.iter().cloned().map(Some).collect::<Vec<_>>(),
            |_, _| {},
        )
    }
}
fn pin_artifact(artifact: &mut serde_json::Value, bytes: &[u8]) {
    artifact["downloadBytes"] = bytes.len().into();
    artifact["installedBytes"] = bytes.len().into();
    artifact["sha256"] = format!("{:x}", Sha256::digest(bytes)).into();
}

#[test]
fn first_failure_never_creates_registration_and_preserves_originals()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    assert_eq!(fixture.state()?, ModelRegistrationState::NotInstalled);
    std::fs::write(&fixture.paths[1], b"wrong quantisation or revision")?;
    let before = std::fs::read(&fixture.paths[0])?;
    assert!(fixture.register().is_err());
    assert_eq!(fixture.state()?, ModelRegistrationState::NotInstalled);
    assert_eq!(std::fs::read(&fixture.paths[0])?, before);
    Ok(())
}

#[test]
fn missing_and_relocated_files_keep_identity_and_locate_restores_availability()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    fixture.register()?;
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    let original_registration = fixture
        .core
        .external_registration(model)?
        .ok_or("registration missing")?;
    assert_eq!(
        Path::new(&original_registration.files[0].path),
        fixture.paths[0].canonicalize()?
    );
    let moved = fixture.directory.path().join("moved.gguf");
    std::fs::rename(&fixture.paths[0], &moved)?;
    assert_eq!(fixture.state()?, ModelRegistrationState::Unavailable);
    assert!(!fixture.core.catalogue_model_installed(&fixture.key)?);
    let selection = ipc_types::ModelSelection {
        model_id: fixture.key.id().to_owned(),
        model_version: fixture.key.version().to_owned(),
    };
    assert!(matches!(
        fixture.core.validated_selection(&selection),
        Err(AppCoreError::ExternalModel(ExternalModelError::Unavailable))
    ));
    assert!(fixture.core.external_registration(model)?.is_some());
    assert!(fixture.register().is_err());
    assert_eq!(
        fixture.core.external_registration(model)?,
        Some(original_registration)
    );
    fixture.paths[0] = moved;
    fixture.register()?;
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    let before = fixture
        .paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<Vec<_>, _>>()?;
    fixture.core.forget_external_model(&fixture.key)?;
    assert_eq!(fixture.state()?, ModelRegistrationState::NotInstalled);
    assert_eq!(
        fixture
            .paths
            .iter()
            .map(std::fs::read)
            .collect::<Result<Vec<_>, _>>()?,
        before
    );
    Ok(())
}

#[test]
fn changed_companion_fails_verification_without_erasing_registration()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fixture.register()?;
    std::fs::write(&fixture.paths[2], b"replacement companion")?;
    assert_eq!(fixture.state()?, ModelRegistrationState::VerificationFailed);
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    assert!(
        fixture
            .core
            .prepare_external_runtime_references(model)
            .is_err()
    );
    assert!(fixture.core.external_registration(model)?.is_some());
    Ok(())
}

#[cfg(unix)]
#[test]
fn imported_symlink_is_rejected_without_touching_target() -> Result<(), Box<dyn std::error::Error>>
{
    let mut fixture = Fixture::new()?;
    let alias = fixture.directory.path().join("alias.gguf");
    std::os::unix::fs::symlink(&fixture.paths[0], &alias)?;
    let before = std::fs::read(&fixture.paths[0])?;
    fixture.paths[0] = alias;
    assert!(fixture.register().is_err());
    assert_eq!(fixture.state()?, ModelRegistrationState::NotInstalled);
    assert_eq!(std::fs::read(&fixture.paths[0])?, before);
    Ok(())
}

#[test]
fn optional_companions_can_be_skipped_but_required_files_cannot()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    assert!(
        fixture
            .core
            .register_external_model(
                &fixture.key,
                &[Some(fixture.paths[0].clone()), None, None],
                |_, _| {}
            )
            .is_err()
    );
    fixture.core.register_external_model(
        &fixture.key,
        &[
            Some(fixture.paths[0].clone()),
            Some(fixture.paths[1].clone()),
            None,
        ],
        |_, _| {},
    )?;
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    for companion in model
        .companions()
        .iter()
        .filter(|companion| !companion.required_for_setup())
    {
        assert!(
            !fixture.core.catalogue_companion_installed_by_id(
                &fixture.key,
                companion.artifact().key().id()
            )?
        );
    }
    assert!(
        fixture
            .core
            .register_external_model(&fixture.key, &[None, None, None], |_, _| {})
            .is_err()
    );
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    Ok(())
}

#[test]
fn runtime_preparation_uses_import_receipt_without_rehashing_but_reimport_verifies()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    fixture.register()?;
    // Tiny fixture weights cannot use pinned production provider metadata.
    let runtime_store = fixture.core.ollama_models_directory.take();
    let before = std::fs::metadata(&fixture.paths[0])?.modified()?;
    let mut bytes = std::fs::read(&fixture.paths[0])?;
    bytes[0] ^= 1;
    std::fs::write(&fixture.paths[0], bytes)?;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&fixture.paths[0])?
        .set_times(std::fs::FileTimes::new().set_modified(before))?;
    // This is deliberately not continuous tamper detection: same-size changes
    // preserving mtime are outside the import-time verification contract.
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    fixture.core.prepare_external_runtime_references(model)?;
    fixture.core.prepare_external_runtime_references(model)?;
    fixture.core.ollama_models_directory = runtime_store;
    // Explicit import still authenticates the selected bytes, before publication.
    assert!(matches!(
        fixture.register(),
        Err(AppCoreError::ExternalModel(ExternalModelError::Mismatch(_)))
    ));
    let catalogue = fixture.core.model_catalogue.clone();
    fixture.core = AppCore::initialize(fixture.directory.path().join("app"))?;
    fixture.core.model_catalogue = catalogue;
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    fixture.core.prepare_external_runtime_references(model)?;
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    Ok(())
}

#[test]
fn download_cannot_replace_an_external_registration() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    fixture.register()?;
    let before = fixture
        .paths
        .iter()
        .map(std::fs::read)
        .collect::<Result<Vec<_>, _>>()?;
    let result = fixture.core.install_catalogue_model(
        &fixture.key,
        &[],
        &model_runtime::DownloadCancellation::new(),
        |_| {},
        "2026-10-20T00:00:00Z",
    );
    assert!(matches!(
        result,
        Err(AppCoreError::ExternalModel(ExternalModelError::Registered))
    ));
    assert_eq!(
        fixture
            .paths
            .iter()
            .map(std::fs::read)
            .collect::<Result<Vec<_>, _>>()?,
        before
    );
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    Ok(())
}

#[test]
fn a_file_moved_while_later_companions_verify_does_not_create_first_registration()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let paths = fixture.paths.iter().cloned().map(Some).collect::<Vec<_>>();
    let primary_bytes = std::fs::metadata(&fixture.paths[0])?.len();
    let moved = fixture.directory.path().join("moved-during-import.gguf");
    let mut primary_boundary_count = 0;
    let mut relocation = None;
    let result = fixture
        .core
        .register_external_model(&fixture.key, &paths, |verified, _| {
            if verified == primary_bytes {
                primary_boundary_count += 1;
                // First occurrence is the primary's last read; second is the next
                // artifact's initial progress, after primary verification completed.
                if primary_boundary_count == 2 {
                    relocation = Some(std::fs::rename(&fixture.paths[0], &moved));
                }
            }
        });
    relocation.ok_or("relocation callback did not run")??;
    assert!(result.is_err());
    assert_eq!(fixture.state()?, ModelRegistrationState::NotInstalled);
    assert!(moved.is_file());
    Ok(())
}

#[test]
fn external_development_provider_cannot_register_user_owned_files()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let provider = model_runtime::OllamaProvider::new(
        model_runtime::OllamaProviderConfig::loopback("127.0.0.1:11434".parse()?)?,
    )?;
    let service = crate::models::ollama_service::OllamaService::external(provider);
    fixture.core.ollama_shutdown = service.shutdown_handle();
    fixture.core.ollama_service = std::sync::Mutex::new(service);
    assert!(!fixture.core.external_import_supported(&fixture.key)?);
    assert!(matches!(
        fixture.register(),
        Err(AppCoreError::ExternalModel(ExternalModelError::Unsupported))
    ));
    assert_eq!(fixture.state()?, ModelRegistrationState::NotInstalled);
    Ok(())
}

#[test]
fn import_capability_remains_readable_during_provider_operation()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        // Streaming holds this mutex until the response finishes. Catalogue
        // refreshes must still be able to read immutable import capability.
        let operation = fixture
            .core
            .ollama_service
            .lock()
            .map_err(|_| "poisoned service")?;
        let (sender, receiver) = std::sync::mpsc::channel();
        let core = &fixture.core;
        let key = &fixture.key;
        let query = scope.spawn(move || {
            let result = core.external_import_supported(key);
            let _ = sender.send(result);
        });
        let result = receiver.recv_timeout(std::time::Duration::from_secs(2));
        // Release before joining even on timeout, so a regression fails rather
        // than hanging the test process with the blocked query thread.
        drop(operation);
        query.join().map_err(|_| "catalogue query panicked")?;
        assert_eq!(result??, cfg!(unix));
        Ok(())
    })
}

#[test]
fn module_import_retains_progress_until_every_required_file_is_verified()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_required_projector(true)?;
    let modules = fixture.core.catalogue_modules(&fixture.key)?;
    assert!(modules[0].required);
    assert!(modules[1].required);
    assert!(!modules[2].required);
    let mut reported = Vec::new();
    fixture.core.register_catalogue_module(
        &fixture.key,
        &modules[0].artifact_id,
        &fixture.paths[0],
        |done, total| reported.push((done, total)),
    )?;
    assert!(!reported.is_empty());
    assert_eq!(
        reported.last().copied(),
        Some((
            std::fs::metadata(&fixture.paths[0])?.len(),
            std::fs::metadata(&fixture.paths[0])?.len()
        ))
    );
    assert_eq!(fixture.state()?, ModelRegistrationState::Incomplete);
    assert!(!fixture.core.catalogue_model_installed(&fixture.key)?);
    assert_eq!(
        fixture.core.catalogue_modules(&fixture.key)?[0].registration_state,
        ModelRegistrationState::Available
    );
    fixture.core.register_catalogue_module(
        &fixture.key,
        &modules[1].artifact_id,
        &fixture.paths[1],
        |_, _| {},
    )?;
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    let modules = fixture.core.catalogue_modules(&fixture.key)?;
    assert_eq!(
        modules[2].registration_state,
        ModelRegistrationState::NotInstalled
    );
    assert!(fixture.core.catalogue_model_installed(&fixture.key)?);
    assert!(modules[1].required);
    assert!(!modules[2].required);
    Ok(())
}

#[test]
fn companions_can_be_verified_first_without_publishing_a_usable_primary()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_required_projector(true)?;
    let modules = fixture.core.catalogue_modules(&fixture.key)?;
    fixture.core.register_catalogue_module(
        &fixture.key,
        &modules[1].artifact_id,
        &fixture.paths[1],
        |_, _| {},
    )?;
    assert_eq!(fixture.state()?, ModelRegistrationState::Incomplete);
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    assert!(fixture.core.external_primary(model).is_err());
    fixture.core.register_catalogue_module(
        &fixture.key,
        &modules[0].artifact_id,
        &fixture.paths[0],
        |_, _| {},
    )?;
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    assert_eq!(
        fixture
            .core
            .external_primary(model)?
            .ok_or("primary missing")?
            .local_path,
        fixture.paths[0].canonicalize()?.to_string_lossy()
    );
    Ok(())
}

#[test]
fn bad_module_replacement_preserves_verified_siblings_and_external_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_required_projector(true)?;
    let modules = fixture.core.catalogue_modules(&fixture.key)?;
    fixture.core.register_catalogue_module(
        &fixture.key,
        &modules[0].artifact_id,
        &fixture.paths[0],
        |_, _| {},
    )?;
    let wrong = fixture.directory.path().join("wrong.gguf");
    std::fs::write(&wrong, b"GGUF wrong revision")?;
    assert!(
        fixture
            .core
            .register_catalogue_module(&fixture.key, &modules[1].artifact_id, &wrong, |_, _| {})
            .is_err()
    );
    assert_eq!(
        fixture.core.catalogue_modules(&fixture.key)?[0].registration_state,
        ModelRegistrationState::Available
    );
    assert_eq!(fixture.state()?, ModelRegistrationState::Incomplete);
    assert_eq!(std::fs::read(&wrong)?, b"GGUF wrong revision");
    assert!(
        fixture
            .core
            .register_catalogue_module(
                &fixture.key,
                "not-in-catalogue",
                &fixture.paths[1],
                |_, _| {}
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn downloaded_projector_joins_imported_primary_without_redownloading_or_copying_it()
-> Result<(), Box<dyn std::error::Error>> {
    use ipc_types::ModelModuleSource;
    let fixture = Fixture::with_required_projector(true)?;
    let modules = fixture.core.catalogue_modules(&fixture.key)?;
    fixture.core.register_catalogue_module(
        &fixture.key,
        &modules[0].artifact_id,
        &fixture.paths[0],
        |_, _| {},
    )?;
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    let artifact = model.companions()[0].artifact();
    // Seed a tiny, already-published download. The downloader must reuse it offline.
    let installed = fixture
        .directory
        .path()
        .join("app/models/installed")
        .join(format!("{}.gguf", artifact.sha256()));
    std::fs::copy(&fixture.paths[1], &installed)?;
    fixture.core.download_catalogue_module(
        &fixture.key,
        &modules[1].artifact_id,
        &model_runtime::DownloadCancellation::new(),
        |_| {},
    )?;
    let modules = fixture.core.catalogue_modules(&fixture.key)?;
    assert_eq!(modules[0].source, Some(ModelModuleSource::Imported));
    assert_eq!(modules[1].source, Some(ModelModuleSource::Downloaded));
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    let registration = fixture
        .core
        .external_registration(model)?
        .ok_or("missing registration")?;
    assert_eq!(
        registration.files[0].path,
        fixture.paths[0].canonicalize()?.to_string_lossy()
    );
    assert_eq!(
        registration.files[1].path,
        installed.canonicalize()?.to_string_lossy()
    );
    fixture.core.forget_external_model(&fixture.key)?;
    assert!(fixture.paths[0].exists());
    assert!(installed.exists());
    Ok(())
}

#[test]
fn legacy_managed_primary_requires_a_projector_but_not_the_optional_draft()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::new()?;
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    let artifact = model.artifact();
    let installed = fixture
        .directory
        .path()
        .join("app/models/installed")
        .join(format!("{}.gguf", artifact.sha256()));
    std::fs::copy(&fixture.paths[0], &installed)?;
    let installed = installed.canonicalize()?;
    fixture.core.with_storage(|storage| {
        storage.save_model(&app_storage::ModelMetadata {
            id: artifact.key().to_string(),
            display_name: model.display_name().to_owned(),
            format: "gguf".to_owned(),
            local_path: installed.to_string_lossy().into_owned(),
            content_digest: artifact.sha256().to_owned(),
            byte_size: artifact.installed_bytes(),
            imported_at: "2026-10-23T00:00:00Z".to_owned(),
        })
    })?;
    assert!(!fixture.core.catalogue_model_installed(&fixture.key)?);
    let companion = &model.companions()[0];
    assert!(companion.required_for_setup());
    assert!(!companion.required()); // Setup availability must not force image loading for text.
    let path = installed
        .parent()
        .ok_or("missing parent")?
        .join(format!("{}.gguf", companion.artifact().sha256()));
    std::fs::copy(&fixture.paths[1], &path)?;
    fixture.core.with_storage(|storage| {
        storage.save_model_companion(&app_storage::ModelCompanionMetadata {
            model_id: artifact.key().to_string(),
            artifact_id: companion.artifact().key().id().to_owned(),
            kind: companion.kind().as_str().to_owned(),
            required: companion.required(),
            local_path: path.to_string_lossy().into_owned(),
            content_digest: companion.artifact().sha256().to_owned(),
            byte_size: companion.artifact().installed_bytes(),
            compatibility_version: companion.compatibility_version().to_owned(),
            memory_requirement_bytes: companion.memory_requirement_bytes(),
            installed_at: "2026-10-23T00:00:00Z".to_owned(),
        })
    })?;
    assert!(fixture.core.catalogue_model_installed(&fixture.key)?);
    let modules = fixture.core.catalogue_modules(&fixture.key)?;
    assert!(modules[0].required);
    assert!(modules[1].required);
    assert!(!modules[2].required);
    assert_eq!(
        modules[2].registration_state,
        ModelRegistrationState::NotInstalled
    );
    assert!(crate::models::lifecycle::selected_companions(model, &[])?.is_empty());
    Ok(())
}

#[derive(Default)]
struct TestFileAccess {
    denied: std::sync::atomic::AtomicBool,
    restored: std::sync::Mutex<Vec<PathBuf>>,
    released: std::sync::Mutex<Vec<PathBuf>>,
}
impl crate::ExternalModelAccess for TestFileAccess {
    fn restore(&self, path: &Path, bookmark: Option<&[u8]>) -> Result<(), ExternalModelError> {
        if bookmark.is_some() {
            if self.denied.load(std::sync::atomic::Ordering::SeqCst)
                || bookmark != Some(b"read authority".as_slice())
            {
                return Err(ExternalModelError::Unavailable);
            }
            self.restored
                .lock()
                .map_err(|_| ExternalModelError::Unavailable)?
                .push(path.to_path_buf());
        }
        Ok(())
    }
    fn release(&self, path: &Path, bookmark: Option<&[u8]>) {
        if bookmark.is_some()
            && let Ok(mut released) = self.released.lock()
        {
            released.push(path.to_path_buf());
        }
    }
}

#[test]
fn persisted_native_access_is_restored_before_availability_and_runtime_use()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    let access = std::sync::Arc::new(TestFileAccess::default());
    fixture.core = fixture.core.with_external_model_access(access.clone())?;
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    let artifacts: Vec<_> = super::runtime::bundle_artifacts(model)
        .iter()
        .map(|artifact| artifact.key().id().to_owned())
        .collect();
    for (artifact, path) in artifacts.iter().zip(&fixture.paths) {
        fixture.core.register_catalogue_module_with_access(
            &fixture.key,
            artifact,
            path,
            Some(b"read authority".to_vec()),
            |_, _| {},
        )?;
    }
    assert_eq!(fixture.state()?, ModelRegistrationState::Available);
    access
        .denied
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(fixture.state()?, ModelRegistrationState::Unavailable);
    assert!(
        fixture
            .core
            .catalogue_modules(&fixture.key)?
            .iter()
            .all(|module| module.registration_state == ModelRegistrationState::Unavailable)
    );
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    assert!(
        fixture
            .core
            .prepare_external_runtime_references(model)
            .is_err()
    );
    let catalogue = fixture.core.model_catalogue.clone();
    drop(fixture.core);
    let fresh_access = std::sync::Arc::new(TestFileAccess::default());
    let mut reopened = AppCore::initialize(fixture.directory.path().join("app"))?;
    reopened.model_catalogue = catalogue;
    let reopened = reopened.with_external_model_access(fresh_access.clone())?;
    // Startup must restore all grants before any catalogue/provider query can
    // relaunch the sidecar; otherwise that child misses the restored access.
    assert_eq!(
        fresh_access.restored.lock().map_err(|_| "poisoned")?.len(),
        artifacts.len()
    );
    assert_eq!(
        reopened.catalogue_registration_state(&fixture.key)?,
        ModelRegistrationState::Available
    );
    let model = reopened
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    reopened.prepare_external_runtime_references(model)?;
    assert!(
        !fresh_access
            .restored
            .lock()
            .map_err(|_| "poisoned")?
            .is_empty()
    );
    reopened.forget_external_model(&fixture.key)?;
    assert_eq!(
        fresh_access.released.lock().map_err(|_| "poisoned")?.len(),
        artifacts.len()
    );
    for path in fixture.paths {
        assert!(path.is_file());
    }
    Ok(())
}

#[test]
fn rejected_native_access_preserves_registration_and_bookmark_and_content_checks()
-> Result<(), Box<dyn std::error::Error>> {
    let mut fixture = Fixture::new()?;
    fixture.register()?;
    let access = std::sync::Arc::new(TestFileAccess::default());
    fixture.core = fixture.core.with_external_model_access(access.clone())?;
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    let artifact_id = model.artifact().key().id().to_owned();
    let before = fixture
        .core
        .external_registration(model)?
        .ok_or("registration missing")?;
    assert!(
        fixture
            .core
            .register_catalogue_module_with_access(
                &fixture.key,
                &artifact_id,
                &fixture.paths[0],
                Some(b"invalid".to_vec()),
                |_, _| {}
            )
            .is_err()
    );
    let model = fixture
        .core
        .model_catalogue
        .model(&fixture.key)
        .ok_or("model missing")?;
    assert_eq!(fixture.core.external_registration(model)?, Some(before));
    fixture.core.register_catalogue_module_with_access(
        &fixture.key,
        &artifact_id,
        &fixture.paths[0],
        Some(b"read authority".to_vec()),
        |_, _| {},
    )?;
    std::fs::write(&fixture.paths[0], b"GGUF changed bytes")?;
    assert_eq!(fixture.state()?, ModelRegistrationState::VerificationFailed);
    Ok(())
}
