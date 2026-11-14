use std::fs;
use std::path::PathBuf;

fn find_flake_path() -> PathBuf {
    let mut current = std::env::current_dir().expect("Failed to get current directory");
    loop {
        let candidate = current.join("flake.nix");
        if candidate.exists() {
            return candidate;
        }
        if !current.pop() {
            break;
        }
    }
    PathBuf::from("flake.nix")
}

#[test]
fn test_dev_shells_default_exists_and_contains_required_packages() {
    let flake_path = find_flake_path();
    assert!(
        flake_path.exists(),
        "flake.nix must exist at workspace root (looked at: {})",
        flake_path.display()
    );

    let content =
        fs::read_to_string(&flake_path).expect("flake.nix must be readable as a UTF-8 string");

    // Verify devShells output definition
    assert!(
        content.contains("devShells"),
        "flake.nix must define a `devShells` output attribute"
    );

    assert!(
        content.contains("default =") || content.contains("default="),
        "flake.nix devShells must define a `default` shell attribute"
    );

    assert!(
        content.contains("mkShell"),
        "devShells.default must be constructed using `mkShell`"
    );

    // Required packages for Step 3:
    // rustc, cargo, rustfmt, clippy, sqlite, and pkg-config
    let required_packages = ["rustc", "cargo", "rustfmt", "clippy", "sqlite"];

    for pkg in &required_packages {
        assert!(
            content.contains(pkg),
            "devShells.default must provide `{}` in its dependencies, but it was not found in flake.nix",
            pkg
        );
    }

    // pkg-config in nixpkgs is typically pkg-config or pkgconfig
    let has_pkg_config = content.contains("pkg-config") || content.contains("pkgconfig");
    assert!(
        has_pkg_config,
        "devShells.default must provide `pkg-config` (or `pkgconfig`) for C/system library resolution"
    );
}

#[test]
fn test_dev_shells_target_systems_supported() {
    let flake_path = find_flake_path();
    assert!(flake_path.exists(), "flake.nix must exist");

    let content = fs::read_to_string(&flake_path).expect("flake.nix must be readable");

    // Ensure development shell is not hardcoded to a single platform
    let systems = ["x86_64-linux", "aarch64-linux"];
    for system in &systems {
        assert!(
            content.contains(system),
            "flake.nix must declare support for `{}` across devShells/packages",
            system
        );
    }
}
