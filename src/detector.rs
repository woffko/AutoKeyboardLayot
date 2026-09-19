//! Deterministic, offline wrong-layout detector.

use crate::input_capabilities::{InputBinding, bind_conservative_profile};
use crate::language::Language;
use crate::{DictionaryRegistry, UserLexicon};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

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
    /// Opt-in: one-character words are decided by the list-only policy in
    /// `detect_single_letter`. Statistical thresholds never apply to them.
    pub single_letter_words: bool,
}

impl Default for DetectorConfig {
    fn default() -> Self {
        Self {
            minimum_word_characters: 2,
            minimum_target_score: 12.0,
            minimum_score_margin: 5.0,
            single_letter_words: false,
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
    dictionaries: Arc<DictionaryRegistry>,
    // Code-owned exact-profile bindings, never OS-profile attestation.
    input_bindings: BTreeMap<Language, InputBinding>,
    resolved_input_packs: Option<BTreeSet<Language>>,
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
        Self::with_registry(config, Arc::new(DictionaryRegistry::embedded()))
    }

    /// Bind immutable data for this detector's lifetime. A runtime reload must
    /// construct a new detector/session snapshot rather than mutate active data.
    pub fn with_registry(config: DetectorConfig, dictionaries: Arc<DictionaryRegistry>) -> Self {
        Self::with_profile_selections(config, dictionaries, &Default::default())
    }

    /// Resolve explicit choices once when constructing the immutable snapshot.
    /// Choices cannot bypass descriptor, capability, collision or OS checks.
    pub fn with_profile_selections(
        config: DetectorConfig,
        dictionaries: Arc<DictionaryRegistry>,
        choices: &crate::input_profile_selection::InputProfileSelections,
    ) -> Self {
        let mut profile_claims = BTreeMap::new();
        for id in dictionaries.enabled_ids() {
            if let Some(descriptor) = dictionaries
                .active(id)
                .and_then(|pack| pack.input_descriptor())
            {
                for (profile, _) in descriptor.profiles() {
                    // A chosen profile reserves only itself. Without a choice,
                    // retain conservative collision blocking for ambiguous packs.
                    if choices.get(id).is_some_and(|chosen| chosen != *profile) {
                        continue;
                    }
                    *profile_claims.entry(*profile).or_insert(0_usize) += 1;
                }
            }
        }
        let input_bindings = dictionaries
            .enabled_ids()
            .copied()
            .filter_map(|id| {
                let pack = dictionaries.active(&id)?;
                pack.scoring_model()?;
                let (profile, requirements) = choices.resolve(pack.input_descriptor()?)?;
                if profile_claims.get(&profile) != Some(&1) {
                    return None;
                }
                Some((id, bind_conservative_profile(profile, requirements)?))
            })
            .collect();
        Self {
            config,
            dictionaries,
            input_bindings,
            resolved_input_packs: None,
            user_dictionary: UserLexicon::default(),
            word_exclusions: UserLexicon::default(),
        }
    }

    pub const fn config(&self) -> DetectorConfig {
        self.config
    }

    /// Full implementation assessment for an active package's exact requirement.
    /// This is deliberately independent of the conservative binding scope
    /// and OS resolution. It must never be treated as conversion authorization.
    pub fn profile_implementation(
        &self,
        language: Language,
        profile: crate::WindowsKeyboardProfile,
    ) -> Option<crate::input_capabilities::ProfileImplementation> {
        let requirements = self
            .dictionaries
            .active(&language)?
            .input_descriptor()?
            .profile(profile)?;
        Some(crate::input_capabilities::assess_profile_implementation(
            profile,
            requirements,
        ))
    }

    /// Targets from this immutable runtime snapshot, not the static language
    /// catalog. Exact OS layout validation is still the platform's responsibility.
    /// Unsupported or ambiguous profile requirements cannot borrow a binding.
    pub fn automatic_targets(&self, source: Language) -> impl Iterator<Item = Language> + '_ {
        self.input_bindings.keys().copied().filter(move |target| {
            self.input_pack_is_eligible(source)
                && self.input_pack_is_eligible(*target)
                && *target != source
        })
    }

    fn input_pack_is_eligible(&self, language: Language) -> bool {
        self.input_bindings.contains_key(&language)
            && self
                .resolved_input_packs
                .as_ref()
                .is_none_or(|packs| packs.contains(&language))
    }

    /// Platform mode: missing evidence means no input participation. Offline
    /// detector construction remains available for core tests/text-only callers.
    pub fn set_resolved_profiles(
        &mut self,
        profiles: Option<&crate::profile_resolver::ResolvedKeyboardProfiles>,
    ) {
        self.resolved_input_packs = Some(
            self.input_bindings
                .keys()
                .copied()
                .filter(|&language| {
                    self.input_profile(language).is_some_and(|profile| {
                        profiles
                            .and_then(|snapshot| snapshot.unique_layout(profile))
                            .is_some()
                    })
                })
                .collect(),
        );
    }

    /// Exact profile for this package's code-owned adapter binding. This is a
    /// requirement, not evidence that Windows currently has this profile loaded.
    pub fn input_profile(&self, language: Language) -> Option<crate::WindowsKeyboardProfile> {
        self.input_bindings
            .get(&language)
            .map(|binding| binding.profile())
    }

    pub fn input_scope(&self, language: Language) -> Option<crate::input_capabilities::InputScope> {
        self.input_bindings
            .get(&language)
            .map(|binding| binding.scope())
    }

    /// Resolve a platform-confirmed exact profile without language/variant fallback.
    pub fn language_for_profile(&self, profile: crate::WindowsKeyboardProfile) -> Option<Language> {
        let mut matches = self.input_bindings.keys().copied().filter(|&id| {
            self.input_pack_is_eligible(id) && self.input_profile(id) == Some(profile)
        });
        let language = matches.next()?;
        matches.next().is_none().then_some(language)
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
        if !self.input_pack_is_eligible(current_language) {
            return false;
        }
        self.accepts_character(current_language, character)
            || self.automatic_targets(current_language).any(|target| {
                self.input_bindings[&current_language]
                    .transpose_character(character, self.input_bindings[&target])
                    .is_some_and(|mapped| self.accepts_character(target, mapped))
            })
    }

