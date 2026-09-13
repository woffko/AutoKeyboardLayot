//! Platform-independent text replacement and one-step undo model.

use crate::{Detection, Language};
use unicode_segmentation::UnicodeSegmentation;

/// One caret-local edit expressed as Backspace count plus Unicode insertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub erase_characters: usize,
    pub insert_text: String,
}

/// Symmetric forward and undo edits for one detector decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionTransaction {
    pub source_language: Language,
    pub target_language: Language,
    pub original: String,
    pub replacement: String,
    pub delimiter: Option<char>,
    pub forward: TextEdit,
    pub undo: TextEdit,
}

impl ConversionTransaction {
    /// Build a transaction for a printable single-character word boundary.
    pub fn new(detection: &Detection, delimiter: char) -> Option<Self> {
        if delimiter.is_control() {
            return None;
        }
        Self::with_optional_delimiter(detection, Some(delimiter))
    }

    /// Build a transaction for an explicit hotkey conversion before a word
    /// boundary has been typed.
    pub fn without_delimiter(detection: &Detection) -> Option<Self> {
        Self::with_optional_delimiter(detection, None)
    }

    fn with_optional_delimiter(detection: &Detection, delimiter: Option<char>) -> Option<Self> {
        if detection.original.is_empty() || detection.replacement.is_empty() {
            return None;
        }

        let mut forward_text = detection.replacement.clone();
        let mut undo_text = detection.original.clone();
        if let Some(delimiter) = delimiter {
            forward_text.push(delimiter);
            undo_text.push(delimiter);
        }
        // The existing adapter issues individual Backspace keystrokes. Never
        // infer a scalar erase count for a multi-scalar grapheme (including a
        // delimiter that combines with the word). Both forward and undo must
        // be representable before either edit may be issued. Composition-aware
        // replacement needs a separate, host-validated edit protocol.
        let original_characters = conservative_erase_units(&undo_text)?;
        let replacement_characters = conservative_erase_units(&forward_text)?;

        Some(Self {
            source_language: detection.source_language,
            target_language: detection.target_language,
            original: detection.original.clone(),
            replacement: detection.replacement.clone(),
            delimiter,
            forward: TextEdit {
                erase_characters: original_characters,
                insert_text: forward_text,
            },
            undo: TextEdit {
                erase_characters: replacement_characters,
                insert_text: undo_text,
            },
        })
    }
}

fn conservative_erase_units(text: &str) -> Option<usize> {
    let mut units = 0;
    for grapheme in text.graphemes(true) {
        let mut scalars = grapheme.chars();
        let scalar = scalars.next()?;
        if scalars.next().is_some() || scalar.len_utf16() != 1 {
            return None;
        }
        units += 1;
    }
    Some(units)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detection(original: &str, replacement: &str) -> Detection {
        Detection {
            source_language: Language::English,
            target_language: Language::Russian,
            original: original.to_owned(),
            replacement: replacement.to_owned(),
            source_score: 0.0,
            target_score: 20.0,
        }
    }

    #[test]
    fn creates_symmetric_space_delimited_edits() {
        let transaction = ConversionTransaction::new(&detection("ghbdtn", "привет"), ' ')
            .expect("space transaction should be valid");
        assert_eq!(transaction.forward.erase_characters, 7);
        assert_eq!(transaction.forward.insert_text, "привет ");
        assert_eq!(transaction.undo.erase_characters, 7);
        assert_eq!(transaction.undo.insert_text, "ghbdtn ");
    }

    #[test]
    fn counts_unicode_scalars_instead_of_utf8_bytes() {
        let transaction = ConversionTransaction::new(&detection("hello", "привет"), '.')
            .expect("punctuation transaction should be valid");
        assert_eq!(transaction.forward.erase_characters, 6);
        assert_eq!(transaction.undo.erase_characters, 7);
    }

    #[test]
    fn creates_a_hotkey_transaction_without_a_boundary() {
        let transaction = ConversionTransaction::without_delimiter(&detection("yt", "не"))
            .expect("hotkey transaction should be valid");
        assert_eq!(transaction.delimiter, None);
        assert_eq!(transaction.forward.erase_characters, 2);
        assert_eq!(transaction.forward.insert_text, "не");
        assert_eq!(transaction.undo.erase_characters, 2);
        assert_eq!(transaction.undo.insert_text, "yt");
    }

    #[test]
    fn rejects_control_delimiters_and_empty_text() {
        assert_eq!(
            ConversionTransaction::new(&detection("hello", "привет"), '\n'),
            None
        );
        assert_eq!(
            ConversionTransaction::new(&detection("", "привет"), ' '),
            None
        );
    }

    #[test]
    fn scalar_backspace_edits_reject_composite_text_in_both_directions() {
        // Unicode Alphabetic includes some marks. Model range validation
        // therefore cannot substitute for edit-unit validation.
        for mark in ['\u{0345}', '\u{093e}', '\u{09be}'] {
            assert!(mark.is_alphabetic());
            assert!(mark.to_lowercase().eq(std::iter::once(mark)));
        }
        for text in [
            "e\u{301}",
            "a\u{0345}",
            "क\u{093e}",
            "ক\u{09be}",
            "\u{1100}\u{1161}",
            "\u{1f1ea}\u{1f1ea}",
            "\u{20000}",
        ] {
            assert!(
                ConversionTransaction::new(&detection(text, "word"), ' ').is_none(),
                "source {text:?}"
            );
            assert!(
                ConversionTransaction::without_delimiter(&detection("word", text)).is_none(),
                "undo {text:?}"
            );
        }
        assert!(ConversionTransaction::new(&detection("test", "word"), '\u{301}').is_none());
        assert!(ConversionTransaction::without_delimiter(&detection("õun", "ёжик")).is_some());
    }
}
