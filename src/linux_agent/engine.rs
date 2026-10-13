//! The engine's state machine for one input context, without D-Bus: IBus key events in, edits and
//! indicator updates out.
//!
//! IBus hands the engine one key at a time and waits for the answer, so the keys after a Space
//! wait in the compositor while the engine decides: no correction gate is needed. In the layout of
//! the engine's own keymap (US) keys pass through untouched; in any other layout the engine commits
//! the character that layout gives for the physical key. A word is replaced by deleting it, through
//! the surrounding text when the field reports it and it ends with exactly that word, or else with
//! one forwarded Backspace per character (terminals), and committing the replacement.

use super::diagnostics::Diagnostics;
use super::keymap::{KeyOutput, Keymaps, Levels, TypedKey};
use autokeyboardlayot::{
    ConversionTransaction, Detection, Detector, Hotkey, InputEvent, InputSession, Language,
    SessionAction, Settings,
};
use xkbcommon::xkb;

// IBus modifier masks (ibustypes.h).
pub const SHIFT_MASK: u32 = 1 << 0;
pub const LOCK_MASK: u32 = 1 << 1;
pub const CONTROL_MASK: u32 = 1 << 2;
pub const MOD1_MASK: u32 = 1 << 3;
pub const MOD4_MASK: u32 = 1 << 6;
pub const SUPER_MASK: u32 = 1 << 26;
pub const RELEASE_MASK: u32 = 1 << 30;

// IBus input purposes and hints.
const PURPOSE_PASSWORD: u32 = 8;
const PURPOSE_PIN: u32 = 9;
const PURPOSE_TERMINAL: u32 = 10;
const HINT_PRIVATE: u32 = 1 << 11;

/// X keysyms the engine reacts to.
pub mod keysym {
    pub const SPACE: u32 = 0x20;
    pub const BACKSPACE: u32 = 0xff08;
    pub const TAB: u32 = 0xff09;
    pub const RETURN: u32 = 0xff0d;
    pub const PAUSE: u32 = 0xff13;
    pub const SCROLL_LOCK: u32 = 0xff14;
    pub const ESCAPE: u32 = 0xff1b;
    pub const HOME: u32 = 0xff50;
    pub const LEFT: u32 = 0xff51;
    pub const UP: u32 = 0xff52;
    pub const RIGHT: u32 = 0xff53;
    pub const DOWN: u32 = 0xff54;
    pub const PAGE_UP: u32 = 0xff55;
    pub const PAGE_DOWN: u32 = 0xff56;
    pub const END: u32 = 0xff57;
    pub const INSERT: u32 = 0xff63;
    pub const BREAK: u32 = 0xff6b;
    pub const KP_ENTER: u32 = 0xff8d;
    pub const KP_HOME: u32 = 0xff95;
    pub const KP_DELETE: u32 = 0xff9f;
    pub const F1: u32 = 0xffbe;
    pub const ISO_LEFT_TAB: u32 = 0xfe20;
    pub const DELETE: u32 = 0xffff;
}

// evdev key codes.
const KEY_BACKSPACE: u32 = 14;
const KEY_LEFTCTRL: u32 = 29;
const KEY_LEFTSHIFT: u32 = 42;
const KEY_RIGHTSHIFT: u32 = 54;
const KEY_LEFTALT: u32 = 56;
const KEY_SPACE: u32 = 57;
const KEY_CAPSLOCK: u32 = 58;
const KEY_NUMLOCK: u32 = 69;
const KEY_RIGHTCTRL: u32 = 97;
const KEY_RIGHTALT: u32 = 100;
const KEY_LEFTMETA: u32 = 125;
const KEY_RIGHTMETA: u32 = 126;

/// Languages of recent words given to the layout model.
const RECENT_LANGUAGES: usize = 4;

/// What the engine asks IBus to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Commit(String),
    ForwardKey {
        keyval: u32,
        keycode: u32,
        state: u32,
    },
    DeleteSurrounding {
        offset: i32,
        chars: u32,
    },
    /// The layout changed or must be shown: update the indicator.
    Layout(Language),
    RequireSurrounding,
}

/// The answer to one key event: whether IBus must keep the key from the application.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub handled: bool,
    pub actions: Vec<Action>,
}

impl Outcome {
    const fn pass() -> Self {
        Self {
            handled: false,
            actions: Vec::new(),
        }
    }

    const fn consumed(actions: Vec<Action>) -> Self {
        Self {
            handled: true,
            actions,
        }
    }
}

/// The force/undo hotkey translated to X keysyms and IBus modifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HotkeyMatch {
    keysyms: Vec<u32>,
    modifiers: u32,
}

impl HotkeyMatch {
    fn new(hotkey: Hotkey) -> Self {
        use autokeyboardlayot::hotkey::{
            HOTKEY_MOD_ALT, HOTKEY_MOD_CONTROL, HOTKEY_MOD_SHIFT, HOTKEY_MOD_WIN,
        };
        let mut modifiers = 0;
        for (bit, mask) in [
            (HOTKEY_MOD_CONTROL, CONTROL_MASK),
            (HOTKEY_MOD_ALT, MOD1_MASK),
            (HOTKEY_MOD_SHIFT, SHIFT_MASK),
            (HOTKEY_MOD_WIN, MOD4_MASK),
        ] {
            if hotkey.modifiers & bit != 0 {
                modifiers |= mask;
            }
        }
        let key = hotkey.virtual_key;
        let mut keysyms = match key {
            0x13 => vec![keysym::PAUSE],
            0x2d => vec![keysym::INSERT],
            0x91 => vec![keysym::SCROLL_LOCK],
            0x70..=0x87 => vec![keysym::F1 + u32::from(key - 0x70)],
            0x41..=0x5a => vec![u32::from(key) + 0x20, u32::from(key)],
            0x30..=0x39 => vec![u32::from(key)],
            _ => Vec::new(),
        };
        // Ctrl+Pause gives Break, as Windows gives VK_CANCEL.
        if key == 0x13 && modifiers & CONTROL_MASK != 0 {
            keysyms.push(keysym::BREAK);
        }
        Self { keysyms, modifiers }
    }

    fn matches(&self, keyval: u32, state: u32) -> bool {
        let held = state & (CONTROL_MASK | MOD1_MASK | SHIFT_MASK | MOD4_MASK | SUPER_MASK);
        let held = if held & SUPER_MASK != 0 {
            (held & !SUPER_MASK) | MOD4_MASK
        } else {
            held
        };
        self.keysyms.contains(&keyval) && held == self.modifiers
    }
}

/// State shared by every input context of the process.
pub struct Shared {
    pub detector: Detector,
    pub keymaps: Keymaps,
    pub settings: Settings,
    pub diagnostics: Diagnostics,
    layouts: Vec<Language>,
    layout: Language,
    passthrough: Option<Language>,
    automatic: bool,
    level3: Vec<Language>,
    compose: Option<xkb::compose::Table>,
    hotkey: HotkeyMatch,
}

