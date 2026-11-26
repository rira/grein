use std::fs;
use std::path::PathBuf;

fn resolve_repo_file(relative_path: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        manifest_dir.join(relative_path),
        manifest_dir.join("../..").join(relative_path),
        PathBuf::from(relative_path),
    ];

    candidates
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(relative_path))
}

fn read_repo_file(relative_path: &str) -> String {
    let path = resolve_repo_file(relative_path);
    assert!(
        path.exists(),
        "Expected {} to exist at {:?}",
        relative_path,
        path
    );
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("Failed to read {}: {}", relative_path, e))
}

fn extract_desc(content: &str) -> String {
    content
        .lines()
        .find_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("desc \"")
                .and_then(|rest| rest.strip_suffix('"'))
                .map(str::to_owned)
        })
        .expect("Expected a desc \"...\" declaration")
}

#[test]
fn test_formula_contains_install_and_test_blocks() {
    let content = read_repo_file("Formula/grein.rb");

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

#[test]
fn test_formula_metadata_and_dependencies_are_hardened() {
    let content = read_repo_file("Formula/grein.rb");

    assert!(
        content.starts_with("class Grein < Formula\n")
            || content.starts_with("class Grein < Formula\r\n"),
        "Formula/grein.rb must begin at column 0 with 'class Grein < Formula'"
    );

    let desc = extract_desc(&content);
    assert!(!desc.is_empty(), "Formula desc must not be empty");
    assert!(
        desc.chars().count() <= 80,
        "Formula desc exceeds 80-character Homebrew limit ({} chars): {}",
        desc.chars().count(),
        desc
    );

    assert!(
        !content.contains("uses_from_macos \"sqlite\""),
        "Formula/grein.rb must not declare uses_from_macos \"sqlite\" because rusqlite bundles SQLite"
    );
    assert!(
        !content.contains("depends_on \"pkg-config\""),
        "Formula/grein.rb must not declare depends_on \"pkg-config\""
    );

    assert!(
        content.contains("depends_on \"rust\" => :build"),
        "Formula/grein.rb must keep depends_on \"rust\" => :build"
    );
    assert!(
        content.contains("homepage \"https://github.com/rira/grein\""),
        "Formula/grein.rb must keep homepage"
    );
    let expected_url = format!(
        "url \"https://github.com/rira/grein/archive/refs/tags/v{}.tar.gz\"",
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        content.contains(&expected_url),
        "Formula/grein.rb must keep current version archive URL ({})",
        expected_url
    );
    assert!(
        content.contains("license \"MIT\""),
        "Formula/grein.rb must specify MIT license"
    );
    assert!(
        content.contains("sha256 "),
        "Formula/grein.rb must retain sha256 field"
    );
}

#[test]
fn test_release_workflow_hardening_and_tap_formula() {
    let content = read_repo_file(".github/workflows/release.yml");

    assert!(
        content.contains("Verify tag matches Cargo.toml version"),
        "release.yml must verify tag matches Cargo.toml version"
    );
    assert!(
        content.contains("import tomllib") && content.contains("[workspace.package]"),
        "release.yml tag check must use tomllib with [workspace.package] fallback"
    );

    assert!(
        !content.contains("pkg-config") && !content.contains("libsqlite3-dev"),
        "release.yml must not install redundant pkg-config or libsqlite3-dev"
    );

    assert!(
        content.contains("Run Apple Silicon macOS PTY tests"),
        "release.yml must run Apple Silicon macOS PTY tests before packaging"
    );
    let mac_pty_start = content
        .find("Run Apple Silicon macOS PTY tests")
        .expect("release.yml must have Run Apple Silicon macOS PTY tests step");
    let mac_pty_end = content[mac_pty_start..]
        .find("Build release binary")
        .map(|offset| mac_pty_start + offset)
        .unwrap_or(content.len());
    let mac_pty_step = &content[mac_pty_start..mac_pty_end];
    assert!(
        mac_pty_step.contains("matrix.target == 'aarch64-apple-darwin'")
            && mac_pty_step.contains("--lib shell")
            && mac_pty_step.contains("--test pty_session_test")
            && mac_pty_step.contains("--test workspace_coverage_hardening_test")
            && mac_pty_step.contains("--test port_contracts_test"),
        "Apple Silicon macOS PTY test step must execute shell and PTY targets, got:\n{mac_pty_step}"
    );
    assert!(
        content.contains("Smoke-test native binary version"),
        "release.yml must smoke-test native binary --version"
    );
    assert!(
        content.contains("matrix.target != 'x86_64-apple-darwin'"),
        "release.yml must skip native smoke-test on cross-compiled x86_64-apple-darwin"
    );

    assert!(
        content.contains("codesign --force --sign - \"$BIN_PATH\""),
        "release.yml must ad-hoc codesign macOS binaries after strip -x"
    );

    assert!(
        content.contains("Require HOMEBREW_TAP_TOKEN secret"),
        "release.yml must fail when HOMEBREW_TAP_TOKEN is unset"
    );
    assert!(
        !content.contains("Skip tap update"),
        "release.yml must not silently skip tap updates"
    );

    assert!(
        content.contains("sed 's/^          //'"),
        "release.yml must strip leading YAML indentation when generating tap/Formula/grein.rb"
    );

    let heredoc_start = content
        .find("tap/Formula/grein.rb <<EOF")
        .expect("Expected tap/Formula/grein.rb heredoc");
    let heredoc_body = &content[heredoc_start..];
    let mut lines = heredoc_body.lines().skip(1);
    let mut formula_lines = Vec::new();
    for line in &mut lines {
        if line.trim() == "EOF" {
            break;
        }
        formula_lines.push(line);
    }
    assert!(
        !formula_lines.is_empty(),
        "Expected non-empty tap/Formula/grein.rb heredoc"
    );
    assert_eq!(
        formula_lines[0].strip_prefix("          "),
        Some("class Grein < Formula"),
        "Generated tap formula class line must begin at column 0 after sed strip"
    );
    for line in &formula_lines {
        if !line.is_empty() {
            assert!(
                line.starts_with("          "),
                "Every non-empty line in tap formula heredoc must have >=10 leading spaces: {:?}",
                line
            );
        }
    }

    let tap_formula = formula_lines.join("\n");
    let tap_desc = extract_desc(&tap_formula);
    assert!(!tap_desc.is_empty(), "Tap formula desc must not be empty");
    assert!(
        tap_desc.chars().count() <= 80,
        "Tap formula desc exceeds 80-character Homebrew limit ({} chars): {}",
        tap_desc.chars().count(),
        tap_desc
    );

    assert!(
        !content.contains("uses_from_macos \"sqlite\""),
        "release.yml tap formula must not declare uses_from_macos \"sqlite\""
    );
    assert!(
        content.contains("ruby -c tap/Formula/grein.rb"),
        "release.yml must syntax-check generated tap/Formula/grein.rb with ruby -c"
    );
}
