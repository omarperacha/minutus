use ipc_types::{ModelSelection, SpeechInstallState, SpeechModelRequest};
use tempfile::TempDir;

use crate::AppCore;

#[test]
fn speech_setup_is_optional_and_cannot_change_chat_selection()
-> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let app = AppCore::initialize(root.path())?;
    let view = app.speech_models()?.remove(0);
    assert_eq!(view.state, SpeechInstallState::Missing);
    assert_eq!(view.download_bytes, 574_041_195);
    assert!(!app.cancel_speech_download()?);
    let original = app.default_model()?;
    let chat_selection = ModelSelection {
        model_id: view.selection.model_id.clone(),
        model_version: view.selection.model_version.clone(),
    };
    assert!(
        app.set_default_model(&chat_selection, "2026-09-03T00:00:00Z")
            .is_err()
    );
    app.remove_speech_model(&view.selection)?;
    assert_eq!(app.default_model()?, original);
    let chat = &app.model_catalogue().models()[0];
    let wrong_task = SpeechModelRequest {
        model_id: chat.key().id().to_owned(),
        model_version: chat.key().version().to_owned(),
    };
    assert!(app.install_speech_model(&wrong_task).is_err());
    assert!(app.remove_speech_model(&wrong_task).is_err());
    assert_eq!(app.default_model()?, original);
    Ok(())
}
