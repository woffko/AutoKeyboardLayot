//! Deterministic, offline wrong-layout detector.

use crate::UserLexicon;
use crate::dictionary;
use crate::language::{Language, can_extend_word, transpose_word};

const DICTIONARY_SCORE_BONUS: f32 = 10.0;
const MINIMUM_STATISTICAL_CHARACTERS: usize = 4;

/// Configuration for precision-first automatic detection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectorConfig {
    /// Absolute length floor. Two/three-letter words also require the common
    /// short-word tier or an explicitly added user-dictionary target.
    pub minimum_word_characters: usize,
    /// Minimum score required for the converted candidate.
    pub minimum_target_score: f32,
    /// Minimum lead over the word as typed in the current layout.
    pub minimum_score_margin: f32,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            minimum_word_characters: 2,
            minimum_target_score: 12.0,
            minimum_score_margin: 5.0,
        }
    }
}

/// A high-confidence candidate observed at a word boundary.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub source_language: Language,
    pub target_language: Language,
    pub original: String,
    pub replacement: String,
    pub source_score: f32,
    pub target_score: f32,
}

impl Detection {
    pub fn score_margin(&self) -> f32 {
        self.target_score - self.source_score
    }
}

/// Offline detector based on layout transposition and compact language models.
#[derive(Debug, Clone)]
pub struct Detector {
    config: DetectorConfig,
    user_dictionary: UserLexicon,
    word_exclusions: UserLexicon,
}

impl Default for Detector {
    fn default() -> Self {
        Self::new(DetectorConfig::default())
    }
}

impl Detector {
    pub fn new(config: DetectorConfig) -> Self {
        Self {
            config,
            user_dictionary: UserLexicon::default(),
            word_exclusions: UserLexicon::default(),
        }
    }

    pub const fn config(&self) -> DetectorConfig {
        self.config
    }

    pub fn replace_user_lexicons(
        &mut self,
        user_dictionary: UserLexicon,
        word_exclusions: UserLexicon,
    ) {
        self.user_dictionary = user_dictionary;
        self.word_exclusions = word_exclusions;
    }

    /// Whether a visible character can still be part of a word after the same
    /// physical key is interpreted under an enabled target layout.
    pub fn can_extend_word(&self, character: char, current_language: Language) -> bool {
        can_extend_word(character, current_language)
    }

    /// Detect whether `word` is substantially more plausible under the other
    /// enabled layout. No cloud service or persistent input history is used.
    pub fn detect(&self, word: &str, current_language: Language) -> Option<Detection> {
        self.detect_mapped_candidates(word, current_language, &[])
    }

    /// Evaluate target strings already mapped from the same physical keys by
    /// a platform adapter. More than one plausible target fails closed.
    pub fn detect_mapped_candidates(
        &self,
        word: &str,
        current_language: Language,
        candidates: &[(Language, String)],
    ) -> Option<Detection> {
        let character_count = word.chars().count();
        if character_count < self.config.minimum_word_characters {
            return None;
        }
        if self.word_exclusions.contains(current_language, word) {
            return None;
        }

        let source_is_word = word
            .chars()
            .all(|character| current_language.accepts_character(character));
        let short_word = character_count < MINIMUM_STATISTICAL_CHARACTERS;
        let source_is_known = if short_word {
            dictionary::common_short_contains(current_language, word)
                || self.user_dictionary.contains(current_language, word)
        } else {
            self.dictionary_contains(current_language, word)
        };
        if source_is_word && source_is_known {
            return None;
        }

        let mut source_score = score_word(word, current_language, &self.user_dictionary);
        if short_word && source_is_word && dictionary::contains(current_language, word) {
            // A general-list abbreviation is not as strong as a common word.
            source_score -= DICTIONARY_SCORE_BONUS;
        }
        let mut dictionary_candidate = None;
        let mut statistical_candidate = None;

        let resolved_candidates = resolve_candidates(word, current_language, candidates);

        for (target_language, replacement) in &resolved_candidates {
            let target_language = *target_language;
            if target_language == current_language {
                continue;
            }
            if !replacement
                .chars()
                .all(|character| target_language.accepts_character(character))
            {
                continue;
            }

            let target_in_dictionary = self.dictionary_contains(target_language, replacement);
            if short_word
                && !dictionary::common_short_contains(target_language, replacement)
                && !self.user_dictionary.contains(target_language, replacement)
            {
                continue;
            }
            if !source_is_word && !target_in_dictionary {
                continue;
            }
            let target_score = score_word(replacement, target_language, &self.user_dictionary);
            let detection = Detection {
                source_language: current_language,
                target_language,
                original: word.to_owned(),
                replacement: replacement.clone(),
                source_score,
                target_score,
            };
            if detection.target_score < self.config.minimum_target_score
                || detection.score_margin() < self.config.minimum_score_margin
            {
                continue;
            }

            if target_in_dictionary {
                if dictionary_candidate.is_some() {
                    return None;
                }
                dictionary_candidate = Some(detection);
            } else {
                if statistical_candidate.is_some() {
                    return None;
                }
                statistical_candidate = Some(detection);
            }
        }

        dictionary_candidate.or(statistical_candidate)
    }

