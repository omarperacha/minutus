//! Minutus's native desktop entry point and explicitly exposed IPC commands.

use std::{
    ffi::OsStr,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use app_core::{AppCore, AppCoreError, CatalogueInstallProgress, MAX_IMAGE_IMPORT_BYTES};
use filesystem_tools::{
    FilesystemCreateDirectoryError, FilesystemToolError, FilesystemUndoChange,
    FilesystemWriteError, MAX_READ_TEXT_BYTES,
};
use ipc_types::{
    AppStatus, AssetId, CatalogueCompanion, CatalogueModel, Conversation, ConversationId,
    ConversationSummary, CreateProjectRequest, FilesystemListRequest, FilesystemListResult,
    FilesystemMetadata, FilesystemReadTextRequest, FilesystemReadTextResult,
    FilesystemReplaceTextRequest, FilesystemScanRequest, FilesystemScanResult,
    FilesystemSearchRequest, FilesystemSearchResult, FilesystemStatRequest, FilesystemUndoRequest,
    FilesystemWriteResult, FilesystemWriteTextRequest, ImageAttachmentReceipt, IpcError,
    IpcErrorCode, MAX_CHAT_ASSETS, MessagePart, ModelCompanionKind, ModelDownloadEvent,
    ModelDownloadPhase, ModelDownloadRequest, ModelInstallReceipt, ModelPreferences,
    ModelSelection, ProjectId, ProjectSummary, ProjectWorkspaceSummary, RenameConversationRequest,
    RequestId, SetConversationModelRequest, SetDefaultModelRequest, ToolCallId, ToolOutcome,
    ToolResultPart, UpdateProjectWorkspacePermissionRequest, ValidateIpc, WorkspaceGrantId,
};
#[cfg(all(
    debug_assertions,
    not(all(feature = "password-vault-prototype", target_os = "macos"))
))]
use model_runtime::OllamaProviderConfig;
use model_runtime::{
    CompanionKind, DownloadCancellation, DownloadError, DownloadPhase, ImportError,
    InferenceBackend, InferenceError, ModelOutputFailureKind, OLLAMA_GUARDIAN_ARGUMENT,
    OllamaError, run_ollama_guardian,
};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

mod app_state;
mod chat_runtime;
mod chat_tools;
mod error_mapping;
mod evm_approval;
mod evm_authorizations;
mod evm_balances;
mod evm_rpc_settings;
mod evm_transactions;
mod evm_transfers;
mod external_model_commands;
mod filesystem_commands;
mod image_commands;
mod model_commands;
mod model_file_access;
use external_model_commands::{
    download_model_module, forget_external_model, register_external_model,
};
use model_file_access::ModelFileAccess;
mod speech_commands;
use speech_commands::{
    cancel_dictation, cancel_speech_download, dictation_events, finish_dictation,
    install_speech_model, remove_speech_model, speech_model_selection, speech_models,
    start_dictation,
};
#[cfg(all(feature = "password-vault-prototype", target_os = "macos"))]
mod password_wallet;
mod project_commands;
mod search_credentials;
mod updates;
mod wallet_lifecycle;
mod web_approval;
mod workspace_bookmark;
mod x402_approval;
mod x402_release_qualification;
use updates::{
    ReleaseUpdateControl, check_release_update, release_updates_configured,
    save_verified_release_update,
};

