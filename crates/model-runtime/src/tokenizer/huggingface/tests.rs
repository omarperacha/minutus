use std::error::Error;

use tokenizers::{
    AddedToken, TruncationParams, models::wordlevel::WordLevel,
    pre_tokenizers::whitespace::WhitespaceSplit,
};

use super::*;

#[test]
fn counts_special_tokens_and_ignores_artifact_truncation()
-> Result<(), Box<dyn Error + Send + Sync>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("tokenizer.json");
    let model = WordLevel::builder()
        .vocab(
            [
                ("[UNK]".to_owned(), 0),
                ("hello".to_owned(), 1),
                ("中文".to_owned(), 2),
            ]
            .into_iter()
            .collect(),
        )
        .unk_token("[UNK]".to_owned())
        .build()?;
    let mut tokenizer = Tokenizer::new(model);
    tokenizer.with_pre_tokenizer(Some(WhitespaceSplit));
    tokenizer.add_special_tokens(&[AddedToken::from("<|im_start|>", true)]);
    tokenizer.with_truncation(Some(TruncationParams {
        max_length: 1,
        ..TruncationParams::default()
    }))?;
    tokenizer.save(&path, false)?;
    let digest = format!("{:x}", Sha256::digest(fs::read(&path)?));
    let counter = HuggingFaceTokenizer::default();
    assert_eq!(counter.count(&path, &digest, "hello <|im_start|> 中文")?, 3);
    assert_eq!(counter.count(&path, &digest, "hello")?, 1);
    assert!(matches!(
        counter.count(&path, &"0".repeat(64), "hello"),
        Err(TokenizerError::InvalidResponse)
    ));
    assert!(matches!(
        counter.count(&path, &digest, ""),
        Err(TokenizerError::InvalidPrompt)
    ));
    Ok(())
}

#[test]
#[ignore = "requires the pinned local MLX tokenizer JSON"]
fn live_pinned_qwen_matches_native_mlx_token_counts() -> Result<(), Box<dyn Error>> {
    let models = std::path::PathBuf::from(
        std::env::var_os("ZETA_LIVE_OLLAMA_MODELS").ok_or("missing model store")?,
    );
    let catalogue = crate::bundled_catalogue()?;
    let mlx = catalogue
        .models()
        .iter()
        .find(|m| m.is_mlx())
        .ok_or("missing MLX")?;
    let blob = mlx
        .ollama_distribution()
        .and_then(|d| d.tokenizer_blob())
        .ok_or("missing tokenizer")?;
    let tokenizer = HuggingFaceTokenizer::default();
    let path = models
        .join("blobs")
        .join(format!("sha256-{}", blob.sha256()));
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("qwen-ollama-0.32.14.json"))?;
    for case in fixtures["cases"].as_array().ok_or("missing fixtures")? {
        let text = case["text"].as_str().ok_or("missing text")?;
        let expected = case["tokens"].as_array().ok_or("missing tokens")?.len() as u64;
        assert_eq!(
            tokenizer.count(&path, blob.sha256(), text)?,
            expected,
            "input: {text:?}"
        );
    }
    Ok(())
}
