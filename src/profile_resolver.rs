//! Exact keyboard-profile resolution protocol. Run on an isolated platform
//! thread: activation belongs only to that thread, never a foreground thread.

use crate::WindowsKeyboardProfile;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

pub const MAX_LOADED_KEYBOARD_LAYOUTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileResolutionError {
    Unavailable,
    InvalidLayoutList,
    ActivationFailed,
    IdentityUnavailable,
    RestoreFailed,
    LayoutListChanged,
}

impl std::fmt::Display for ProfileResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "profile_resolution_{self:?}")
    }
}
impl std::error::Error for ProfileResolutionError {}

/// An OS-specific provider for the resolver's own thread. Handles are opaque;
/// only the documented low-word LANGID is read, never a guessed high-word KLID.
pub trait KeyboardProfileProbe {
    fn loaded_layouts(&mut self) -> Option<Vec<usize>>;
    fn current_layout(&mut self) -> Option<usize>;
    fn is_ime(&mut self, layout: usize) -> bool;
    /// Activate on the calling thread with flags zero; return the previous HKL.
    fn activate_layout(&mut self, layout: usize) -> Option<usize>;
    fn current_layout_name(&mut self) -> Option<[u16; 9]>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedKeyboardProfiles {
    loaded: BTreeSet<usize>,
    profiles: BTreeMap<usize, WindowsKeyboardProfile>,
}
impl ResolvedKeyboardProfiles {
    pub fn profile(&self, layout: usize) -> Option<WindowsKeyboardProfile> {
        self.profiles.get(&layout).copied()
    }
    pub fn loaded_layouts(&self) -> impl Iterator<Item = usize> + '_ {
        self.loaded.iter().copied()
    }
    /// Multiple handles for one exact profile are ambiguous, not first-match.
    pub fn unique_layout(&self, profile: WindowsKeyboardProfile) -> Option<usize> {
        let mut matches = self.profiles.iter().filter(|(_, value)| **value == profile);
        let (&layout, _) = matches.next()?;
        matches.next().is_none().then_some(layout)
    }
}

/// Worker-owned freshness boundary. Unchanged successful refreshes preserve a
/// word/transaction generation; loss of evidence or changed bindings invalidate it.
#[derive(Debug, Default)]
pub struct ProfileSnapshotCache {
    snapshot: Option<ResolvedKeyboardProfiles>,
    confirmed_at: Option<Instant>,
    generation: u64,
}
impl ProfileSnapshotCache {
    pub fn current(&self, now: Instant, max_age: Duration) -> Option<&ResolvedKeyboardProfiles> {
        let age = now.checked_duration_since(self.confirmed_at?)?;
        (age < max_age).then_some(self.snapshot.as_ref()).flatten()
    }
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    pub fn invalidate(&mut self) -> bool {
        self.confirmed_at = None;
        if self.snapshot.take().is_some() {
            self.generation = self.generation.wrapping_add(1);
            true
        } else {
            false
        }
    }
    pub fn expire(&mut self, now: Instant, max_age: Duration) -> bool {
        if self.current(now, max_age).is_none() {
            self.invalidate()
        } else {
            false
        }
    }
    pub fn accept(
        &mut self,
        snapshot: ResolvedKeyboardProfiles,
        now: Instant,
        _max_age: Duration,
    ) -> bool {
        // Only a real binding change invalidates buffered input. Re-accepting an
        // identical snapshot after the age limit must not churn the generation,
        // otherwise periodic refreshes keep clearing the current word.
        let changed = self.snapshot.as_ref() != Some(&snapshot);
        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
        self.snapshot = Some(snapshot);
        self.confirmed_at = Some(now);
        changed
    }
}

fn checked_layouts(layouts: Option<Vec<usize>>) -> Result<BTreeSet<usize>, ProfileResolutionError> {
    let layouts = layouts.ok_or(ProfileResolutionError::Unavailable)?;
    if layouts.is_empty() || layouts.len() > MAX_LOADED_KEYBOARD_LAYOUTS {
        return Err(ProfileResolutionError::InvalidLayoutList);
    }
    let mut unique = BTreeSet::new();
    for layout in layouts {
        // Zero and one are the special HKL_PREV/HKL_NEXT activation constants.
        if layout <= 1 || !unique.insert(layout) {
            return Err(ProfileResolutionError::InvalidLayoutList);
        }
    }
    Ok(unique)
}

