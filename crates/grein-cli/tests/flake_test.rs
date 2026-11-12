use std::path::Path;
use std::process::Command;

#[test]
fn test_flake_file_exists_and_contains_required_definitions() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("Failed to locate workspace root");
    let flake_path = root_dir.join("flake.nix");

    assert!(
        flake_path.exists(),
        "flake.nix must exist at repository workspace root: {:?}",
        flake_path
    );

    let content = std::fs::read_to_string(&flake_path).expect("Failed to read flake.nix");

    assert!(
        content.contains("packages") && content.contains("default"),
        "flake.nix must define packages.default for hermetic package build"
    );
    assert!(
        content.contains("apps") && content.contains("default"),
        "flake.nix must define apps.default pointing to the built binary"
    );
    assert!(
        content.contains("devShells") && content.contains("default"),
        "flake.nix must define devShells.default"
    );
    assert!(
        content.contains("buildRustPackage")
            || content.contains("craneLib")
            || content.contains("crane"),
        "flake.nix must configure Rust package builder derivation"
    );
}

#[test]
fn test_nix_flake_check_and_build_hermeticity() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("Failed to locate workspace root");

    let nix_check = Command::new("nix")
        .current_dir(root_dir)
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "flake",
            "check",
        ])
        .output();

    match nix_check {
        Ok(output) => {
            assert!(
                output.status.success(),
                "nix flake check failed:\nSTDOUT: {}\nSTDERR: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("nix binary not present in environment; skipping live execution check");
        }
        Err(err) => panic!("Unexpected error executing nix flake check: {}", err),
    }

    let nix_build = Command::new("nix")
        .current_dir(root_dir)
        .args([
            "--extra-experimental-features",
            "nix-command flakes",
            "build",
            "--no-link",
        ])
        .output();

    match nix_build {
        Ok(output) => {
            assert!(
                output.status.success(),
                "nix build failed:\nSTDOUT: {}\nSTDERR: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("nix binary not present in environment; skipping live build check");
        }
        Err(err) => panic!("Unexpected error executing nix build: {}", err),
    }
}
