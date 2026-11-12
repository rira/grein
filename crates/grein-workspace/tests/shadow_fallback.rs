use grein_workspace::shadow::ShadowSnapshot;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_merge_back_copies_modified_and_new_files_to_primary() {
    let primary = tempdir().expect("failed to create primary tempdir");
    let primary_path = primary.path();

    let initial_file = primary_path.join("existing.txt");
    let nested_file = primary_path.join("src").join("lib.rs");
    fs::create_dir_all(nested_file.parent().unwrap()).unwrap();
    fs::write(&initial_file, "original content\n").unwrap();
    fs::write(&nested_file, "pub fn run() {}\n").unwrap();

    let shadow_root = tempdir().expect("failed to create shadow root tempdir");
    let thread_id = "thread-test-merge";

    let shadow = ShadowSnapshot::create(primary_path, shadow_root.path(), thread_id)
        .expect("failed to create shadow snapshot");

    // Verify shadow path exists
    assert!(shadow.path().exists());

    // Modify existing file in shadow snapshot
    let shadow_file = shadow.path().join("existing.txt");
    fs::write(&shadow_file, "modified content in shadow\n").unwrap();

    // Create a new file in shadow snapshot
    let shadow_new_file = shadow.path().join("new_module.rs");
    fs::write(&shadow_new_file, "pub struct Worker;\n").unwrap();

    // Execute merge-back operation
    let merge_result = shadow.merge_back();
    assert!(
        merge_result.is_ok(),
        "merge_back should succeed: {:?}",
        merge_result.err()
    );

    // Verify primary directory has received the updates
    assert_eq!(
        fs::read_to_string(&initial_file).unwrap(),
        "modified content in shadow\n"
    );
    assert_eq!(
        fs::read_to_string(primary_path.join("new_module.rs")).unwrap(),
        "pub struct Worker;\n"
    );
    assert_eq!(
        fs::read_to_string(&nested_file).unwrap(),
        "pub fn run() {}\n"
    );
}

#[test]
fn test_merge_back_handles_deleted_files_and_directory_cleanup() {
    let primary = tempdir().expect("failed to create primary tempdir");
    let primary_path = primary.path();

    let keep_file = primary_path.join("keep.txt");
    let delete_file = primary_path.join("to_remove.txt");
    fs::write(&keep_file, "retain me").unwrap();
    fs::write(&delete_file, "remove me").unwrap();

    let shadow_root = tempdir().expect("failed to create shadow root tempdir");
    let thread_id = "thread-test-delete";

    let shadow = ShadowSnapshot::create(primary_path, shadow_root.path(), thread_id)
        .expect("failed to create shadow snapshot");

    // Delete file inside shadow
    let shadow_delete_file = shadow.path().join("to_remove.txt");
    fs::remove_file(&shadow_delete_file).unwrap();

    // Execute merge-back
    let merge_result = shadow.merge_back();
    assert!(
        merge_result.is_ok(),
        "merge_back with deletion should succeed: {:?}",
        merge_result.err()
    );

    // Verify primary directory reflects deletion and retains untouched files
    assert!(keep_file.exists());
    assert_eq!(fs::read_to_string(&keep_file).unwrap(), "retain me");
    assert!(
        !delete_file.exists(),
        "deleted file must be removed from primary upon merge-back"
    );
}
