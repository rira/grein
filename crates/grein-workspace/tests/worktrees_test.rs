#![allow(clippy::needless_borrows_for_generic_args)]

use std::fs;
use std::path::Path;
use std::process::Command;

use grein_workspace::allocator::{ThreadId, WorkspaceError, WorktreeAllocator, WorktreeMode};
use grein_workspace::git::GitRepository;
use grein_workspace::worktrees::{
    current_branch_name, list_repo_worktrees, list_worktrees_across_paths, prune_worktrees,
    remove_worktree, resolve_primary_repo_root, resolve_worktree_toplevel,
    sanitize_worktree_dir_slug, worktree_diff_preview, DiscoveredWorktree,
};
use tempfile::TempDir;

fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("failed to execute git");
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn branch_exists(dir: &Path, branch: &str) -> bool {
    let ref_name = format!("refs/heads/{branch}");
    Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &ref_name])
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("failed to execute git show-ref")
        .success()
}

fn init_repo_with_branch(branch: &str) -> TempDir {
    let temp_dir = TempDir::new().expect("create temp dir");
    let root = temp_dir.path();

    run_git(root, &["init", "-b", branch]);
    run_git(root, &["config", "user.name", "Grein Test"]);
    run_git(root, &["config", "user.email", "test@grein.dev"]);

    fs::write(root.join("README.md"), "base\n").expect("write README");
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "Initial commit"]);

    temp_dir
}

#[test]
fn sanitize_worktree_dir_slug_handles_edge_cases() {
    assert_eq!(sanitize_worktree_dir_slug("feat/auth"), "feat-auth");
    assert_eq!(sanitize_worktree_dir_slug("feat\\auth"), "feat-auth");
    assert_eq!(sanitize_worktree_dir_slug("feat / auth  v2"), "feat-auth-v2");
    assert_eq!(
        sanitize_worktree_dir_slug("release/v1.2_rc1"),
        "release-v1.2_rc1"
    );
    assert_eq!(
        sanitize_worktree_dir_slug("feat@user:test!"),
        "feat-user-test"
    );

    let traversal = sanitize_worktree_dir_slug("../escape/../path");
    assert_eq!(traversal, "escape-path");
    assert!(!traversal.contains(".."));

    assert_eq!(sanitize_worktree_dir_slug("a..b"), "a-b");
    assert_eq!(sanitize_worktree_dir_slug("a...b"), "a-b");
    assert_eq!(sanitize_worktree_dir_slug("a..--b"), "a-b");
    assert_eq!(sanitize_worktree_dir_slug(".leading-and-trailing."), "leading-and-trailing");
    assert_eq!(sanitize_worktree_dir_slug("-leading-and-trailing-"), "leading-and-trailing");

    assert_eq!(sanitize_worktree_dir_slug(".."), "worktree");
    assert_eq!(sanitize_worktree_dir_slug("..."), "worktree");
    assert_eq!(sanitize_worktree_dir_slug(".-.-."), "worktree");
    assert_eq!(sanitize_worktree_dir_slug(""), "worktree");
    assert_eq!(sanitize_worktree_dir_slug("   "), "worktree");
    assert_eq!(sanitize_worktree_dir_slug("///"), "worktree");
}

#[test]
fn root_and_branch_resolution_handles_primary_linked_detached_and_invalid_paths() {
    let plain = TempDir::new().expect("create plain temp dir");
    assert_eq!(resolve_primary_repo_root(plain.path()), None);
    assert_eq!(resolve_worktree_toplevel(plain.path()), None);
    assert_eq!(current_branch_name(plain.path()), None);

    let missing = plain.path().join("missing");
    assert_eq!(resolve_primary_repo_root(&missing), None);
    assert_eq!(resolve_worktree_toplevel(&missing), None);
    assert_eq!(current_branch_name(&missing), None);

    let repo = init_repo_with_branch("feat/auth");
    let canonical_primary = fs::canonicalize(repo.path()).expect("canonicalize primary");

    let nested = repo.path().join("src").join("nested");
    fs::create_dir_all(&nested).expect("create nested dir");

    assert_eq!(resolve_primary_repo_root(&nested), Some(canonical_primary.clone()));
    assert_eq!(resolve_worktree_toplevel(&nested), Some(canonical_primary.clone()));
    assert_eq!(current_branch_name(&nested).as_deref(), Some("feat/auth"));

    let linked_parent = TempDir::new().expect("create linked parent");
    let linked_wt = linked_parent.path().join("linked-wt");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "topic/one",
            linked_wt.to_str().expect("utf8 path"),
        ],
    );
    let canonical_linked = fs::canonicalize(&linked_wt).expect("canonicalize linked");
    let linked_nested = linked_wt.join("sub");
    fs::create_dir_all(&linked_nested).expect("create linked nested");

    assert_eq!(
        resolve_primary_repo_root(&linked_nested),
        Some(canonical_primary.clone())
    );
    assert_eq!(
        resolve_worktree_toplevel(&linked_nested),
        Some(canonical_linked)
    );
    assert_eq!(
        current_branch_name(&linked_nested).as_deref(),
        Some("topic/one")
    );

    run_git(&linked_wt, &["checkout", "--detach", "HEAD"]);
    assert_eq!(current_branch_name(&linked_wt), None);
}

