use grein_workspace::diff::compute_directory_diff;
use grein_workspace::shadow::{detect_git_environment, ShadowSnapshot};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_fallback_detection_in_non_git_directory() {
    let temp = tempdir().expect("failed to create temp dir");
    let non_git_path = temp.path();

    let is_git = detect_git_environment(non_git_path);
    assert!(
        !is_git,
        "non-git temporary directory must trigger fallback mode"
    );
}

#[test]
fn test_shadow_snapshot_lifecycle_and_creation() {
    let temp = tempdir().expect("failed to create temp dir");
    let primary_dir = temp.path().join("primary");
    fs::create_dir_all(&primary_dir).expect("failed to create primary dir");

    let file_a = primary_dir.join("file_a.txt");
    fs::write(&file_a, "initial content\n").expect("failed to write file_a");

    let thread_id = "thread-12345";
    let shadow =
        ShadowSnapshot::create(&primary_dir, thread_id).expect("failed to create shadow snapshot");

    let expected_shadow_path = primary_dir.join(".grein/shadow").join(thread_id);
    assert_eq!(shadow.shadow_path(), expected_shadow_path.as_path());
    assert!(shadow.shadow_path().exists(), "shadow path must exist");

    let shadow_file_a = shadow.shadow_path().join("file_a.txt");
    assert!(shadow_file_a.exists(), "shadow file_a must exist");
    assert_eq!(
        fs::read_to_string(&shadow_file_a).expect("failed to read shadow file_a"),
        "initial content\n"
    );

    shadow.cleanup().expect("failed to cleanup shadow snapshot");
    assert!(
        !shadow.shadow_path().exists(),
        "shadow path should be removed after cleanup"
    );
}

#[test]
fn test_diff_generation_against_primary() {
    let temp = tempdir().expect("failed to create temp dir");
    let primary_dir = temp.path().join("primary");
    fs::create_dir_all(&primary_dir).expect("failed to create primary dir");

    let file_a = primary_dir.join("hello.rs");
    fs::write(&file_a, "fn main() {\n    println!(\"hello\");\n}\n").expect("failed to write file");

    let thread_id = "thread-diff-test";
    let shadow =
        ShadowSnapshot::create(&primary_dir, thread_id).expect("failed to create shadow snapshot");

    let shadow_file_a = shadow.shadow_path().join("hello.rs");
    fs::write(
        &shadow_file_a,
        "fn main() {\n    println!(\"hello, world!\");\n}\n",
    )
    .expect("failed to modify shadow file");

    let diff =
        compute_directory_diff(&primary_dir, shadow.shadow_path()).expect("failed to compute diff");

    assert!(
        !diff.is_empty(),
        "diff must not be empty for modified files"
    );
    assert!(
        diff.contains("-    println!(\"hello\");"),
        "diff must show removed line"
    );
    assert!(
        diff.contains("+    println!(\"hello, world!\");"),
        "diff must show added line"
    );

    shadow.cleanup().expect("failed to cleanup shadow snapshot");
}

#[test]
fn test_merge_back_to_primary() {
    let temp = tempdir().expect("failed to create temp dir");
    let primary_dir = temp.path().join("primary");
    fs::create_dir_all(&primary_dir).expect("failed to create primary dir");

    let file_to_update = primary_dir.join("config.toml");
    fs::write(&file_to_update, "version = 1\n").expect("failed to write config");

    let thread_id = "thread-merge-test";
    let shadow =
        ShadowSnapshot::create(&primary_dir, thread_id).expect("failed to create shadow snapshot");

    let shadow_file_update = shadow.shadow_path().join("config.toml");
    fs::write(&shadow_file_update, "version = 2\nenabled = true\n")
        .expect("failed to update shadow config");

    let shadow_new_file = shadow.shadow_path().join("new_file.txt");
    fs::write(&shadow_new_file, "created in shadow\n").expect("failed to write new file");

    shadow
        .merge_back()
        .expect("failed to merge back to primary");

    assert_eq!(
        fs::read_to_string(&file_to_update).expect("failed to read updated primary config"),
        "version = 2\nenabled = true\n"
    );

    let merged_new_file = primary_dir.join("new_file.txt");
    assert!(
        merged_new_file.exists(),
        "new file must be merged into primary"
    );
    assert_eq!(
        fs::read_to_string(&merged_new_file).expect("failed to read new file"),
        "created in shadow\n"
    );

    shadow.cleanup().expect("failed to cleanup shadow snapshot");
}

