use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

use grein_workspace::shadow::{ShadowError, ShadowWorkspace};

#[test]
fn test_create_shadow_workspace_hardlinks_files() {
    let temp = tempdir().expect("failed to create temp dir");
    let root_path = temp.path();

    // Setup initial source files
    let src_dir = root_path.join("src");
    fs::create_dir_all(&src_dir).expect("failed to create src dir");

    let file_a = src_dir.join("main.rs");
    let mut f_a = File::create(&file_a).expect("failed to create main.rs");
    writeln!(f_a, "fn main() {{ println!(\"hello\"); }}").expect("write failed");

    let file_b = root_path.join("README.md");
    let mut f_b = File::create(&file_b).expect("failed to create README.md");
    writeln!(f_b, "# Test Project").expect("write failed");

    let thread_id = "thread-12345";
    let shadow =
        ShadowWorkspace::create(root_path, thread_id).expect("failed to create shadow workspace");

    let expected_shadow_dir = root_path.join(".grein/shadow").join(thread_id);
    assert_eq!(shadow.path(), expected_shadow_dir.as_path());
    assert!(shadow.path().exists(), "shadow directory should exist");

    // Verify shadow copies exist
    let shadow_main = shadow.path().join("src").join("main.rs");
    let shadow_readme = shadow.path().join("README.md");
    assert!(shadow_main.exists());
    assert!(shadow_readme.exists());

    // Verify content parity
    let main_content = fs::read_to_string(&shadow_main).expect("read shadow main");
    assert_eq!(main_content.trim(), "fn main() { println!(\"hello\"); }");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let orig_meta = fs::metadata(&file_a).expect("orig meta");
        let shadow_meta = fs::metadata(&shadow_main).expect("shadow meta");
        assert_eq!(
            orig_meta.ino(),
            shadow_meta.ino(),
            "shadow files should share inodes when hardlinked"
        );
    }
}

#[test]
fn test_create_shadow_workspace_ignores_grein_internal_dir() {
    let temp = tempdir().expect("failed to create temp dir");
    let root_path = temp.path();

    // Create existing .grein directory to verify recursion exclusion
    let internal_dir = root_path.join(".grein").join("state");
    fs::create_dir_all(&internal_dir).expect("create .grein/state");
    fs::write(internal_dir.join("state.json"), "{}").expect("write state.json");

    let regular_file = root_path.join("file.txt");
    fs::write(&regular_file, "content").expect("write regular file");

    let thread_id = "thread-test";
    let shadow =
        ShadowWorkspace::create(root_path, thread_id).expect("failed to create shadow workspace");

    assert!(shadow.path().join("file.txt").exists());
    assert!(
        !shadow.path().join(".grein").exists(),
        ".grein should not be cloned into shadow"
    );
}

#[test]
fn test_shadow_workspace_cleanup_removes_directory() {
    let temp = tempdir().expect("failed to create temp dir");
    let root_path = temp.path();
    fs::write(root_path.join("hello.txt"), "world").expect("write hello.txt");

    let thread_id = "thread-cleanup";
    let shadow =
        ShadowWorkspace::create(root_path, thread_id).expect("failed to create shadow workspace");

    let shadow_path = shadow.path().to_path_buf();
    assert!(shadow_path.exists());

    shadow.cleanup().expect("cleanup failed");
    assert!(
        !shadow_path.exists(),
        "shadow path should be removed after cleanup"
    );
}

#[test]
fn test_create_shadow_workspace_nonexistent_root_returns_error() {
    let non_existent = Path::new("/non/existent/path/for/eldi/test/shadow");
    let result = ShadowWorkspace::create(non_existent, "thread-err");
    assert!(result.is_err());
    match result.unwrap_err() {
        ShadowError::SourceNotFound(_) => {}
        err => panic!("unexpected error: {:?}", err),
    }
}
