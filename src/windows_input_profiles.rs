//! Native exact-profile provider. Its only activation occurs on a dedicated
//! windowless thread; never attach it to another input queue or use process flags.
//! Native verification under Windows input-method settings remains required.

use crate::WindowsKeyboardProfile;
use crate::{
    bounded_probe::{BoundedProbe, ProbeFailure},
    profile_resolver::{
        KeyboardProfileProbe, MAX_LOADED_KEYBOARD_LAYOUTS, ProfileResolutionError,
        ProfileSnapshotCache, ResolvedKeyboardProfiles, resolve_keyboard_profiles,
    },
};
use std::time::{Duration, Instant};
use windows::Win32::UI::{
    Input::{
        Ime::ImmIsIME,
        KeyboardAndMouse::{
            ACTIVATE_KEYBOARD_LAYOUT_FLAGS, ActivateKeyboardLayout, GetKeyboardLayout,
            GetKeyboardLayoutList, GetKeyboardLayoutNameW, HKL,
        },
    },
    WindowsAndMessaging::{MSG, PM_NOREMOVE, PeekMessageW},
};

struct NativeProbe;
impl KeyboardProfileProbe for NativeProbe {
    fn loaded_layouts(&mut self) -> Option<Vec<usize>> {
        unsafe {
            let count = usize::try_from(GetKeyboardLayoutList(None)).ok()?;
            if count == 0 || count > MAX_LOADED_KEYBOARD_LAYOUTS {
                return None;
            }
            let mut handles = vec![HKL::default(); count];
            let returned = usize::try_from(GetKeyboardLayoutList(Some(&mut handles))).ok()?;
            // Retry belongs to a future request, never an unbounded resize loop.
            if returned != count {
                return None;
            }
            Some(handles.into_iter().map(|h| h.0 as usize).collect())
        }
    }
    fn current_layout(&mut self) -> Option<usize> {
        let handle = unsafe { GetKeyboardLayout(0) }.0 as usize;
        (handle > 1).then_some(handle)
    }
    fn is_ime(&mut self, layout: usize) -> bool {
        unsafe { ImmIsIME(HKL(layout as *mut core::ffi::c_void)).as_bool() }
    }
    fn activate_layout(&mut self, layout: usize) -> Option<usize> {
        if layout <= 1 {
            return None;
        }
        unsafe {
            ActivateKeyboardLayout(
                HKL(layout as *mut core::ffi::c_void),
                ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0),
            )
        }
        .ok()
        .map(|previous| previous.0 as usize)
        .filter(|previous| *previous > 1)
    }
    fn current_layout_name(&mut self) -> Option<[u16; 9]> {
        let mut name = [u16::MAX; 9];
        unsafe { GetKeyboardLayoutNameW(&mut name) }.ok()?;
        Some(name)
    }
}

pub type KeyboardProfileResolver =
    BoundedProbe<(), Result<ResolvedKeyboardProfiles, ProfileResolutionError>>;

/// The owner polls with a bounded/zero wait and rejects expired responses.
/// A stalled OS call occupies this one provider; it must not cause thread retries.
/// Spawning alone does not probe or change a layout: a query is explicit.
pub fn spawn_keyboard_profile_resolver() -> std::io::Result<KeyboardProfileResolver> {
    BoundedProbe::spawn("autokey-layout-profiles", || {
        // Materialize this thread's queue without creating/activating any window.
        let mut message = MSG::default();
        unsafe {
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
        }
        let mut probe = NativeProbe;
        move |()| resolve_keyboard_profiles(&mut probe)
    })
}

const PROFILE_MAX_AGE: Duration = Duration::from_secs(2);
const PROFILE_REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const PROFILE_RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// Persistent single-provider cache. Call `poll` outside hooks; it never waits
/// for the provider. A timeout cannot start an additional provider thread.
pub struct KeyboardProfileCache {
    resolver: Option<KeyboardProfileResolver>,
    cache: ProfileSnapshotCache,
    next_refresh: Instant,
}
impl Default for KeyboardProfileCache {
    fn default() -> Self {
        Self {
            resolver: spawn_keyboard_profile_resolver().ok(),
            cache: ProfileSnapshotCache::default(),
            next_refresh: Instant::now(),
        }
    }
}
impl KeyboardProfileCache {
    /// Returns true only when bindings or availability changed, not on every
    /// identical periodic refresh. Consumers must invalidate buffered state then.
    pub fn poll(&mut self) -> bool {
        let now = Instant::now();
        if now < self.next_refresh {
            return false;
        }
        let Some(resolver) = self.resolver.as_mut() else {
            return self.cache.expire(now, PROFILE_MAX_AGE);
        };
        match resolver.query_result((), Duration::ZERO, PROFILE_MAX_AGE) {
            Ok(Ok(snapshot)) => {
                let changed = self.cache.accept(snapshot, now, PROFILE_MAX_AGE);
                self.next_refresh = now + PROFILE_REFRESH_INTERVAL;
                changed
            }
            // Keep the last snapshot until it is actually too old; a timeout is
            // not a binding change.
            Err(ProbeFailure::Timeout) => self.cache.expire(now, PROFILE_MAX_AGE),
            failure => {
                let changed = self.cache.invalidate();
                self.next_refresh = now + PROFILE_RETRY_INTERVAL;
                if matches!(failure, Err(ProbeFailure::Disconnected)) {
                    self.resolver = None;
                }
                changed
            }
        }
    }
    pub fn generation(&self) -> u64 {
        self.cache.generation()
    }
    pub fn snapshot(&self) -> Option<&ResolvedKeyboardProfiles> {
        self.cache.current(Instant::now(), PROFILE_MAX_AGE)
    }
    pub fn profile(&self, layout: usize) -> Option<WindowsKeyboardProfile> {
        self.cache
            .current(Instant::now(), PROFILE_MAX_AGE)?
            .profile(layout)
    }
    pub fn unique_layout(&self, profile: WindowsKeyboardProfile) -> Option<usize> {
        self.cache
            .current(Instant::now(), PROFILE_MAX_AGE)?
            .unique_layout(profile)
    }
    /// Read-only final inventory check outside hooks. Identity bindings are
    /// still scoped to this snapshot; a changed inventory requires a new probe.
    pub fn inventory_is_current(&self) -> bool {
        let Some(snapshot) = self.cache.current(Instant::now(), PROFILE_MAX_AGE) else {
            return false;
        };
        let Some(mut loaded) = NativeProbe.loaded_layouts() else {
            return false;
        };
        loaded.sort_unstable();
        snapshot.loaded_layouts().eq(loaded)
    }
}
