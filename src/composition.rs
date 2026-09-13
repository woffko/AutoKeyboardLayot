//! Fail-closed composition/IME state contract.
//!
//! This module owns the portable decision: a caller (the Windows worker) may ask
//! the platform whether a composition is active for the exact foreground input
//! thread, and must suppress conversion whenever the answer is `Active` or
//! `Indeterminate`. It does not detect composition itself and does not read
//! composed text; the native query is a separate, reviewed adapter.
//!
//! The capability id is code-owned. A package can require it, but it must not be
//! reported as implemented until the native adapter is reviewed and physically
//! accepted.

/// Code-owned capability id for the composition guard implementation.
pub const COMPOSITION_GUARD_CAPABILITY: &str = "composition-guard-v1";

/// Observed composition state for one foreground input thread.
///
/// `Indeterminate` is the fail-closed answer for a failed, expired or ambiguous
/// native query and behaves exactly like `Active` for conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionState {
    /// No composition is active; ordinary conversion is permitted.
    Inactive,
    /// A composition is active; conversion must not run.
    Active,
    /// The state could not be established; conversion must not run.
    Indeterminate,
}

impl CompositionState {
    /// Conversion may run only when composition is known to be inactive.
    pub const fn permits_conversion(self) -> bool {
        matches!(self, Self::Inactive)
    }

    /// True when the caller must suppress conversion and clear tracked input.
    pub const fn requires_suppression(self) -> bool {
        !self.permits_conversion()
    }
}

/// Fail-closed conversion gate for an observed composition state.
pub const fn suppress_conversion(state: CompositionState) -> bool {
    state.requires_suppression()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_inactive_composition_permits_conversion() {
        assert!(CompositionState::Inactive.permits_conversion());
        assert!(!suppress_conversion(CompositionState::Inactive));
        for state in [CompositionState::Active, CompositionState::Indeterminate] {
            assert!(!state.permits_conversion());
            assert!(state.requires_suppression());
            assert!(suppress_conversion(state));
        }
    }

    #[test]
    fn capability_id_is_stable_code_owned_ascii() {
        assert_eq!(COMPOSITION_GUARD_CAPABILITY, "composition-guard-v1");
        assert!(
            COMPOSITION_GUARD_CAPABILITY
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        );
    }
}
