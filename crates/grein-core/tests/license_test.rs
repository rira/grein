use std::fs;
use std::path::Path;

#[test]
fn test_root_license_exists_and_contains_mit_terms_with_author_attribution() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let license_path = Path::new(manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root directory should be accessible from CARGO_MANIFEST_DIR")
        .join("LICENSE");

    assert!(
        license_path.exists(),
        "Expected root LICENSE file to exist at {}",
        license_path.display()
    );

    let content = fs::read_to_string(&license_path).expect("Failed to read root LICENSE file");

    assert!(
        content.contains("MIT License")
            || content.contains("Permission is hereby granted, free of charge"),
        "LICENSE must contain standard MIT License terms"
    );

    assert!(
        content.contains("Richard <rira>"),
        "LICENSE must contain copyright attribution to 'Richard <rira>'"
    );
}
