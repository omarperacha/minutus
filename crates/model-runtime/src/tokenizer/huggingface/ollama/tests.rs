use super::*;
use tokenizers::{
    OffsetReferential, OffsetType, PreTokenizedString, PreTokenizer, SplitDelimiterBehavior,
    models::bpe::BPE, pre_tokenizers::sequence::Sequence,
};

#[test]
fn whitespace_boundaries_match_the_pinned_go_runner()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let split = Split::new(
        SplitPattern::Regex(QWEN_PATTERN.to_owned()),
        SplitDelimiterBehavior::Isolated,
        false,
    )?;
    let mut tokenizer = Tokenizer::new(BPE::default());
    tokenizer.with_pre_tokenizer(Some(Sequence::new(vec![split.into()])));
    assert!(configure_qwen_split(&mut tokenizer).is_ok());
    let pretokenizer = tokenizer
        .get_pre_tokenizer()
        .ok_or("missing pretokenizer")?;
    for (input, expected) in [
        ("  123", vec!["  ", "1", "2", "3"]),
        ("  word", vec![" ", " word"]),
        ("  !", vec!["  ", "!"]),
        (" \u{a0}word", vec![" ", "\u{a0}word"]),
        ("\u{a0}\u{a0}中文", vec!["\u{a0}", "\u{a0}中文"]),
        ("e\u{301}", vec!["e\u{301}"]),
    ] {
        let mut text = PreTokenizedString::from(input);
        pretokenizer.pre_tokenize(&mut text)?;
        let actual = text
            .get_splits(OffsetReferential::Original, OffsetType::Byte)
            .into_iter()
            .map(|(text, _, _)| text)
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "input: {input:?}");
    }
    Ok(())
}
