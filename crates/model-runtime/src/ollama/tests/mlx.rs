use super::*;
use std::fmt::Write;

#[test]
fn mlx_tokenizer_contract_rejects_an_unqualified_provider_upgrade() -> Result<(), Box<dyn Error>> {
    if crate::ModelPlatform::current() != crate::ModelPlatform::AppleSilicon {
        return Ok(());
    }
    let catalogue = bundled_catalogue()?;
    let model = catalogue
        .models()
        .iter()
        .find(|m| m.is_mlx())
        .ok_or("missing MLX")?;
    if !model.supports_current_runtime() {
        let provider =
            OllamaProvider::new(OllamaProviderConfig::loopback("127.0.0.1:9".parse()?)?)?;
        assert!(matches!(
            provider.verify_model(model),
            Err(OllamaError::InvalidConfiguration)
        ));
        return Ok(());
    }
    let (address, requests, server) =
        mock_server(vec![serde_json::json!({"version":"0.32.15"}).to_string()]);
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(address)?)?;
    assert!(matches!(
        provider.verify_model(model),
        Err(OllamaError::IncompatibleServer)
    ));
    assert_eq!(requests.recv()?, "GET /api/version");
    server.join().map_err(|_| "mock server panicked")?;
    Ok(())
}

#[test]
fn tensor_pull_accounts_for_all_layers_and_rechecks_identity() -> Result<(), Box<dyn Error>> {
    if crate::ModelPlatform::current() != crate::ModelPlatform::AppleSilicon {
        return Ok(());
    }
    let catalogue = bundled_catalogue()?;
    let model = catalogue
        .models()
        .iter()
        .find(|m| m.is_mlx())
        .ok_or("missing MLX")?;
    if !model.supports_current_runtime() {
        let provider =
            OllamaProvider::new(OllamaProviderConfig::loopback("127.0.0.1:9".parse()?)?)?;
        assert!(matches!(
            provider.pull_and_verify(model, &DownloadCancellation::new(), |_| {}),
            Err(OllamaError::InvalidConfiguration)
        ));
        return Ok(());
    }
    let distribution = model.ollama_distribution().ok_or("missing distribution")?;
    let blobs = distribution.tensor_blobs().ok_or("missing tensor blobs")?;
    let mut pull = String::new();
    for blob in blobs {
        writeln!(
            pull,
            "{}",
            serde_json::json!({
                "digest": format!("sha256:{}", blob.sha256()), "completed": blob.bytes(), "total": blob.bytes()
            })
        )?;
    }
    let version = serde_json::json!({"version":"0.32.14"}).to_string();
    let tags = serde_json::json!({"models":[{"name":distribution.tag(),"digest":distribution.manifest_sha256()}]}).to_string();
    let (address, requests, server) = mock_server(vec![pull, version, tags]);
    let provider = OllamaProvider::new(OllamaProviderConfig::loopback(address)?)?;
    let mut progress = Vec::new();
    let verified =
        provider.pull_and_verify(model, &DownloadCancellation::new(), |p| progress.push(p))?;
    assert!(!verified.runtime_profile(32_768).is_measured_baseline());
    let downloading = progress
        .iter()
        .filter(|p| p.phase() == DownloadPhase::Downloading)
        .collect::<Vec<_>>();
    assert_eq!(downloading.len(), blobs.len() + 1);
    assert_eq!(
        downloading.last().map(|p| p.downloaded_bytes()),
        Some(18_174_721_847)
    );
    assert_eq!(requests.recv()?, "POST /api/pull");
    assert_eq!(requests.recv()?, "GET /api/version");
    assert_eq!(requests.recv()?, "GET /api/tags");
    server.join().map_err(|_| "mock server panicked")?;
    Ok(())
}