    /// Explicitly convert the current word without automatic thresholds or a
    /// source-dictionary veto. A unique dictionary-backed target is preferred;
    /// otherwise the best target must have a clear score lead.
    pub fn force_mapped_candidates(
        &self,
        word: &str,
        current_language: Language,
        candidates: &[(Language, String)],
    ) -> Option<Detection> {
        if word.is_empty() {
            return None;
        }
        let source_score = score_word(word, current_language, &self.user_dictionary);
        let mut dictionary_candidates = Vec::new();
        let mut other_candidates = Vec::new();
        for (target_language, replacement) in resolve_candidates(word, current_language, candidates)
        {
            if replacement.is_empty()
                || !replacement
                    .chars()
                    .all(|character| target_language.accepts_character(character))
            {
                continue;
            }
            let target_score = score_word(&replacement, target_language, &self.user_dictionary);
            let detection = Detection {
                source_language: current_language,
                target_language,
                original: word.to_owned(),
                replacement,
                source_score,
                target_score,
            };
            if self.dictionary_contains(target_language, &detection.replacement) {
                dictionary_candidates.push(detection);
            } else {
                other_candidates.push(detection);
            }
        }

        if dictionary_candidates.is_empty() {
            choose_clear_best(other_candidates)
        } else {
            choose_clear_best(dictionary_candidates)
        }
    }

    fn dictionary_contains(&self, language: Language, word: &str) -> bool {
        dictionary::contains(language, word)
            || dictionary::common_short_contains(language, word)
            || self.user_dictionary.contains(language, word)
    }
}

fn resolve_candidates(
    word: &str,
    current_language: Language,
    candidates: &[(Language, String)],
) -> Vec<(Language, String)> {
    let mut resolved = Vec::new();
    for (target_language, replacement) in candidates {
        if *target_language != current_language
            && current_language
                .automatic_targets()
                .contains(target_language)
            && !resolved
                .iter()
                .any(|(existing, _)| existing == target_language)
        {
            resolved.push((*target_language, replacement.clone()));
        }
    }
    for &target_language in current_language.automatic_targets() {
        if resolved
            .iter()
            .any(|(existing, _)| *existing == target_language)
        {
            continue;
        }
        if let Some(replacement) = transpose_word(word, current_language, target_language) {
            resolved.push((target_language, replacement));
        }
    }
    resolved
}

fn choose_clear_best(mut candidates: Vec<Detection>) -> Option<Detection> {
    candidates.sort_by(|left, right| right.target_score.total_cmp(&left.target_score));
    let best = candidates.first()?;
    if candidates
        .get(1)
        .is_some_and(|second| best.target_score - second.target_score < 1.0)
    {
        return None;
    }
    Some(best.clone())
}