use app_state::{
    ChatGenerationControl, InferenceExecutable, ModelDownloadControl, SpeechInferenceExecutable,
};
use chat_runtime::{start_chat_generation, stop_chat_generation};
use error_mapping::{app_core_error, bookmark_access_error, current_timestamp};
use evm_approval::{EvmApprovalControl, review_evm_transaction};
use evm_authorizations::decode_evm_authorization;
use evm_balances::{EvmBalanceCache, evm_balances};
use evm_rpc_settings::{configure_evm_rpc, evm_rpc_settings, remove_evm_rpc};
use evm_transactions::evm_transactions;
use evm_transfers::{prepare_evm_native_transfer, prepare_evm_token_transfer};
use filesystem_commands::{
    filesystem_list, filesystem_read_text, filesystem_replace_text, filesystem_scan,
    filesystem_search, filesystem_stat, filesystem_undo_available, filesystem_write_text,
    undo_filesystem_write,
};
use image_commands::{
    discard_chat_image, import_chat_image, managed_image_preview, pick_chat_images,
};
use model_commands::{
    cancel_model_download, conversation_model, download_catalogue_model, model_catalogue,
    model_preferences, project_model, set_conversation_model, set_default_model, set_project_model,
};
use project_commands::{
    add_project_workspace, app_status, create_conversation, create_project, delete_conversation,
    list_conversations, list_project_workspaces, list_projects, load_conversation,
    remove_project_workspace, rename_conversation, update_project_workspace_permission,
};
use search_credentials::{brave_search_configured, configure_brave_search, remove_brave_search};
use wallet_lifecycle::{
    WalletControl, change_wallet_password, create_wallet, delete_wallet_vault,
    import_wallet_private_key, lock_wallet, remove_wallet_account, reset_wallet,
    reset_wallet_private_key, restore_wallet, select_wallet, unlock_wallet, wallet_inventory,
    wallet_state,
};
use x402_approval::{X402ApprovalControl, review_x402_payment};

fn setup_app(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let data_directory = app.path().app_data_dir()?;
    #[cfg(all(feature = "password-vault-prototype", target_os = "macos"))]
    let data_directory = data_directory.join("password-vault-experiment");
    let native_executable = std::env::current_exe()?;
    let inference_executable = native_executable
        .parent()
        .ok_or_else(|| std::io::Error::other("The application executable has no parent"))?
        .join("zeta-tokenizer");
    #[cfg(not(all(
        feature = "password-vault-prototype",
        target_os = "macos",
        debug_assertions
    )))]
    let tokenizer_executable = inference_executable.clone();
    let bundled_speech_inference_executable = inference_executable
        .parent()
        .ok_or_else(|| std::io::Error::other("The application executable has no parent"))?
        .join("zeta-whisper-helper");
    #[cfg(debug_assertions)]
    let speech_inference_executable = development_speech_helper(
        bundled_speech_inference_executable,
        std::env::var_os("ZETA_DEV_WHISPER_HELPER").map(PathBuf::from),
    );
    #[cfg(not(debug_assertions))]
    let speech_inference_executable = bundled_speech_inference_executable;
    #[cfg(not(all(
        feature = "password-vault-prototype",
        target_os = "macos",
        debug_assertions
    )))]
    let ollama_executable = app.path().resource_dir()?.join("ollama-runtime/ollama");
    // The explicitly selected wallet-only experiment omits model runtimes and
    // uses isolated application records solely for durable approval consumption.
    #[cfg(all(
        feature = "password-vault-prototype",
        target_os = "macos",
        debug_assertions
    ))]
    let core = AppCore::initialize(&data_directory)?;
    #[cfg(all(
        debug_assertions,
        not(all(feature = "password-vault-prototype", target_os = "macos"))
    ))]
    let core = match std::env::var("ZETA_DEV_OLLAMA_ADDRESS") {
        Ok(address) => {
            let address = address.parse().map_err(|_| {
                std::io::Error::other("ZETA_DEV_OLLAMA_ADDRESS must be a loopback socket")
            })?;
            eprintln!(
                "Minutus development override: using external Ollama at {address}; production always uses the bundled runtime"
            );
            AppCore::initialize_with_development_ollama(
                &data_directory,
                OllamaProviderConfig::loopback(address)?,
                &tokenizer_executable,
                std::env::var_os("ZETA_DEV_OLLAMA_MODELS")
                    .map(PathBuf::from)
                    .unwrap_or(app.path().home_dir()?.join(".ollama/models")),
            )?
        }
        Err(std::env::VarError::NotPresent) => AppCore::initialize_with_guarded_bundled_ollama(
            &data_directory,
            &ollama_executable,
            &tokenizer_executable,
            &native_executable,
        )?,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(
                std::io::Error::other("ZETA_DEV_OLLAMA_ADDRESS must be valid Unicode").into(),
            );
        }
    };
    #[cfg(not(debug_assertions))]
    let core = AppCore::initialize_with_guarded_bundled_ollama(
        &data_directory,
        &ollama_executable,
        &tokenizer_executable,
        &native_executable,
    )?;
    let core = core.with_external_model_access(Arc::new(ModelFileAccess::default()))?;
    #[cfg(not(all(feature = "password-vault-prototype", target_os = "macos")))]
    let selected_wallet_id = core.latest_public_wallet()?.map(|wallet| wallet.id.0);
    #[cfg(all(feature = "password-vault-prototype", target_os = "macos"))]
    let selected_wallet_id: Option<String> = None;
    #[cfg(not(all(feature = "password-vault-prototype", target_os = "macos")))]
    let wallet_directory = data_directory.join("app-vault");
    #[cfg(all(feature = "password-vault-prototype", target_os = "macos"))]
    let wallet_directory = password_wallet::initialize(app, &data_directory)?;
    let wallet_control =
        WalletControl::initialize(wallet_directory, selected_wallet_id.as_deref())?;
    app.manage(Arc::new(core));
    app.manage(Arc::new(ModelDownloadControl::new()));
    app.manage(Arc::new(ChatGenerationControl::new()));
    app.manage(EvmBalanceCache::new());
    app.manage(EvmApprovalControl::new());
    app.manage(X402ApprovalControl::new());
    app.manage(wallet_control);
    wallet_lifecycle::install_host(app.handle())?;
    wallet_lifecycle::start_watchdog(app.handle());
    app.manage(InferenceExecutable(inference_executable));
    app.manage(SpeechInferenceExecutable(speech_inference_executable));
    Ok(())
}