impl Shared {
    /// `None` when fewer than one layout could be compiled.
    pub fn new(
        detector: Detector,
        keymaps: Keymaps,
        settings: Settings,
        diagnostics: Diagnostics,
    ) -> Option<Self> {
        let layouts = layout_order(keymaps.languages().collect());
        let layout = *layouts.first()?;
        let passthrough = layouts
            .iter()
            .copied()
            .find(|language| super::keymap::xkb_layout(*language) == Some(BASE_LAYOUT));
        let level3 = layouts
            .iter()
            .copied()
            .filter(|language| keymaps.right_alt_is_level3(*language))
            .collect();
        let locale = std::env::var_os("LC_ALL")
            .or_else(|| std::env::var_os("LC_CTYPE"))
            .or_else(|| std::env::var_os("LANG"))
            .unwrap_or_else(|| "en_US.UTF-8".into());
        let compose = xkb::compose::Table::new_from_locale(
            keymaps.context(),
            &locale,
            xkb::compose::COMPILE_NO_FLAGS,
        )
        .ok();
        let hotkey = HotkeyMatch::new(
            Hotkey::new(
                settings.force_hotkey_virtual_key,
                settings.force_hotkey_modifiers,
            )
            .unwrap_or_default(),
        );
        Some(Self {
            detector,
            keymaps,
            automatic: settings.automatic_conversion_on_startup,
            settings,
            diagnostics,
            layouts,
            layout,
            passthrough,
            level3,
            compose,
            hotkey,
        })
    }

    pub fn layout(&self) -> Language {
        self.layout
    }

    pub fn layouts(&self) -> &[Language] {
        &self.layouts
    }

    pub const fn automatic(&self) -> bool {
        self.automatic
    }

    pub fn set_automatic(&mut self, automatic: bool) {
        self.automatic = automatic;
        self.log(
            "automatic",
            &format!("result={}", if automatic { "on" } else { "off" }),
        );
    }

    /// The layout chosen from the indicator menu.
    pub fn choose_layout(&mut self, language: Language) {
        if self.layouts.contains(&language) {
            self.layout = language;
            self.log(
                "layout",
                &format!("result=switched by=menu to={}", language.id()),
            );
        }
    }

    /// After a reload: the current layout, when it is still enabled, and the automatic switch as
    /// the user left it (the setting only decides it at startup).
    pub fn carry_over(&mut self, previous: &Shared) {
        if self.layouts.contains(&previous.layout) {
            self.layout = previous.layout;
        }
        self.automatic = previous.automatic;
    }

    fn right_alt_is_level3(&self, language: Language) -> bool {
        self.level3.contains(&language)
    }

    fn log(&self, event: &str, details: &str) {
        self.diagnostics.record(event, details);
    }
}

/// The engine's own keymap, which IBus applies while the engine is the input source. The layout
/// with this XKB name is typed by passing keys through.
pub const BASE_LAYOUT: &str = "us";

/// The order Alt+Shift walks the layouts: English, Russian and Estonian in that order, as the
/// owner's desktop lists them, then any other language with a layout.
fn layout_order(mut languages: Vec<Language>) -> Vec<Language> {
    const PREFERRED: [&str; 3] = ["en-US", "ru-RU", "et-EE"];
    languages.sort_by_key(|language| {
        PREFERRED
            .iter()
            .position(|id| *id == language.id())
            .unwrap_or(PREFERRED.len())
    });
    languages
}

/// The order in which the hotkey walks a word through the layouts: its own language, then the
/// most plausible readings first, then any other layout in the configured order.
fn layout_cycle_order(
    source: Language,
    enabled: &[Language],
    ranked: &[Language],
) -> Vec<Language> {
    let mut order = Vec::with_capacity(enabled.len());
    order.push(source);
    for language in ranked.iter().chain(enabled) {
        if !order.contains(language) {
            order.push(*language);
        }
    }
    order
}

#[derive(Debug, Clone)]
struct LastWord {
    keys: Vec<TypedKey>,
    delimiter: Option<char>,
    layout: Language,
}

#[derive(Debug, Clone)]
struct Undo {
    keys: Vec<TypedKey>,
    /// The text the conversion typed (replacement and delimiter) and what restores it.
    typed: String,
    original: String,
    delimiter: Option<char>,
    source: Language,
    target: Language,
}

#[derive(Debug, Clone)]
struct Cycle {
    keys: Vec<TypedKey>,
    delimiter: Option<char>,
    order: Vec<Language>,
    language: Language,
}

#[derive(Debug, Clone)]
struct Surrounding {
    text: String,
    cursor: usize,
    anchor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Chord {
    #[default]
    Idle,
    /// Alt and Shift are down and nothing else was pressed.
    Armed,
    /// Another key was pressed while they were down.
    Spoiled,
}

/// Modifier keys as the engine needs them: right Alt is AltGr in some layouts, and Alt+Shift
/// pressed and released alone switches the layout.
#[derive(Debug, Default)]
struct Modifiers {
    left_alt: bool,
    right_alt: bool,
    right_alt_is_alt: bool,
    shifts: u8,
    chord: Chord,
}

impl Modifiers {
    fn alt(&self) -> bool {
        self.left_alt || (self.right_alt && self.right_alt_is_alt)
    }

    /// Notes a key going down; true when it is a modifier key.
    fn press(&mut self, keycode: u32, right_alt_is_level3: bool) -> bool {
        match keycode {
            KEY_LEFTALT => self.left_alt = true,
            KEY_RIGHTALT => {
                self.right_alt = true;
                self.right_alt_is_alt = !right_alt_is_level3;
            }
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => self.shifts = self.shifts.saturating_add(1),
            KEY_LEFTCTRL | KEY_RIGHTCTRL | KEY_LEFTMETA | KEY_RIGHTMETA | KEY_CAPSLOCK
            | KEY_NUMLOCK => {
                self.chord = Chord::Spoiled;
                return true;
            }
            _ => return false,
        }
        if self.alt() && self.shifts > 0 && self.chord == Chord::Idle {
            self.chord = Chord::Armed;
        }
        true
    }

    fn other_key(&mut self) {
        if self.alt() || self.shifts > 0 {
            self.chord = Chord::Spoiled;
        }
    }

    /// Notes a key going up; true when Alt+Shift was just completed alone.
    fn release(&mut self, keycode: u32) -> bool {
        match keycode {
            KEY_LEFTALT => self.left_alt = false,
            KEY_RIGHTALT => self.right_alt = false,
            KEY_LEFTSHIFT | KEY_RIGHTSHIFT => self.shifts = self.shifts.saturating_sub(1),
            _ => return false,
        }
        if self.alt() || self.shifts > 0 {
            return false;
        }
        let completed = self.chord == Chord::Armed;
        self.chord = Chord::Idle;
        completed
    }
}

/// The state of one input context.
pub struct Engine {
    session: InputSession,
    word: Vec<TypedKey>,
    word_layout: Option<Language>,
    manual_only: bool,
    last_word: Option<LastWord>,
    undo: Option<Undo>,
    cycle: Option<Cycle>,
    recent: Vec<Language>,
    purpose: u32,
    hints: u32,
    surrounding: Option<Surrounding>,
    modifiers: Modifiers,
    consumed: Vec<u32>,
    compose: Option<xkb::compose::State>,
}

impl Engine {
    pub fn new(shared: &Shared) -> Self {
        let mut session = InputSession::default();
        session
            .set_recheck_first_word_after_erasing(shared.settings.recheck_first_word_after_erasing);
        Self {
            session,
            word: Vec::new(),
            word_layout: None,
            manual_only: false,
            last_word: None,
            undo: None,
            cycle: None,
            recent: Vec::new(),
            purpose: 0,
            hints: 0,
            surrounding: None,
            modifiers: Modifiers::default(),
            consumed: Vec::new(),
            compose: None,
        }
    }

