//! Native build permissions and main-window grants must agree for external-file actions.
use serde_json::Value;
use std::{error::Error, fs, path::Path};

#[test]
fn external_file_actions_are_granted_by_the_packaged_native_acl() -> Result<(), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let capability: Value =
        serde_json::from_str(&fs::read_to_string(root.join("capabilities/main.json"))?)?;
    let generated: Value = serde_json::from_str(&fs::read_to_string(
        root.join("gen/schemas/acl-manifests.json"),
    )?)?;
    let grants = capability["permissions"]
        .as_array()
        .ok_or("main-window grants missing")?;
    for (permission, command) in [
        ("allow-register-external-model", "register_external_model"),
        ("allow-forget-external-model", "forget_external_model"),
    ] {
        assert!(
            grants.iter().any(|grant| grant == permission),
            "main window must grant {permission}"
        );
        let allowed = generated["__app-acl__"]["permissions"][permission]["commands"]["allow"]
            .as_array()
            .ok_or("native generated command permission missing")?;
        assert_eq!(allowed, &[Value::String(command.to_owned())]);
    }
    assert!(!grants.iter().any(|grant| grant == "allow-import-gguf"));
    Ok(())
}