#[test]
fn open_session_auto_named_creates_branch_based_worktree_and_disambiguates() {
    let repo = init_repo_with_branch("feat/auth");
    let canonical_primary = fs::canonicalize(repo.path()).expect("canonicalize primary");

    let allocator = WorktreeAllocator::new(GitRepository::new(repo.path()));

    let mut s1 = allocator.open_session_auto_named(ThreadId::from("thread-1"));
    assert_eq!(s1.mode(), WorktreeMode::SharedRead);
    s1.transition_to_isolated_worktree()
        .expect("transition s1 to isolated worktree");

    let expected_dir_1 = canonical_primary
        .join(".grein")
        .join("worktrees")
        .join("feat-auth");
    assert_eq!(s1.working_directory(), expected_dir_1.as_path());
    assert!(expected_dir_1.is_dir());
    assert_eq!(
        current_branch_name(&expected_dir_1).as_deref(),
        Some("feat/auth-wt")
    );

    let mut s2 = allocator.open_session_auto_named(ThreadId::from("thread-2"));
    s2.transition_to_isolated_worktree()
        .expect("transition s2 to isolated worktree");

    let expected_dir_2 = canonical_primary
        .join(".grein")
        .join("worktrees")
        .join("feat-auth-2");
    assert_eq!(s2.working_directory(), expected_dir_2.as_path());
    assert!(expected_dir_2.is_dir());
    assert_eq!(
        current_branch_name(&expected_dir_2).as_deref(),
        Some("feat/auth-wt-2")
    );
}

#[test]
fn open_session_auto_named_skips_when_existing_branch_ref_blocks_name() {
    let repo = init_repo_with_branch("feat/auth");
    let canonical_primary = fs::canonicalize(repo.path()).expect("canonicalize primary");

    run_git(repo.path(), &["branch", "feat/auth-wt"]);

    let allocator = WorktreeAllocator::new(GitRepository::new(repo.path()));
    let mut session = allocator.open_session_auto_named(ThreadId::from("thread-ref-block"));
    session
        .transition_to_isolated_worktree()
        .expect("transition should disambiguate past existing branch ref");

    let expected_dir = canonical_primary
        .join(".grein")
        .join("worktrees")
        .join("feat-auth-2");
    assert_eq!(session.working_directory(), expected_dir.as_path());
    assert_eq!(
        current_branch_name(&expected_dir).as_deref(),
        Some("feat/auth-wt-2")
    );
}

#[test]
fn open_session_auto_named_preserves_thread_id_on_main_master_and_detached() {
    for (branch_opt, thread_id) in [
        (Some("main"), "t-main"),
        (Some("master"), "t-master"),
        (None, "t-detached"),
    ] {
        let repo = init_repo_with_branch(branch_opt.unwrap_or("main"));
        if branch_opt.is_none() {
            run_git(repo.path(), &["checkout", "--detach", "HEAD"]);
        }
        let canonical_primary = fs::canonicalize(repo.path()).expect("canonicalize primary");

        let allocator = WorktreeAllocator::new(GitRepository::new(repo.path()));
        let mut session = allocator.open_session_auto_named(ThreadId::from(thread_id));
        session
            .transition_to_isolated_worktree()
            .expect("transition should succeed");

        let expected_dir = canonical_primary
            .join(".grein")
            .join("worktrees")
            .join(thread_id);
        let expected_branch = format!("worktree-{thread_id}");
        assert_eq!(session.working_directory(), expected_dir.as_path());
        assert_eq!(
            current_branch_name(&expected_dir).as_deref(),
            Some(expected_branch.as_str())
        );
    }
}

