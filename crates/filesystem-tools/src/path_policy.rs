//! Host-owned exclusions for credentials and execution-control configuration.

use std::{ffi::OsStr, path::Path};

use crate::SafePathError;

// Applied to every component, including the selected root's ancestors. Granting
// a protected directory directly must not turn its children into ordinary paths.
fn validate_component(component: &OsStr) -> Result<(), SafePathError> {
    let Some(name) = component.to_str() else {
        return Ok(());
    };
    let name = name.to_ascii_lowercase();
    let protected = matches!(
        name.as_str(),
        ".git"
            | ".hg"
            | ".svn"
            | ".github"
            | ".ssh"
            | ".aws"
            | ".azure"
            | ".gnupg"
            | ".kube"
            | ".cargo"
            | ".config"
            | ".docker"
            | ".vscode"
            | ".idea"
            | ".devcontainer"
            | ".husky"
            | ".gitconfig"
            | ".gitmodules"
            | ".npmrc"
            | ".yarnrc.yml"
            | ".netrc"
            | "_netrc"
            | ".pypirc"
            | ".bashrc"
            | ".bash_profile"
            | ".bash_login"
            | ".profile"
            | ".zshrc"
            | ".zshenv"
            | ".zprofile"
            | ".zlogin"
            | ".envrc"
            | "credentials.json"
            | "app-vault"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
    ) || (name == ".env" || name.starts_with(".env."))
        && !matches!(
            name.as_str(),
            ".env.example" | ".env.sample" | ".env.template"
        )
        || [
            ".pem",
            ".key",
            ".p12",
            ".pfx",
            ".keychain",
            ".keychain-db",
            ".password-vault.json",
        ]
        .iter()
        .any(|extension| name.ends_with(extension));
    if protected {
        return Err(SafePathError::ProtectedPath);
    }
    Ok(())
}

pub(crate) fn validate_path(path: &Path) -> Result<(), SafePathError> {
    for component in path.components() {
        validate_component(component.as_os_str())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nested_credentials_and_execution_configuration() {
        for path in [
            ".env",
            "nested/.ENV.production",
            ".git/config",
            ".git/hooks/post-checkout",
            ".ssh/config",
            ".aws/credentials",
            "nested/.vscode/tasks.json",
            ".github/WORKFLOWS/build.yml",
            "nested/private.PEM",
            ".zshrc",
            ".npmrc",
        ] {
            assert!(
                matches!(
                    validate_path(Path::new(path)),
                    Err(SafePathError::ProtectedPath)
                ),
                "{path}"
            );
        }
    }

    #[test]
    fn preserves_ordinary_source_and_explicit_environment_templates() {
        for path in [
            "src/key.rs",
            "package.json",
            ".gitignore",
            ".env.example",
            ".env.sample",
            ".env.template",
        ] {
            assert!(validate_path(Path::new(path)).is_ok(), "{path}");
        }
    }
}