#[cfg(debug_assertions)]
fn development_speech_helper(bundled: PathBuf, configured: Option<PathBuf>) -> PathBuf {
    if let Some(configured) = configured {
        return configured;
    }
    let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.cache/asr-003/zeta-whisper-helper");
    if cache.is_file() { cache } else { bundled }
}

fn app_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

fn run() -> tauri::Result<()> {
    let app = tauri::Builder::default()
        .menu(tauri::menu::Menu::default)
        .plugin(tauri_plugin_dialog::init())
        .manage(ReleaseUpdateControl::default())
        .setup(setup_app)
        .on_window_event(handle_window_event)
        .invoke_handler(tauri::generate_handler![
            release_updates_configured,
            check_release_update,
            save_verified_release_update,
            app_status,
            list_projects,
            create_project,
            list_project_workspaces,
            add_project_workspace,
            remove_project_workspace,
            update_project_workspace_permission,
            list_conversations,
            load_conversation,
            create_conversation,
            rename_conversation,
            delete_conversation,
            filesystem_list,
            filesystem_stat,
            filesystem_read_text,
            filesystem_write_text,
            filesystem_replace_text,
            undo_filesystem_write,
            filesystem_undo_available,
            filesystem_search,
            filesystem_scan,
            start_chat_generation,
            stop_chat_generation,
            pick_chat_images,
            import_chat_image,
            managed_image_preview,
            discard_chat_image,
            speech_model_selection,
            speech_models,
            install_speech_model,
            cancel_speech_download,
            remove_speech_model,
            start_dictation,
            finish_dictation,
            cancel_dictation,
            dictation_events,
            model_catalogue,
            model_preferences,
            set_default_model,
            project_model,
            set_project_model,
            brave_search_configured,
            configure_brave_search,
            remove_brave_search,
            evm_rpc_settings,
            configure_evm_rpc,
            remove_evm_rpc,
            evm_balances,
            decode_evm_authorization,
            prepare_evm_native_transfer,
            prepare_evm_token_transfer,
            review_evm_transaction,
            review_x402_payment,
            evm_transactions,
            wallet_state,
            wallet_inventory,
            select_wallet,
            create_wallet,
            restore_wallet,
            import_wallet_private_key,
            unlock_wallet,
            lock_wallet,
            change_wallet_password,
            remove_wallet_account,
            delete_wallet_vault,
            reset_wallet,
            reset_wallet_private_key,
            conversation_model,
            set_conversation_model,
            download_catalogue_model,
            cancel_model_download,
            register_external_model,
            download_model_module,
            forget_external_model
        ])
        .build(app_context())?;
    app.run(|app, event| match event {
        tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. } => shutdown_app(app),
        tauri::RunEvent::Resumed => app
            .state::<Arc<AppCore>>()
            .cancel_active_dictation(app_core::DictationCancelReason::SystemSleep),
        _ => {}
    });
    Ok(())
}

fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if matches!(
        event,
        tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
    ) {
        window.state::<WalletControl>().lock();
        #[cfg(all(feature = "password-vault-prototype", target_os = "macos"))]
        password_wallet::lock(window.app_handle());
        window
            .state::<Arc<AppCore>>()
            .cancel_active_dictation(app_core::DictationCancelReason::WindowClosed);
    }
}

fn shutdown_app(app: &tauri::AppHandle) {
    app.state::<WalletControl>().lock();
    #[cfg(all(feature = "password-vault-prototype", target_os = "macos"))]
    password_wallet::lock(app);
    app.state::<Arc<ChatGenerationControl>>().cancel_active();
    if let Err(error) = app.state::<Arc<AppCore>>().shutdown() {
        eprintln!("Minutus could not stop local inference cleanly: {error}");
    }
}

fn main() -> ExitCode {
    if std::env::args_os().nth(1).as_deref() == Some(OsStr::new("--contract-approval-preview")) {
        #[cfg(all(debug_assertions, target_os = "macos"))]
        return match evm_approval::preview_contract_approval() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Synthetic contract review preview failed: {error:?}");
                ExitCode::FAILURE
            }
        };
        #[cfg(not(all(debug_assertions, target_os = "macos")))]
        {
            eprintln!(
                "The synthetic native review preview is available only in macOS debug builds"
            );
            return ExitCode::FAILURE;
        }
    }

    if std::env::args_os().nth(1).as_deref() == Some(OsStr::new("--qualify-contract-release")) {
        #[cfg(not(debug_assertions))]
        return match chat_runtime::qualify_contract_release_surface() {
            Ok(()) => {
                println!(
                    "contract release surface: writes disabled; catalogue, startup and forecast schemas absent; no app setup, vault, RPC, native review or signing executed"
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("Contract release qualification failed: {error}");
                ExitCode::FAILURE
            }
        };
        #[cfg(debug_assertions)]
        {
            eprintln!("Contract release qualification requires a release executable");
            return ExitCode::FAILURE;
        }
    }

    if std::env::args_os().nth(1).as_deref() == Some(OsStr::new("--qualify-x402-release")) {
        return match x402_release_qualification::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("x402 release qualification failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    #[cfg(debug_assertions)]
    if std::env::args_os().nth(1).as_deref() == Some(OsStr::new("--qwen-efficiency-eval")) {
        return match chat_runtime::run_efficiency_eval() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Qwen evaluation failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() == Some(OsStr::new(OLLAMA_GUARDIAN_ARGUMENT)) {
        let Some(executable) = arguments.next() else {
            eprintln!("Minutus's Ollama guardian is missing its executable");
            return ExitCode::FAILURE;
        };
        if arguments.next().is_some() {
            eprintln!("Minutus's Ollama guardian received unexpected arguments");
            return ExitCode::FAILURE;
        }
        return match run_ollama_guardian(Path::new(&executable)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Minutus's Ollama guardian failed: {error}");
                ExitCode::FAILURE
            }
        };
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Minutus failed to start: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod capability_tests {
    #[test]
    fn main_window_allows_every_ui_command() -> Result<(), Box<dyn std::error::Error>> {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/main.json"))?;
        let permissions = capability["permissions"]
            .as_array()
            .ok_or("main-window permissions must be an array")?;
        for required in [
            "allow-model-catalogue",
            "allow-model-preferences",
            "allow-set-default-model",
            "allow-project-model",
            "allow-set-project-model",
            "allow-conversation-model",
            "allow-set-conversation-model",
            "allow-brave-search-configured",
            "allow-configure-brave-search",
            "allow-remove-brave-search",
            "allow-evm-rpc-settings",
            "allow-configure-evm-rpc",
            "allow-remove-evm-rpc",
            "allow-evm-balances",
            "allow-prepare-evm-native-transfer",
            "allow-prepare-evm-token-transfer",
            "allow-review-evm-transaction",
            "allow-evm-transactions",
            "allow-wallet-state",
            "allow-wallet-inventory",
            "allow-select-wallet",
            "allow-create-wallet",
            "allow-restore-wallet",
            "allow-import-wallet-private-key",
            "allow-unlock-wallet",
            "allow-lock-wallet",
            "allow-filesystem-undo-available",
            "allow-undo-filesystem-write",
        ] {
            assert!(
                permissions.iter().any(|value| value == required),
                "missing {required}"
            );
        }
        Ok(())
    }
}
