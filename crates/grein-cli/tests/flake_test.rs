use std::fs;
use std::path::{Path, PathBuf};

fn find_repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root_candidate = manifest_dir.join("../..");
    if root_candidate.join("flake.nix").exists() || root_candidate.join("Cargo.toml").exists() {
        root_candidate
    } else if Path::new("flake.nix").exists() {
        PathBuf::from(".")
    } else {
        root_candidate
    }
}

#[test]
fn test_flake_nix_exists_and_declares_build_rust_package() {
    let root = find_repo_root();
    let flake_path = root.join("flake.nix");

    assert!(
        flake_path.exists(),
        "Expected flake.nix to exist at repo root: {}",
        flake_path.display()
    );

    let content = fs::read_to_string(&flake_path).expect("Failed to read flake.nix");

    assert!(
        content.contains("buildRustPackage"),
        "flake.nix must define package using rustPlatform.buildRustPackage"
    );

    assert!(
        content.contains("cargoLock") || content.contains("lockFile"),
        "flake.nix must configure Cargo.lock vendoring (cargoLock.lockFile)"
    );

    assert!(
        content.contains("pkg-config"),
        "flake.nix must include pkg-config in nativeBuildInputs"
    );

    assert!(
        content.contains("sqlite"),
        "flake.nix must include sqlite in buildInputs"
    );

    assert!(
        content.contains("packages.default")
            || (content.contains("packages") && content.contains("default =")),
        "flake.nix must expose packages.default derivation"
    );
}