struct RestoreGuard<'a, P: KeyboardProfileProbe> {
    probe: &'a mut P,
    original: usize,
    needs_restore: bool,
}
impl<P: KeyboardProfileProbe> RestoreGuard<'_, P> {
    fn restore(&mut self) -> bool {
        if !self.needs_restore {
            return true;
        }
        let previous = self.probe.current_layout();
        let activated = self.probe.activate_layout(self.original);
        let restored = self.probe.current_layout() == Some(self.original);
        let verified = previous.is_some() && activated == previous && restored;
        self.needs_restore = !verified;
        verified
    }
}
impl<P: KeyboardProfileProbe> Drop for RestoreGuard<'_, P> {
    fn drop(&mut self) {
        // Best-effort cleanup also runs during unwinding. A failed explicit
        // restore still rejects the result even if this final retry succeeds.
        let _ = self.restore();
    }
}

/// All-or-nothing resolution. IME handles remain in the inventory but receive
/// no keyboard-profile binding. The caller owns freshness and generation checks.
/// This protocol does not prove that arbitrary TSF composition is inactive.
pub fn resolve_keyboard_profiles(
    probe: &mut impl KeyboardProfileProbe,
) -> Result<ResolvedKeyboardProfiles, ProfileResolutionError> {
    let loaded = checked_layouts(probe.loaded_layouts())?;
    let original = probe
        .current_layout()
        .ok_or(ProfileResolutionError::Unavailable)?;
    if !loaded.contains(&original) {
        return Err(ProfileResolutionError::LayoutListChanged);
    }
    let mut profiles = BTreeMap::new();
    for &layout in &loaded {
        if probe.is_ime(layout) {
            continue;
        }
        if probe.current_layout() != Some(original) {
            return Err(ProfileResolutionError::ActivationFailed);
        }
        let mut guard = RestoreGuard {
            probe,
            original,
            needs_restore: true,
        };
        let activated = guard.probe.activate_layout(layout) == Some(original)
            && guard.probe.current_layout() == Some(layout);
        let identity = if activated {
            guard.probe.current_layout_name().and_then(|name| {
                if guard.probe.current_layout() != Some(layout)
                    || name[8] != 0
                    || name[..8]
                        .iter()
                        .any(|&c| c > 0x7f || !(c as u8).is_ascii_hexdigit())
                {
                    return None;
                }
                // GetKeyboardLayout documents the low word as LANGID. The
                // KLID itself is obtained exclusively from the name API.
                let name = String::from_utf16(&name[..8]).ok()?;
                WindowsKeyboardProfile::parse(&format!("{:04X}:{name}", layout & 0xffff)).ok()
            })
        } else {
            None
        };
        if !guard.restore() {
            return Err(ProfileResolutionError::RestoreFailed);
        }
        if !activated {
            return Err(ProfileResolutionError::ActivationFailed);
        }
        profiles.insert(
            layout,
            identity.ok_or(ProfileResolutionError::IdentityUnavailable)?,
        );
    }
    if checked_layouts(probe.loaded_layouts())? != loaded {
        return Err(ProfileResolutionError::LayoutListChanged);
    }
    if probe.current_layout() != Some(original) {
        return Err(ProfileResolutionError::RestoreFailed);
    }
    Ok(ResolvedKeyboardProfiles { loaded, profiles })
}

