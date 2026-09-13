//! Real signed artifacts, isolated temporary store; no application profile use.
use autokeyboardlayot::{
    Language, PackId,
    configuration::PackageMode,
    installed_packages::InstalledPackages,
    language_package::PackageTrust,
    package_store::{PackageStore, PreparedImport, PreparedStoreInitialization, StoreError},
};
use std::{collections::BTreeSet, io::Write, path::Path};

fn rehearse_selection(
    directory: &Path,
    parent: &Path,
    trust: &PackageTrust,
) -> Result<(), Box<dyn std::error::Error>> {
    use autokeyboardlayot::{
        installer_session::InstallerSession, package_catalog::DEFAULT_PACKAGE_REPOSITORY,
        package_download::DownloadedPackage, package_install::PreparedCatalog,
    };
    use std::{
        cell::Cell,
        sync::atomic::AtomicBool,
        time::{SystemTime, UNIX_EPOCH},
    };
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let raw = std::fs::read(directory.join("catalog.aklc"))?;
    let store = PackageStore::initialize(&parent.join("selected-only"))?;
    let initial = store.load(trust)?;
    let mut session = InstallerSession::default();
    session
        .show_catalog(PreparedCatalog::from_bytes(
            raw.clone(),
            autokeyboardlayot::package_catalog::DEFAULT_PACKAGE_REPOSITORY,
            &initial,
            trust,
            now,
        )?)
        .map_err(|e| format!("session: {e:?}"))?;
    let page = session
        .selection()
        .map_err(|e| format!("selection: {e:?}"))?;
    assert_eq!(page.rows().len(), 13);
    assert_eq!(page.selected_count(), 0);
    assert_eq!(page.total_bytes(), 0);
    let ru = PackId::parse("ru-RU")?;
    page.set_selected(ru, true, trust, now)?;
    assert_eq!(store.load(trust)?.state_sha256(), initial.state_sha256());
    let (ticket, selected) = session
        .begin_download(trust, now)
        .map_err(|e| format!("download: {e:?}"))?
        .ok_or("missing selected plan")?;
    assert_eq!(selected.plan().packages().len(), 1);
    let calls = Cell::new(0);
    let ready = selected.accept_and_download_with(
        &store,
        trust,
        &AtomicBool::new(false),
        || Ok(now),
        |pin| {
            assert_eq!(pin.id(), ru);
            calls.set(calls.get() + 1);
            // This fixture supplies only the selected local signed asset. It does
            // not access the network or model WinHTTP/redirect behavior.
            let bytes =
                std::fs::read(directory.join("ru-RU-r1.aklp")).expect("reviewed local fixture");
            DownloadedPackage::from_bytes(pin, bytes, trust, now)
        },
    )?;
    assert_eq!(calls.get(), 1);
    assert_eq!(store.load(trust)?.inventory().packages().count(), 0);
    assert!(
        store
            .load(trust)?
            .inventory()
            .catalog_checkpoint()
            .is_some()
    );
    session
        .finish_download(ticket, Ok(ready))
        .map_err(|e| format!("review: {e:?}"))?;
    assert_eq!(
        session
            .review()
            .map_err(|e| format!("review: {e:?}"))?
            .1
            .packages()
            .len(),
        1
    );
    let approved = session
        .begin_install(ticket)
        .map_err(|e| format!("approval: {e:?}"))?;
    let installed = approved.confirm(&store, trust, now)?;
    session
        .finish_install(ticket)
        .map_err(|e| format!("finish: {e:?}"))?;
    assert_eq!(installed.inventory().packages().count(), 1);
    let projected =
        InstalledPackages::from_store(&installed, &BTreeSet::from([Language::English]))?;
    assert_eq!(
        projected
            .catalogs
            .ok_or("selected catalogs")?
            .locales()
            .len(),
        2
    );
    assert_eq!(projected.dictionaries.installed_ids().count(), 1);

    let checked = PreparedCatalog::from_bytes(
        raw,
        DEFAULT_PACKAGE_REPOSITORY,
        &store.load(trust)?,
        trust,
        now,
    )?;
    let cached = checked
        .select(&BTreeSet::from([ru]), trust, now)?
        .accept_and_download_with(
            &store,
            trust,
            &AtomicBool::new(false),
            || Ok(now),
            |_| panic!("exact cache hit unexpectedly fetched"),
        )?;
    assert_eq!(cached.packages().len(), 1);
    // Discarding the second review never installs another package.
    drop(cached);
    assert_eq!(store.load(trust)?.inventory().packages().count(), 1);
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: rehearse_ui_packages ABS_ARTIFACT_DIRECTORY NEW_RECEIPT".into());
    }
    let directory = Path::new(&args[0]);
    if !directory.is_absolute() || Path::new(&args[1]).try_exists()? {
        return Err("absolute source and new receipt required".into());
    }
    let ids = [
        "ar-SA", "bn-BD", "de-DE", "es-ES", "et-EE", "fr-FR", "hi-IN", "id-ID", "ja-JP", "pt-BR",
        "ru-RU", "ur-PK", "zh-CN",
    ];
    let paths: Vec<_> = ids
        .iter()
        .map(|id| directory.join(format!("{id}-r1.aklp")))
        .collect();
    let trust = PackageTrust::release()?;
    let preview = PreparedStoreInitialization::from_files(&paths, &BTreeSet::new(), &trust)?;
    if preview.packages().len() != 13
        || preview.packages().iter().any(|p| {
            p.input().is_some()
                || p.ui_locale().is_none()
                || p.license() != include_str!("../LICENSE")
        })
    {
        return Err("expected thirteen UI-only MIT candidates".into());
    }
    let total_bytes = preview.total_bytes();
    let scratch = tempfile::tempdir()?;
    let root = scratch.path().join("isolated packages ü");
    assert!(!root.exists()); // preview made no store
    let original = preview.confirm(&root, &trust)?;
    let store = PackageStore::open(&root)?;
    let selected = BTreeSet::from([Language::English]);
    let installed = InstalledPackages::load(&root, PackageMode::Managed, &trust, &selected)?;
    assert_eq!(
        installed
            .dictionaries
            .installed_ids()
            .copied()
            .collect::<Vec<_>>(),
        vec![Language::English]
    );
    assert_eq!(
        installed
            .dictionaries
            .enabled_ids()
            .copied()
            .collect::<BTreeSet<_>>(),
        selected
    );
    let catalogs = installed
        .catalogs
        .as_ref()
        .ok_or("missing managed catalogs")?;
    assert_eq!(catalogs.locales().len(), 14);
    for locale in catalogs.locales() {
        assert!(catalogs.select(locale).missing_message_ids().is_empty());
    }
    assert_eq!(store.load(&trust)?.state_sha256(), original.state_sha256());

    let ru = PackId::parse("ru-RU")?;
    let ru_path = directory.join("ru-RU-r1.aklp");
    let stale = PreparedImport::from_file(&ru_path, &original, &trust)?;
    let removed = original.inventory().stage_remove(&BTreeSet::from([ru]))?;
    let after_remove = store.commit(&original, &removed, &[], &trust)?;
    assert_eq!(after_remove.inventory().packages().count(), 12);
    assert!(matches!(
        stale.confirm(&store, &trust),
        Err(StoreError::StaleSnapshot)
    ));
    let restored =
        PreparedImport::from_file(&ru_path, &after_remove, &trust)?.confirm(&store, &trust)?;
    assert_eq!(restored.inventory().packages().count(), 13);

    let mut corrupt = std::fs::read(&ru_path)?;
    corrupt[0] ^= 1;
    let corrupt_path = scratch.path().join("corrupt.aklp");
    std::fs::write(&corrupt_path, corrupt)?;
    assert!(PreparedImport::from_file(&corrupt_path, &restored, &trust).is_err());
    assert_eq!(store.load(&trust)?.state_sha256(), restored.state_sha256());
    let reloaded = InstalledPackages::load(&root, PackageMode::Managed, &trust, &selected)?;
    assert_eq!(
        reloaded
            .catalogs
            .ok_or("missing restored catalogs")?
            .locales()
            .len(),
        14
    );
    rehearse_selection(directory, scratch.path(), &trust)?;
    // Remove only this example's fresh temporary directory, through its owner.
    scratch.close()?;
    let receipt = serde_json::json!({"state":"passed","packages":13,"locales_including_base":14,
        "artifact_bytes":total_bytes,"temporary_store_removed":true,"live_profile_modified":false,
        "checks":["release_signatures","MIT_text","UI_only","strict_catalog_coverage","English_only_input",
        "store_reload","remove_reimport","stale_confirmation_refused","corrupt_artifact_refused",
        "signed_catalog_selection","one_selected_fetch","no_install_before_review","catalog_receipt",
        "single_package_commit","exact_cache_reuse"],
        "native_typing_acceptance":false,"published":false});
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[1])?;
    file.write_all(serde_json::to_string_pretty(&receipt)?.as_bytes())?;
    file.sync_all()?;
    println!("ISOLATED_REAL_UI_PACKAGE_REHEARSAL_PASSED");
    Ok(())
}
