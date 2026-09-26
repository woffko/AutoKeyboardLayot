//! In-memory input session and suppression policy.

use crate::{CompositionState, Detection, Detector, Language};

const DEFAULT_MAX_WORD_CHARACTERS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetReason {
    Backspace,
    Delete,
    Navigation,
    Mouse,
    FocusChanged,
    LayoutChanged,
    Shortcut,
    UnsupportedInput,
    Composition,
    QueueOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Printable(char),
    Boundary,
    Backspace,
    Delete,
    Navigation,
    Mouse,
    FocusChanged,
    LayoutChanged,
    Shortcut,
    UnsupportedInput,
    QueueOverflow,
    Injected,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionAction {
    None,
    Candidate(Detection),
    Reset(ResetReason),
}

/// Platform-mapped target strings and the languages of recent words.
type MappedContext<'a> = (&'a [(Language, String)], &'a [Language]);

/// Holds only the current word and volatile suppression state.
///
/// Nothing in this type persists typed text or exposes it to diagnostics.
#[derive(Debug, Clone)]
pub struct InputSession {
    current_word: String,
    suppressed_until_boundary: bool,
    backspace_remaining: Option<usize>,
    max_word_characters: usize,
    at_line_start: bool,
    current_word_started_at_line_start: bool,
    recheck_first_word_after_erasing: bool,
}

impl Default for InputSession {
    fn default() -> Self {
        Self {
            current_word: String::new(),
            suppressed_until_boundary: false,
            backspace_remaining: None,
            max_word_characters: DEFAULT_MAX_WORD_CHARACTERS,
            at_line_start: false,
            current_word_started_at_line_start: false,
            recheck_first_word_after_erasing: true,
        }
    }
}

impl InputSession {
    pub fn handle(
        &mut self,
        event: InputEvent,
        current_language: Option<Language>,
        detector: &Detector,
    ) -> SessionAction {
        match event {
            InputEvent::Injected => SessionAction::None,
            InputEvent::Printable(character) => {
                if self.suppressed_until_boundary {
                    self.backspace_remaining = None;
                    return SessionAction::None;
                }
                let Some(language) = current_language else {
                    return self.reset(ResetReason::UnsupportedInput, true);
                };
                if !detector.can_extend_word(character, language) {
                    return self.reset(ResetReason::UnsupportedInput, true);
                }
                if self.current_word.chars().count() >= self.max_word_characters {
                    return self.reset(ResetReason::UnsupportedInput, true);
                }
                if self.current_word.is_empty() {
                    self.current_word_started_at_line_start = self.at_line_start;
                }
                self.at_line_start = false;
                self.current_word.push(character);
                SessionAction::None
            }
            InputEvent::Boundary => self.finish_boundary(current_language, detector, None),
            InputEvent::Backspace => self.handle_backspace(),
            InputEvent::Delete => self.reset(ResetReason::Delete, true),
            InputEvent::Navigation => self.reset(ResetReason::Navigation, true),
            InputEvent::Mouse => self.reset(ResetReason::Mouse, false),
            InputEvent::FocusChanged => self.reset(ResetReason::FocusChanged, false),
            InputEvent::LayoutChanged => self.reset(ResetReason::LayoutChanged, true),
            InputEvent::Shortcut => self.reset(ResetReason::Shortcut, true),
            InputEvent::UnsupportedInput => self.reset(ResetReason::UnsupportedInput, true),
            InputEvent::QueueOverflow => self.reset(ResetReason::QueueOverflow, true),
        }
    }

    pub fn clear(&mut self) {
        self.current_word.clear();
        self.suppressed_until_boundary = false;
        self.backspace_remaining = None;
        self.at_line_start = false;
        self.current_word_started_at_line_start = false;
    }

    /// Finish the current word using target strings mapped from the same
    /// physical keys by a platform adapter.
    pub fn finish_boundary_with_candidates(
        &mut self,
        current_language: Option<Language>,
        detector: &Detector,
        candidates: &[(Language, String)],
    ) -> SessionAction {
        self.finish_boundary(current_language, detector, Some((candidates, &[])))
    }

    /// As finish_boundary_with_candidates, also passing the languages (not
    /// the text) of recent words to the optional layout model.
    pub fn finish_boundary_in_context(
        &mut self,
        current_language: Option<Language>,
        detector: &Detector,
        candidates: &[(Language, String)],
        previous: &[Language],
    ) -> SessionAction {
        self.finish_boundary(current_language, detector, Some((candidates, previous)))
    }

    /// Explicitly evaluate the word still under the caret without typing a
    /// boundary. The buffer is consumed only when a target is selected.
    pub fn force_current_word_with_candidates(
        &mut self,
        current_language: Option<Language>,
        detector: &Detector,
        candidates: &[(Language, String)],
    ) -> SessionAction {
        if self.suppressed_until_boundary || self.current_word.is_empty() {
            return SessionAction::None;
        }
        let Some(language) = current_language else {
            return SessionAction::None;
        };
        let Some(detection) =
            detector.force_mapped_candidates(&self.current_word, language, candidates)
        else {
            return SessionAction::None;
        };
        self.current_word.clear();
        self.backspace_remaining = None;
        self.current_word_started_at_line_start = false;
        SessionAction::Candidate(detection)
    }

    pub fn buffered_character_count(&self) -> usize {
        self.current_word.chars().count()
    }

    pub const fn is_suppressed(&self) -> bool {
        self.suppressed_until_boundary
    }

    /// Apply an observed composition state from the platform adapter.
    ///
    /// Fail-closed: `Active` and `Indeterminate` clear the tracked word and
    /// suppress conversion until the next trusted boundary, so no edit is
    /// produced for text that may belong to a composition. `Inactive` leaves the
    /// current word untouched, preserving ordinary physical-key typing.
    pub fn observe_composition(&mut self, state: CompositionState) -> SessionAction {
        if state.requires_suppression() {
            self.reset(ResetReason::Composition, true)
        } else {
            SessionAction::None
        }
    }

    pub fn set_recheck_first_word_after_erasing(&mut self, enabled: bool) {
        self.recheck_first_word_after_erasing = enabled;
    }

    pub fn mark_line_start(&mut self) {
        self.clear();
        self.at_line_start = true;
    }

    /// Remove the last tracked character after an observed plain Backspace and
    /// keep the corrected word eligible. Returns false when the erased
    /// character is not part of a known word, so the caller must fail closed.
    pub fn erase_last_character(&mut self) -> bool {
        if self.suppressed_until_boundary || self.current_word.pop().is_none() {
            return false;
        }
        self.backspace_remaining = None;
        if self.current_word.is_empty() {
            self.at_line_start = self.current_word_started_at_line_start;
            self.current_word_started_at_line_start = false;
        }
        true
    }

    /// Resume a completed word after the user erased its delimiter. The caller
    /// validates that the word is still adjacent to the caret.
    pub fn restore_word(&mut self, word: &str) -> bool {
        if word.is_empty() || word.chars().count() > self.max_word_characters {
            return false;
        }
        self.clear();
        self.current_word.push_str(word);
        true
    }

    /// Handle Ctrl+Backspace only when the tracked word is known to be the
    /// first word after an observed Enter. Arbitrary selection deletion stays
    /// fail-closed because the caret range is unknown.
    pub fn erase_first_word_by_shortcut(&mut self) -> bool {
        if !self.recheck_first_word_after_erasing
            || self.current_word.is_empty()
            || !self.current_word_started_at_line_start
        {
            return false;
        }
        self.clear();
        self.at_line_start = true;
        true
    }

    fn finish_boundary(
        &mut self,
        current_language: Option<Language>,
        detector: &Detector,
        candidates: Option<MappedContext<'_>>,
    ) -> SessionAction {
        if self.suppressed_until_boundary {
            self.current_word.clear();
            self.suppressed_until_boundary = false;
            self.backspace_remaining = None;
            self.at_line_start = false;
            self.current_word_started_at_line_start = false;
            return SessionAction::None;
        }

        self.backspace_remaining = None;
        let word = core::mem::take(&mut self.current_word);
        self.at_line_start = false;
        self.current_word_started_at_line_start = false;
        let Some(language) = current_language else {
            return SessionAction::None;
        };
        let detection = candidates.map_or_else(
            || detector.detect(&word, language),
            |(candidates, previous)| {
                detector.detect_mapped_candidates_in_context(&word, language, candidates, previous)
            },
        );
        detection.map_or(SessionAction::None, SessionAction::Candidate)
    }

    fn reset(&mut self, reason: ResetReason, suppress_until_boundary: bool) -> SessionAction {
        self.current_word.clear();
        self.suppressed_until_boundary = suppress_until_boundary;
        self.backspace_remaining = None;
        self.at_line_start = false;
        self.current_word_started_at_line_start = false;
        SessionAction::Reset(reason)
    }

    fn handle_backspace(&mut self) -> SessionAction {
        if self.suppressed_until_boundary {
            if let Some(remaining) = self.backspace_remaining {
                let _ = self.current_word.pop();
                let remaining = remaining.saturating_sub(1);
                if remaining == 0 {
                    let was_first_word = self.current_word_started_at_line_start;
                    self.current_word.clear();
                    self.suppressed_until_boundary = false;
                    self.backspace_remaining = None;
                    self.current_word_started_at_line_start = false;
                    if was_first_word && self.recheck_first_word_after_erasing {
                        self.at_line_start = true;
                    }
                } else {
                    self.backspace_remaining = Some(remaining);
                }
            }
            return SessionAction::Reset(ResetReason::Backspace);
        }

        if self.current_word.pop().is_some() {
            let remaining = self.current_word.chars().count();
            self.suppressed_until_boundary = remaining != 0;
            self.backspace_remaining = (remaining != 0).then_some(remaining);
            if remaining == 0 {
                let was_first_word = self.current_word_started_at_line_start;
                self.current_word_started_at_line_start = false;
                if was_first_word && self.recheck_first_word_after_erasing {
                    self.at_line_start = true;
                }
            }
        } else {
            self.suppressed_until_boundary = true;
            self.backspace_remaining = None;
        }
        SessionAction::Reset(ResetReason::Backspace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_word(
        session: &mut InputSession,
        word: &str,
        language: Language,
        detector: &Detector,
    ) -> SessionAction {
        for character in word.chars() {
            assert_eq!(
                session.handle(InputEvent::Printable(character), Some(language), detector),
                SessionAction::None
            );
        }
        session.handle(InputEvent::Boundary, Some(language), detector)
    }

    #[test]
    fn current_word_limit_stays_64_characters_independent_of_dictionary_storage() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for _ in 0..64 {
            assert_eq!(
                session.handle(
                    InputEvent::Printable('a'),
                    Some(Language::English),
                    &detector
                ),
                SessionAction::None
            );
        }
        assert_eq!(session.buffered_character_count(), 64);
        assert_eq!(
            session.handle(
                InputEvent::Printable('a'),
                Some(Language::English),
                &detector
            ),
            SessionAction::Reset(ResetReason::UnsupportedInput)
        );
        assert_eq!(session.buffered_character_count(), 0);
        assert!(session.is_suppressed());
        for _ in 0..256 {
            session.handle(
                InputEvent::Printable('a'),
                Some(Language::English),
                &detector,
            );
            assert_eq!(session.buffered_character_count(), 0);
        }
        assert_eq!(
            session.handle(InputEvent::Boundary, Some(Language::English), &detector),
            SessionAction::None
        );
        assert!(!session.is_suppressed());
        assert!(matches!(
            type_word(&mut session, "ghbdtn", Language::English, &detector),
            SessionAction::Candidate(_)
        ));
    }

    #[test]
    fn reports_candidate_only_at_a_boundary() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        let action = type_word(&mut session, "ghbdtn", Language::English, &detector);
        let SessionAction::Candidate(detection) = action else {
            panic!("expected a candidate");
        };
        assert_eq!(detection.replacement, "привет");
        assert_eq!(session.buffered_character_count(), 0);
    }

    #[test]
    fn active_or_unknown_composition_suppresses_until_a_trusted_boundary() {
        let detector = crate::test_support::detector();
        for state in [CompositionState::Active, CompositionState::Indeterminate] {
            let mut session = InputSession::default();
            for character in "ghbdtn".chars() {
                assert_eq!(
                    session.handle(
                        InputEvent::Printable(character),
                        Some(Language::English),
                        &detector
                    ),
                    SessionAction::None
                );
            }
            assert_eq!(
                session.observe_composition(state),
                SessionAction::Reset(ResetReason::Composition)
            );
            assert_eq!(session.buffered_character_count(), 0);
            assert!(session.is_suppressed());
            assert_eq!(
                session.handle(InputEvent::Boundary, Some(Language::English), &detector),
                SessionAction::None
            );
            assert!(!session.is_suppressed());
            assert!(matches!(
                type_word(&mut session, "ghbdtn", Language::English, &detector),
                SessionAction::Candidate(_)
            ));
        }
    }

    #[test]
    fn inactive_composition_preserves_ordinary_conversion() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for character in "ghbdtn".chars() {
            assert_eq!(
                session.handle(
                    InputEvent::Printable(character),
                    Some(Language::English),
                    &detector
                ),
                SessionAction::None
            );
        }
        assert_eq!(
            session.observe_composition(CompositionState::Inactive),
            SessionAction::None
        );
        assert_eq!(session.buffered_character_count(), 6);
        assert!(matches!(
            session.handle(InputEvent::Boundary, Some(Language::English), &detector),
            SessionAction::Candidate(_)
        ));
    }

    #[test]
    fn keeps_target_layout_letters_that_look_like_source_punctuation() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        let action = type_word(&mut session, "gthtrk.xtybt", Language::English, &detector);
        let SessionAction::Candidate(detection) = action else {
            panic!("expected punctuation-backed Russian candidate");
        };
        assert_eq!(detection.replacement, "переключение");
    }

    #[test]
    fn platform_candidates_use_the_same_boundary_and_clear_the_buffer() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for character in "t;re".chars() {
            assert_eq!(
                session.handle(
                    InputEvent::Printable(character),
                    Some(Language::English),
                    &detector,
                ),
                SessionAction::None
            );
        }
        let action = session.finish_boundary_with_candidates(
            Some(Language::English),
            &detector,
            &[(Language::Estonian, "tere".to_owned())],
        );
        let SessionAction::Candidate(detection) = action else {
            panic!("expected Estonian candidate");
        };
        assert_eq!(detection.target_language, Language::Estonian);
        assert_eq!(session.buffered_character_count(), 0);
    }

    #[test]
    fn force_conversion_consumes_a_short_word_without_a_boundary() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for character in "yt".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }
        let action = session.force_current_word_with_candidates(
            Some(Language::English),
            &detector,
            &[
                (Language::Russian, "не".to_owned()),
                (Language::Estonian, "yt".to_owned()),
            ],
        );
        let SessionAction::Candidate(detection) = action else {
            panic!("expected forced short-word candidate");
        };
        assert_eq!(detection.replacement, "не");
        assert_eq!(session.buffered_character_count(), 0);
    }

    #[test]
    fn ctrl_backspace_can_rearm_a_tracked_first_word_after_enter() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        session.mark_line_start();
        for character in "ghbdtn".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }
        assert!(session.erase_first_word_by_shortcut());
        let action = type_word(&mut session, "ghbdtn", Language::English, &detector);
        assert!(matches!(action, SessionAction::Candidate(_)));
    }

    #[test]
    fn extra_backspace_at_empty_line_stays_fail_closed() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        session.mark_line_start();
        session.handle(
            InputEvent::Printable('g'),
            Some(Language::English),
            &detector,
        );
        session.handle(InputEvent::Backspace, Some(Language::English), &detector);
        assert!(!session.is_suppressed());
        session.handle(InputEvent::Backspace, Some(Language::English), &detector);
        assert!(session.is_suppressed());
    }

    #[test]
    fn erasing_a_character_keeps_the_corrected_word() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for character in "ghbdnb".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }
        assert!(session.erase_last_character());
        assert!(session.erase_last_character());
        assert_eq!(session.buffered_character_count(), 4);
        assert!(!session.is_suppressed());
        assert!(matches!(
            type_word(&mut session, "tn", Language::English, &detector),
            SessionAction::Candidate(detection) if detection.replacement == "привет"
        ));
    }

    #[test]
    fn erasing_outside_a_known_word_is_refused() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        assert!(!session.erase_last_character());
        session.handle(InputEvent::UnsupportedInput, None, &detector);
        assert!(!session.erase_last_character());
        assert!(session.is_suppressed());
    }

    #[test]
    fn erasing_the_first_word_returns_to_line_start() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        session.mark_line_start();
        session.handle(
            InputEvent::Printable('g'),
            Some(Language::English),
            &detector,
        );
        assert!(session.erase_last_character());
        assert!(session.at_line_start);
    }

    #[test]
    fn restored_word_can_be_forced_and_bounds_are_checked() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        session.handle(InputEvent::UnsupportedInput, None, &detector);
        assert!(!session.restore_word(""));
        assert!(!session.restore_word(&"g".repeat(DEFAULT_MAX_WORD_CHARACTERS + 1)));
        assert!(session.restore_word("ghbdtn"));
        assert!(!session.is_suppressed());
        assert_eq!(session.buffered_character_count(), 6);
    }

    #[test]
    fn backspace_suppresses_the_edited_word_until_its_boundary() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for character in "ghb".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }
        assert_eq!(
            session.handle(InputEvent::Backspace, Some(Language::English), &detector),
            SessionAction::Reset(ResetReason::Backspace)
        );
        for character in "dtn".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }
        assert_eq!(
            session.handle(InputEvent::Boundary, Some(Language::English), &detector),
            SessionAction::None
        );
        assert!(!session.is_suppressed());
    }

    #[test]
    fn erasing_the_entire_tracked_word_allows_a_fresh_word_immediately() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for character in "abc".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }
        for _ in 0..3 {
            session.handle(InputEvent::Backspace, Some(Language::English), &detector);
        }

        assert!(!session.is_suppressed());
        assert_eq!(session.buffered_character_count(), 0);
        assert!(matches!(
            type_word(&mut session, "ghbdtn", Language::English, &detector),
            SessionAction::Candidate(_)
        ));
    }

    #[test]
    fn typing_during_backspace_recovery_keeps_the_word_suppressed() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for character in "abc".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }
        session.handle(InputEvent::Backspace, Some(Language::English), &detector);
        session.handle(
            InputEvent::Printable('x'),
            Some(Language::English),
            &detector,
        );
        session.handle(InputEvent::Backspace, Some(Language::English), &detector);
        session.handle(InputEvent::Backspace, Some(Language::English), &detector);

        assert!(session.is_suppressed());
        assert_eq!(
            session.handle(InputEvent::Boundary, Some(Language::English), &detector),
            SessionAction::None
        );
        assert!(!session.is_suppressed());
    }

    #[test]
    fn manual_layout_change_suppresses_the_current_word() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        session.handle(
            InputEvent::Printable('g'),
            Some(Language::English),
            &detector,
        );
        assert_eq!(
            session.handle(
                InputEvent::LayoutChanged,
                Some(Language::Russian),
                &detector,
            ),
            SessionAction::Reset(ResetReason::LayoutChanged)
        );
        assert!(session.is_suppressed());
    }

    #[test]
    fn focus_change_starts_a_fresh_context_without_blocking_the_next_word() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        session.handle(
            InputEvent::Printable('x'),
            Some(Language::English),
            &detector,
        );
        session.handle(InputEvent::FocusChanged, Some(Language::English), &detector);
        assert!(!session.is_suppressed());
        assert!(matches!(
            type_word(&mut session, "ghbdtn", Language::English, &detector),
            SessionAction::Candidate(_)
        ));
    }

    #[test]
    fn mouse_click_starts_a_fresh_context_without_blocking_the_next_word() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        session.handle(
            InputEvent::Printable('x'),
            Some(Language::English),
            &detector,
        );
        assert_eq!(
            session.handle(InputEvent::Mouse, Some(Language::English), &detector),
            SessionAction::Reset(ResetReason::Mouse)
        );
        assert!(!session.is_suppressed());
        assert!(matches!(
            type_word(&mut session, "ghbdtn", Language::English, &detector),
            SessionAction::Candidate(_)
        ));
    }

    #[test]
    fn injected_input_does_not_change_the_buffer() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        session.handle(
            InputEvent::Printable('g'),
            Some(Language::English),
            &detector,
        );
        assert_eq!(
            session.handle(InputEvent::Injected, Some(Language::English), &detector),
            SessionAction::None
        );
        assert_eq!(session.buffered_character_count(), 1);
    }

    #[test]
    fn mid_word_uncertainty_suppresses_the_tail_until_its_boundary() {
        let detector = crate::test_support::detector();
        let mut session = InputSession::default();
        for character in "ghb".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }
        assert_eq!(
            session.handle(
                InputEvent::UnsupportedInput,
                Some(Language::English),
                &detector,
            ),
            SessionAction::Reset(ResetReason::UnsupportedInput)
        );
        for character in "dtn".chars() {
            session.handle(
                InputEvent::Printable(character),
                Some(Language::English),
                &detector,
            );
        }

        assert_eq!(
            session.handle(InputEvent::Boundary, Some(Language::English), &detector),
            SessionAction::None
        );
        assert!(!session.is_suppressed());
    }
}
