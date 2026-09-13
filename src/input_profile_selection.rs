//! Explicit package/profile choices, independent of installation and OS state.

use crate::{InputPackDescriptor, InputProfileRequirements, PackId, WindowsKeyboardProfile};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSelectionError {
    InvalidPackId,
    InvalidProfile,
    DuplicatePackId,
    LimitExceeded,
}

/// Owned, bounded preferences. Missing or disabled packages retain their choice;
/// a preference never installs a layout or grants adapter capability.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputProfileSelections(BTreeMap<PackId, WindowsKeyboardProfile>);

impl InputProfileSelections {
    pub fn parse<I: AsRef<str>, P: AsRef<str>>(
        rows: impl IntoIterator<Item = (I, P)>,
    ) -> Result<Self, ProfileSelectionError> {
        let mut choices = BTreeMap::new();
        for (index, (id, profile)) in rows.into_iter().enumerate() {
            if index >= 64 {
                return Err(ProfileSelectionError::LimitExceeded);
            }
            let id =
                PackId::parse(id.as_ref()).map_err(|_| ProfileSelectionError::InvalidPackId)?;
            let profile = WindowsKeyboardProfile::parse(profile.as_ref())
                .map_err(|_| ProfileSelectionError::InvalidProfile)?;
            if choices.insert(id, profile).is_some() {
                return Err(ProfileSelectionError::DuplicatePackId);
            }
        }
        Ok(Self(choices))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PackId, &WindowsKeyboardProfile)> {
        self.0.iter()
    }

    pub fn get(&self, id: &PackId) -> Option<WindowsKeyboardProfile> {
        self.0.get(id).copied()
    }

    /// An explicit missing profile fails closed, even for a single-profile pack.
    /// Only absence of a preference permits the sole declared profile default.
    pub fn resolve<'a>(
        &self,
        descriptor: &'a InputPackDescriptor,
    ) -> Option<(WindowsKeyboardProfile, &'a InputProfileRequirements)> {
        if let Some(profile) = self.get(&descriptor.pack_id()) {
            return Some((profile, descriptor.profile(profile)?));
        }
        let mut profiles = descriptor.profiles();
        let (&profile, requirements) = profiles.next()?;
        profiles.next().is_none().then_some((profile, requirements))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_preserves_unknown_choices_and_rejects_invalid_or_duplicate_rows() {
        let choices = InputProfileSelections::parse([("custom-missing", "0409:00020409")]).unwrap();
        assert_eq!(choices.iter().count(), 1);
        assert_eq!(
            choices.iter().next().unwrap().1.to_string(),
            "0409:00020409"
        );
        assert_eq!(
            InputProfileSelections::parse([("../en", "0409:00000409")]),
            Err(ProfileSelectionError::InvalidPackId)
        );
        assert_eq!(
            InputProfileSelections::parse([("en-US", "0409:409")]),
            Err(ProfileSelectionError::InvalidProfile)
        );
        assert_eq!(
            InputProfileSelections::parse([("EN-us", "0409:00000409"), ("en-US", "0409:00020409")]),
            Err(ProfileSelectionError::DuplicatePackId)
        );
    }

    #[test]
    fn parser_bounds_raw_rows() {
        let rows = |count| (0..count).map(|index| (format!("custom-{index}"), "0409:00000409"));
        assert_eq!(
            InputProfileSelections::parse(rows(64))
                .unwrap()
                .iter()
                .count(),
            64
        );
        assert_eq!(
            InputProfileSelections::parse(rows(65)),
            Err(ProfileSelectionError::LimitExceeded)
        );
    }
}
