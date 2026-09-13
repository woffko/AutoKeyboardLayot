//! Offline validation of external translation catalogs against embedded English.
use autokeyboardlayot::localization::CatalogRegistry;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let Some(directory) = arguments.next() else {
        eprintln!("Usage: validate_locales <catalog-directory> [--require-complete]");
        std::process::exit(2);
    };
    let require_complete = match (arguments.next(), arguments.next()) {
        (None, None) => false,
        (Some(flag), None) if flag == "--require-complete" => true,
        _ => {
            eprintln!("Usage: validate_locales <catalog-directory> [--require-complete]");
            std::process::exit(2);
        }
    };
    if !std::path::Path::new(&directory).is_dir() {
        eprintln!("Catalog directory is missing or inaccessible.");
        std::process::exit(2);
    }
    let mut registry = CatalogRegistry::default();
    let errors = registry.load_directory(std::path::Path::new(&directory));
    for error in &errors {
        eprintln!("{error}");
    }
    if !errors.is_empty() {
        std::process::exit(1);
    }
    if registry.locales().len() == 1 {
        eprintln!("No external translation catalogs were found.");
        std::process::exit(1);
    }
    if require_complete {
        let mut incomplete = false;
        for locale in registry.locales() {
            let selected = registry.select(locale);
            let missing = selected.missing_message_ids();
            if !missing.is_empty() {
                eprintln!(
                    "{locale}: missing {} message IDs: {}",
                    missing.len(),
                    missing.join(", ")
                );
                incomplete = true;
            }
        }
        if incomplete {
            std::process::exit(1);
        }
    }
    println!("Validated locales: {}", registry.locales().join(", "));
}
