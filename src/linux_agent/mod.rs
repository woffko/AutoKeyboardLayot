//! The Linux front end: an IBus engine on the shared core (see docs/linux-port-plan.md).

mod diagnostics;
#[cfg(test)]
mod e2e_tests;
mod engine;
mod ibus;
mod keymap;
mod localization;
mod paths;
mod runtime;
mod settings;

use std::{path::PathBuf, process::ExitCode};

// Names the package pages shared with the Windows settings window look up in their grandparent.
use localization as ui_localization;

/// The directory that holds the package store (`packages/`): the XDG data directory.
fn package_root() -> Option<PathBuf> {
    paths::data_dir()
}

fn try_load_configuration_document() -> std::io::Result<autokeyboardlayot::ConfigurationDocument> {
    let directory = paths::config_dir()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))?;
    autokeyboardlayot::configuration::load_configuration_snapshot(&directory)
        .map(|loaded| loaded.document)
}

pub const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AUTOKEY_BUILD_COMMIT"),
    ")"
);

const USAGE: &str = "\
Usage: autokeyboardlayot [OPTION]

  --ibus                  run as the IBus engine (IBus starts it from its component file)
  --register              run and register the engine with the running IBus (development)
  --settings              open the settings window
  --print-component EXEC  print the IBus component file for an engine started as EXEC
  --version               print the version
  --help                  print this help
";

/// Entry point on Linux.
pub fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--ibus"] => run_engine(false),
        ["--register"] => run_engine(true),
        ["--settings"] => match settings::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("autokeyboardlayot: {error}");
                ExitCode::FAILURE
            }
        },
        ["--print-component", exec] => {
            print!("{}", ibus::component_xml(exec, VERSION));
            ExitCode::SUCCESS
        }
        ["--version"] => {
            println!("AutoKeyboardLayot {VERSION}");
            ExitCode::SUCCESS
        }
        ["--help"] | [] => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Connects to IBus and serves the engine until IBus closes the connection.
fn run_engine(register: bool) -> ExitCode {
    if let Some(directory) = paths::state_dir() {
        autokeyboardlayot::crash_log::install(directory, VERSION);
    }
    match serve(register) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("autokeyboardlayot: {error}");
            ExitCode::FAILURE
        }
    }
}

fn serve(register: bool) -> Result<(), Box<dyn std::error::Error>> {
    let address = ibus::address().ok_or("the IBus daemon address is unknown; is IBus running?")?;
    let shared = runtime::load_shared()?;
    let connection = ibus::connect(&address)?;
    if register {
        let exec = std::env::current_exe()?;
        ibus::register_component(&connection, &format!("{} --ibus", exec.display()), VERSION)?;
    } else {
        connection.request_name(ibus::COMPONENT_NAME)?;
    }
    shared.diagnostics.record(
        "engine",
        &format!("result=started version={}", VERSION.replace(' ', "_")),
    );
    let mut stamp = runtime::config_stamp();
    let mut host = ibus::Host::new(shared)
        .with_reload(move |_| {
            let current = runtime::config_stamp();
            if current == stamp {
                return None;
            }
            stamp = current;
            runtime::load_shared().ok()
        })
        .with_settings(open_settings);
    host.serve(&connection)?;
    Ok(())
}

/// Opens the settings window as a process of its own and reaps it when it ends.
fn open_settings() {
    let Ok(exec) = std::env::current_exe() else {
        return;
    };
    if let Ok(mut child) = std::process::Command::new(exec).arg("--settings").spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}
