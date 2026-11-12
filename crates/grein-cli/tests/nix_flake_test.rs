use std::path::{Path, PathBuf};

fn find_flake_nix() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(&manifest_dir);
    let flake_path = workspace_root.join("flake.nix");
    if flake_path.exists() {
        return flake_path;
    }
    PathBuf::from("flake.nix")
}

#[test]
fn test_flake_defines_apps_default_for_nix_run() {
    let flake_path = find_flake_nix();
    assert!(
        flake_path.exists(),
        "flake.nix must exist at repository root: {}",
        flake_path.display()
    );

    let content = std::fs::read_to_string(&flake_path).expect("flake.nix should be readable");

    // Verify apps block is declared
    assert!(
        content.contains("apps =") || content.contains("apps."),
        "flake.nix must define apps output"
    );

    // Verify apps.default or apps.${system}.default is configured
    assert!(
        content.contains("default =") || content.contains("apps.default"),
        "flake.nix must define default app in apps"
    );

    // Verify app metadata satisfies nix run app contract
    assert!(
        content.contains("type = \"app\"") || content.contains("type = \"app\""),
        "apps.default must specify type = \"app\""
    );

    // Verify program points to compiled grein binary
    assert!(
        content.contains("/bin/grein") || content.contains("getExe") || content.contains("grein"),
        "apps.default must specify program pointing to the grein executable"
    );
}
