//! Compatibility with the pinned Ollama 0.32.14 Qwen tensor tokenizer.

use tokenizers::{
    Tokenizer,
    models::ModelWrapper,
    pre_tokenizers::{
        PreTokenizerWrapper,
        split::{Split, SplitPattern},
    },
};

use super::TokenizerError;

const QWEN_PATTERN: &str = r"(?i:'s|'t|'re|'ve|'m|'ll|'d)|[^\r\n\p{L}\p{N}]?[\p{L}\p{M}]+|\p{N}| ?[^\s\p{L}\p{M}\p{N}]+[\r\n]*|\s*[\r\n]+|\s+(?!\S)|\s+";

pub(super) fn configure(tokenizer: &mut Tokenizer) -> Result<(), TokenizerError> {
    tokenizer
        .with_truncation(None)
        .map_err(|_| TokenizerError::InvalidResponse)?;
    tokenizer.with_padding(None);
    // x/tokenizer ignores JSON normalization, including this bundle's NFC rule.
    tokenizer.with_normalizer(None::<tokenizers::normalizers::NormalizerWrapper>);
    let mut model = tokenizer.get_model().clone();
    if let ModelWrapper::BPE(bpe) = &mut model {
        // encodeChunkInto checks the complete chunk before consulting merge ranks.
        bpe.ignore_merges = true;
        configure_qwen_split(tokenizer)?;
    }
    tokenizer.with_model(model);
    Ok(())
}

fn configure_qwen_split(tokenizer: &mut Tokenizer) -> Result<(), TokenizerError> {
    let Some(PreTokenizerWrapper::Sequence(mut sequence)) = tokenizer.get_pre_tokenizer().cloned()
    else {
        return Err(TokenizerError::InvalidResponse);
    };
    let Some(PreTokenizerWrapper::Split(split)) = sequence.as_mut().first_mut() else {
        return Err(TokenizerError::InvalidResponse);
    };
    if !matches!(&split.pattern, SplitPattern::Regex(pattern) if pattern == QWEN_PATTERN) {
        return Err(TokenizerError::InvalidResponse);
    }
    // Go's RE2 rewrite consumes a whole whitespace run, then moves its last
    // character to the following chunk only when that chunk begins with a letter.
    // This lookahead expresses that same boundary without splitting spaces before
    // digits ("  123" is four tokens in the runner, five with the HF pattern).
    // Qwen's pattern does not enable Ollama's separate punctuation adjustment.
    let pattern = QWEN_PATTERN
        // RE2 treats these Unicode whitespace characters as punctuation. When an
        // entire punctuation match is whitespace, Ollama also moves its last
        // character to a following letter chunk. Split that prefix explicitly.
        .replace(
            r"| ?[^\s\p{L}\p{M}\p{N}]+[\r\n]*",
            &format!(
                r"|(?: {w}*|{w}+)(?={w}\p{{L}})| ?[^\s\p{{L}}\p{{M}}\p{{N}}]+[\r\n]*",
                w = r"[\x0B\x85\xA0\u1680\u2000-\u200A\u2028\u2029\u202F\u205F\u3000]"
            ),
        )
        .replace(r"\s+(?!\S)|\s+", r"\s+(?=\s\p{L})|\s+")
        .replace(
            r"(?i:'s|'t|'re|'ve|'m|'ll|'d)",
            r"(?:'[sS]|'[tT]|'[rR][eE]|'[vV][eE]|'[mM]|'[lL][lL]|'[dD])",
        )
        // Go's Perl whitespace class is ASCII; preserve its meaning inside sets too.
        .replace(r"\s", r"[\t\n\f\r ]");
    *split = Split::new(SplitPattern::Regex(pattern), split.behavior, split.invert)
        .map_err(|_| TokenizerError::InvalidResponse)?;
    tokenizer.with_pre_tokenizer(Some(PreTokenizerWrapper::Sequence(sequence)));
    Ok(())
}

#[cfg(test)]
mod tests;