fn score_word(word: &str, language: Language, user_dictionary: &UserLexicon) -> f32 {
    let normalized = word.to_lowercase();
    let characters: Vec<char> = normalized.chars().collect();
    if characters.is_empty() {
        return f32::NEG_INFINITY;
    }

    let matching_script = characters
        .iter()
        .filter(|character| belongs_to_language(**character, language))
        .count();
    if matching_script != characters.len() {
        return -20.0;
    }

    let mut score = characters.len() as f32 * 1.25;
    if dictionary::contains(language, &normalized)
        || dictionary::common_short_contains(language, &normalized)
        || user_dictionary.contains(language, &normalized)
    {
        score += DICTIONARY_SCORE_BONUS;
    }

    score += ngram_score(&characters, bigrams(language), 2, 0.9, -0.2);
    score += ngram_score(&characters, trigrams(language), 3, 1.5, -0.1);

    let vowel_count = characters
        .iter()
        .filter(|character| vowels(language).contains(**character))
        .count();
    if characters.len() >= 4 && vowel_count == 0 {
        score -= 5.0;
    } else {
        let ratio = vowel_count as f32 / characters.len() as f32;
        if (0.15..=0.70).contains(&ratio) {
            score += 1.5;
        }
    }

    for sequence in rare_sequences(language) {
        if normalized.contains(sequence) {
            score -= 1.5;
        }
    }

    score
}

fn ngram_score(
    characters: &[char],
    common: &[&str],
    width: usize,
    hit_score: f32,
    miss_score: f32,
) -> f32 {
    if characters.len() < width {
        return 0.0;
    }

    characters
        .windows(width)
        .map(|window| {
            let sequence: String = window.iter().collect();
            if common.contains(&sequence.as_str()) {
                hit_score
            } else {
                miss_score
            }
        })
        .sum()
}

fn belongs_to_language(character: char, language: Language) -> bool {
    language.accepts_character(character)
}

fn vowels(language: Language) -> &'static str {
    match language {
        Language::English => "aeiouy",
        Language::Russian => "аеёиоуыэюя",
        Language::Estonian => "aeiouõäöü",
        Language::Japanese => "",
    }
}

fn bigrams(language: Language) -> &'static [&'static str] {
    match language {
        Language::English => &[
            "al", "an", "ar", "as", "at", "ca", "ch", "co", "de", "ea", "ed", "el", "en", "er",
            "es", "ha", "he", "hi", "ic", "in", "io", "is", "it", "le", "li", "ll", "lo", "me",
            "nd", "ne", "ng", "nt", "of", "on", "or", "ou", "ra", "re", "ri", "ro", "se", "st",
            "te", "th", "ti", "to", "ve",
        ],
        Language::Russian => &[
            "ал", "ва", "ве", "во", "го", "де", "ен", "ер", "ес", "ет", "ие", "ив", "ия", "ка",
            "ко", "ла", "ли", "на", "не", "ни", "но", "ов", "ор", "ос", "от", "по", "пр", "ра",
            "ре", "ри", "ро", "ст", "та", "те", "то", "ть", "ый", "ых",
        ],
        Language::Estonian | Language::Japanese => &[],
    }
}

fn trigrams(language: Language) -> &'static [&'static str] {
    match language {
        Language::English => &[
            "and", "ati", "ell", "ent", "ere", "for", "hat", "hel", "her", "ing", "ion", "lay",
            "ter", "tha", "the", "tio", "ver", "win",
        ],
        Language::Russian => &[
            "ать", "его", "ени", "иве", "как", "ого", "пер", "при", "про", "рас", "ста", "стр",
            "тек", "это", "язы",
        ],
        Language::Estonian | Language::Japanese => &[],
    }
}