#[test]
fn open_session_auto_named_from_linked_worktree_targets_primary_root() {
    let repo = init_repo_with_branch("main");
    let canonical_primary = fs::canonicalize(repo.path()).expect("canonicalize primary");

    let linked_dir = canonical_primary.join(".grein").join("worktrees").join("seed");
    fs::create_dir_all(linked_dir.parent().expect("parent")).expect("create parent");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "topic/from-linked",
            linked_dir.to_str().expect("utf8 path"),
        ],
    );

    let allocator = WorktreeAllocator::new(GitRepository::new(&linked_dir));
    let mut session = allocator.open_session_auto_named(ThreadId::from("ignored-tid"));
    session
        .transition_to_isolated_worktree()
        .expect("transition from linked worktree should place worktree under primary root");

    let expected_dir = canonical_primary
        .join(".grein")
        .join("worktrees")
        .join("topic-from-linked");
    assert_eq!(session.working_directory(), expected_dir.as_path());
    assert!(expected_dir.is_dir());
    assert!(!linked_dir.join(".grein").exists());
    assert_eq!(
        current_branch_name(&expected_dir).as_deref(),
        Some("topic/from-linked-wt")
    );
}

#[test]
fn list_worktrees_across_paths_deduplicates_sorts_and_inspects_metadata() {
    let workspace = TempDir::new().expect("create workspace temp dir");

    let alpha_path = workspace.path().join("Alpha");
    let beta_path = workspace.path().join("beta");
    let zeta_path = workspace.path().join("Zeta");
    for (p, br) in [(&alpha_path, "main"), (&beta_path, "main"), (&zeta_path, "zzz-primary")] {
        fs::create_dir_all(p).expect("create repo dir");
        run_git(p, &["init", "-b", br]);
        run_git(p, &["config", "user.name", "Grein Test"]);
        run_git(p, &["config", "user.email", "test@grein.dev"]);
        fs::write(p.join("README.md"), "base\n").expect("write README");
        run_git(p, &["add", "."]);
        run_git(p, &["commit", "-m", "Init"]);
    }

    let zeta_canon = fs::canonicalize(&zeta_path).expect("canonical zeta");
    let zeta_head = run_git(&zeta_canon, &["rev-parse", "HEAD"]);
    let zeta_head_short: String = zeta_head.chars().take(7).collect();
    fs::write(zeta_canon.join(".git").join("info").join("exclude"), ".grein/\n")
        .expect("exclude .grein in zeta");

    let zeta_grein_wt = zeta_canon.join(".grein").join("worktrees").join("feat-alpha");
    fs::create_dir_all(zeta_grein_wt.parent().expect("parent")).expect("create .grein/worktrees");
    run_git(
        &zeta_canon,
        &[
            "worktree",
            "add",
            "-b",
            "feat/alpha-wt",
            zeta_grein_wt.to_str().expect("utf8"),
        ],
    );
    fs::write(zeta_grein_wt.join("dirty1.txt"), "1\n").expect("write dirty1");
    fs::write(zeta_grein_wt.join("dirty2.txt"), "2\n").expect("write dirty2");

    let external_parent = TempDir::new().expect("create external parent");
    let zeta_ext_wt = external_parent.path().join("manual-ext");
    run_git(
        &zeta_canon,
        &[
            "worktree",
            "add",
            "-b",
            "manual/br",
            zeta_ext_wt.to_str().expect("utf8"),
        ],
    );

    let zeta_detached_wt = external_parent.path().join("detached-aaa");
    run_git(
        &zeta_canon,
        &[
            "worktree",
            "add",
            "--detach",
            zeta_detached_wt.to_str().expect("utf8"),
        ],
    );

    let zeta_stale_wt = external_parent.path().join("stale-prunable");
    run_git(
        &zeta_canon,
        &[
            "worktree",
            "add",
            "-b",
            "stale/br",
            zeta_stale_wt.to_str().expect("utf8"),
        ],
    );
    fs::remove_dir_all(&zeta_stale_wt).expect("delete stale dir manually");

    let alpha_sub = alpha_path.join("subdir");
    fs::create_dir_all(&alpha_sub).expect("create alpha subdir");

    let discovered: Vec<DiscoveredWorktree> = list_worktrees_across_paths(
        &[
            beta_path.clone(),
            zeta_ext_wt.clone(),
            alpha_sub,
            alpha_path.clone(),
            workspace.path().join("non-git-or-missing"),
        ],
        Some(&zeta_grein_wt),
    );

    assert_eq!(discovered.len(), 6);

    // Preferred repo (Zeta) comes first; primary first inside Zeta, then sorted by branch/basename:
    // detached-aaa (detached, basename "detached-aaa"), feat/alpha-wt, manual/br
    assert_eq!(discovered[0].repo_name, "Zeta");
    assert!(discovered[0].is_primary);
    assert!(!discovered[0].is_grein_managed);
    assert!(!discovered[0].is_detached);
    assert_eq!(discovered[0].branch.as_deref(), Some("zzz-primary"));
    assert_eq!(discovered[0].head_short.as_deref(), Some(zeta_head_short.as_str()));
    assert_eq!(discovered[0].dirty_count, 0);

    assert_eq!(discovered[1].repo_name, "Zeta");
    assert!(!discovered[1].is_primary);
    assert!(discovered[1].is_detached);
    assert_eq!(discovered[1].branch, None);
    assert!(!discovered[1].is_grein_managed);
    assert_eq!(
        discovered[1].path,
        fs::canonicalize(&zeta_detached_wt).expect("canon detached")
    );

    assert_eq!(discovered[2].repo_name, "Zeta");
    assert!(!discovered[2].is_primary);
    assert!(!discovered[2].is_detached);
    assert_eq!(discovered[2].branch.as_deref(), Some("feat/alpha-wt"));
    assert!(discovered[2].is_grein_managed);
    assert_eq!(discovered[2].dirty_count, 2);

    assert_eq!(discovered[3].repo_name, "Zeta");
    assert!(!discovered[3].is_primary);
    assert!(!discovered[3].is_detached);
    assert_eq!(discovered[3].branch.as_deref(), Some("manual/br"));
    assert!(!discovered[3].is_grein_managed);
    assert_eq!(discovered[3].dirty_count, 0);

    // Remaining repos alphabetical (case-insensitive): Alpha before beta
    assert_eq!(discovered[4].repo_name, "Alpha");
    assert!(discovered[4].is_primary);
    assert_eq!(discovered[4].repo_root, fs::canonicalize(&alpha_path).unwrap());

    assert_eq!(discovered[5].repo_name, "beta");
    assert!(discovered[5].is_primary);
    assert_eq!(discovered[5].repo_root, fs::canonicalize(&beta_path).unwrap());
}

