use bip39::{Language, Mnemonic};

use super::{
    RECOVERY_ENTROPY_BYTES, RECOVERY_PHRASE_WORDS, RECOVERY_VERIFICATION_WORDS, RecoveryPhrase,
    RecoveryPhraseError, RecoveryVerificationPrompt, RecoveryWordAnswers,
};

#[test]
fn generated_phrase_is_valid_english_bip39() -> Result<(), RecoveryPhraseError> {
    let mut phrase = RecoveryPhrase::generate()?;

    phrase.present_once_for_onboarding(|words| {
        let parsed = Mnemonic::parse_in_normalized(Language::English, words);
        assert!(parsed.is_ok());
        assert_eq!(
            words.split_ascii_whitespace().count(),
            RECOVERY_PHRASE_WORDS
        );
    })?;
    Ok(())
}

#[test]
fn phrase_can_only_be_presented_once() -> Result<(), RecoveryPhraseError> {
    let entropy = [0_u8; RECOVERY_ENTROPY_BYTES];
    let mut phrase = RecoveryPhrase::from_test_entropy(&entropy)?;

    phrase.present_once_for_onboarding(|words| {
        assert_eq!(
            words.split_ascii_whitespace().count(),
            RECOVERY_PHRASE_WORDS
        );
    })?;
    let second = phrase.present_once_for_onboarding(|_| ());

    assert_eq!(second, Err(RecoveryPhraseError::AlreadyPresented));
    Ok(())
}

#[test]
fn derived_seed_has_bip39_width() -> Result<(), RecoveryPhraseError> {
    let entropy = [0_u8; RECOVERY_ENTROPY_BYTES];
    let mut phrase = RecoveryPhrase::from_test_entropy(&entropy)?;
    assert_eq!(
        phrase.derive_seed().err(),
        Some(RecoveryPhraseError::VerificationRequired)
    );

    phrase.present_once_for_onboarding(|_| ())?;
    verify_zero_entropy_phrase(&mut phrase)?;
    let seed = phrase.derive_seed()?;

    assert_eq!(seed.expose_secret().len(), 64);
    assert!(seed.expose_secret().iter().any(|byte| *byte != 0));
    Ok(())
}

#[test]
fn verification_selects_three_distinct_bounded_positions() -> Result<(), RecoveryPhraseError> {
    let entropy = [0_u8; RECOVERY_ENTROPY_BYTES];
    let mut phrase = RecoveryPhrase::from_test_entropy(&entropy)?;
    phrase.present_once_for_onboarding(|_| ())?;

    let prompt = phrase.begin_verification()?;
    let positions = prompt.positions();

    assert_eq!(positions.len(), RECOVERY_VERIFICATION_WORDS);
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        positions.iter().all(|position| {
            position.number() >= 1 && position.number() <= RECOVERY_PHRASE_WORDS
        })
    );
    Ok(())
}

#[test]
fn verification_requires_presentation_and_all_warning_acknowledgements()
-> Result<(), RecoveryPhraseError> {
    let entropy = [0_u8; RECOVERY_ENTROPY_BYTES];
    let mut phrase = RecoveryPhrase::from_test_entropy(&entropy)?;
    assert_eq!(
        phrase.begin_verification().err(),
        Some(RecoveryPhraseError::NotPresented)
    );

    phrase.present_once_for_onboarding(|_| ())?;
    let prompt = phrase.begin_verification()?;
    assert_eq!(
        prompt.acknowledge_warnings(true, false, true).err(),
        Some(RecoveryPhraseError::WarningsNotAcknowledged)
    );

    let warnings = prompt.warnings();
    assert_eq!(
        warnings.no_cloud_backup(),
        "There is no cloud backup for this wallet."
    );
    assert!(
        warnings
            .device_and_phrase_loss()
            .contains("permanently lost")
    );
    assert!(
        warnings
            .support_cannot_recover()
            .starts_with("Support cannot recover")
    );
    Ok(())
}

#[test]
fn incorrect_words_consume_the_challenge_and_do_not_unlock_seed() -> Result<(), RecoveryPhraseError>
{
    let entropy = [0_u8; RECOVERY_ENTROPY_BYTES];
    let mut phrase = RecoveryPhrase::from_test_entropy(&entropy)?;
    phrase.present_once_for_onboarding(|_| ())?;
    let prompt = phrase.begin_verification()?;
    let acknowledgement = prompt.acknowledge_warnings(true, true, true)?;
    let answers = RecoveryWordAnswers::new(std::array::from_fn(|_| "wrong".to_owned()))?;

    assert_eq!(
        phrase
            .verify_recovery_words(&prompt, &acknowledgement, answers)
            .err(),
        Some(RecoveryPhraseError::IncorrectVerificationWords)
    );
    assert_eq!(
        phrase.derive_seed().err(),
        Some(RecoveryPhraseError::VerificationRequired)
    );
    assert!(phrase.begin_verification().is_ok());
    Ok(())
}

#[test]
fn verification_answers_are_strictly_bounded() {
    let answers = ["abandon".to_owned(), "UPPER".to_owned(), "art".to_owned()];
    assert_eq!(
        RecoveryWordAnswers::new(answers).err(),
        Some(RecoveryPhraseError::InvalidVerificationWord)
    );
}

fn verify_zero_entropy_phrase(phrase: &mut RecoveryPhrase) -> Result<(), RecoveryPhraseError> {
    let prompt = phrase.begin_verification()?;
    let acknowledgement = prompt.acknowledge_warnings(true, true, true)?;
    let answers = answers_for_zero_entropy_phrase(&prompt)?;
    phrase.verify_recovery_words(&prompt, &acknowledgement, answers)
}

fn answers_for_zero_entropy_phrase(
    prompt: &RecoveryVerificationPrompt,
) -> Result<RecoveryWordAnswers, RecoveryPhraseError> {
    let words = prompt.positions().map(|position| {
        if position.number() == RECOVERY_PHRASE_WORDS {
            "art".to_owned()
        } else {
            "abandon".to_owned()
        }
    });
    RecoveryWordAnswers::new(words)
}
