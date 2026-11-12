use std::fs;
use std::path::PathBuf;

#[test]
fn test_formula_contains_install_and_test_blocks() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest_dir.join("Formula/grein.rb"),
        manifest_dir.join("../../Formula/grein.rb"),
        PathBuf::from("Formula/grein.rb"),
    ];

    let formula_path = candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("Formula/grein.rb"));

    assert!(
        formula_path.exists(),
        "Expected Formula/grein.rb to exist at {:?}",
        formula_path
    );

    let content = fs::read_to_string(&formula_path).expect("Failed to read Formula/grein.rb");

    assert!(
        content.contains("def install"),
        "Formula must define an install method"
    );

    assert!(
        content.contains("*std_cargo_args(path: \"crates/grein-cli\")"),
        "Formula install block must invoke cargo install with *std_cargo_args(path: \"crates/grein-cli\")"
    );

    assert!(
        content.contains("test do"),
        "Formula must define a test do block"
    );

    assert!(
        content.contains("assert_match version.to_s, shell_output(\"#{bin}/grein --version\")"),
        "Formula test block must verify #{{bin}}/grein --version matches formula version"
    );
}