#[test]
fn remove_worktree_enforces_safety_force_and_branch_deletion_rules() {
    let repo = init_repo_with_branch("feat/auth");
    let canonical_primary = fs::canonicalize(repo.path()).expect("canonicalize primary");

    // Refuses primary worktree
    let err_primary = remove_worktree(repo.path(), repo.path(), true, true)
        .expect_err("must refuse primary worktree removal");
    assert!(matches!(err_primary, WorkspaceError::GitError(_)));

    // Refuses unknown path
    let unknown = canonical_primary.join("unknown-dir");
    let err_unknown = remove_worktree(repo.path(), &unknown, true, true)
        .expect_err("must refuse unknown path");
    assert!(matches!(err_unknown, WorkspaceError::GitError(_)));

    // Create two grein-managed worktrees via allocator
    let allocator = WorktreeAllocator::new(GitRepository::new(repo.path()));
    let mut s1 = allocator.open_session_auto_named(ThreadId::from("t1"));
    s1.transition_to_isolated_worktree().expect("create s1");
    let wt1 = s1.working_directory().to_path_buf();

    let mut s2 = allocator.open_session_auto_named(ThreadId::from("t2"));
    s2.transition_to_isolated_worktree().expect("create s2");
    let wt2 = s2.working_directory().to_path_buf();

    // Dirty wt1 -> refused without force
    fs::write(wt1.join("untracked.txt"), "dirty\n").expect("dirty wt1");
    let err_dirty = remove_worktree(repo.path(), &wt1, false, false)
        .expect_err("must refuse dirty worktree without force");
    assert!(matches!(err_dirty, WorkspaceError::GitError(_)));
    assert!(wt1.exists());

    // Force remove dirty wt1 with delete_branch = false -> preserves feat/auth-wt
    let deleted_1 = remove_worktree(repo.path(), &wt1, true, false)
        .expect("force remove wt1 with preserved branch");
    assert_eq!(deleted_1, None);
    assert!(!wt1.exists());
    assert!(
        branch_exists(repo.path(), "feat/auth-wt"),
        "expected feat/auth-wt preserved"
    );

    // Force remove wt2 with delete_branch = true -> deletes feat/auth-wt-2
    fs::write(wt2.join("untracked.txt"), "dirty\n").expect("dirty wt2");
    let deleted_2 = remove_worktree(repo.path(), &wt2, true, true)
        .expect("force remove wt2 with branch deletion");
    assert_eq!(deleted_2.as_deref(), Some("feat/auth-wt-2"));
    assert!(!wt2.exists());
    assert!(
        !branch_exists(repo.path(), "feat/auth-wt-2"),
        "expected feat/auth-wt-2 deleted"
    );

    // Non-grein-managed external worktree preserves branch even when delete_branch = true
    let ext_parent = TempDir::new().expect("ext parent");
    let ext_wt = ext_parent.path().join("user-wt");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "user/feature",
            ext_wt.to_str().expect("utf8"),
        ],
    );
    let deleted_ext = remove_worktree(repo.path(), &ext_wt, false, true)
        .expect("remove non-grein worktree");
    assert_eq!(deleted_ext, None);
    assert!(!ext_wt.exists());
    assert!(
        branch_exists(repo.path(), "user/feature"),
        "expected non-grein branch preserved"
    );

    // Clean up list check
    let remaining = list_repo_worktrees(repo.path());
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].is_primary);
}

