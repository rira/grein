use std::fs;
use std::path::PathBuf;

#[test]
fn test_homebrew_formula_grein_metadata_and_dependencies() {
    let formula_path = if PathBuf::from("Formula/grein.rb").exists() {
        PathBuf::from("Formula/grein.rb")
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("Formula/grein.rb"))
            .unwrap_or_else(|| PathBuf::from("Formula/grein.rb"))
    };

    assert!(
        formula_path.exists(),
        "Formula/grein.rb must exist at {}",
        formula_path.display()
    );

    let content = fs::read_to_string(&formula_path).expect("Failed to read Formula/grein.rb");

    assert!(
        content.contains("class Grein < Formula"),
        "Formula must declare 'class Grein < Formula'"
    );
    assert!(
        content.contains("desc "),
        "Formula must define a description (desc)"
    );
    assert!(
        content.contains("homepage "),
        "Formula must define a homepage"
    );
    assert!(
        content.contains("license "),
        "Formula must define a license"
    );
    assert!(
        content.contains("depends_on \"rust\" => :build")
            || content.contains("depends_on 'rust' => :build"),
        "Formula must specify rust build dependency: depends_on 'rust' => :build"
    );
}