fn rare_sequences(language: Language) -> &'static [&'static str] {
    match language {
        Language::English => &["bd", "dt", "hb", "tn", "wq", "zx"],
        Language::Russian => &["дд", "дщ", "жщ", "йй", "ъъ", "ьы"],
        Language::Estonian | Language::Japanese => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_english_keys_typed_under_english_layout_as_russian() {
        let detection = Detector::default()
            .detect("ghbdtn", Language::English)
            .expect("wrong-layout word should be detected");
        assert_eq!(detection.replacement, "привет");
        assert_eq!(detection.target_language, Language::Russian);
        assert!(detection.score_margin() >= DetectorConfig::default().minimum_score_margin);
    }

    #[test]
    fn detects_russian_keys_typed_under_russian_layout_as_english() {
        let detection = Detector::default()
            .detect("руддщ", Language::Russian)
            .expect("wrong-layout word should be detected");
        assert_eq!(detection.replacement, "hello");
        assert_eq!(detection.target_language, Language::English);
    }

    #[test]
    fn detects_additional_dictionary_backed_layout_mistakes() {
        let detector = Detector::default();
        let cases = [
            ("цщкдв", Language::Russian, "world"),
            ("дфнщге", Language::Russian, "layout"),
            ("hfcrkflrf", Language::English, "раскладка"),
            ("rkfdbfnehf", Language::English, "клавиатура"),
        ];

        for (typed, current_language, expected) in cases {
            let detection = detector
                .detect(typed, current_language)
                .unwrap_or_else(|| panic!("expected detection for {typed}"));
            assert_eq!(detection.replacement, expected);
        }
    }

    #[test]
    fn detects_real_phrase_words_including_letter_keys_shown_as_punctuation() {
        let detector = Detector::default();
        let cases = [
            ("ctqxfc", "сейчас"),
            ("dhjlt", "вроде"),
            ("hf,jnftn", "работает"),
            ("gthtrk.xtybt", "переключение"),
            ("cktle.ott", "следующее"),
            (",skj", "было"),
            ("gkfye", "плану"),
        ];

        for (typed, expected) in cases {
            let detection = detector
                .detect(typed, Language::English)
                .unwrap_or_else(|| panic!("expected detection for {typed}"));
            assert_eq!(detection.replacement, expected);
        }
    }

    #[test]
    fn leaves_correct_words_unchanged() {
        let detector = Detector::default();
        assert_eq!(detector.detect("hello", Language::English), None);
        assert_eq!(detector.detect("привет", Language::Russian), None);
        assert_eq!(detector.detect("switcher", Language::English), None);
        assert_eq!(detector.detect("переключение", Language::Russian), None);
    }

    #[test]
    fn user_dictionary_can_add_a_target_word_without_rebuilding() {
        let mut detector = Detector::default();
        let mut user_dictionary = UserLexicon::default();
        assert!(user_dictionary.insert(Language::Russian, "фывафыва"));
        detector.replace_user_lexicons(user_dictionary, UserLexicon::default());

        let detection = detector
            .detect("asdfasdf", Language::English)
            .expect("user target word should be detected");
        assert_eq!(detection.replacement, "фывафыва");
    }

    #[test]
    fn explicit_source_word_exclusion_vetoes_conversion() {
        let mut detector = Detector::default();
        let mut exclusions = UserLexicon::default();
        assert!(exclusions.insert(Language::English, "ghbdtn"));
        detector.replace_user_lexicons(UserLexicon::default(), exclusions);
        assert_eq!(detector.detect("ghbdtn", Language::English), None);
    }

    #[test]
    fn accepts_a_platform_mapped_estonian_dictionary_candidate() {
        let detector = Detector::default();
        let candidates = [(Language::Estonian, "tere".to_owned())];
        let detection = detector
            .detect_mapped_candidates("t;re", Language::English, &candidates)
            .expect("mapped Estonian word should be detected");
        assert_eq!(detection.target_language, Language::Estonian);
        assert_eq!(detection.replacement, "tere");
    }

    #[test]
    fn multiple_dictionary_targets_fail_closed() {
        let detector = Detector::default();
        let candidates = [
            (Language::English, "hello".to_owned()),
            (Language::Estonian, "tere".to_owned()),
        ];
        assert_eq!(
            detector.detect_mapped_candidates("абвгд", Language::Russian, &candidates),
            None
        );
    }

    #[test]
    fn force_conversion_handles_a_short_word_without_automatic_thresholds() {
        let detector = Detector::new(DetectorConfig {
            minimum_word_characters: 4,
            ..DetectorConfig::default()
        });
        let candidates = [
            (Language::Russian, "не".to_owned()),
            (Language::Estonian, "yt".to_owned()),
        ];
        assert_eq!(detector.detect("yt", Language::English), None);
        let forced = detector
            .force_mapped_candidates("yt", Language::English, &candidates)
            .expect("explicit conversion should select Russian");
        assert_eq!(forced.target_language, Language::Russian);
        assert_eq!(forced.replacement, "не");
    }

    #[test]
    fn force_conversion_bypasses_an_automatic_word_exclusion() {
        let mut detector = Detector::default();
        let mut exclusions = UserLexicon::default();
        assert!(exclusions.insert(Language::English, "ghbdtn"));
        detector.replace_user_lexicons(UserLexicon::default(), exclusions);
        let candidates = [(Language::Russian, "привет".to_owned())];
        assert!(
            detector
                .force_mapped_candidates("ghbdtn", Language::English, &candidates)
                .is_some()
        );
    }

    #[test]
    fn refuses_short_or_mixed_tokens() {
        let detector = Detector::default();
        assert_eq!(detector.detect("руд", Language::Russian), None);
        assert_eq!(detector.detect("hello42", Language::English), None);
        assert_eq!(detector.detect("hello.rs", Language::English), None);
    }

    #[test]
    fn converts_common_short_words_without_promoting_arbitrary_abbreviations() {
        let detector = Detector::default();
        for (original, language, expected) in [
            ("yt", Language::English, "не"),
            ("kju", Language::English, "лог"),
            ("Kju", Language::English, "Лог"),
            ("KJU", Language::English, "ЛОГ"),
            ("рш", Language::Russian, "hi"),
            ("рщц", Language::Russian, "how"),
            ("фку", Language::Russian, "are"),
            ("нщг", Language::Russian, "you"),
        ] {
            let result = detector
                .detect(original, language)
                .unwrap_or_else(|| panic!("missing {original}"));
            assert_eq!(result.replacement, expected);
        }
        for (word, language) in [
            ("не", Language::Russian),
            ("лог", Language::Russian),
            ("hi", Language::English),
            ("how", Language::English),
            ("are", Language::English),
            ("you", Language::English),
            ("cd", Language::English),
            ("on", Language::Estonian),
            ("руд", Language::Russian),
            ("ш", Language::Russian),
        ] {
            assert!(
                detector.detect(word, language).is_none(),
                "unexpected correction: {word}"
            );
        }
    }

    #[test]
    fn explicit_short_word_dictionary_and_exclusion_entries_are_authoritative() {
        let mut detector = Detector::default();
        detector.replace_user_lexicons(
            UserLexicon::from_lines(["en-US yt"]),
            UserLexicon::default(),
        );
        assert!(detector.detect("yt", Language::English).is_none());
        detector.replace_user_lexicons(
            UserLexicon::default(),
            UserLexicon::from_lines(["en-US yt"]),
        );
        assert!(detector.detect("yt", Language::English).is_none());
    }

    #[test]
    fn configuration_can_disable_borderline_detection() {
        let detector = Detector::new(DetectorConfig {
            minimum_word_characters: 4,
            minimum_target_score: 100.0,
            minimum_score_margin: 100.0,
        });
        assert_eq!(detector.detect("ghbdtn", Language::English), None);
    }
}