    /// A password or PIN field, or one whose text must not be learned: typed in the current layout,
    /// never tracked or converted.
    fn private(&self) -> bool {
        matches!(self.purpose, PURPOSE_PASSWORD | PURPOSE_PIN) || self.hints & HINT_PRIVATE != 0
    }

    pub fn focus_in(&mut self, shared: &Shared) -> Vec<Action> {
        self.forget();
        vec![Action::Layout(shared.layout), Action::RequireSurrounding]
    }

    pub fn focus_out(&mut self) {
        self.forget();
        self.purpose = 0;
        self.hints = 0;
        self.surrounding = None;
    }

    /// The application reset the input context: the caret moved, a click, a new text.
    pub fn reset(&mut self) {
        self.forget();
    }

    pub fn set_content_type(&mut self, shared: &Shared, purpose: u32, hints: u32) {
        let was_private = self.private();
        self.purpose = purpose;
        self.hints = hints;
        if self.private() {
            self.forget();
        }
        if self.private() != was_private {
            shared.log(
                "field",
                &format!("private={} purpose={purpose}", self.private()),
            );
        }
    }

    pub fn set_surrounding_text(&mut self, text: String, cursor: u32, anchor: u32) {
        self.surrounding = Some(Surrounding {
            text,
            cursor: cursor as usize,
            anchor: anchor as usize,
        });
    }

    fn forget(&mut self) {
        self.session.clear();
        self.forget_word_state();
    }

    /// Everything about the word except the session, whose suppression a caller may keep.
    fn forget_word_state(&mut self) {
        self.word.clear();
        self.word_layout = None;
        self.manual_only = false;
        self.last_word = None;
        self.undo = None;
        self.cycle = None;
        self.compose = None;
    }

    pub fn process_key(
        &mut self,
        shared: &mut Shared,
        keyval: u32,
        keycode: u32,
        state: u32,
    ) -> Outcome {
        if state & RELEASE_MASK != 0 {
            return self.release(shared, keycode);
        }
        let layout = shared.layout;
        if self
            .modifiers
            .press(keycode, shared.right_alt_is_level3(layout))
        {
            return Outcome::pass();
        }
        self.modifiers.other_key();
        let levels = Levels {
            shift: state & SHIFT_MASK != 0,
            caps_lock: state & LOCK_MASK != 0,
            level3: self.modifiers.right_alt && shared.right_alt_is_level3(layout),
        };
        if shared.hotkey.matches(keyval, state) {
            self.consume(keycode);
            return self.hotkey(shared);
        }
        let control = state & CONTROL_MASK != 0;
        let alt = state & MOD1_MASK != 0 && self.modifiers.alt();
        let windows = state & (MOD4_MASK | SUPER_MASK) != 0;
        if control || alt || windows {
            self.shortcut(shared, keyval, control, alt);
            return Outcome::pass();
        }
        let settings = &shared.settings;
        let rule = match keyval {
            keysym::BACKSPACE => return self.backspace(shared),
            keysym::RETURN | keysym::KP_ENTER => {
                self.forget();
                self.session.mark_line_start();
                return Outcome::pass();
            }
            keysym::TAB | keysym::ISO_LEFT_TAB | keysym::ESCAPE => {
                self.forget();
                return Outcome::pass();
            }
            keysym::DELETE | keysym::KP_DELETE => {
                Some((InputEvent::Delete, settings.suppress_after_delete))
            }
            keysym::LEFT => Some((InputEvent::Navigation, settings.suppress_after_left)),
            keysym::RIGHT => Some((InputEvent::Navigation, settings.suppress_after_right)),
            keysym::UP => Some((InputEvent::Navigation, settings.suppress_after_up)),
            keysym::DOWN => Some((InputEvent::Navigation, settings.suppress_after_down)),
            keysym::HOME | keysym::END => {
                Some((InputEvent::Navigation, settings.suppress_after_home_end))
            }
            keysym::PAGE_UP | keysym::PAGE_DOWN | keysym::INSERT => {
                Some((InputEvent::Navigation, true))
            }
            value if (keysym::KP_HOME..keysym::KP_DELETE).contains(&value) => {
                Some((InputEvent::Navigation, true))
            }
            _ => None,
        };
        if let Some((event, suppress)) = rule {
            self.switching_rule(shared, event, suppress);
            return Outcome::pass();
        }
        if keycode == KEY_SPACE || keyval == keysym::SPACE {
            return self.space(shared);
        }
        self.printable(shared, keycode, levels)
    }

    fn release(&mut self, shared: &mut Shared, keycode: u32) -> Outcome {
        let mut outcome = Outcome::pass();
        if let Some(position) = self.consumed.iter().position(|code| *code == keycode) {
            self.consumed.swap_remove(position);
            outcome.handled = true;
        }
        if self.modifiers.release(keycode) {
            outcome.actions.extend(self.switch_layout(shared));
        }
        outcome
    }

    /// Alt+Shift: the next layout. The word typed so far ends without automatic conversion but
    /// stays available to Pause, in the layout it was typed in.
    fn switch_layout(&mut self, shared: &mut Shared) -> Vec<Action> {
        if shared.layouts.len() < 2 {
            return Vec::new();
        }
        let position = shared
            .layouts
            .iter()
            .position(|language| *language == shared.layout)
            .unwrap_or(0);
        let next = shared.layouts[(position + 1) % shared.layouts.len()];
        let word = std::mem::take(&mut self.word);
        let word_layout = self.word_layout.take();
        let last_word = self.last_word.take();
        self.forget();
        self.last_word = match (word.is_empty(), word_layout) {
            (false, Some(layout)) => Some(LastWord {
                keys: word,
                delimiter: None,
                layout,
            }),
            _ => last_word,
        };
        shared.layout = next;
        shared.log(
            "layout",
            &format!("result=switched by=alt-shift to={}", next.id()),
        );
        vec![Action::Layout(next)]
    }

    fn shortcut(&mut self, shared: &Shared, keyval: u32, control: bool, alt: bool) {
        let kind = match keyval {
            keysym::BACKSPACE if control => "erase-word",
            0x76 | 0x56 if control => "paste",
            0x63 | 0x43 if control => "copy",
            0x78 | 0x58 if control => "cut",
            0x7a | 0x5a | 0x79 | 0x59 if control => "undo",
            _ if control && alt => "ctrl-alt",
            _ if control => "ctrl",
            _ if alt => "alt",
            _ => "win",
        };
        if !self.private() && self.session.buffered_character_count() != 0 {
            shared.log(
                "input",
                &format!("result=suppressed reason=shortcut kind={kind}"),
            );
        }
        self.session
            .handle(InputEvent::Shortcut, Some(shared.layout), &shared.detector);
        self.forget_word_state();
    }

