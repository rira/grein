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
fn test_flake_enables_do_check() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("Failed to locate workspace root");
    let flake_path = root_dir.join("flake.nix");
    let content = std::fs::read_to_string(&flake_path).expect("Failed to read flake.nix");

    assert!(
        content.contains("doCheck = true;") || content.contains("doCheck = true"),
        "flake.nix must set doCheck = true inside buildRustPackage derivation"
    );
}

#[test]
fn test_formula_grein_specifies_mit_license() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("Failed to locate workspace root");
    let formula_path = root_dir.join("Formula").join("grein.rb");

    assert!(
        formula_path.exists(),
        "Formula/grein.rb must exist at repository workspace: {:?}",
        formula_path
    );

    let content = std::fs::read_to_string(&formula_path).expect("Failed to read Formula/grein.rb");

    assert!(
        content.contains("license \"MIT\""),
        "Formula/grein.rb must specify license \"MIT\" matching Cargo.toml and LICENSE"
    );
    assert!(
        !content.contains("license \"Apache-2.0\""),
        "Formula/grein.rb must not retain stale Apache-2.0 license declaration"
    );
}

#[test]
fn test_specification_has_clean_grein_naming() {
    let root_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("Failed to locate workspace root");
    let spec_path = root_dir.join("SPECIFICATION.md");

    assert!(
        spec_path.exists(),
        "SPECIFICATION.md must exist at repository workspace root: {:?}",
        spec_path
    );

    let content = std::fs::read_to_string(&spec_path).expect("Failed to read SPECIFICATION.md");

    let forbidden_patterns = [
        "agent-tui",
        ".agent-tui",
        ".tui-manager",
        ".tui-policy.toml",
    ];

    for pattern in &forbidden_patterns {
        assert!(
            !content.contains(pattern),
            "SPECIFICATION.md must not contain stale identifier: '{}'",
            pattern
        );
    }
}

#[test]
#[ignore = "recursive nix invocation must not run inside nix build checkPhase"]
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
            ".#default",
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
            eprintln!("nix binary not present in environment; skipping live execution check");
        }
        Err(err) => panic!("Unexpected error executing nix build: {}", err),
    }
}
