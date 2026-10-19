//! The mutation generator and the harness of `tests/parser_mutation_smoke.rs`, shared with
//! `tests/parser_mutation_harness.rs`, which checks that the harness notices a panicking parser.
//! That check silences the process-wide panic hook, so it runs in a test binary of its own.
//!
//! Inputs are derived from seeds by xorshift-driven mutations (bit flips, replaced, deleted and
//! inserted bytes, truncation, duplicated chunks, hostile snippets and chunks of sibling seeds).
//! The generator is deterministic: a failure reports the target, the iteration and the input in
//! hex, and running the test again reproduces it.

use std::{
    panic::{self, AssertUnwindSafe},
    time::{Duration, Instant},
};

pub struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }
}

const SNIPPETS: [&str; 15] = [
    "é",
    // Lowercases to two characters.
    "İ",
    "я",
    "ü",
    "€",
    "😀",
    "\u{0301}",
    "\u{202e}",
    "{",
    "}",
    "\u{0}",
    "\\",
    "\"",
    "99999999999999999999",
    "-1",
];

/// One to four mutations of `seed`. Positions stay below `window` bytes, which keeps mutations of
/// a large seed (a model file) in its header, where the format checks live.
pub fn mutate(rng: &mut Rng, seed: &[u8], pool: &[Vec<u8>], window: usize) -> Vec<u8> {
    let mut data = seed.to_vec();
    for _ in 0..1 + rng.below(4) {
        let reach = data.len().min(window);
        match rng.below(9) {
            0 if reach > 0 => {
                let index = rng.below(reach);
                data[index] ^= 1 << rng.below(8);
            }
            1 if reach > 0 => {
                let index = rng.below(reach);
                data[index] = rng.next() as u8;
            }
            2 if reach > 0 => {
                let index = rng.below(reach);
                data.remove(index);
            }
            3 => {
                let index = rng.below(reach + 1);
                data.insert(index, rng.next() as u8);
            }
            4 if reach > 0 => {
                let length = rng.below(reach);
                data.truncate(length);
            }
            5 if reach > 0 => {
                let start = rng.below(reach);
                let end = (start + 1 + rng.below(64)).min(data.len());
                let chunk = data[start..end].to_vec();
                let at = rng.below(reach + 1);
                data.splice(at..at, chunk);
            }
            6 => {
                let snippet = SNIPPETS[rng.below(SNIPPETS.len())];
                let at = rng.below(reach + 1);
                data.splice(at..at, snippet.bytes());
            }
            7 if !pool.is_empty() && reach > 0 => {
                let other = &pool[rng.below(pool.len())];
                if !other.is_empty() {
                    let start = rng.below(other.len());
                    let end = (start + 1 + rng.below(200)).min(other.len());
                    let at = rng.below(reach + 1);
                    data.splice(at..at, other[start..end].to_vec());
                }
            }
            _ => {}
        }
    }
    data
}

/// One parser entry point, fed arbitrary bytes. It must return, never panic, and says whether the
/// parser accepted the input.
pub type ParserCall<'a> = Box<dyn Fn(&[u8]) -> bool + 'a>;

pub struct Target<'a> {
    pub name: &'static str,
    /// Inputs the parser accepts; mutations start from them.
    pub seeds: Vec<Vec<u8>>,
    pub iterations: usize,
    /// How far into a seed mutations may reach.
    pub window: usize,
    pub run: ParserCall<'a>,
}

#[derive(Debug)]
pub struct Failure {
    pub iteration: usize,
    pub input: Vec<u8>,
}

#[derive(Debug)]
pub struct Report {
    pub panics: usize,
    /// Mutated inputs the parser accepted.
    pub accepted: usize,
    pub first_failure: Option<Failure>,
    pub slowest: Duration,
    pub elapsed: Duration,
}

/// The generator is seeded from the target name only, so every run is the same run.
pub fn rng_for(name: &str) -> Rng {
    let spread = name.bytes().fold(0u64, |sum, byte| {
        sum.wrapping_mul(131).wrapping_add(u64::from(byte))
    });
    Rng(0x9E37_79B9_7F4A_7C15 ^ spread.wrapping_mul(7919))
}

/// Feeds every seed unchanged, then mutated inputs, to the target and counts panics.
pub fn run_target(target: &Target<'_>) -> Report {
    let started = Instant::now();
    let mut rng = rng_for(target.name);
    let mut report = Report {
        panics: 0,
        accepted: 0,
        first_failure: None,
        slowest: Duration::ZERO,
        elapsed: Duration::ZERO,
    };
    for iteration in 0..target.iterations {
        let seed = &target.seeds[iteration % target.seeds.len()];
        let mutated = iteration >= target.seeds.len();
        let input = if mutated {
            mutate(&mut rng, seed, &target.seeds, target.window)
        } else {
            seed.clone()
        };
        let begin = Instant::now();
        let result = panic::catch_unwind(AssertUnwindSafe(|| (target.run)(&input)));
        report.slowest = report.slowest.max(begin.elapsed());
        match result {
            Ok(accepted) => report.accepted += usize::from(mutated && accepted),
            Err(_) => {
                report.panics += 1;
                if report.first_failure.is_none() {
                    report.first_failure = Some(Failure { iteration, input });
                }
            }
        }
    }
    report.elapsed = started.elapsed();
    report
}

/// One line about a finished target.
pub fn summary(target: &Target<'_>, report: &Report) -> String {
    format!(
        "{:<34} iterations={:>6} panics={} accepted={:>5} slowest={:>9.1?} total={:>8.1?}",
        target.name,
        target.iterations,
        report.panics,
        report.accepted,
        report.slowest,
        report.elapsed
    )
}

/// What to report when the target panicked: how often, and the first input that did it.
pub fn failure(target: &Target<'_>, report: &Report) -> Option<String> {
    let failure = report.first_failure.as_ref()?;
    Some(format!(
        "{}: {} panics; first at iteration {} with input ({} bytes, hex of the first 300): {}",
        target.name,
        report.panics,
        failure.iteration,
        failure.input.len(),
        hex(&failure.input)
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take(300)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