    fn switching_rule(&mut self, shared: &Shared, event: InputEvent, suppress: bool) {
        self.last_word = None;
        self.undo = None;
        self.cycle = None;
        self.word.clear();
        self.word_layout = None;
        self.manual_only = false;
        if suppress {
            self.session
                .handle(event, Some(shared.layout), &shared.detector);
        } else {
            self.session.clear();
        }
    }

    fn backspace(&mut self, shared: &Shared) -> Outcome {
        self.undo = None;
        self.cycle = None;
        if self.compose.take().is_some() {
            // The dead key showed nothing; Backspace only cancels it.
            self.consume(KEY_BACKSPACE);
            return Outcome::consumed(Vec::new());
        }
        if self.private() {
            return Outcome::pass();
        }
        if shared.settings.suppress_after_backspace || !self.erase_or_resume(shared) {
            self.last_word = None;
            self.word.clear();
            self.word_layout = None;
            self.manual_only = false;
            if shared.settings.suppress_after_backspace {
                self.session
                    .handle(InputEvent::Backspace, Some(shared.layout), &shared.detector);
            } else {
                self.session.clear();
            }
        }
        Outcome::pass()
    }

    /// Without suppression a Backspace corrects the word, or after a Space resumes the word before
    /// it for Pause (not for automatic conversion). False when the word must be discarded.
    fn erase_or_resume(&mut self, shared: &Shared) -> bool {
        if !self.word.is_empty() {
            if self.word.len() != self.session.buffered_character_count()
                || !self.session.erase_last_character()
            {
                return false;
            }
            self.word.pop();
            if self.word.is_empty() {
                self.word_layout = None;
            }
            return true;
        }
        let Some(last) = self.last_word.take() else {
            return false;
        };
        if last.delimiter.is_none() {
            return false;
        }
        let Some(word) = shared.keymaps.replay(last.layout, &last.keys) else {
            return false;
        };
        if !self.session.restore_word(&word) {
            return false;
        }
        self.word = last.keys;
        self.word_layout = Some(last.layout);
        self.manual_only = true;
        true
    }

    fn printable(&mut self, shared: &mut Shared, keycode: u32, levels: Levels) -> Outcome {
        let layout = shared.layout;
        let key = TypedKey { keycode, levels };
        let output = shared.keymaps.key(layout, key);
        let (outcome, typed) = if shared.passthrough == Some(layout) {
            let typed = match &output {
                KeyOutput::Text { text, .. } => text.chars().next(),
                _ => None,
            };
            (Outcome::pass(), typed)
        } else {
            self.type_emulated(shared, keycode, output)
        };
        self.undo = None;
        self.cycle = None;
        if self.private() {
            return outcome;
        }
        match typed {
            Some(character) => self.track(shared, character, key),
            None => {
                self.session
                    .handle(InputEvent::UnsupportedInput, Some(layout), &shared.detector);
                self.word.clear();
                self.word_layout = None;
                self.last_word = None;
            }
        }
        outcome
    }

    /// Types a key in a layout other than the engine's keymap. A dead key starts a compose
    /// sequence; the composed character is typed but ends the word.
    fn type_emulated(
        &mut self,
        shared: &Shared,
        keycode: u32,
        output: KeyOutput,
    ) -> (Outcome, Option<char>) {
        match output {
            KeyOutput::Dead { keysym } => {
                let Some(table) = &shared.compose else {
                    return (Outcome::pass(), None);
                };
                let mut state = xkb::compose::State::new(table, xkb::compose::STATE_NO_FLAGS);
                state.feed(xkb::Keysym::new(keysym));
                self.compose = Some(state);
                self.consume(keycode);
                (Outcome::consumed(Vec::new()), None)
            }
            KeyOutput::Text { text, keysym } => {
                self.consume(keycode);
                if let Some(mut state) = self.compose.take() {
                    state.feed(xkb::Keysym::new(keysym));
                    let committed = match state.status() {
                        xkb::compose::Status::Composed => state.utf8().unwrap_or(text),
                        _ => text,
                    };
                    return (Outcome::consumed(vec![Action::Commit(committed)]), None);
                }
                let typed = text.chars().next();
                (Outcome::consumed(vec![Action::Commit(text)]), typed)
            }
            KeyOutput::Other => (Outcome::pass(), None),
        }
    }

    fn track(&mut self, shared: &Shared, character: char, key: TypedKey) {
        let layout = shared.layout;
        if self
            .word_layout
            .is_some_and(|word_layout| word_layout != layout)
        {
            self.word.clear();
            self.session.clear();
        }
        let before = self.session.buffered_character_count();
        self.session.handle(
            InputEvent::Printable(character),
            Some(layout),
            &shared.detector,
        );
        let after = self.session.buffered_character_count();
        if after == before + 1 {
            self.word.push(key);
            self.word_layout = Some(layout);
        } else if after == 0 {
            self.word.clear();
            self.word_layout = None;
        }
        self.last_word = None;
    }

    /// Keeps the release of a key whose press the engine handled from the program; a held key
    /// repeats its press, so each key is listed once.
    fn consume(&mut self, keycode: u32) {
        if !self.consumed.contains(&keycode) {
            self.consumed.push(keycode);
        }
    }

    fn readings(
        &self,
        shared: &Shared,
        keys: &[TypedKey],
        source: Language,
    ) -> Vec<(Language, String)> {
        shared
            .layouts
            .iter()
            .copied()
            .filter(|language| *language != source)
            .filter_map(|language| Some((language, shared.keymaps.replay(language, keys)?)))
            .collect()
    }

    fn remember_language(&mut self, language: Language) {
        if self.recent.len() == RECENT_LANGUAGES {
            self.recent.remove(0);
        }
        self.recent.push(language);
    }

    fn space(&mut self, shared: &mut Shared) -> Outcome {
        self.undo = None;
        self.cycle = None;
        let keys = std::mem::take(&mut self.word);
        let layout = self.word_layout.take().unwrap_or(shared.layout);
        let manual_only = std::mem::take(&mut self.manual_only);
        if self.private() || keys.is_empty() {
            self.session
                .handle(InputEvent::Boundary, Some(shared.layout), &shared.detector);
            self.last_word = None;
            return Outcome::pass();
        }
        let candidates = self.readings(shared, &keys, layout);
        let action = self.session.finish_boundary_in_context(
            Some(layout),
            &shared.detector,
            &candidates,
            &self.recent,
        );
        self.last_word = Some(LastWord {
            keys: keys.clone(),
            delimiter: Some(' '),
            layout,
        });
        let detection = match action {
            SessionAction::Candidate(detection) if shared.automatic && !manual_only => detection,
            SessionAction::Candidate(_) => {
                let reason = if manual_only {
                    "manual-only"
                } else {
                    "automatic-off"
                };
                shared.log("word", &format!("result={reason}"));
                self.remember_language(layout);
                return Outcome::pass();
            }
            _ => {
                shared.log("word", "result=no-candidate");
                self.remember_language(layout);
                return Outcome::pass();
            }
        };
        let Some(outcome) = self.convert_word(shared, &keys, &detection) else {
            self.remember_language(layout);
            return Outcome::pass();
        };
        outcome
    }

