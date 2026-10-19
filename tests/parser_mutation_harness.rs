//! The harness of `tests/parser_mutation_smoke.rs` must notice a parser that panics, and report
//! where. The panics it provokes on purpose would each print a message, so this test silences the
//! process-wide panic hook while it runs; it is the only test of its binary, so no other test can
//! lose its panic message to that.

#[path = "support/mutation_harness.rs"]
mod harness;

use std::panic;

use harness::{Target, failure, mutate, rng_for, run_target, summary};

#[test]
fn the_harness_reports_a_panicking_parser_and_where() {
    let target = Target {
        name: "synthetic panic",
        seeds: vec![b"aaaa".to_vec()],
        iterations: 500,
        window: usize::MAX,
        run: Box::new(|data| {
            assert!(data.iter().all(|byte| *byte >= 0x20), "synthetic failure");
            data.len() == 4
        }),
    };
    let previous = panic::take_hook();
    panic::set_hook(Box::new(|_| {}));
    let report = run_target(&target);
    panic::set_hook(previous);
    println!("{}", summary(&target, &report));
    assert!(
        report.panics > 0,
        "no mutated input contained a control byte: {report:?}"
    );
    let first = report.first_failure.as_ref().unwrap();
    assert!(first.input.iter().any(|byte| *byte < 0x20) && first.iteration >= 1);
    let message = failure(&target, &report).unwrap();
    assert!(
        message.starts_with(&format!(
            "synthetic panic: {} panics; first at iteration {} ",
            report.panics, first.iteration
        )),
        "{message}"
    );
    let hex: String = first
        .input
        .iter()
        .take(300)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert!(message.ends_with(&hex), "{message}");
    // Only mutated inputs the parser accepted count, not the seed: replaying the generator gives
    // the same number.
    let mut rng = rng_for(target.name);
    let accepted = (1..target.iterations)
        .map(|_| mutate(&mut rng, &target.seeds[0], &target.seeds, target.window))
        .filter(|input| input.len() == 4 && input.iter().all(|byte| *byte >= 0x20))
        .count();
    assert!(accepted > 0);
    assert_eq!(report.accepted, accepted);
}
