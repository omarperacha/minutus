//! Host capability and recommendation policy, independent of presentation.

use super::ModelManifest;

/// Platform capabilities relevant to the available inference distributions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelPlatform {
    /// Native macOS on Apple Silicon, with the bundled Metal/MLX runtime.
    AppleSilicon,
    /// Hosts using the portable GGUF distribution.
    Other,
}

impl ModelPlatform {
    /// Detects the native application target. Desktop distribution ships native binaries.
    #[must_use]
    pub const fn current() -> Self {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Self::AppleSilicon
        } else {
            Self::Other
        }
    }

    /// Preferred model identities in order, used only when no explicit choice exists.
    #[must_use]
    pub const fn preferred_model_ids(self) -> &'static [&'static str] {
        &["qwen/qwen3.8-27b"]
    }
}

impl ModelManifest {
    /// Whether this distribution can execute on the supplied platform.
    #[must_use]
    pub fn supports_platform(&self, platform: ModelPlatform) -> bool {
        !self.is_mlx() || platform == ModelPlatform::AppleSilicon
    }

    /// Whether this process may expose, install or execute this distribution.
    /// MLX is an explicit development experiment; release builds never enable it.
    #[must_use]
    pub fn supports_current_runtime(&self) -> bool {
        self.supports_runtime(ModelPlatform::current(), experimental_mlx_enabled())
    }

    fn supports_runtime(&self, platform: ModelPlatform, experimental: bool) -> bool {
        self.supports_platform(platform) && (!self.is_mlx() || experimental)
    }

    /// Whether this profile is the default download recommendation on this host.
    #[must_use]
    pub fn is_recommended(&self, platform: ModelPlatform) -> bool {
        self.supports_platform(platform)
            && platform.preferred_model_ids().first() == Some(&self.key().id())
    }

    /// Whether the provider owns an MLX tensor bundle for this profile.
    #[must_use]
    pub fn is_mlx(&self) -> bool {
        self.ollama_distribution()
            .is_some_and(|d| d.tensor_bundle.is_some())
    }
}

fn experimental_mlx_enabled() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("ZETA_DEV_EXPERIMENTAL_MLX").is_some_and(|value| value == "1")
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

#[cfg(test)]
mod tests;