    /// The automatic conversion of the word just ended by a Space: the space is typed with the
    /// replacement, and the layout switches to the target.
    fn convert_word(
        &mut self,
        shared: &mut Shared,
        keys: &[TypedKey],
        detection: &Detection,
    ) -> Option<Outcome> {
        let Some(transaction) = ConversionTransaction::new(detection, ' ') else {
            shared.log("conversion", "result=refused reason=edit-unit");
            return None;
        };
        let (mut actions, strategy) =
            self.edit(&detection.original, transaction.forward.insert_text.clone());
        actions.push(Action::Layout(detection.target_language));
        shared.layout = detection.target_language;
        self.remember_language(detection.target_language);
        self.undo = Some(Undo {
            keys: keys.to_vec(),
            typed: transaction.forward.insert_text,
            original: detection.original.clone(),
            delimiter: Some(' '),
            source: detection.source_language,
            target: detection.target_language,
        });
        self.last_word = Some(LastWord {
            keys: keys.to_vec(),
            delimiter: Some(' '),
            layout: detection.target_language,
        });
        shared.log(
            "conversion",
            &format!(
                "result=applied strategy={strategy} source={} target={} forced=false",
                detection.source_language.id(),
                detection.target_language.id()
            ),
        );
        Some(Outcome::consumed(actions))
    }

    /// Replaces `erase`, which ends at the caret, with `insert`.
    fn edit(&mut self, erase: &str, insert: String) -> (Vec<Action>, &'static str) {
        let count = erase.chars().count();
        let verified = self.purpose != PURPOSE_TERMINAL
            && self.surrounding.as_ref().is_some_and(|surrounding| {
                surrounding.cursor == surrounding.anchor
                    && surrounding
                        .text
                        .char_indices()
                        .nth(surrounding.cursor)
                        .map_or(surrounding.text.as_str(), |(index, _)| {
                            &surrounding.text[..index]
                        })
                        .ends_with(erase)
                    && surrounding.text.chars().count() >= surrounding.cursor
            });
        // The client reports the new text after the edit; until then the old one is stale.
        self.surrounding = None;
        let mut actions = Vec::with_capacity(count * 2 + 1);
        let strategy = if verified {
            actions.push(Action::DeleteSurrounding {
                offset: -i32::try_from(count).unwrap_or(i32::MAX),
                chars: u32::try_from(count).unwrap_or(u32::MAX),
            });
            "surrounding"
        } else {
            for _ in 0..count {
                for state in [0, RELEASE_MASK] {
                    actions.push(Action::ForwardKey {
                        keyval: keysym::BACKSPACE,
                        keycode: KEY_BACKSPACE,
                        state,
                    });
                }
            }
            "backspace"
        };
        actions.push(Action::Commit(insert));
        (actions, strategy)
    }