#[test]
fn open_session_auto_named_skips_when_dir_exists_and_primary_wt_branch_not_grein_managed() {
    let repo = init_repo_with_branch("feat/custom-wt");
    let canonical_primary = fs::canonicalize(repo.path()).expect("canonicalize primary");

    let primary_listed = list_repo_worktrees(repo.path());
    assert_eq!(primary_listed.len(), 1);
    assert!(primary_listed[0].is_primary);
    assert!(!primary_listed[0].is_grein_managed);

    let blocking_dir = canonical_primary
        .join(".grein")
        .join("worktrees")
        .join("feat-custom-wt");
    fs::create_dir_all(&blocking_dir).expect("create blocking dir");

    let allocator = WorktreeAllocator::new(GitRepository::new(repo.path()));
    let mut session = allocator.open_session_auto_named(ThreadId::from("tid-dir-block"));
    session
        .transition_to_isolated_worktree()
        .expect("transition should skip existing dir");

    let expected_dir = canonical_primary
        .join(".grein")
        .join("worktrees")
        .join("feat-custom-wt-2");
    assert_eq!(session.working_directory(), expected_dir.as_path());
    assert_eq!(
        current_branch_name(&expected_dir).as_deref(),
        Some("feat/custom-wt-wt-2")
    );

    // Detached grein-managed worktree removes cleanly with delete_branch = true and returns None
    let detached_grein = canonical_primary
        .join(".grein")
        .join("worktrees")
        .join("detached-grein");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "--detach",
            detached_grein.to_str().expect("utf8"),
        ],
    );
    let listed = list_repo_worktrees(repo.path());
    let detached_entry = listed
        .iter()
        .find(|w| w.path == detached_grein)
        .expect("detached grein entry found");
    assert!(detached_entry.is_detached);
    assert!(detached_entry.is_grein_managed);

    let removed = remove_worktree(repo.path(), &detached_grein, false, true)
        .expect("remove detached grein worktree");
    assert_eq!(removed, None);
    assert!(!detached_grein.exists());
}

