//! No inference, downloads, or real project access: package presence must resolve
//! every built-in image model through the same pinned catalog used for loading.
#[test]
fn registered_model_packages_have_pinned_assets() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    let runtime =
        koharu_runtime::Runtime::new(temp.path(), koharu_runtime::ComputePolicy::CpuOnly)?;
    // Reference the app registry so its engine/model dependencies are linked.
    let _ = koharu_app::Registry::default();
    let mut count = 0;
    for package in koharu_runtime::Catalog::discover().all() {
        if package.kind == koharu_runtime::PackageKind::Model
            // Versioned vendor archive, not a Hugging Face repository.
            && package.id != "model:korean-pp-ocr-v5"
        {
            assert!(
                !(package.present)(&runtime)?,
                "unexpected cached package {}",
                package.id
            );
            count += 1;
        }
    }
    assert!(count >= 20, "expected model registrations to be linked");
    Ok(())
}

#[tokio::test]
#[ignore = "read-only check against the user's existing model cache; never prepares missing packages"]
async fn existing_image_packages_resolve_offline() -> anyhow::Result<()> {
    let root = std::env::var("KOHARU_PIN_CACHE_ROOT")?;
    let runtime = koharu_runtime::Runtime::new(root, koharu_runtime::ComputePolicy::CpuOnly)?;
    let _ = koharu_app::Registry::default();
    let mut count = 0;
    for package in koharu_runtime::Catalog::discover().all() {
        if package.kind == koharu_runtime::PackageKind::Model
            && package.id != "model:korean-pp-ocr-v5"
            && (package.present)(&runtime)?
        {
            // Both callbacks use the same lookup. A present artifact returns
            // immediately without entering metadata/download or writing refs.
            (package.ensure)(&runtime).await?;
            count += 1;
        }
    }
    assert!(
        count >= 20,
        "expected the twenty verified cached image assets, found {count}"
    );
    println!(
        "Resolved {count} existing image model packages without preparing any missing package."
    );
    Ok(())
}
