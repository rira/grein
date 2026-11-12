use grein_workspace::diff::compute_unified_diff;
use std::fs::{self, File};
use std::io::Write;
use tempfile::tempdir;

#[test]
fn test_compute_unified_diff_identical_directories() -> Result<(), Box<dyn std::error::Error>> {
    let old_dir = tempdir()?;
    let new_dir = tempdir()?;

    fs::write(old_dir.path().join("hello.txt"), "Hello, world!\n")?;
    fs::write(new_dir.path().join("hello.txt"), "Hello, world!\n")?;

    let diff = compute_unified_diff(old_dir.path(), new_dir.path())?;
    assert!(
        diff.is_empty(),
        "Diff of identical directories should be empty"
    );

    Ok(())
}

#[test]
fn test_compute_unified_diff_modified_file() -> Result<(), Box<dyn std::error::Error>> {
    let old_dir = tempdir()?;
    let new_dir = tempdir()?;

    fs::write(
        old_dir.path().join("main.rs"),
        "fn main() {\n    println!(\"v1\");\n}\n",
    )?;
    fs::write(
        new_dir.path().join("main.rs"),
        "fn main() {\n    println!(\"v2\");\n}\n",
    )?;

    let diff = compute_unified_diff(old_dir.path(), new_dir.path())?;
    assert!(
        !diff.is_empty(),
        "Diff must not be empty for modified files"
    );
    assert!(
        diff.contains("--- a/main.rs"),
        "Diff must include old header"
    );
    assert!(
        diff.contains("+++ b/main.rs"),
        "Diff must include new header"
    );
    assert!(
        diff.contains("-    println!(\"v1\");"),
        "Diff must show removed line"
    );
    assert!(
        diff.contains("+    println!(\"v2\");"),
        "Diff must show added line"
    );

    Ok(())
}

#[test]
fn test_compute_unified_diff_added_file() -> Result<(), Box<dyn std::error::Error>> {
    let old_dir = tempdir()?;
    let new_dir = tempdir()?;

    fs::write(
        new_dir.path().join("added.txt"),
        "first line\nsecond line\n",
    )?;

    let diff = compute_unified_diff(old_dir.path(), new_dir.path())?;
    assert!(!diff.is_empty(), "Diff must reflect added file");
    assert!(diff.contains("+++ b/added.txt"));
    assert!(diff.contains("+first line"));
    assert!(diff.contains("+second line"));

    Ok(())
}

#[test]
fn test_compute_unified_diff_deleted_file() -> Result<(), Box<dyn std::error::Error>> {
    let old_dir = tempdir()?;
    let new_dir = tempdir()?;

    fs::write(old_dir.path().join("deleted.txt"), "removed content\n")?;

    let diff = compute_unified_diff(old_dir.path(), new_dir.path())?;
    assert!(!diff.is_empty(), "Diff must reflect removed file");
    assert!(diff.contains("--- a/deleted.txt"));
    assert!(diff.contains("-removed content"));

    Ok(())
}

#[test]
fn test_compute_unified_diff_nested_subdirectories() -> Result<(), Box<dyn std::error::Error>> {
    let old_dir = tempdir()?;
    let new_dir = tempdir()?;

    let old_sub = old_dir.path().join("src").join("nested");
    let new_sub = new_dir.path().join("src").join("nested");
    fs::create_dir_all(&old_sub)?;
    fs::create_dir_all(&new_sub)?;

    fs::write(old_sub.join("lib.rs"), "pub fn run() -> bool { false }\n")?;
    fs::write(new_sub.join("lib.rs"), "pub fn run() -> bool { true }\n")?;

    let diff = compute_unified_diff(old_dir.path(), new_dir.path())?;
    assert!(diff.contains("--- a/src/nested/lib.rs"));
    assert!(diff.contains("+++ b/src/nested/lib.rs"));
    assert!(diff.contains("-pub fn run() -> bool { false }"));
    assert!(diff.contains("+pub fn run() -> bool { true }"));

    Ok(())
}