    fn accepts_character(&self, language: Language, character: char) -> bool {
        self.dictionaries
            .active_language(language)
            .and_then(|pack| pack.scoring_model())
            .is_some_and(|model| character.to_lowercase().all(|c| model.accepts(c)))
    }

    fn accepts_word(&self, language: Language, word: &str) -> bool {
        self.dictionaries
            .active_language(language)
            .and_then(|pack| pack.scoring_model())
            .is_some_and(|model| model.accepts_word(&word.to_lowercase()))
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
        self.dictionaries.active_language(current_language)?;
        let character_count = word.chars().count();
        if character_count == 1 {
            if !self.config.single_letter_words {
                return None;
            }
            return self.detect_single_letter(word, current_language, candidates);
        }
        if character_count < self.config.minimum_word_characters {
            return None;
        }
        if self.word_exclusions.contains(current_language, word) {
            return None;
        }

        let source_is_word = self.accepts_word(current_language, word);
        let short_word = character_count < MINIMUM_STATISTICAL_CHARACTERS;
        let source_is_known = if short_word {
            self.common_short_contains(current_language, word)
                || self.user_dictionary.contains(current_language, word)
        } else {
            self.dictionary_contains(current_language, word)
        };
        if source_is_word && source_is_known {
            return None;
        }

        let mut source_score = score_word(
            word,
            current_language,
            &self.user_dictionary,
            &self.dictionaries,
        );
        if short_word && source_is_word && self.base_dictionary_contains(current_language, word) {
            // A general-list abbreviation is not as strong as a common word.
            source_score -= DICTIONARY_SCORE_BONUS;
        }
        let mut dictionary_candidate = None;
        let mut statistical_candidate = None;

        let resolved_candidates = self.resolve_candidates(word, current_language, candidates);

        for (target_language, replacement) in &resolved_candidates {
            let target_language = *target_language;
            if target_language == current_language
                || self.dictionaries.active_language(target_language).is_none()
            {
                continue;
            }
            if !self.accepts_word(target_language, replacement) {
                continue;
            }

            let target_in_dictionary = self.dictionary_contains(target_language, replacement);
            if !target_in_dictionary
                && !self
                    .dictionaries
                    .active_language(target_language)
                    .and_then(|pack| pack.scoring_model())
                    .is_some_and(|model| model.allows_statistical_targets())
            {
                continue;
            }
            if short_word
                && !self.common_short_contains(target_language, replacement)
                && !self.user_dictionary.contains(target_language, replacement)
            {
                continue;
            }
            if !source_is_word && !target_in_dictionary {
                continue;
            }
            let target_score = score_word(
                replacement,
                target_language,
                &self.user_dictionary,
                &self.dictionaries,
            );
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

    /// List-only policy for one-character words. A single letter has no
    /// statistical evidence, so membership in the target's short tier or the
    /// user dictionary is the whole decision. The base dictionary tier is
    /// ignored on purpose: runtime packages may list every letter there.
    fn detect_single_letter(
        &self,
        word: &str,
        current_language: Language,
        candidates: &[(Language, String)],
    ) -> Option<Detection> {
        if self.word_exclusions.contains(current_language, word) {
            return None;
        }
        let source_is_known = self.common_short_contains(current_language, word)
            || self.user_dictionary.contains(current_language, word);
        if self.accepts_word(current_language, word) && source_is_known {
            return None;
        }
        let source_score = score_word(
            word,
            current_language,
            &self.user_dictionary,
            &self.dictionaries,
        );
        let mut chosen = None;
        for (target_language, replacement) in
            self.resolve_candidates(word, current_language, candidates)
        {
            if target_language == current_language
                || self.dictionaries.active_language(target_language).is_none()
                || replacement.chars().count() != 1
                || replacement == word
                || !self.accepts_word(target_language, &replacement)
            {
                continue;
            }
            if !(self.common_short_contains(target_language, &replacement)
                || self.user_dictionary.contains(target_language, &replacement))
            {
                continue;
            }
            if chosen.is_some() {
                // Ambiguous targets fail closed.
                return None;
            }
            chosen = Some(Detection {
                source_language: current_language,
                target_language,
                original: word.to_owned(),
                replacement: replacement.clone(),
                source_score,
                target_score: score_word(
                    &replacement,
                    target_language,
                    &self.user_dictionary,
                    &self.dictionaries,
                ),
            });
        }
        chosen
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
        self.dictionaries.active_language(current_language)?;
        if word.is_empty() {
            return None;
        }
        let source_score = score_word(
            word,
            current_language,
            &self.user_dictionary,
            &self.dictionaries,
        );
        let mut dictionary_candidates = Vec::new();
        let mut other_candidates = Vec::new();
        for (target_language, replacement) in
            self.resolve_candidates(word, current_language, candidates)
        {
            if self.dictionaries.active_language(target_language).is_none()
                || replacement.is_empty()
                || !self.accepts_word(target_language, &replacement)
            {
                continue;
            }
            let target_score = score_word(
                &replacement,
                target_language,
                &self.user_dictionary,
                &self.dictionaries,
            );
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
        self.base_dictionary_contains(language, word)
            || self.common_short_contains(language, word)
            || self.user_dictionary.contains(language, word)
    }

    fn base_dictionary_contains(&self, language: Language, word: &str) -> bool {
        self.dictionaries
            .active_language(language)
            .is_some_and(|pack| pack.contains(word))
    }

    fn common_short_contains(&self, language: Language, word: &str) -> bool {
        self.dictionaries
            .active_language(language)
            .is_some_and(|pack| pack.common_short_contains(word))
    }
    fn resolve_candidates(
        &self,
        word: &str,
        current_language: Language,
        candidates: &[(Language, String)],
    ) -> Vec<(Language, String)> {
        let mut resolved = Vec::new();
        for (target_language, replacement) in candidates {
            if *target_language != current_language
                && self
                    .automatic_targets(current_language)
                    .any(|target| target == *target_language)
                && !resolved
                    .iter()
                    .any(|(existing, _)| existing == target_language)
            {
                resolved.push((*target_language, replacement.clone()));
            }
        }
        for target_language in self.automatic_targets(current_language) {
            if resolved
                .iter()
                .any(|(existing, _)| *existing == target_language)
            {
                continue;
            }
            if let Some(replacement) = self.input_bindings[&current_language]
                .transpose_word(word, self.input_bindings[&target_language])
            {
                resolved.push((target_language, replacement));
            }
        }
        resolved
    }
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

fn score_word(
    word: &str,
    language: Language,
    user_dictionary: &UserLexicon,
    dictionaries: &DictionaryRegistry,
) -> f32 {
    let Some(pack) = dictionaries.active_language(language) else {
        return f32::NEG_INFINITY;
    };
    let Some(model) = pack.scoring_model() else {
        return f32::NEG_INFINITY;
    };
    let normalized = word.to_lowercase();
    let characters: Vec<char> = normalized.chars().collect();
    if characters.is_empty() {
        return f32::NEG_INFINITY;
    }

    if !model.accepts_word(&normalized) {
        return -20.0;
    }

    let mut score = characters.len() as f32 * 1.25;
    if pack.contains(&normalized)
        || pack.common_short_contains(&normalized)
        || user_dictionary.contains(language, &normalized)
    {
        score += DICTIONARY_SCORE_BONUS;
    }

    if model.uses_bigrams() {
        score += ngram_score(&characters, model.bigrams(), 2, 0.9, -0.2);
    }
    if model.uses_trigrams() {
        score += ngram_score(&characters, model.trigrams(), 3, 1.5, -0.1);
    }

    if model.uses_vowels() {
        let vowel_count = characters
            .iter()
            .filter(|character| model.is_vowel(**character))
            .count();
        if characters.len() >= 4 && vowel_count == 0 {
            score -= 5.0;
        } else {
            let ratio = vowel_count as f32 / characters.len() as f32;
            if (0.15..=0.70).contains(&ratio) {
                score += 1.5;
            }
        }
    }

    for sequence in model.rare() {
        if normalized.contains(sequence) {
            score -= 1.5;
        }
    }

    score
}

fn ngram_score(
    characters: &[char],
    common: &std::collections::BTreeSet<String>,
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
            if common.contains(sequence.as_str()) {
                hit_score
            } else {
                miss_score
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DictionaryPack, PackId};

    fn custom_us_pack(id: PackId, profiles: &[&str]) -> DictionaryPack {
        let rows: Vec<_> = profiles
            .iter()
            .map(|profile| {
                serde_json::json!({
                    "profile":profile,"required_capabilities":["physical-key-v1"]
                })
            })
            .collect();
        let descriptor = crate::InputPackDescriptor::from_json(&serde_json::to_vec(
            &serde_json::json!({"format":1,"pack_id":id.as_str(),"windows_keyboard_profiles":rows})
        ).unwrap()).unwrap();
        DictionaryPack::from_words(id, ["hello"], [])
            .unwrap()
            .with_scoring_model(
                crate::ScoringModel::from_json(include_bytes!("../data/scoring/en-US.json"))
                    .unwrap(),
            )
            .with_input_descriptor(descriptor)
            .unwrap()
    }

    #[test]
    fn explicit_marks_score_but_do_not_bypass_token_or_edit_boundaries() {
        let id = PackId::parse("custom-us").unwrap();
        let model = crate::ScoringModel::from_json(&serde_json::to_vec(&serde_json::json!({
            "format":3,"ranges":[["a","z"]],"marks":"\u{0301}",
            "vowels":"","bigrams":[],"trigrams":[],"rare":[],
            "policy":{"bigrams":false,"trigrams":false,"vowels":false,"statistical_targets":false}
        })).unwrap()).unwrap();
        let descriptor = custom_us_pack(id, &["0409:00000409"])
            .input_descriptor()
            .unwrap()
            .clone();
        let pack = DictionaryPack::from_words(id, ["cafe\u{0301}", "\u{0301}cafe"], [])
            .unwrap()
            .with_scoring_model(model)
            .with_input_descriptor(descriptor)
            .unwrap();
        let mut registry = crate::test_support::registry();
        registry.insert(pack).unwrap();
        registry.set_enabled([id, Language::Russian]).unwrap();
        assert_eq!(
            score_word("cafe\u{0301}", id, &UserLexicon::default(), &registry),
            6.25 + DICTIONARY_SCORE_BONUS
        );
        assert_eq!(
            score_word("\u{0301}cafe", id, &UserLexicon::default(), &registry),
            -20.0
        );
        let detector = Detector::with_registry(
            DetectorConfig {
                minimum_target_score: -100.0,
                minimum_score_margin: -100.0,
                ..Default::default()
            },
            Arc::new(registry),
        );
        assert!(detector.can_extend_word('\u{0301}', id));
        assert!(detector.detect("cafe\u{0301}", id).is_none());
        let candidates = [(id, "cafe\u{0301}".to_owned())];
        let detection = detector
            .detect_mapped_candidates("жжжжж", Language::Russian, &candidates)
            .unwrap();
        assert!(crate::ConversionTransaction::without_delimiter(&detection).is_none());
        let invalid = [(id, "\u{0301}cafe".to_owned())];
        assert!(
            detector
                .detect_mapped_candidates("жжжжж", Language::Russian, &invalid)
                .is_none()
        );
        assert!(
            detector
                .force_mapped_candidates("жжжжж", Language::Russian, &invalid)
                .is_none()
        );
    }

    #[test]
    fn disabled_features_do_not_penalize_words_or_enable_statistical_targets() {
        let id = PackId::parse("custom-us").unwrap();
        let model = crate::ScoringModel::from_json(
            br#"{"format":2,
            "ranges":[["a","z"]],"vowels":"","bigrams":[],"trigrams":[],"rare":[],
            "policy":{"bigrams":false,"trigrams":false,"vowels":false,
            "statistical_targets":false}}"#,
        )
        .unwrap();
        let mut registry = crate::test_support::registry();
        registry
            .insert(custom_us_pack(id, &["0409:00000409"]).with_scoring_model(model))
            .unwrap();
        registry.set_enabled([id, Language::Russian]).unwrap();
        assert_eq!(
            score_word("bcdfgh", id, &UserLexicon::default(), &registry),
            7.5
        );
        assert_eq!(
            score_word("hello", id, &UserLexicon::default(), &registry),
            6.25 + DICTIONARY_SCORE_BONUS
        );
        let detector = Detector::with_registry(
            DetectorConfig {
                minimum_target_score: -100.0,
                minimum_score_margin: -100.0,
                ..Default::default()
            },
            Arc::new(registry),
        );
        assert!(
            detector
                .detect_mapped_candidates("жжжжжж", Language::Russian, &[(id, "bcdfgh".to_owned())])
                .is_none()
        );
        assert!(
            detector
                .detect_mapped_candidates("руддщ", Language::Russian, &[(id, "hello".to_owned())])
                .is_some()
        );
    }

    #[test]
    fn custom_id_uses_exact_profile_for_offline_and_mapped_conversion() {
        let custom = PackId::parse("custom-us").unwrap();
        let us = crate::WindowsKeyboardProfile::parse("0409:00000409").unwrap();
        let mut registry = crate::test_support::registry();
        registry
            .insert(custom_us_pack(custom, &["0409:00000409"]))
            .unwrap();
        registry.set_enabled([custom, Language::Russian]).unwrap();
        let detector = Detector::with_registry(Default::default(), Arc::new(registry));
        assert_eq!(detector.input_profile(custom), Some(us));
        assert_eq!(detector.language_for_profile(us), Some(custom));
        assert_eq!(
            detector.input_scope(custom),
            Some(crate::input_capabilities::InputScope::ConservativePhysicalKeys)
        );
        assert!(detector.can_extend_word('[', custom));
        let detection = detector.detect("ghbdtn", custom).unwrap();
        assert_eq!(detection.source_language, custom);
        assert_eq!(detection.replacement, "привет");
        let detection = detector.detect("руддщ", Language::Russian).unwrap();
        assert_eq!(detection.target_language, custom);
        assert_eq!(detection.replacement, "hello");
        assert!(
            detector
                .force_mapped_candidates("ghbdtn", custom, &[])
                .is_some()
        );
        assert!(
            detector
                .detect_mapped_candidates(
                    "руддщ",
                    Language::Russian,
                    &[(custom, "hello".to_owned())]
                )
                .is_some()
        );
    }

    #[test]
    fn duplicate_profile_claims_block_every_owner_until_selection_changes() {
        let custom = PackId::parse("custom-us").unwrap();
        let mut registry = crate::test_support::registry();
        registry
            .insert(custom_us_pack(custom, &["0409:00000409"]))
            .unwrap();
        registry
            .set_enabled([custom, Language::English, Language::Russian])
            .unwrap();
        let previous = Detector::with_registry(Default::default(), Arc::new(registry.clone()));
        for id in [custom, Language::English] {
            assert_eq!(previous.input_profile(id), None);
            assert!(previous.automatic_targets(id).next().is_none());
            assert!(
                previous
                    .force_mapped_candidates(
                        "ghbdtn",
                        id,
                        &[(Language::Russian, "привет".to_owned())]
                    )
                    .is_none()
            );
        }
        registry.set_enabled([custom, Language::Russian]).unwrap();
        let next = Detector::with_registry(Default::default(), Arc::new(registry));
        assert!(next.detect("ghbdtn", custom).is_some());
        assert_eq!(previous.input_profile(custom), None);
    }

    #[test]
    fn multiple_profile_requirements_need_selection_even_if_only_one_is_supported() {
        let custom = PackId::parse("custom-us").unwrap();
        let mut registry = crate::test_support::registry();
        registry
            .insert(custom_us_pack(custom, &["0409:00000409", "0409:00020409"]))
            .unwrap();
        registry.set_enabled([custom, Language::Russian]).unwrap();
        let detector = Detector::with_registry(Default::default(), Arc::new(registry));
        assert_eq!(detector.input_profile(custom), None);
        assert!(detector.detect("ghbdtn", custom).is_none());
        assert!(
            detector
                .force_mapped_candidates("ghbdtn", custom, &[])
                .is_none()
        );
    }

    #[test]
    fn explicit_profile_choice_is_exact_and_snapshot_owned() {
        use crate::input_profile_selection::InputProfileSelections;
        let custom = PackId::parse("custom-us").unwrap();
        let us = crate::WindowsKeyboardProfile::parse("0409:00000409").unwrap();
        let mut registry = crate::test_support::registry();
        registry
            .insert(custom_us_pack(custom, &["0409:00000409", "0409:00020409"]))
            .unwrap();
        registry.set_enabled([custom, Language::Russian]).unwrap();
        let registry = Arc::new(registry);
        let choices = InputProfileSelections::parse([("custom-us", "0409:00000409")]).unwrap();
        let mut selected =
            Detector::with_profile_selections(Default::default(), registry.clone(), &choices);
        assert_eq!(selected.input_profile(custom), Some(us));
        assert_eq!(
            selected.detect("ghbdtn", custom).unwrap().replacement,
            "привет"
        );
        for profile in ["0409:00020409", "0809:00000409", "0419:00000419"] {
            let changed = InputProfileSelections::parse([("custom-us", profile)]).unwrap();
            let next =
                Detector::with_profile_selections(Default::default(), registry.clone(), &changed);
            assert_eq!(next.input_profile(custom), None);
            assert!(next.detect("ghbdtn", custom).is_none());
            assert!(
                next.force_mapped_candidates("ghbdtn", custom, &[])
                    .is_none()
            );
            assert_eq!(selected.input_profile(custom), Some(us));
        }
        selected.set_resolved_profiles(None);
        assert!(selected.detect("ghbdtn", custom).is_none());
        assert!(
            selected
                .automatic_targets(Language::Russian)
                .next()
                .is_none()
        );
    }

    #[test]
    fn explicit_choices_reserve_only_the_chosen_declared_profile() {
        use crate::input_profile_selection::InputProfileSelections;
        let custom = PackId::parse("custom-us").unwrap();
        let us = crate::WindowsKeyboardProfile::parse("0409:00000409").unwrap();
        let ru = crate::WindowsKeyboardProfile::parse("0419:00000419").unwrap();
        let mut registry = crate::test_support::registry();
        registry
            .insert(custom_us_pack(custom, &["0409:00000409", "0419:00000419"]))
            .unwrap();
        registry
            .set_enabled([custom, Language::English, Language::Russian])
            .unwrap();
        let registry = Arc::new(registry);
        let choices = InputProfileSelections::parse([("custom-us", "0409:00000409")]).unwrap();
        let selected =
            Detector::with_profile_selections(Default::default(), registry.clone(), &choices);
        assert_eq!(selected.input_profile(custom), None);
        assert_eq!(selected.input_profile(Language::English), None);
        assert_eq!(selected.input_profile(Language::Russian), Some(ru));
        let choices = InputProfileSelections::parse([("custom-us", "0419:00000419")]).unwrap();
        let selected =
            Detector::with_profile_selections(Default::default(), registry.clone(), &choices);
        assert_eq!(selected.input_profile(custom), None);
        assert_eq!(selected.input_profile(Language::Russian), None);
        assert_eq!(selected.input_profile(Language::English), Some(us));
        let choices = InputProfileSelections::parse([("custom-us", "0409:00020409")]).unwrap();
        let selected = Detector::with_profile_selections(Default::default(), registry, &choices);
        assert_eq!(selected.input_profile(custom), None);
        assert_eq!(selected.input_profile(Language::Russian), Some(ru));
        assert_eq!(selected.input_profile(Language::English), Some(us));
    }

    #[test]
    fn explicit_stale_single_profile_choice_never_falls_back() {
        use crate::input_profile_selection::InputProfileSelections;
        let choices = InputProfileSelections::parse([
            ("en-US", "0409:00020409"),
            ("missing-pack", "0419:00000419"),
        ])
        .unwrap();
        let selected = Detector::with_profile_selections(
            Default::default(),
            Arc::new(crate::test_support::registry()),
            &choices,
        );
        assert_eq!(selected.input_profile(Language::English), None);
        assert!(selected.input_profile(Language::Russian).is_some());
        assert!(selected.detect("ghbdtn", Language::English).is_none());
        assert_eq!(choices.iter().count(), 2);
    }

    #[test]
    fn implementation_assessment_does_not_bypass_platform_eligibility() {
        use crate::input_capabilities::ProfileImplementation;
        let us = crate::WindowsKeyboardProfile::parse("0409:00000409").unwrap();
        let et = crate::WindowsKeyboardProfile::parse("0425:00000425").unwrap();
        let mut detector = crate::test_support::detector();
        detector.set_resolved_profiles(None);
        assert_eq!(
            detector.profile_implementation(Language::English, us),
            Some(ProfileImplementation::Implemented)
        );
        assert!(matches!(
            detector.profile_implementation(Language::Estonian, et),
            Some(ProfileImplementation::MissingCapabilities(_))
        ));
        assert_eq!(detector.profile_implementation(Language::English, et), None);
        assert!(
            detector
                .automatic_targets(Language::English)
                .next()
                .is_none()
        );
        assert_eq!(detector.language_for_profile(us), None);

        let mut registry = crate::test_support::registry();
        registry.remove(&Language::English).unwrap();
        let detector = Detector::with_registry(Default::default(), Arc::new(registry));
        assert_eq!(detector.profile_implementation(Language::English, us), None);
    }

    #[test]
    fn word_data_with_a_builtin_id_does_not_supply_missing_input_requirements() {
        let mut registry = crate::test_support::registry();
        registry.remove(&Language::English).unwrap();
        let bare = DictionaryPack::from_words(Language::English, ["hello"], []).unwrap();
        assert!(bare.input_descriptor().is_none());
        assert!(bare.scoring_model().is_some());
        registry.insert(bare).unwrap();
        let detector = Detector::with_registry(Default::default(), Arc::new(registry));
        assert!(
            detector
                .automatic_targets(Language::English)
                .next()
                .is_none()
        );
        assert!(
            !detector
                .automatic_targets(Language::Russian)
                .any(|target| target == Language::English)
        );
        assert!(!detector.can_extend_word('h', Language::English));
        for (source, target, word, mapped) in [
            (Language::English, Language::Russian, "ghbdtn", "привет"),
            (Language::Russian, Language::English, "руддщ", "hello"),
        ] {
            let candidates = [(target, mapped.to_owned())];
            assert!(detector.detect(word, source).is_none());
            assert!(
                detector
                    .detect_mapped_candidates(word, source, &candidates)
                    .is_none()
            );
            assert!(
                detector
                    .force_mapped_candidates(word, source, &candidates)
                    .is_none()
            );
        }
    }

    #[test]
    fn changed_profile_requirements_cannot_borrow_the_legacy_input_route() {
        let original = crate::test_support::registry();
        let old_detector = Detector::with_registry(Default::default(), Arc::new(original.clone()));
        let baseline: serde_json::Value =
            serde_json::from_slice(include_bytes!("../data/input/en-US.json")).unwrap();
        let mut variants = Vec::new();
        let mut value = baseline.clone();
        value["windows_keyboard_profiles"][0]["profile"] = serde_json::json!("0409:00020409");
        variants.push(value);
        let mut value = baseline.clone();
        value["windows_keyboard_profiles"][0]["profile"] = serde_json::json!("0809:00000409");
        variants.push(value);
        let mut value = baseline.clone();
        value["windows_keyboard_profiles"][0]["required_capabilities"] =
            serde_json::json!(["physical-key-v1", "future-adapter-v9"]);
        variants.push(value);
        let mut value = baseline;
        value["windows_keyboard_profiles"][0]["required_capabilities"] =
            serde_json::json!(["unrecognized-v1"]);
        variants.push(value);
        for value in variants {
            let mut next = original.clone();
            next.remove(&Language::English).unwrap();
            let descriptor =
                crate::InputPackDescriptor::from_json(&serde_json::to_vec(&value).unwrap())
                    .unwrap();
            next.insert(
                DictionaryPack::from_words(Language::English, ["hello"], [])
                    .unwrap()
                    .with_input_descriptor(descriptor)
                    .unwrap(),
            )
            .unwrap();
            let detector = Detector::with_registry(Default::default(), Arc::new(next));
            assert!(!detector.can_extend_word('h', Language::English));
            assert!(
                detector
                    .automatic_targets(Language::English)
                    .next()
                    .is_none()
            );
            assert!(
                !detector
                    .automatic_targets(Language::Russian)
                    .any(|target| target == Language::English)
            );
            for (source, target, word, mapped) in [
                (Language::English, Language::Russian, "ghbdtn", "привет"),
                (Language::Russian, Language::English, "руддщ", "hello"),
            ] {
                let candidates = [(target, mapped.to_owned())];
                assert!(detector.detect(word, source).is_none());
                assert!(
                    detector
                        .detect_mapped_candidates(word, source, &candidates)
                        .is_none()
                );
                assert!(
                    detector
                        .force_mapped_candidates(word, source, &candidates)
                        .is_none()
                );
                assert!(old_detector.detect(word, source).is_some());
            }
        }
    }

    #[test]
    fn runtime_target_list_respects_active_snapshot_and_normalized_descriptors() {
        let mut registry = crate::test_support::registry();
        registry.remove(&Language::English).unwrap();
        let descriptor = crate::InputPackDescriptor::from_json(
            br#"{
            "format":1,"pack_id":"EN-us","windows_keyboard_profiles":[
                {"profile":"0409:00000409","required_capabilities":["physical-key-v1"]}
            ]
        }"#,
        )
        .unwrap();
        registry
            .insert(
                DictionaryPack::from_words(Language::English, ["hello"], [])
                    .unwrap()
                    .with_input_descriptor(descriptor)
                    .unwrap(),
            )
            .unwrap();
        registry
            .set_enabled([Language::English, Language::Russian])
            .unwrap();
        let previous = Detector::with_registry(Default::default(), Arc::new(registry.clone()));
        assert_eq!(
            previous
                .automatic_targets(Language::English)
                .collect::<Vec<_>>(),
            [Language::Russian]
        );
        assert_eq!(
            previous
                .automatic_targets(Language::Russian)
                .collect::<Vec<_>>(),
            [Language::English]
        );
        assert!(
            previous
                .automatic_targets(Language::Estonian)
                .next()
                .is_none()
        );
        assert!(previous.detect("ghbdtn", Language::English).is_some());
        registry.remove(&Language::Russian).unwrap();
        let next = Detector::with_registry(Default::default(), Arc::new(registry));
        assert!(next.automatic_targets(Language::English).next().is_none());
        assert!(next.automatic_targets(Language::Russian).next().is_none());
        assert!(next.detect("ghbdtn", Language::English).is_none());
        assert!(previous.detect("ghbdtn", Language::English).is_some());
    }

    #[test]
    fn dynamic_identity_with_overlay_cannot_borrow_a_conflicting_profile() {
        let unknown = PackId::parse("de-DE").unwrap();
        let mut registry = crate::test_support::registry();
        registry
            .insert(
                DictionaryPack::from_words(unknown, ["hallo"], [])
                    .unwrap()
                    .with_scoring_model(
                        crate::ScoringModel::from_json(include_bytes!(
                            "../data/scoring/en-US.json"
                        ))
                        .unwrap(),
                    )
                    .with_input_descriptor(
                        crate::InputPackDescriptor::from_json(
                            br#"{
                        "format":1,"pack_id":"de-DE","windows_keyboard_profiles":[
                            {"profile":"0409:00000409","required_capabilities":["physical-key-v1"]}
                        ]
                    }"#,
                        )
                        .unwrap(),
                    )
                    .unwrap(),
            )
            .unwrap();
        registry.set_enabled([Language::English, unknown]).unwrap();
        let mut detector = Detector::with_registry(DetectorConfig::default(), Arc::new(registry));
        detector.replace_user_lexicons(
            UserLexicon::from_lines(["de-DE: hallo"]),
            UserLexicon::default(),
        );
        assert!(detector.user_dictionary.contains(unknown, "hallo"));
        assert!(
            score_word(
                "hallo",
                unknown,
                &detector.user_dictionary,
                &detector.dictionaries
            ) > 10.0
        );
        assert!(!detector.can_extend_word('h', unknown));
        for (source, target, word, replacement) in [
            (unknown, Language::English, "hallo", "hello"),
            (Language::English, unknown, "xxxxx", "hallo"),
        ] {
            let candidates = [(target, replacement.to_owned())];
            assert!(detector.detect(word, source).is_none());
            assert!(
                detector
                    .detect_mapped_candidates(word, source, &candidates)
                    .is_none()
            );
            assert!(
                detector
                    .force_mapped_candidates(word, source, &candidates)
                    .is_none()
            );
        }
    }

    #[test]
    fn runtime_scoring_and_character_data_are_snapshot_owned() {
        let original = crate::test_support::registry();
        let mut changed = original.clone();
        changed.remove(&Language::English).unwrap();
        let model = crate::ScoringModel::from_json(br#"{"format":1,"ranges":[["a","c"]],"vowels":"a","bigrams":[],"trigrams":[],"rare":[]}"#).unwrap();
        changed
            .insert(
                DictionaryPack::from_words(Language::English, ["abc"], [])
                    .unwrap()
                    .with_scoring_model(model)
                    .with_input_descriptor(
                        crate::InputPackDescriptor::from_json(include_bytes!(
                            "../data/input/en-US.json"
                        ))
                        .unwrap(),
                    )
                    .unwrap(),
            )
            .unwrap();
        changed.set_enabled([Language::English]).unwrap();
        let old = Detector::with_registry(DetectorConfig::default(), Arc::new(original));
        let new = Detector::with_registry(DetectorConfig::default(), Arc::new(changed));
        assert!(old.can_extend_word('d', Language::English));
        assert!(!new.can_extend_word('d', Language::English));
        assert!(new.can_extend_word('A', Language::English));
        assert!(!new.can_extend_word('@', Language::English));
        for (word, language, expected) in [
            ("привет", Language::Russian, 26.3),
            ("tere", Language::Estonian, 15.7),
        ] {
            assert!(
                (score_word(word, language, &old.user_dictionary, &old.dictionaries) - expected)
                    .abs()
                    < 0.001
            );
        }
        let unknown = PackId::parse("de-DE").unwrap();
        let mut missing_model = DictionaryRegistry::default();
        let bare = DictionaryPack::from_words(unknown, ["hallo"], []).unwrap();
        assert!(bare.scoring_model().is_none());
        missing_model.insert(bare).unwrap();
        missing_model.set_enabled([unknown]).unwrap();
        assert_eq!(
            score_word("hallo", unknown, &UserLexicon::default(), &missing_model),
            f32::NEG_INFINITY
        );
        assert!(
            (score_word(
                "hello",
                Language::English,
                &old.user_dictionary,
                &old.dictionaries
            ) - 24.25)
                .abs()
                < 0.001
        );
        assert_eq!(
            score_word(
                "hello",
                Language::English,
                &new.user_dictionary,
                &new.dictionaries
            ),
            -20.0
        );
        assert!(
            score_word(
                "abc",
                Language::English,
                &new.user_dictionary,
                &new.dictionaries
            ) > 10.0
        );
    }

    #[test]
    fn absent_source_or_target_fails_closed_even_with_user_words_and_forcing() {
        for selected in [
            vec![],
            vec!["en-US"],
            vec!["ru-RU"],
            vec!["de-DE"],
            vec!["en", "ru-RU"],
        ] {
            let mut registry = crate::test_support::registry();
            registry
                .set_enabled(selected.into_iter().map(|id| PackId::parse(id).unwrap()))
                .unwrap();
            let mut detector =
                Detector::with_registry(DetectorConfig::default(), Arc::new(registry));
            detector.replace_user_lexicons(
                UserLexicon::from_lines(["ru-RU: привет", "en-US: hello"]),
                UserLexicon::default(),
            );
            for (word, source, target, mapped) in [
                ("ghbdtn", Language::English, Language::Russian, "привет"),
                ("руддщ", Language::Russian, Language::English, "hello"),
            ] {
                assert!(detector.detect(word, source).is_none());
                let candidates = [(target, mapped.to_owned())];
                assert!(
                    detector
                        .detect_mapped_candidates(word, source, &candidates)
                        .is_none()
                );
                assert!(
                    detector
                        .force_mapped_candidates(word, source, &candidates)
                        .is_none()
                );
            }
        }
    }

    #[test]
    fn removal_does_not_change_an_existing_detector_or_erase_its_overlays() {
        let original = crate::test_support::registry();
        let mut next = original.clone();
        next.remove(&PackId::parse("ru-RU").unwrap()).unwrap();
        let mut before =
            Detector::with_registry(DetectorConfig::default(), Arc::new(original.clone()));
        let mut after = Detector::with_registry(DetectorConfig::default(), Arc::new(next));
        let words = UserLexicon::from_lines(["ru-RU: привет"]);
        let exclusions = UserLexicon::from_lines(["en-US: hfcrkflrf"]);
        before.replace_user_lexicons(words.clone(), exclusions.clone());
        after.replace_user_lexicons(words.clone(), exclusions.clone());
        assert!(before.detect("ghbdtn", Language::English).is_some());
        assert!(after.detect("ghbdtn", Language::English).is_none());
        assert_eq!(after.user_dictionary, words);
        assert_eq!(after.word_exclusions, exclusions);
        let mut restored = Detector::with_registry(DetectorConfig::default(), Arc::new(original));
        restored.replace_user_lexicons(after.user_dictionary, after.word_exclusions);
        assert!(restored.detect("ghbdtn", Language::English).is_some());
        assert!(restored.detect("hfcrkflrf", Language::English).is_none());
    }

    #[test]
    fn owned_runtime_words_are_used_but_do_not_grant_an_ime_adapter() {
        let mut registry = DictionaryRegistry::default();
        for (id, words) in [
            ("en-US", vec!["hello"]),
            ("ru-RU", vec!["привет"]),
            ("ja-JP", vec!["こんにちは"]),
        ] {
            let id = PackId::parse(id).unwrap();
            let mut pack = DictionaryPack::from_words(id, words, []).unwrap();
            if let Some(model) = crate::test_support::registry()
                .active(&id)
                .and_then(|pack| pack.scoring_model().cloned())
            {
                pack = pack.with_scoring_model(model);
            }
            if let Some(descriptor) = crate::test_support::descriptor(&id) {
                pack = pack
                    .with_input_descriptor(descriptor.as_ref().clone())
                    .unwrap();
            }
            registry.insert(pack).unwrap();
        }
        registry
            .set_enabled(["en-US", "ru-RU", "ja-JP"].map(|id| PackId::parse(id).unwrap()))
            .unwrap();
        let detector =
            Detector::with_registry(DetectorConfig::default(), Arc::new(registry.clone()));
        assert_eq!(
            detector
                .detect("ghbdtn", Language::English)
                .unwrap()
                .replacement,
            "привет"
        );
        registry
            .set_enabled(["en-US", "ja-JP"].map(|id| PackId::parse(id).unwrap()))
            .unwrap();
        let detector = Detector::with_registry(DetectorConfig::default(), Arc::new(registry));
        assert!(
            detector
                .force_mapped_candidates(
                    "hello",
                    Language::English,
                    &[(Language::Japanese, "こんにちは".to_owned())]
                )
                .is_none()
        );
        assert!(
            detector
                .detect_mapped_candidates(
                    "hello",
                    Language::Japanese,
                    &[(Language::Russian, "привет".to_owned())]
                )
                .is_none()
        );
    }

    #[test]
    fn detects_english_keys_typed_under_english_layout_as_russian() {
        let detection = crate::test_support::detector()
            .detect("ghbdtn", Language::English)
            .expect("wrong-layout word should be detected");
        assert_eq!(detection.replacement, "привет");
        assert_eq!(detection.target_language, Language::Russian);
        assert!(detection.score_margin() >= DetectorConfig::default().minimum_score_margin);
    }

    #[test]
    fn detects_russian_keys_typed_under_russian_layout_as_english() {
        let detection = crate::test_support::detector()
            .detect("руддщ", Language::Russian)
            .expect("wrong-layout word should be detected");
        assert_eq!(detection.replacement, "hello");
        assert_eq!(detection.target_language, Language::English);
    }

    #[test]
    fn detects_additional_dictionary_backed_layout_mistakes() {
        let detector = crate::test_support::detector();
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
        let detector = crate::test_support::detector();
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
        let detector = crate::test_support::detector();
        assert_eq!(detector.detect("hello", Language::English), None);
        assert_eq!(detector.detect("привет", Language::Russian), None);
        assert_eq!(detector.detect("switcher", Language::English), None);
        assert_eq!(detector.detect("переключение", Language::Russian), None);
    }

    #[test]
    fn user_dictionary_can_add_a_target_word_without_rebuilding() {
        let mut detector = crate::test_support::detector();
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
        let mut detector = crate::test_support::detector();
        let mut exclusions = UserLexicon::default();
        assert!(exclusions.insert(Language::English, "ghbdtn"));
        detector.replace_user_lexicons(UserLexicon::default(), exclusions);
        assert_eq!(detector.detect("ghbdtn", Language::English), None);
    }

    #[test]
    fn accepts_a_platform_mapped_estonian_dictionary_candidate() {
        let detector = crate::test_support::detector();
        let candidates = [(Language::Estonian, "tere".to_owned())];
        let detection = detector
            .detect_mapped_candidates("t;re", Language::English, &candidates)
            .expect("mapped Estonian word should be detected");
        assert_eq!(detection.target_language, Language::Estonian);
        assert_eq!(detection.replacement, "tere");
    }

    #[test]
    fn multiple_dictionary_targets_fail_closed() {
        let detector = crate::test_support::detector();
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
        let detector = crate::test_support::configured_detector(DetectorConfig {
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
        let mut detector = crate::test_support::detector();
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
        let detector = crate::test_support::detector();
        assert_eq!(detector.detect("руд", Language::Russian), None);
        assert_eq!(detector.detect("hello42", Language::English), None);
        assert_eq!(detector.detect("hello.rs", Language::English), None);
    }

    #[test]
    fn converts_common_short_words_without_promoting_arbitrary_abbreviations() {
        let detector = crate::test_support::detector();
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
        let mut detector = crate::test_support::detector();
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
    fn single_letter_conversion_is_opt_in() {
        let detector = crate::test_support::detector();
        for (word, language) in [
            ("z", Language::English),
            ("Z", Language::English),
            ("f", Language::English),
            ("b", Language::English),
            ("d", Language::English),
            ("r", Language::English),
            ("j", Language::English),
            ("c", Language::English),
            ("e", Language::English),
            ("a", Language::English),
            ("i", Language::English),
            ("я", Language::Russian),
        ] {
            assert_eq!(detector.detect(word, language), None, "{word}");
        }
    }

    #[test]
    fn single_letter_detection_uses_lists_and_fails_closed() {
        let detector = crate::test_support::configured_detector(DetectorConfig {
            single_letter_words: true,
            ..Default::default()
        });
        for (original, language, expected) in [
            ("z", Language::English, "я"),
            ("Z", Language::English, "Я"),
            ("f", Language::English, "а"),
            ("b", Language::English, "и"),
            ("d", Language::English, "в"),
            ("r", Language::English, "к"),
            ("j", Language::English, "о"),
            ("c", Language::English, "с"),
            ("e", Language::English, "у"),
            ("ф", Language::Russian, "a"),
            ("Ш", Language::Russian, "I"),
            ("ш", Language::Russian, "i"),
        ] {
            let result = detector
                .detect(original, language)
                .unwrap_or_else(|| panic!("missing {original}"));
            assert_eq!(result.replacement, expected, "{original}");
        }
        // A correctly typed single letter is never converted, and a letter that
        // is not listed as a target stays untouched.
        for (word, language) in [
            ("я", Language::Russian),
            ("a", Language::English),
            ("I", Language::English),
            ("q", Language::English),
            ("'", Language::English),
            ("и", Language::Russian),
            ("в", Language::Russian),
            ("а", Language::Russian),
            ("с", Language::Russian),
            ("к", Language::Russian),
            ("о", Language::Russian),
            ("у", Language::Russian),
        ] {
            assert_eq!(detector.detect(word, language), None, "{word}");
        }
        // An identical candidate is ignored while the listed one is chosen.
        let candidates = [
            (Language::Estonian, "z".to_owned()),
            (Language::Russian, "я".to_owned()),
        ];
        let detection = detector
            .detect_mapped_candidates("z", Language::English, &candidates)
            .expect("listed russian single letter");
        assert_eq!(detection.replacement, "я");
        // A user-dictionary entry is enough evidence on either side.
        let mut with_user_dictionary = crate::test_support::configured_detector(DetectorConfig {
            single_letter_words: true,
            ..Default::default()
        });
        with_user_dictionary
            .replace_user_lexicons(UserLexicon::from_lines(["ru-RU й"]), UserLexicon::default());
        assert_eq!(
            with_user_dictionary
                .detect("q", Language::English)
                .map(|detection| detection.replacement),
            Some("й".to_owned())
        );
        // An explicit word exclusion always wins.
        let mut with_exclusion = crate::test_support::configured_detector(DetectorConfig {
            single_letter_words: true,
            ..Default::default()
        });
        with_exclusion
            .replace_user_lexicons(UserLexicon::default(), UserLexicon::from_lines(["en-US z"]));
        assert!(with_exclusion.detect("z", Language::English).is_none());
    }

    #[test]
    fn expanded_single_letters_preserve_case_and_english_user_exceptions() {
        let config = DetectorConfig {
            single_letter_words: true,
            ..Default::default()
        };
        let mut detector = crate::test_support::configured_detector(config);
        for (source, target) in [
            ("F", "А"),
            ("B", "И"),
            ("D", "В"),
            ("R", "К"),
            ("J", "О"),
            ("C", "С"),
            ("E", "У"),
            ("Z", "Я"),
        ] {
            assert_eq!(
                detector
                    .detect(source, Language::English)
                    .unwrap()
                    .replacement,
                target
            );
            assert!(detector.detect(target, Language::Russian).is_none());
        }
        detector.replace_user_lexicons(
            UserLexicon::from_lines(["en-US b", "en-US c"]),
            UserLexicon::default(),
        );
        for source in ["b", "B", "c", "C"] {
            assert!(detector.detect(source, Language::English).is_none());
        }
        // An exception for one letter must not disable all single-letter targets.
        assert_eq!(
            detector.detect("d", Language::English).unwrap().replacement,
            "в"
        );
    }

    #[test]
    fn user_dictionary_suppresses_a_two_letter_conversion() {
        // The two-letter tier is not curated by default; a user who dislikes a
        // specific conversion suppresses it through the user dictionary.
        let detector = crate::test_support::detector();
        assert_eq!(
            detector
                .detect("vs", Language::English)
                .map(|detection| detection.replacement),
            Some("мы".to_owned())
        );
        let mut suppressed = crate::test_support::detector();
        suppressed.replace_user_lexicons(
            UserLexicon::from_lines(["en-US vs"]),
            UserLexicon::default(),
        );
        assert!(suppressed.detect("vs", Language::English).is_none());
    }

    #[test]
    fn configuration_can_disable_borderline_detection() {
        let detector = crate::test_support::configured_detector(DetectorConfig {
            minimum_word_characters: 4,
            minimum_target_score: 100.0,
            minimum_score_margin: 100.0,
            ..Default::default()
        });
        assert_eq!(detector.detect("ghbdtn", Language::English), None);
    }
}