#[cfg(test)]
mod tests {
    use super::*;
    const US: usize = 0x04090409;
    const VARIANT: usize = 0xf0020409;
    const IME: usize = 0xe0010411;
    struct Fake {
        loaded: Vec<usize>,
        current: usize,
        activated: Vec<usize>,
        ime: bool,
        activation_fails: bool,
        restore_fails: bool,
        wrong_restore_previous: bool,
        name_fails: bool,
        name_override: Option<[u16; 9]>,
        switch_during_name: bool,
        wrong_handle: bool,
        changed: bool,
        reads: usize,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                loaded: vec![US, VARIANT, IME],
                current: US,
                activated: Vec::new(),
                ime: true,
                activation_fails: false,
                restore_fails: false,
                wrong_restore_previous: false,
                name_fails: false,
                name_override: None,
                switch_during_name: false,
                wrong_handle: false,
                changed: false,
                reads: 0,
            }
        }
    }
    impl KeyboardProfileProbe for Fake {
        fn loaded_layouts(&mut self) -> Option<Vec<usize>> {
            self.reads += 1;
            let mut result = self.loaded.clone();
            if self.changed && self.reads > 1 {
                result.pop();
            }
            Some(result)
        }
        fn current_layout(&mut self) -> Option<usize> {
            Some(self.current)
        }
        fn is_ime(&mut self, layout: usize) -> bool {
            self.ime && layout == IME
        }
        fn activate_layout(&mut self, layout: usize) -> Option<usize> {
            self.activated.push(layout);
            if self.restore_fails && layout == US && self.current != US {
                return None;
            }
            let old = self.current;
            if self.activation_fails && layout == VARIANT {
                self.current = VARIANT; // An API failure need not leave state unchanged.
                return None;
            }
            if !(self.wrong_handle && layout == VARIANT) {
                self.current = layout;
            }
            if self.wrong_restore_previous && layout == US && old != US {
                return Some(IME);
            }
            Some(old)
        }
        fn current_layout_name(&mut self) -> Option<[u16; 9]> {
            if self.name_fails && self.current == VARIANT {
                return None;
            }
            if self.switch_during_name && self.current == VARIANT {
                self.current = US;
            }
            if let Some(name) = self.name_override {
                return Some(name);
            }
            let text = if self.current == VARIANT {
                "00020409"
            } else if self.current == 0x04190419 {
                "00000419"
            } else {
                "00000409"
            };
            let mut name = [0; 9];
            for (slot, byte) in name.iter_mut().zip(text.bytes()) {
                *slot = u16::from(byte);
            }
            Some(name)
        }
    }

    #[test]
    fn resolves_exact_names_restores_each_probe_and_never_activates_ime_targets() {
        let mut fake = Fake::default();
        let resolved = resolve_keyboard_profiles(&mut fake).unwrap();
        assert_eq!(resolved.profile(US).unwrap().to_string(), "0409:00000409");
        assert_eq!(
            resolved.profile(VARIANT).unwrap().to_string(),
            "0409:00020409"
        );
        assert!(resolved.profile(IME).is_none());
        assert_eq!(resolved.loaded_layouts().count(), 3);
        assert_eq!(fake.current, US);
        assert_eq!(fake.activated, [US, US, VARIANT, US]);
        assert_eq!(
            resolved.unique_layout(WindowsKeyboardProfile::parse("0409:00020409").unwrap()),
            Some(VARIANT)
        );
        assert!(
            resolved
                .unique_layout(WindowsKeyboardProfile::parse("0809:00000409").unwrap())
                .is_none()
        );
    }
    #[test]
    fn activation_and_identity_failures_restore_without_returning_partial_data() {
        for (kind, expected) in [
            (0, ProfileResolutionError::ActivationFailed),
            (1, ProfileResolutionError::IdentityUnavailable),
            (2, ProfileResolutionError::ActivationFailed),
        ] {
            let mut fake = Fake {
                activation_fails: kind == 0,
                name_fails: kind == 1,
                wrong_handle: kind == 2,
                ..Default::default()
            };
            assert_eq!(resolve_keyboard_profiles(&mut fake), Err(expected));
            assert_eq!(fake.current, US);
            assert_eq!(fake.activated.last(), Some(&US));
        }
    }
    #[test]
    fn restoration_or_inventory_races_reject_the_whole_snapshot() {
        let mut fake = Fake {
            restore_fails: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_keyboard_profiles(&mut fake),
            Err(ProfileResolutionError::RestoreFailed)
        );
        assert_eq!(&fake.activated[fake.activated.len() - 2..], [US, US]);
        let mut fake = Fake {
            changed: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_keyboard_profiles(&mut fake),
            Err(ProfileResolutionError::LayoutListChanged)
        );
        assert_eq!(fake.current, US);
        let mut fake = Fake {
            wrong_restore_previous: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_keyboard_profiles(&mut fake),
            Err(ProfileResolutionError::RestoreFailed)
        );
        assert_eq!(fake.current, US);
    }
    #[test]
    fn invalid_and_excessive_lists_never_activate_anything() {
        for loaded in [vec![], vec![0], vec![1], vec![US, US], vec![US; 65]] {
            let mut fake = Fake {
                loaded,
                ..Default::default()
            };
            assert_eq!(
                resolve_keyboard_profiles(&mut fake),
                Err(ProfileResolutionError::InvalidLayoutList)
            );
            assert!(fake.activated.is_empty());
        }
        let mut fake = Fake {
            current: 0x9999,
            ..Default::default()
        };
        assert_eq!(
            resolve_keyboard_profiles(&mut fake),
            Err(ProfileResolutionError::LayoutListChanged)
        );
        assert!(fake.activated.is_empty());
    }
    #[test]
    fn malformed_names_and_context_change_during_name_lookup_reject_identity() {
        for name in [
            [0; 9],
            [b'0' as u16; 9],
            [u16::MAX; 9],
            [0xd800, 48, 48, 48, 48, 48, 48, 48, 0],
            [48, 48, 48, 48, 48, 48, 48, 48, 0],
        ] {
            let mut fake = Fake {
                name_override: Some(name),
                ..Default::default()
            };
            assert_eq!(
                resolve_keyboard_profiles(&mut fake),
                Err(ProfileResolutionError::IdentityUnavailable)
            );
            assert_eq!(fake.current, US);
            assert_eq!(fake.activated.last(), Some(&US));
        }
        let mut fake = Fake {
            switch_during_name: true,
            ..Default::default()
        };
        assert_eq!(
            resolve_keyboard_profiles(&mut fake),
            Err(ProfileResolutionError::IdentityUnavailable)
        );
        assert_eq!(fake.current, US);
    }

    #[test]
    fn detector_platform_mode_requires_resolved_exact_source_and_target_profiles() {
        let snapshot = resolve_keyboard_profiles(&mut Fake {
            loaded: vec![US, VARIANT, 0x04190419],
            ..Default::default()
        })
        .unwrap();
        let mut detector = crate::test_support::detector();
        detector.set_resolved_profiles(None);
        assert!(!detector.can_extend_word('h', crate::Language::English));
        assert!(
            detector
                .detect("ghbdtn", crate::Language::English)
                .is_none()
        );
        detector.set_resolved_profiles(Some(&snapshot));
        assert_eq!(
            detector.language_for_profile(snapshot.profile(US).unwrap()),
            Some(crate::Language::English)
        );
        assert_eq!(
            detector.language_for_profile(snapshot.profile(VARIANT).unwrap()),
            None
        );
        assert!(
            detector
                .detect("ghbdtn", crate::Language::English)
                .is_some()
        );
        assert!(!detector.can_extend_word('t', crate::Language::Estonian));
        let mut missing = snapshot.clone();
        missing.profiles.remove(&0x04190419);
        detector.set_resolved_profiles(Some(&missing));
        assert!(
            detector
                .detect("ghbdtn", crate::Language::English)
                .is_none()
        );
        assert!(
            detector
                .force_mapped_candidates(
                    "ghbdtn",
                    crate::Language::English,
                    &[(crate::Language::Russian, "привет".to_owned())]
                )
                .is_none()
        );
        detector.set_resolved_profiles(Some(&snapshot));
        assert!(detector.detect("руддщ", crate::Language::Russian).is_some());
        let mut ambiguous = snapshot;
        ambiguous.profiles.insert(VARIANT, ambiguous.profiles[&US]);
        detector.set_resolved_profiles(Some(&ambiguous));
        assert!(detector.detect("руддщ", crate::Language::Russian).is_none());
    }

    #[test]
    fn cache_refresh_preserves_generation_but_loss_and_change_invalidate_it() {
        let now = Instant::now();
        let ttl = Duration::from_secs(2);
        let snapshot = resolve_keyboard_profiles(&mut Fake::default()).unwrap();
        let mut cache = ProfileSnapshotCache::default();
        assert!(cache.current(now, ttl).is_none());
        assert!(!cache.invalidate());
        assert!(cache.accept(snapshot.clone(), now, ttl));
        let generation = cache.generation();
        assert!(!cache.accept(snapshot.clone(), now + Duration::from_secs(1), ttl));
        assert_eq!(cache.generation(), generation);
        assert!(cache.current(now, ttl).is_none()); // Never accept future-dated evidence.
        assert!(cache.current(now + Duration::from_secs(2), ttl).is_some());
        assert!(cache.expire(now + Duration::from_secs(3), ttl));
        assert_eq!(cache.generation(), generation + 1);
        assert!(!cache.expire(now + Duration::from_secs(4), ttl));
        assert!(cache.accept(snapshot.clone(), now + Duration::from_secs(4), ttl));
        let mut changed = snapshot;
        changed.profiles.remove(&VARIANT);
        assert!(cache.accept(changed.clone(), now + Duration::from_secs(4), ttl));
        assert_eq!(cache.generation(), generation + 3);
        assert!(cache.invalidate());
        assert!(cache.current(now + Duration::from_secs(4), ttl).is_none());
        assert!(cache.accept(changed.clone(), now + Duration::from_secs(5), ttl));
        let generation = cache.generation();
        // An identical refresh after the age limit must not churn the generation,
        // otherwise periodic refreshes keep clearing the current word.
        assert!(!cache.accept(changed, now + Duration::from_secs(7), ttl));
        assert_eq!(cache.generation(), generation);
    }

    #[test]
    fn duplicate_exact_identity_does_not_choose_an_arbitrary_handle() {
        let profile = WindowsKeyboardProfile::parse("0409:00000409").unwrap();
        let resolved = ResolvedKeyboardProfiles {
            loaded: [US, VARIANT].into_iter().collect(),
            profiles: [(US, profile), (VARIANT, profile)].into_iter().collect(),
        };
        assert!(resolved.unique_layout(profile).is_none());
        assert_eq!(resolved.profile(US), Some(profile));
    }
}
