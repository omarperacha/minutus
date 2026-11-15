//! Opt-in fixture evaluation using the production desktop generation loop.
//!
//! This debug-only composition entry point creates a new private store and has
//! no webview or IPC surface. It does not replace admission, tools or inference.

mod provider;
mod recording;
mod tool_scope;
mod trials;
mod workspace;
pub(crate) use tool_scope::tool_denial;

use std::{error::Error, fs, path::PathBuf, sync::Arc};

use ipc_types::ModelSelection;

use crate::{
    app_state::ChatGenerationControl, evm_approval::EvmApprovalControl,
    wallet_lifecycle::WalletControl, x402_approval::X402ApprovalControl,
};

type EvalResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub(crate) fn run() -> EvalResult<()> {
    let args: Vec<_> = std::env::args_os().skip(2).collect();
    if args.len() != 2 {
        return Err("usage: --qwen-efficiency-eval SUITE_JSON NEW_RUN_DIRECTORY".into());
    }
    let suite_path = fs::canonicalize(&args[0])?;
    let suite = trials::Suite::read(&suite_path)?;
    let directory = PathBuf::from(&args[1]);
    // Exclusive creation prevents accidental reuse of ordinary application data.
    fs::create_dir(&directory)?;
    let directory = fs::canonicalize(directory)?;
    let models = std::env::var_os("ZETA_DEV_OLLAMA_MODELS")
        .ok_or("ZETA_DEV_OLLAMA_MODELS must name an isolated Qwen store")?;
    let executable = std::env::current_exe()?
        .parent()
        .ok_or("executable has no parent")?
        .join("zeta-tokenizer");
    let core = Arc::new(provider::initialize(&directory, &executable, &models)?);
    let model = core
        .model_catalogue()
        .models()
        .iter()
        .find(|model| model.key().id() == "qwen/qwen3.8-27b")
        .ok_or("pinned Qwen catalogue entry missing")?;
    if !core.catalogue_model_installed(model.key())? {
        return Err("pinned Qwen bundle is not installed".into());
    }
    core.set_default_model(
        &ModelSelection {
            model_id: model.key().id().to_owned(),
            model_version: model.key().version().to_owned(),
        },
        &trials::timestamp()?,
    )?;
    core.create_project("Qwen efficiency evaluation", &trials::timestamp()?)?;
    fs::copy(&suite_path, directory.join("suite.json"))?;
    let control = Arc::new(ChatGenerationControl::new());
    let shutdown_core = Arc::clone(&core);
    let shutdown_control = Arc::clone(&control);
    let outcome = Arc::new(std::sync::Mutex::new(None));
    let worker_outcome = Arc::clone(&outcome);
    let mut context = crate::app_context();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .manage(Arc::clone(&core))
        .manage(tool_scope::RestrictedTools::new())
        .manage(
            WalletControl::initialize(directory.join("wallets"), None)
                .map_err(|error| error.to_string())?,
        )
        .manage(X402ApprovalControl::new())
        .manage(EvmApprovalControl::new())
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("qwen-efficiency-eval".to_owned())
                .spawn(move || {
                    // A failed worker must exit the evaluator, not leave an
                    // event loop and its inference process resident indefinitely.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        trials::run(&handle, &core, &control, &executable, &directory, &suite)
                    }))
                    .unwrap_or_else(|_| Err("Evaluation worker panicked".into()));
                    let exit_status = i32::from(result.is_err());
                    if let Ok(mut outcome) = worker_outcome.lock() {
                        *outcome = Some(result.map_err(|error| error.to_string()));
                    }
                    handle.exit(exit_status);
                })?;
            Ok(())
        })
        .build(context)?;
    app.run_return(move |_, event| match event {
        // A windowless evaluator stays alive until its worker explicitly exits.
        tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        tauri::RunEvent::Exit => {
            shutdown_control.cancel_active();
            if let Err(error) = shutdown_core.shutdown() {
                eprintln!("Evaluation shutdown failed: {error}");
            }
        }
        _ => {}
    });
    outcome
        .lock()
        .map_err(|_| "evaluation outcome unavailable")?
        .take()
        .ok_or("evaluation interrupted before completion")?
        .map_err(Into::into)
}