    /// Pause: undo the last automatic conversion, or else convert the word at the caret (or the
    /// last word) to the next layout, the most plausible reading first.
    fn hotkey(&mut self, shared: &mut Shared) -> Outcome {
        if self.private() {
            shared.log("hotkey", "result=ignored reason=private");
            return Outcome::consumed(Vec::new());
        }
        if shared.settings.pause_break_undo
            && let Some(undo) = self.undo.take()
        {
            let mut restored = undo.original.clone();
            if let Some(delimiter) = undo.delimiter {
                restored.push(delimiter);
            }
            let (mut actions, strategy) = self.edit(&undo.typed, restored);
            actions.push(Action::Layout(undo.source));
            shared.layout = undo.source;
            self.session.clear();
            self.last_word = Some(LastWord {
                keys: undo.keys,
                delimiter: undo.delimiter,
                layout: undo.source,
            });
            shared.log(
                "undo",
                &format!(
                    "result=applied strategy={strategy} source={} target={}",
                    undo.target.id(),
                    undo.source.id()
                ),
            );
            return Outcome::consumed(actions);
        }
        let (keys, delimiter, language, order) = if let Some(cycle) = self.cycle.take() {
            (
                cycle.keys,
                cycle.delimiter,
                cycle.language,
                Some(cycle.order),
            )
        } else if let (false, Some(layout)) = (self.word.is_empty(), self.word_layout) {
            (std::mem::take(&mut self.word), None, layout, None)
        } else if let Some(last) = self.last_word.take() {
            (last.keys, last.delimiter, last.layout, None)
        } else {
            shared.log("hotkey", "result=ignored reason=no-word");
            return Outcome::consumed(Vec::new());
        };
        self.session.clear();
        self.word.clear();
        self.word_layout = None;
        self.manual_only = false;
        let enabled = shared.layouts.clone();
        let Some(current_text) = shared.keymaps.replay(language, &keys) else {
            shared.log("hotkey", "result=ignored reason=key-without-character");
            return Outcome::consumed(Vec::new());
        };
        let order = order
            .filter(|order| {
                order.len() == enabled.len()
                    && enabled.iter().all(|language| order.contains(language))
            })
            .unwrap_or_else(|| {
                let readings = self.readings(shared, &keys, language);
                layout_cycle_order(
                    language,
                    &enabled,
                    &shared.detector.rank_candidates(&readings),
                )
            });
        let position = order
            .iter()
            .position(|entry| *entry == language)
            .unwrap_or(0);
        let target = (1..order.len()).find_map(|step| {
            let candidate = order[(position + step) % order.len()];
            let text = shared.keymaps.replay(candidate, &keys)?;
            (text != current_text).then_some((candidate, text))
        });
        let Some((target, target_text)) = target else {
            shared.log("hotkey", "result=ignored reason=identical");
            return Outcome::consumed(Vec::new());
        };
        let mut erase = current_text;
        let mut insert = target_text;
        if let Some(delimiter) = delimiter {
            erase.push(delimiter);
            insert.push(delimiter);
        }
        let (mut actions, strategy) = self.edit(&erase, insert);
        actions.push(Action::Layout(target));
        shared.layout = target;
        shared.log(
            "conversion",
            &format!(
                "result=applied strategy={strategy} source={} target={} forced=true cycle={}",
                language.id(),
                target.id(),
                order
                    .iter()
                    .map(|language| language.id())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
        self.cycle = Some(Cycle {
            keys,
            delimiter,
            order,
            language: target,
        });
        Outcome::consumed(actions)
    }
}

// These tests type Russian and Estonian, whose dictionaries the modular base downloads.
#[cfg(all(test, feature = "legacy-bundled-input"))]
mod tests {
    use super::*;
    use autokeyboardlayot::{DetectorConfig, DictionaryRegistry};
    use std::sync::Arc;

    // evdev codes of the keys the tests type.
    const KEY_1: u32 = 2;
    const KEY_Q: u32 = 16;
    const KEY_W: u32 = 17;
    const KEY_E: u32 = 18;
    const KEY_T: u32 = 20;
    const KEY_I: u32 = 23;
    const KEY_O: u32 = 24;
    const KEY_LEFTBRACE: u32 = 26;
    const KEY_S: u32 = 31;
    const KEY_D: u32 = 32;
    const KEY_G: u32 = 34;
    const KEY_H: u32 = 35;
    const KEY_J: u32 = 36;
    const KEY_L: u32 = 38;
    const KEY_GRAVE: u32 = 41;
    const KEY_C: u32 = 46;
    const KEY_V: u32 = 47;
    const KEY_B: u32 = 48;
    const KEY_N: u32 = 49;
    const KEY_PAUSE: u32 = 119;
    const KEY_LEFT: u32 = 105;

    fn shared_with(settings: Settings) -> (Shared, std::sync::mpsc::Receiver<String>) {
        let mut registry = DictionaryRegistry::embedded();
        registry
            .set_enabled(settings.enabled_input_packs.iter().cloned())
            .unwrap();
        let detector = Detector::with_profile_selections(
            DetectorConfig::default(),
            Arc::new(registry),
            &Default::default(),
        );
        let languages: Vec<Language> = settings.enabled_input_packs.iter().copied().collect();
        let keymaps = Keymaps::new(&languages).unwrap();
        let (diagnostics, receiver) = Diagnostics::channel();
        (
            Shared::new(detector, keymaps, settings, diagnostics).unwrap(),
            receiver,
        )
    }

    fn default_shared() -> (Shared, std::sync::mpsc::Receiver<String>) {
        shared_with(Settings::default())
    }

    /// The keysym the US keymap gives for a key, as the compositor reports it to IBus.
    fn keysym_of(shared: &Shared, keycode: u32, state: u32) -> u32 {
        match keycode {
            KEY_SPACE => keysym::SPACE,
            KEY_BACKSPACE => keysym::BACKSPACE,
            KEY_PAUSE => keysym::PAUSE,
            KEY_LEFT => keysym::LEFT,
            KEY_LEFTSHIFT => 0xffe1,
            KEY_LEFTALT => 0xffe9,
            KEY_LEFTCTRL => 0xffe3,
            _ => match shared.keymaps.key(
                Language::English,
                TypedKey {
                    keycode,
                    levels: Levels {
                        shift: state & SHIFT_MASK != 0,
                        ..Levels::default()
                    },
                },
            ) {
                KeyOutput::Text { keysym, .. } => keysym,
                _ => 0,
            },
        }
    }

    /// A press and its release; the actions of both.
    fn tap(engine: &mut Engine, shared: &mut Shared, keycode: u32, state: u32) -> Outcome {
        let keyval = keysym_of(shared, keycode, state);
        let mut outcome = engine.process_key(shared, keyval, keycode, state);
        let release = engine.process_key(shared, keyval, keycode, state | RELEASE_MASK);
        outcome.actions.extend(release.actions);
        outcome
    }

    fn type_keys(engine: &mut Engine, shared: &mut Shared, keys: &[u32]) -> Vec<Outcome> {
        keys.iter()
            .map(|key| tap(engine, shared, *key, 0))
            .collect()
    }

    fn commits(outcomes: &[Outcome]) -> String {
        outcomes
            .iter()
            .flat_map(|outcome| &outcome.actions)
            .filter_map(|action| match action {
                Action::Commit(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn backspaces(outcome: &Outcome) -> usize {
        outcome
            .actions
            .iter()
            .filter(|action| {
                matches!(
                    action,
                    Action::ForwardKey {
                        state: 0,
                        keyval: keysym::BACKSPACE,
                        ..
                    }
                )
            })
            .count()
    }

    const GHBDTN: [u32; 6] = [KEY_G, KEY_H, KEY_B, KEY_D, KEY_T, KEY_N];

    #[test]
    fn keys_pass_through_in_the_base_layout_and_are_committed_in_the_others() {
        let (mut shared, _log) = default_shared();
        let mut engine = Engine::new(&shared);
        assert_eq!(shared.layout(), Language::English);
        let outcomes = type_keys(&mut engine, &mut shared, &[KEY_Q, KEY_1]);
        assert!(
            outcomes
                .iter()
                .all(|outcome| !outcome.handled && outcome.actions.is_empty())
        );
        shared.layout = Language::Russian;
        let outcomes = type_keys(&mut engine, &mut shared, &[KEY_Q, KEY_LEFTBRACE]);
        assert!(outcomes.iter().all(|outcome| outcome.handled));
        assert_eq!(commits(&outcomes), "йх");
        // Shift chooses the upper case; the release of a committed key is kept from the program.
        let outcome = engine.process_key(&mut shared, 0x51, KEY_Q, SHIFT_MASK);
        assert_eq!(outcome.actions, [Action::Commit("Й".into())]);
        assert!(
            engine
                .process_key(&mut shared, 0x51, KEY_Q, SHIFT_MASK | RELEASE_MASK)
                .handled
        );
        shared.layout = Language::Estonian;
        assert_eq!(
            commits(&type_keys(&mut engine, &mut shared, &[KEY_LEFTBRACE])),
            "ü"
        );
    }

    #[test]
    fn a_word_typed_in_the_wrong_layout_is_replaced_on_space_by_backspaces_without_surrounding_text()
     {
        let (mut shared, log) = default_shared();
        let mut engine = Engine::new(&shared);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        let outcome = tap(&mut engine, &mut shared, KEY_SPACE, 0);
        assert!(outcome.handled);
        assert_eq!(backspaces(&outcome), 6);
        assert_eq!(commits(std::slice::from_ref(&outcome)), "привет ");
        assert_eq!(
            outcome.actions.last(),
            Some(&Action::Layout(Language::Russian))
        );
        assert_eq!(shared.layout(), Language::Russian);
        // The next word is typed in Russian.
        assert_eq!(commits(&type_keys(&mut engine, &mut shared, &[KEY_D])), "в");
        let records: Vec<String> = log.try_iter().collect();
        assert!(
            records.iter().any(|record| record.contains(
                "event=conversion result=applied strategy=backspace source=en-US target=ru-RU"
            )),
            "{records:?}"
        );
        assert!(
            records
                .iter()
                .all(|record| !record.contains("привет") && !record.contains("ghbdtn"))
        );
    }

    #[test]
    fn a_verified_surrounding_text_is_edited_directly_and_a_stale_one_is_not() {
        let (mut shared, _log) = default_shared();
        let mut engine = Engine::new(&shared);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        engine.set_surrounding_text("hi ghbdtn".into(), 9, 9);
        let outcome = tap(&mut engine, &mut shared, KEY_SPACE, 0);
        assert_eq!(
            outcome.actions[0],
            Action::DeleteSurrounding {
                offset: -6,
                chars: 6
            }
        );
        assert_eq!(outcome.actions[1], Action::Commit("привет ".into()));
        // A surrounding text that does not end with the word (late, or another field) is not used.
        let (mut shared, _log) = shared_with(Settings::default());
        let mut engine = Engine::new(&shared);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        engine.set_surrounding_text("hi ghbdt".into(), 8, 8);
        let outcome = tap(&mut engine, &mut shared, KEY_SPACE, 0);
        assert_eq!(backspaces(&outcome), 6);
        // A terminal is always edited with Backspace.
        let (mut shared, _log) = shared_with(Settings::default());
        let mut engine = Engine::new(&shared);
        engine.set_content_type(&shared, PURPOSE_TERMINAL, 0);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        engine.set_surrounding_text("ghbdtn".into(), 6, 6);
        assert_eq!(backspaces(&tap(&mut engine, &mut shared, KEY_SPACE, 0)), 6);
    }

    #[test]
    fn a_correct_word_is_left_alone() {
        let (mut shared, log) = default_shared();
        let mut engine = Engine::new(&shared);
        // "hello" typed in the English layout.
        let outcomes = type_keys(
            &mut engine,
            &mut shared,
            &[KEY_H, KEY_E, KEY_L, KEY_L, KEY_O, KEY_SPACE],
        );
        assert!(
            outcomes
                .iter()
                .all(|outcome| !outcome.handled && outcome.actions.is_empty())
        );
        assert_eq!(shared.layout(), Language::English);
        assert!(
            log.try_iter()
                .any(|record| record.contains("event=word result=no-candidate"))
        );
    }

    #[test]
    fn pause_undoes_an_automatic_conversion_and_then_cycles() {
        let (mut shared, _log) = default_shared();
        let mut engine = Engine::new(&shared);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        tap(&mut engine, &mut shared, KEY_SPACE, 0);
        let undo = tap(&mut engine, &mut shared, KEY_PAUSE, 0);
        assert!(undo.handled);
        assert_eq!(backspaces(&undo), 7);
        assert_eq!(commits(std::slice::from_ref(&undo)), "ghbdtn ");
        assert_eq!(shared.layout(), Language::English);
        // A second press converts the restored word again, Russian first.
        let forced = tap(&mut engine, &mut shared, KEY_PAUSE, 0);
        assert_eq!(commits(std::slice::from_ref(&forced)), "привет ");
        assert_eq!(shared.layout(), Language::Russian);
        // The next press goes on to Estonian, which reads these keys like English.
        let third = tap(&mut engine, &mut shared, KEY_PAUSE, 0);
        assert_eq!(shared.layout(), Language::Estonian);
        assert_eq!(commits(std::slice::from_ref(&third)), "ghbdtn ");
        // From there English would give the same text, so the cycle comes back to Russian.
        let fourth = tap(&mut engine, &mut shared, KEY_PAUSE, 0);
        assert_eq!(shared.layout(), Language::Russian);
        assert_eq!(commits(std::slice::from_ref(&fourth)), "привет ");
    }

    #[test]
    fn pause_converts_the_word_at_the_caret_without_a_space() {
        let (mut shared, _log) = default_shared();
        let mut engine = Engine::new(&shared);
        // "[jhjij" in the English layout is "хорошо" in Russian.
        type_keys(
            &mut engine,
            &mut shared,
            &[KEY_LEFTBRACE, KEY_J, KEY_H, KEY_J, KEY_I, KEY_J],
        );
        let outcome = tap(&mut engine, &mut shared, KEY_PAUSE, 0);
        assert_eq!(backspaces(&outcome), 6);
        assert_eq!(commits(std::slice::from_ref(&outcome)), "хорошо");
        assert_eq!(shared.layout(), Language::Russian);
        // Nothing to convert: the press is still kept from the program.
        let (mut shared, log) = shared_with(Settings::default());
        let mut engine = Engine::new(&shared);
        let outcome = tap(&mut engine, &mut shared, KEY_PAUSE, 0);
        assert!(outcome.handled && outcome.actions.is_empty());
        assert!(
            log.try_iter()
                .any(|record| record.contains("result=ignored reason=no-word"))
        );
    }

    #[test]
    fn a_password_field_is_typed_in_the_current_layout_but_never_tracked() {
        let (mut shared, log) = default_shared();
        let mut engine = Engine::new(&shared);
        engine.set_content_type(&shared, PURPOSE_PASSWORD, 0);
        let outcomes = type_keys(&mut engine, &mut shared, &GHBDTN);
        let space = tap(&mut engine, &mut shared, KEY_SPACE, 0);
        assert!(
            outcomes
                .iter()
                .chain([&space])
                .all(|outcome| !outcome.handled)
        );
        assert_eq!(shared.layout(), Language::English);
        shared.layout = Language::Russian;
        assert_eq!(commits(&type_keys(&mut engine, &mut shared, &[KEY_Q])), "й");
        assert!(
            tap(&mut engine, &mut shared, KEY_PAUSE, 0)
                .actions
                .is_empty()
        );
        let records: Vec<String> = log.try_iter().collect();
        assert!(
            records
                .iter()
                .any(|record| record.contains("event=field private=true"))
        );
        assert!(records.iter().all(|record| !record.contains("conversion")));
        // The private hint has the same effect.
        let (mut shared, _log) = shared_with(Settings::default());
        let mut engine = Engine::new(&shared);
        engine.set_content_type(&shared, 0, HINT_PRIVATE);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        assert!(!tap(&mut engine, &mut shared, KEY_SPACE, 0).handled);
    }

    #[test]
    fn shortcuts_and_navigation_end_the_word() {
        let (mut shared, log) = default_shared();
        let mut engine = Engine::new(&shared);
        type_keys(&mut engine, &mut shared, &GHBDTN[..3]);
        let control = engine.process_key(&mut shared, 0xffe3, KEY_LEFTCTRL, 0);
        assert!(!control.handled);
        let paste = engine.process_key(&mut shared, 0x76, KEY_V, CONTROL_MASK);
        assert!(!paste.handled && paste.actions.is_empty());
        engine.process_key(
            &mut shared,
            0xffe3,
            KEY_LEFTCTRL,
            CONTROL_MASK | RELEASE_MASK,
        );
        type_keys(&mut engine, &mut shared, &GHBDTN[3..]);
        assert!(
            !tap(&mut engine, &mut shared, KEY_SPACE, 0).handled,
            "the word was cut by Ctrl+V"
        );
        assert!(
            log.try_iter()
                .any(|record| record.contains("reason=shortcut kind=paste"))
        );
        type_keys(&mut engine, &mut shared, &GHBDTN[..3]);
        tap(&mut engine, &mut shared, KEY_LEFT, 0);
        type_keys(&mut engine, &mut shared, &GHBDTN[3..]);
        assert!(
            !tap(&mut engine, &mut shared, KEY_SPACE, 0).handled,
            "the word was cut by Left"
        );
        // Ctrl+C in the Russian layout reaches the program as a shortcut, untouched.
        shared.layout = Language::Russian;
        let copy = engine.process_key(&mut shared, 0x63, KEY_C, CONTROL_MASK);
        assert!(!copy.handled && copy.actions.is_empty());
    }

    #[test]
    fn alt_shift_switches_the_layout_alone_and_keeps_the_word_for_pause() {
        let (mut shared, log) = default_shared();
        let mut engine = Engine::new(&shared);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        engine.process_key(&mut shared, 0xffe9, KEY_LEFTALT, 0);
        engine.process_key(&mut shared, 0xffe7, KEY_LEFTSHIFT, MOD1_MASK);
        engine.process_key(
            &mut shared,
            0xffe7,
            KEY_LEFTSHIFT,
            MOD1_MASK | SHIFT_MASK | RELEASE_MASK,
        );
        let release =
            engine.process_key(&mut shared, 0xffe9, KEY_LEFTALT, MOD1_MASK | RELEASE_MASK);
        assert_eq!(release.actions, [Action::Layout(Language::Russian)]);
        assert!(log.try_iter().any(|record| record.contains("event=layout result=switched by=alt-shift to=ru-RU")));
        // The switch ended the word without converting it; Pause still converts it.
        let outcome = tap(&mut engine, &mut shared, KEY_PAUSE, 0);
        assert_eq!(commits(std::slice::from_ref(&outcome)), "привет");
        // Alt+Shift+Tab is a shortcut, not a switch.
        let before = shared.layout();
        engine.process_key(&mut shared, 0xffe9, KEY_LEFTALT, 0);
        engine.process_key(&mut shared, 0xffe7, KEY_LEFTSHIFT, MOD1_MASK);
        engine.process_key(
            &mut shared,
            keysym::ISO_LEFT_TAB,
            15,
            MOD1_MASK | SHIFT_MASK,
        );
        engine.process_key(
            &mut shared,
            0xffe7,
            KEY_LEFTSHIFT,
            MOD1_MASK | SHIFT_MASK | RELEASE_MASK,
        );
        let release =
            engine.process_key(&mut shared, 0xffe9, KEY_LEFTALT, MOD1_MASK | RELEASE_MASK);
        assert!(release.actions.is_empty());
        assert_eq!(shared.layout(), before);
        // The layouts go round: English, Russian, Estonian, English.
        let mut seen = Vec::new();
        for _ in 0..3 {
            engine.process_key(&mut shared, 0xffe9, KEY_LEFTALT, 0);
            engine.process_key(&mut shared, 0xffe1, KEY_LEFTSHIFT, MOD1_MASK);
            engine.process_key(
                &mut shared,
                0xffe1,
                KEY_LEFTSHIFT,
                MOD1_MASK | SHIFT_MASK | RELEASE_MASK,
            );
            engine.process_key(&mut shared, 0xffe9, KEY_LEFTALT, MOD1_MASK | RELEASE_MASK);
            seen.push(shared.layout());
        }
        assert_eq!(
            seen,
            [Language::Estonian, Language::English, Language::Russian]
        );
    }

    #[test]
    fn backspace_corrects_the_word_or_resumes_it_for_pause_only() {
        let settings = Settings {
            suppress_after_backspace: false,
            ..Settings::default()
        };
        let (mut shared, log) = shared_with(settings.clone());
        let mut engine = Engine::new(&shared);
        // "ghbdtw", Backspace, "n": corrected to "ghbdtn" and still converted automatically.
        type_keys(
            &mut engine,
            &mut shared,
            &[
                KEY_G,
                KEY_H,
                KEY_B,
                KEY_D,
                KEY_T,
                KEY_W,
                KEY_BACKSPACE,
                KEY_N,
            ],
        );
        assert!(tap(&mut engine, &mut shared, KEY_SPACE, 0).handled);
        // Erasing the space of a finished word resumes it, but only for Pause.
        let (mut shared, log2) = shared_with(settings);
        let mut engine = Engine::new(&shared);
        shared.set_automatic(false);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        tap(&mut engine, &mut shared, KEY_SPACE, 0);
        shared.set_automatic(true);
        tap(&mut engine, &mut shared, KEY_BACKSPACE, 0);
        assert!(
            !tap(&mut engine, &mut shared, KEY_SPACE, 0).handled,
            "manual-only after a resume"
        );
        assert!(
            log2.try_iter()
                .any(|record| record.contains("event=word result=manual-only"))
        );
        drop(log);
        // With suppression on (the default) a Backspace ends the word.
        let (mut shared, _log) = default_shared();
        let mut engine = Engine::new(&shared);
        type_keys(
            &mut engine,
            &mut shared,
            &[
                KEY_G,
                KEY_H,
                KEY_B,
                KEY_D,
                KEY_T,
                KEY_W,
                KEY_BACKSPACE,
                KEY_N,
            ],
        );
        assert!(!tap(&mut engine, &mut shared, KEY_SPACE, 0).handled);
    }

    #[test]
    fn a_dead_key_composes_in_an_emulated_layout_and_ends_the_word() {
        let (mut shared, _log) = default_shared();
        let mut engine = Engine::new(&shared);
        shared.layout = Language::Estonian;
        if shared.compose.is_none() {
            return; // no compose data for the locale on this machine
        }
        let dead = tap(&mut engine, &mut shared, KEY_GRAVE, 0);
        assert!(dead.handled && dead.actions.is_empty());
        let composed = tap(&mut engine, &mut shared, KEY_S, 0);
        assert_eq!(commits(std::slice::from_ref(&composed)), "š");
        // A Backspace after a dead key only cancels it.
        tap(&mut engine, &mut shared, KEY_GRAVE, 0);
        let cancel = tap(&mut engine, &mut shared, KEY_BACKSPACE, 0);
        assert!(cancel.handled && cancel.actions.is_empty());
    }

    #[test]
    fn focus_changes_forget_the_word_and_show_the_layout() {
        let (mut shared, _log) = default_shared();
        let mut engine = Engine::new(&shared);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        engine.focus_out();
        assert_eq!(
            engine.focus_in(&shared),
            [
                Action::Layout(Language::English),
                Action::RequireSurrounding
            ]
        );
        assert!(!tap(&mut engine, &mut shared, KEY_SPACE, 0).handled);
        type_keys(&mut engine, &mut shared, &GHBDTN);
        engine.reset();
        assert!(
            tap(&mut engine, &mut shared, KEY_PAUSE, 0)
                .actions
                .is_empty()
        );
    }

    #[test]
    fn the_hotkey_follows_the_settings() {
        let pause = HotkeyMatch::new(Hotkey::default());
        assert!(pause.matches(keysym::PAUSE, 0));
        assert!(
            pause.matches(keysym::PAUSE, LOCK_MASK),
            "Caps Lock does not matter"
        );
        assert!(!pause.matches(keysym::PAUSE, CONTROL_MASK));
        let control_f9 = HotkeyMatch::new(
            Hotkey::new(0x78, autokeyboardlayot::hotkey::HOTKEY_MOD_CONTROL).unwrap(),
        );
        assert!(control_f9.matches(keysym::F1 + 8, CONTROL_MASK));
        assert!(!control_f9.matches(keysym::F1 + 8, 0));
        let control_alt_k = HotkeyMatch::new(
            Hotkey::new(
                0x4b,
                autokeyboardlayot::hotkey::HOTKEY_MOD_CONTROL
                    | autokeyboardlayot::hotkey::HOTKEY_MOD_ALT,
            )
            .unwrap(),
        );
        assert!(control_alt_k.matches(0x6b, CONTROL_MASK | MOD1_MASK));
        assert!(control_alt_k.matches(0x4b, CONTROL_MASK | MOD1_MASK));
        let control_pause = HotkeyMatch::new(
            Hotkey::new(0x13, autokeyboardlayot::hotkey::HOTKEY_MOD_CONTROL).unwrap(),
        );
        assert!(control_pause.matches(keysym::BREAK, CONTROL_MASK));
    }

    #[test]
    fn the_layouts_go_in_the_owners_order() {
        let order = layout_order(vec![
            Language::Estonian,
            Language::English,
            Language::Russian,
        ]);
        assert_eq!(
            order,
            [Language::English, Language::Russian, Language::Estonian]
        );
        assert_eq!(
            layout_cycle_order(Language::English, &order, &[Language::Russian]),
            [Language::English, Language::Russian, Language::Estonian]
        );
    }
}
