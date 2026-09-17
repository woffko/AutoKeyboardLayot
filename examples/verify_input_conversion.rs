//! One-off verification: a signed input package enables automatic conversion.
//! Not installed, published or used by the agent.
use autokeyboardlayot::{
    Detector, DetectorConfig, Language, PackId,
    installed_packages::InstalledPackages,
    language_package::PackageTrust,
    package_store::{PackageStore, PreparedImport},
    profile_resolver::{KeyboardProfileProbe, resolve_keyboard_profiles},
};
use std::collections::BTreeSet;

struct Fixture(usize);
impl KeyboardProfileProbe for Fixture {
    fn loaded_layouts(&mut self) -> Option<Vec<usize>> {
        Some(vec![0x0409, 0x0419])
    }
    fn current_layout(&mut self) -> Option<usize> {
        Some(self.0)
    }
    fn is_ime(&mut self, _: usize) -> bool {
        false
    }
    fn activate_layout(&mut self, layout: usize) -> Option<usize> {
        Some(std::mem::replace(&mut self.0, layout))
    }
    fn current_layout_name(&mut self) -> Option<[u16; 9]> {
        let name = match self.0 {
            0x0409 => "00000409",
            0x0419 => "00000419",
            _ => return None,
        };
        let mut result = [0; 9];
        for (slot, value) in result.iter_mut().zip(name.encode_utf16()) {
            *slot = value;
        }
        Some(result)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: verify_input_conversion PACKAGE.aklp")?;
    let trust = PackageTrust::release()?;
    let directory = tempfile::tempdir()?;
    let store = PackageStore::initialize(&directory.path().join("packages"))?;
    let empty = store.load(&trust)?;
    let prepared = PreparedImport::from_file(std::path::Path::new(&path), &empty, &trust)?;
    let snapshot = prepared.confirm(&store, &trust)?;
    let selected: BTreeSet<PackId> = ["en-US", "ru-RU"]
        .into_iter()
        .map(|id| PackId::parse(id).unwrap())
        .collect();
    let installed = InstalledPackages::from_store(&snapshot, &selected)?;
    let mut detector = Detector::with_registry(DetectorConfig::default(), installed.dictionaries);
    let profiles = resolve_keyboard_profiles(&mut Fixture(0x0409))?;
    detector.set_resolved_profiles(Some(&profiles));
    let detection = detector
        .detect("ghbdtn", Language::English)
        .ok_or("no conversion candidate")?;
    println!(
        "CONVERSION_OK original={} replacement={} target={}",
        detection.original,
        detection.replacement,
        detection.target_language.id()
    );
    Ok(())
}
