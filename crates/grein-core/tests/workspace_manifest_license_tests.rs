use std::fs;
use std::path::{Path, PathBuf};

fn find_workspace_root() -> PathBuf {
    if Path::new("crates/grein-core/Cargo.toml").exists() {
        PathBuf::from(".")
    } else if Path::new("../../crates/grein-core/Cargo.toml").exists() {
        PathBuf::from("../..")
    } else if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let manifest_path = PathBuf::from(manifest_dir);
        if manifest_path
            .join("../../crates/grein-core/Cargo.toml")
            .exists()
        {
            manifest_path.join("../..")
        } else {
            manifest_path
        }
    } else {
        PathBuf::from("../..")
    }
}

#[test]
fn test_workspace_and_crate_manifests_contain_mit_license_and_author() {
    let workspace_root = find_workspace_root();
    let root_cargo_toml = fs::read_to_string(workspace_root.join("Cargo.toml"))
        .expect("Failed to read root Cargo.toml");

    assert!(
        root_cargo_toml.contains("license = \"MIT\"")
            || root_cargo_toml.contains("license.workspace = true"),
        "Root Cargo.toml must specify license = \"MIT\" or workspace package license configuration"
    );
    assert!(
        root_cargo_toml.contains("Richard <rira>")
            || root_cargo_toml.contains("authors.workspace = true"),
        "Root Cargo.toml must specify authors metadata with 'Richard <rira>'"
    );

    let member_crates = [
        "crates/grein-core/Cargo.toml",
        "crates/grein-acp/Cargo.toml",
        "crates/grein-workspace/Cargo.toml",
        "crates/grein-tui/Cargo.toml",
        "crates/grein-cli/Cargo.toml",
    ];

    for crate_manifest in member_crates {
        let path = workspace_root.join(crate_manifest);
        assert!(
            path.exists(),
            "Manifest path must exist: {}",
            crate_manifest
        );

        let content = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("Failed to read {}", crate_manifest));

        assert!(
            content.contains("license = \"MIT\"") || content.contains("license.workspace = true"),
            "{} must include license = \"MIT\" or license.workspace = true",
            crate_manifest
        );
        assert!(
            content.contains("Richard <rira>") || content.contains("authors.workspace = true"),
            "{} must include author attribution metadata or authors.workspace = true",
            crate_manifest
        );
    }
}