#[test]
fn test_merge_back_removes_deleted_files_and_preserves_grein() {
    let temp = tempdir().expect("failed to create temp dir");
    let primary_dir = temp.path().join("primary");
    fs::create_dir_all(primary_dir.join("nested/deep")).expect("failed to create nested dirs");

    let keep_file = primary_dir.join("keep.txt");
    let doomed_file = primary_dir.join("doomed.txt");
    let nested_file = primary_dir.join("nested/deep/gone.txt");
    fs::write(&keep_file, "keep\n").expect("failed to write keep");
    fs::write(&doomed_file, "doomed\n").expect("failed to write doomed");
    fs::write(&nested_file, "nested\n").expect("failed to write nested");

    let shadow = ShadowSnapshot::create(&primary_dir, "thread-delete-test")
        .expect("failed to create shadow snapshot");

    fs::remove_file(shadow.shadow_path().join("doomed.txt")).expect("failed to delete shadow file");
    fs::remove_file(shadow.shadow_path().join("nested/deep/gone.txt"))
        .expect("failed to delete nested shadow file");

    shadow
        .merge_back()
        .expect("failed to merge back to primary");

    assert!(!doomed_file.exists(), "deleted file must be removed");
    assert!(!nested_file.exists(), "nested deleted file must be removed");
    assert!(
        !primary_dir.join("nested").exists(),
        "empty directories must be pruned"
    );
    assert!(keep_file.exists(), "untouched file must remain");
    assert!(
        primary_dir.join(".grein").exists(),
        ".grein must be preserved"
    );

    shadow.cleanup().expect("failed to cleanup shadow snapshot");
}

#[test]
fn test_shadow_edit_does_not_mutate_primary_before_merge_back() {
    let temp = tempdir().expect("failed to create temp dir");
    let primary_dir = temp.path().join("primary");
    fs::create_dir_all(&primary_dir).expect("failed to create primary dir");

    let primary_file = primary_dir.join("data.txt");
    fs::write(&primary_file, "original\n").expect("failed to write primary file");

    let shadow = ShadowSnapshot::create(&primary_dir, "thread-isolation-test")
        .expect("failed to create shadow snapshot");

    fs::write(shadow.shadow_path().join("data.txt"), "edited\n")
        .expect("failed to edit shadow file");

    assert_eq!(
        fs::read_to_string(&primary_file).expect("failed to read primary file"),
        "original\n",
        "primary must not change before merge_back"
    );

    shadow.cleanup().expect("failed to cleanup shadow snapshot");
}

#[test]
fn test_compute_directory_diff_handles_binary_files() {
    let temp = tempdir().expect("failed to create temp dir");
    let primary_dir = temp.path().join("primary");
    fs::create_dir_all(&primary_dir).expect("failed to create primary dir");

    fs::write(primary_dir.join("blob.bin"), [0xff_u8, 0xfe, 0x00, 0x01])
        .expect("failed to write primary binary");
    fs::write(primary_dir.join("same.bin"), [0xc3_u8, 0x28, 0x00])
        .expect("failed to write primary same binary");

    let shadow = ShadowSnapshot::create(&primary_dir, "thread-binary-test")
        .expect("failed to create shadow snapshot");

    fs::write(
        shadow.shadow_path().join("blob.bin"),
        [0xff_u8, 0xfe, 0x02, 0x03],
    )
    .expect("failed to modify shadow binary");

    let diff = compute_directory_diff(&primary_dir, shadow.shadow_path())
        .expect("binary files must not cause diff errors");

    assert!(
        diff.contains("Binary files a/blob.bin and b/blob.bin differ\n"),
        "diff must report differing binary file"
    );
    assert!(
        !diff.contains("same.bin"),
        "identical binary files must be skipped"
    );

    shadow.cleanup().expect("failed to cleanup shadow snapshot");
}