#[test]
fn worktree_diff_preview_handles_missing_clean_dirty_unborn_and_truncation() {
    let plain = TempDir::new().expect("plain temp dir");
    let missing = plain.path().join("does-not-exist");
    assert_eq!(
        worktree_diff_preview(&missing, 50),
        "Worktree directory not found\n"
    );

    let repo = init_repo_with_branch("feat/preview");
    let clean_preview = worktree_diff_preview(repo.path(), 50);
    assert!(
        clean_preview.starts_with("## feat/preview\n"),
        "expected branch header, got:\n{clean_preview}"
    );
    assert!(
        clean_preview.contains("Clean working tree (no uncommitted changes)"),
        "expected clean message, got:\n{clean_preview}"
    );

    fs::write(repo.path().join("README.md"), "base\nupdated line\n").expect("modify README");
    fs::write(repo.path().join("untracked.txt"), "hello\n").expect("write untracked");

    let dirty_preview = worktree_diff_preview(repo.path(), 100);
    assert!(
        dirty_preview.contains("## feat/preview"),
        "missing branch header:\n{dirty_preview}"
    );
    assert!(
        dirty_preview.contains(" M README.md"),
        "missing modified status line:\n{dirty_preview}"
    );
    assert!(
        dirty_preview.contains("?? untracked.txt"),
        "missing untracked status line:\n{dirty_preview}"
    );
    assert!(
        dirty_preview.contains("1 file changed"),
        "missing --stat summary:\n{dirty_preview}"
    );
    assert!(
        dirty_preview.contains("+updated line"),
        "missing unified diff addition:\n{dirty_preview}"
    );
    assert!(
        !dirty_preview.contains('\x1b'),
        "diff preview must not contain ANSI color escapes:\n{dirty_preview}"
    );

    let full_line_count = dirty_preview.lines().count();
    assert!(full_line_count > 3, "expected >3 lines in dirty preview");

    let truncated = worktree_diff_preview(repo.path(), 3);
    let expected_remaining = full_line_count - 3;
    let expected_suffix = format!("\n... ({expected_remaining} more lines truncated)\n");
    assert!(
        truncated.ends_with(&expected_suffix),
        "expected truncation suffix `{expected_suffix:?}` in:\n{truncated}"
    );
    let trunc_lines: Vec<&str> = truncated.lines().collect();
    assert_eq!(trunc_lines.len(), 4);

    let zero_clamped = worktree_diff_preview(repo.path(), 0);
    let expected_zero_suffix = format!("\n... ({} more lines truncated)\n", full_line_count - 1);
    assert!(
        zero_clamped.ends_with(&expected_zero_suffix),
        "expected max_lines=0 to clamp to 1 line, got:\n{zero_clamped}"
    );

    // Unborn HEAD repository
    let unborn = TempDir::new().expect("unborn temp dir");
    run_git(unborn.path(), &["init", "-b", "main"]);
    let unborn_clean = worktree_diff_preview(unborn.path(), 50);
    assert!(
        unborn_clean.contains("Clean working tree (no uncommitted changes)"),
        "expected clean unborn message, got:\n{unborn_clean}"
    );

    fs::write(unborn.path().join("staged.txt"), "staged content\n").expect("write staged");
    run_git(unborn.path(), &["add", "staged.txt"]);
    let unborn_staged = worktree_diff_preview(unborn.path(), 50);
    assert!(
        unborn_staged.contains("A  staged.txt"),
        "expected staged entry in unborn preview:\n{unborn_staged}"
    );
    assert!(
        unborn_staged.contains("+staged content"),
        "expected cached unified diff in unborn preview:\n{unborn_staged}"
    );
}

#[test]
fn prune_worktrees_removes_stale_registrations_and_handles_clean_and_invalid_repos() {
    let plain = TempDir::new().expect("plain temp dir");
    let err = prune_worktrees(plain.path()).expect_err("non-git dir must fail");
    assert!(matches!(err, WorkspaceError::GitError(_)));

    let repo = init_repo_with_branch("main");
    let clean_msg = prune_worktrees(repo.path()).expect("prune clean repo");
    assert_eq!(clean_msg, "Worktrees clean (nothing to prune)");

    let wt_parent = TempDir::new().expect("wt parent");
    let live_wt = wt_parent.path().join("live-wt");
    let stale_wt = wt_parent.path().join("stale-wt");
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "topic/live",
            live_wt.to_str().expect("utf8"),
        ],
    );
    run_git(
        repo.path(),
        &[
            "worktree",
            "add",
            "-b",
            "topic/stale",
            stale_wt.to_str().expect("utf8"),
        ],
    );

    fs::remove_dir_all(&stale_wt).expect("manually remove stale worktree dir");

    let porcelain_before = run_git(repo.path(), &["worktree", "list", "--porcelain"]);
    assert!(
        porcelain_before.contains("stale-wt"),
        "expected stale-wt registered before prune:\n{porcelain_before}"
    );

    // Invoke via linked live_wt to verify primary root resolution
    let prune_msg = prune_worktrees(&live_wt).expect("prune stale worktree");
    assert!(
        !prune_msg.is_empty() && prune_msg != "Worktrees clean (nothing to prune)",
        "expected non-empty prune output, got: {prune_msg:?}"
    );

    let porcelain_after = run_git(repo.path(), &["worktree", "list", "--porcelain"]);
    assert!(
        !porcelain_after.contains("stale-wt"),
        "stale-wt registration should be pruned:\n{porcelain_after}"
    );
    assert!(
        porcelain_after.contains("live-wt"),
        "live-wt must remain registered:\n{porcelain_after}"
    );

    let second_msg = prune_worktrees(repo.path()).expect("second prune should be clean");
    assert_eq!(second_msg, "Worktrees clean (nothing to prune)");
}
