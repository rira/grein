use std::fs;
use std::path::PathBuf;

fn find_workspace_root() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let mut current = PathBuf::from(manifest_dir);
    loop {
        if current.join("flake.nix").exists() {
            return current;
        }
        if current.join("Cargo.lock").exists() || current.join(".git").exists() {
            return current;
        }
        if !current.pop() {
            break;
        }
    }
    PathBuf::from(".")
}

#[test]
fn test_flake_nix_exists_and_declares_inputs_and_systems() {
    let root = find_workspace_root();
    let flake_path = root.join("flake.nix");

    assert!(
        flake_path.exists(),
        "flake.nix must exist at repository root: {}",
        flake_path.display()
    );

    let content = fs::read_to_string(&flake_path)
        .unwrap_or_else(|err| panic!("Failed to read {}: {}", flake_path.display(), err));

    // Verify hermetic inputs
    assert!(
        content.contains("nixpkgs.url"),
        "flake.nix must declare nixpkgs input with URL"
    );
    assert!(
        content.contains("nixpkgs"),
        "flake.nix must declare nixpkgs input"
    );

    // Verify supported architectures / systems
    assert!(
        content.contains("x86_64-linux"),
        "flake.nix must support x86_64-linux target"
    );
    assert!(
        content.contains("aarch64-linux"),
        "flake.nix must support aarch64-linux target"
    );
    assert!(
        content.contains("aarch64-darwin"),
        "flake.nix must support aarch64-darwin target"
    );

    // Verify flake outputs structure
    assert!(
        content.contains("outputs = { self, nixpkgs, flake-utils") || content.contains("outputs ="),
        "flake.nix must declare an outputs function receiving flake inputs"
    );
}
