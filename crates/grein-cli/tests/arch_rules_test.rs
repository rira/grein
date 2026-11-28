use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
    out.sort();
}

fn all_crate_src_files(root: &Path) -> Vec<PathBuf> {
    let crates_dir = root.join("crates");
    assert!(crates_dir.is_dir(), "missing crates directory");
    let mut files = Vec::new();
    for entry in fs::read_dir(&crates_dir).expect("read crates dir") {
        let src_dir = entry.expect("crate entry").path().join("src");
        if src_dir.is_dir() {
            collect_rs_files(&src_dir, &mut files);
        }
    }
    files.sort();
    assert!(!files.is_empty(), "expected .rs files under crates/*/src");
    files
}

fn is_dedicated_test_submodule(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if file_name == "tests.rs" || file_name.starts_with("tests_") {
        return true;
    }
    path.components()
        .any(|c| c.as_os_str().to_str() == Some("tests"))
}

fn verify_dedicated_test_file_is_cfg_test_gated(path: &Path) {
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
    let parent = path.parent().expect("test file parent");

    let (mod_file, expected_mod_name) = if path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("tests")
    {
        if file_name == "mod.rs" {
            let grand = parent.parent().expect("tests parent");
            let candidate_mod = grand.join("mod.rs");
            let candidate_lib = grand.join("lib.rs");
            let owner = if candidate_mod.is_file() {
                candidate_mod
            } else {
                candidate_lib
            };
            (owner, "tests".to_string())
        } else {
            let tests_mod = parent.join("mod.rs");
            assert!(
                tests_mod.is_file(),
                "missing tests/mod.rs for {}",
                path.display()
            );
            let grand = parent.parent().expect("tests parent");
            let owner = if grand.join("mod.rs").is_file() {
                grand.join("mod.rs")
            } else {
                grand.join("lib.rs")
            };
            let tests_mod_src = fs::read_to_string(&tests_mod).expect("read tests/mod.rs");
            assert!(
                tests_mod_src.contains(&format!("mod {stem};")),
                "expected `mod {stem};` in {}",
                tests_mod.display()
            );
            (owner, "tests".to_string())
        }
    } else {
        let candidate_mod = parent.join("mod.rs");
        let candidate_lib = parent.join("lib.rs");
        let owner = if candidate_mod.is_file() {
            candidate_mod
        } else {
            candidate_lib
        };
        (owner, stem.to_string())
    };

    let owner_src = fs::read_to_string(&mod_file)
        .unwrap_or_else(|e| panic!("read {}: {e}", mod_file.display()));
    let lines: Vec<&str> = owner_src.lines().collect();
    let expected_decl = format!("mod {expected_mod_name};");
    let found_gated = lines.windows(2).any(|w| {
        w[0].trim().replace(' ', "").contains("#[cfg(test)]") && w[1].trim() == expected_decl
    });
    assert!(
        found_gated,
        "dedicated test file {} must be gated by #[cfg(test)] `{expected_decl}` in {}",
        path.display(),
        mod_file.display()
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TokenKind {
    Ident(String),
    Punct(char),
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    line: usize,
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn try_raw_string_end(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut i = start;
    if bytes.get(i) == Some(&b'b') {
        i += 1;
    }
    if bytes.get(i) != Some(&b'r') {
        return None;
    }
    i += 1;
    let mut hashes = 0usize;
    while bytes.get(i) == Some(&b'#') {
        hashes += 1;
        i += 1;
    }
    if bytes.get(i) != Some(&b'"') {
        return None;
    }
    i += 1;
    let mut newlines = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            newlines += 1;
            i += 1;
            continue;
        }
        if bytes[i] == b'"' {
            let mut matched = 0usize;
            while matched < hashes && bytes.get(i + 1 + matched) == Some(&b'#') {
                matched += 1;
            }
            if matched == hashes {
                return Some((i + 1 + hashes, newlines));
            }
        }
        i += 1;
    }
    Some((bytes.len(), newlines))
}

fn try_char_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) != Some(&b'\'') {
        return None;
    }
    let next = *bytes.get(start + 1)?;
    if next == b'\n' || next == b'\r' {
        return None;
    }
    if next == b'\\' {
        let mut i = start + 2;
        while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
            if bytes[i] == b'\'' {
                return Some(i + 1);
            }
            i += 1;
        }
        None
    } else if bytes.get(start + 2) == Some(&b'\'') {
        Some(start + 3)
    } else {
        // Multi-byte UTF-8 char literal (e.g. '─') on same line without spaces
        let rest = std::str::from_utf8(&bytes[start + 1..]).ok()?;
        let ch = rest.chars().next()?;
        let after = start + 1 + ch.len_utf8();
        if bytes.get(after) == Some(&b'\'') {
            Some(after + 1)
        } else {
            None
        }
    }
}

fn tokenize_rust_code(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;

    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Line comment
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        // Nested block comment
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'\n' {
                    line += 1;
                    i += 1;
                } else if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        // Raw string literal (r"...", r#"..."#, br#"..."#)
        if (b == b'r' || b == b'b') && try_raw_string_end(bytes, i).is_some() {
            let (end, newlines) = try_raw_string_end(bytes, i).unwrap();
            line += newlines;
            i = end;
            continue;
        }
        // Normal or byte string literal
        if b == b'"' || (b == b'b' && bytes.get(i + 1) == Some(&b'"')) {
            i += if b == b'b' { 2 } else { 1 };
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    if bytes.get(i + 1) == Some(&b'\n') {
                        line += 1;
                    }
                    i = (i + 2).min(bytes.len());
                    continue;
                }
                if bytes[i] == b'\n' {
                    line += 1;
                }
                if bytes[i] == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        // Char or byte char literal (vs lifetime)
        if b == b'b' && bytes.get(i + 1) == Some(&b'\'') {
            if let Some(end) = try_char_literal_end(bytes, i + 1) {
                i = end;
                continue;
            }
        }
        if b == b'\'' {
            if let Some(end) = try_char_literal_end(bytes, i) {
                i = end;
                continue;
            }
        }
        // Identifier / keyword
        if is_ident_start(b) {
            let start = i;
            i += 1;
            while i < bytes.len() && is_ident_continue(bytes[i]) {
                i += 1;
            }
            let ident = std::str::from_utf8(&bytes[start..i])
                .expect("utf8 ident")
                .to_string();
            tokens.push(Token {
                kind: TokenKind::Ident(ident),
                line,
            });
            continue;
        }
        // Number literal
        if b.is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (is_ident_continue(bytes[i]) || bytes[i] == b'.') {
                i += 1;
            }
            continue;
        }
        tokens.push(Token {
            kind: TokenKind::Punct(b as char),
            line,
        });
        i += 1;
    }

    tokens
}

fn match_cfg_test_attr(tokens: &[Token], idx: usize) -> Option<usize> {
    let is_punct = |pos: usize, ch: char| matches!(tokens.get(pos), Some(Token { kind: TokenKind::Punct(c), .. }) if *c == ch);
    let is_ident = |pos: usize, s: &str| matches!(tokens.get(pos), Some(Token { kind: TokenKind::Ident(id), .. }) if id == s);

    if is_punct(idx, '#')
        && is_punct(idx + 1, '[')
        && is_ident(idx + 2, "cfg")
        && is_punct(idx + 3, '(')
        && is_ident(idx + 4, "test")
        && is_punct(idx + 5, ')')
        && is_punct(idx + 6, ']')
    {
        Some(idx + 7)
    } else {
        None
    }
}

fn skip_balanced_bracket_group(tokens: &[Token], mut idx: usize, open: char, close: char) -> usize {
    let mut depth = 0usize;
    while idx < tokens.len() {
        match tokens[idx].kind {
            TokenKind::Punct(c) if c == open => depth += 1,
            TokenKind::Punct(c) if c == close => {
                depth -= 1;
                if depth == 0 {
                    return idx + 1;
                }
            }
            _ => {}
        }
        idx += 1;
    }
    idx
}

fn skip_attribute(tokens: &[Token], idx: usize) -> Option<usize> {
    if matches!(tokens.get(idx), Some(Token { kind: TokenKind::Punct('#'), .. }))
        && matches!(tokens.get(idx + 1), Some(Token { kind: TokenKind::Punct('['), .. }))
    {
        Some(skip_balanced_bracket_group(tokens, idx + 1, '[', ']'))
    } else {
        None
    }
}

fn is_item_keyword(ident: &str) -> bool {
    matches!(
        ident,
        "mod"
            | "fn"
            | "struct"
            | "enum"
            | "union"
            | "trait"
            | "impl"
            | "macro_rules"
            | "use"
            | "type"
            | "static"
            | "const"
            | "extern"
    )
}

fn skip_cfg_test_item(tokens: &[Token], mut idx: usize) -> usize {
    while let Some(next) = skip_attribute(tokens, idx) {
        idx = next;
    }

    // Check whether decorated item starts with a module-level item keyword (after visibility/qualifiers).
    let mut probe = idx;
    while probe < tokens.len() {
        match &tokens[probe].kind {
            TokenKind::Ident(id)
                if matches!(id.as_str(), "pub" | "async" | "unsafe" | "default") =>
            {
                probe += 1;
                if id == "pub"
                    && matches!(
                        tokens.get(probe),
                        Some(Token {
                            kind: TokenKind::Punct('('),
                            ..
                        })
                    )
                {
                    probe = skip_balanced_bracket_group(tokens, probe, '(', ')');
                }
            }
            _ => break,
        }
    }

    let is_module_item = matches!(
        tokens.get(probe),
        Some(Token {
            kind: TokenKind::Ident(id),
            ..
        }) if is_item_keyword(id)
    );

    if is_module_item {
        let mut brace_depth = 0usize;
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut saw_brace = false;
        while idx < tokens.len() {
            match tokens[idx].kind {
                TokenKind::Punct('(') => paren_depth += 1,
                TokenKind::Punct(')') => paren_depth = paren_depth.saturating_sub(1),
                TokenKind::Punct('[') => bracket_depth += 1,
                TokenKind::Punct(']') => bracket_depth = bracket_depth.saturating_sub(1),
                TokenKind::Punct('{') => {
                    brace_depth += 1;
                    saw_brace = true;
                }
                TokenKind::Punct('}') => {
                    brace_depth = brace_depth.saturating_sub(1);
                    idx += 1;
                    if saw_brace && brace_depth == 0 {
                        return idx;
                    }
                    continue;
                }
                TokenKind::Punct(';') if brace_depth == 0 && paren_depth == 0 && bracket_depth == 0 => {
                    return idx + 1;
                }
                _ => {}
            }
            idx += 1;
        }
        return idx;
    }

    // Field, block, or statement inside a struct/impl/expression
    let mut brace_depth = 0usize;
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut saw_brace = false;
    while idx < tokens.len() {
        match tokens[idx].kind {
            TokenKind::Punct('(') => paren_depth += 1,
            TokenKind::Punct(')') => paren_depth = paren_depth.saturating_sub(1),
            TokenKind::Punct('[') => bracket_depth += 1,
            TokenKind::Punct(']') => bracket_depth = bracket_depth.saturating_sub(1),
            TokenKind::Punct('<') if brace_depth == 0 => angle_depth += 1,
            TokenKind::Punct('>') if brace_depth == 0 => angle_depth = angle_depth.saturating_sub(1),
            TokenKind::Punct('{') => {
                brace_depth += 1;
                saw_brace = true;
            }
            TokenKind::Punct('}') => {
                if brace_depth == 0 && !saw_brace {
                    return idx;
                }
                brace_depth = brace_depth.saturating_sub(1);
                idx += 1;
                if saw_brace && brace_depth == 0 {
                    return idx;
                }
                continue;
            }
            TokenKind::Punct(';') | TokenKind::Punct(',')
                if brace_depth == 0 && paren_depth == 0 && bracket_depth == 0 && angle_depth == 0 =>
            {
                return idx + 1;
            }
            _ => {}
        }
        idx += 1;
    }
    idx
}

fn production_tokens(source: &str) -> Vec<Token> {
    let tokens = tokenize_rust_code(source);
    let mut out = Vec::with_capacity(tokens.len());
    let mut i = 0usize;
    while i < tokens.len() {
        if let Some(after_attr) = match_cfg_test_attr(&tokens, i) {
            i = skip_cfg_test_item(&tokens, after_attr);
            continue;
        }
        out.push(tokens[i].clone());
        i += 1;
    }
    out
}

fn find_forbidden_idents(tokens: &[Token], forbidden: &[&str]) -> Vec<(usize, String)> {
    let set: BTreeSet<&str> = forbidden.iter().copied().collect();
    let mut hits = Vec::new();
    for tok in tokens {
        if let TokenKind::Ident(name) = &tok.kind {
            if set.contains(name.as_str()) {
                hits.push((tok.line, name.clone()));
            }
        }
    }
    hits
}

fn find_test_double_structs(tokens: &[Token]) -> Vec<(usize, String)> {
    let mut hits = Vec::new();
    for pair in tokens.windows(2) {
        if let (
            Token {
                kind: TokenKind::Ident(kw),
                line,
            },
            Token {
                kind: TokenKind::Ident(name),
                ..
            },
        ) = (&pair[0], &pair[1])
        {
            if kw == "struct"
                && (name.starts_with("Mock")
                    || name.starts_with("Fake")
                    || name.starts_with("InMemory"))
            {
                hits.push((*line, name.clone()));
            }
        }
    }
    hits
}

#[test]
fn rule_1_dip_no_ui_leakage_into_backend_engine() {
    let root = workspace_root();
    let cli_src = root.join("crates/grein-cli/src");

    let mut targets = Vec::new();
    collect_rs_files(&cli_src.join("engine"), &mut targets);
    targets.push(cli_src.join("daemon.rs"));
    collect_rs_files(&cli_src.join("subagent_watch"), &mut targets);

    assert!(
        targets.len() >= 10,
        "expected backend engine files to scan, found {}",
        targets.len()
    );

    let mut violations = Vec::new();
    for path in targets {
        if is_dedicated_test_submodule(&path) {
            verify_dedicated_test_file_is_cfg_test_gated(&path);
            continue;
        }
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let tokens = production_tokens(&src);
        for (line, ident) in find_forbidden_idents(&tokens, &["grein_tui"]) {
            violations.push(format!("{}:{}: forbidden `{ident}` reference", path.display(), line));
        }
    }

    assert!(
        violations.is_empty(),
        "Rule 1 (DIP / no UI leakage into backend engine) violated:\n{}",
        violations.join("\n")
    );
}

#[test]
fn rule_2_core_domain_purity_no_rusqlite() {
    let root = workspace_root();
    let core_src = root.join("crates/grein-core/src");

    let mut targets = vec![
        core_src.join("model.rs"),
        core_src.join("dag.rs"),
        core_src.join("sync.rs"),
        core_src.join("anchor.rs"),
        core_src.join("blackboard.rs"),
        core_src.join("error.rs"),
    ];
    collect_rs_files(&core_src.join("port"), &mut targets);

    assert!(
        targets.iter().all(|p| p.is_file()),
        "all pure domain files must exist"
    );

    let mut violations = Vec::new();
    for path in targets {
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let tokens = production_tokens(&src);
        for (line, ident) in find_forbidden_idents(&tokens, &["rusqlite"]) {
            violations.push(format!("{}:{}: forbidden `{ident}` reference", path.display(), line));
        }
    }

    assert!(
        violations.is_empty(),
        "Rule 2 (Core Domain Purity) violated:\n{}",
        violations.join("\n")
    );
}

#[test]
fn rule_3_ttc_004_test_double_segregation() {
    let root = workspace_root();
    let files = all_crate_src_files(&root);

    let mut violations = Vec::new();
    for path in files {
        if is_dedicated_test_submodule(&path) {
            verify_dedicated_test_file_is_cfg_test_gated(&path);
            continue;
        }
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let tokens = production_tokens(&src);
        for (line, name) in find_test_double_structs(&tokens) {
            violations.push(format!(
                "{}:{}: test double `struct {name}` found outside #[cfg(test)]",
                path.display(),
                line
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "Rule 3 (TTC-004 Test-Double Segregation) violated:\n{}",
        violations.join("\n")
    );
}

#[test]
fn rule_4_modular_file_size_ceiling_under_1000_lines() {
    let root = workspace_root();
    let files = all_crate_src_files(&root);

    let mut violations = Vec::new();
    for path in files {
        let src = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let line_count = src.lines().count();
        if line_count >= 1000 {
            violations.push(format!(
                "{}: {line_count} lines (must be strictly < 1000)",
                path.display()
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "Rule 4 (Modular File-Size Ceiling < 1,000 lines) violated:\n{}",
        violations.join("\n")
    );
}

#[test]
fn rule_5_thin_entrypoint_main_rs() {
    let root = workspace_root();
    let main_path = root.join("crates/grein-cli/src/main.rs");
    let src = fs::read_to_string(&main_path).expect("read main.rs");

    let line_count = src.lines().count();
    assert!(
        line_count < 100,
        "Rule 5 violated: main.rs is {line_count} lines (must be < 100)"
    );

    let tokens = production_tokens(&src);
    let forbidden = [
        "rusqlite",
        "SqliteStorage",
        "SqliteDagStorage",
        "UserPrefsStore",
        "Connection",
        "InteractivePtySession",
        "UnixPtySpawner",
        "PtyShellExecutor",
        "PtySpawnerPort",
        "openpty",
        "grein_acp",
        "StdioAgentDriverFactory",
        "AcpStdioDriver",
        "AgentDriver",
        "AcpSpawnConfig",
        "InProcessEngine",
        "EngineDependencies",
    ];
    let hits = find_forbidden_idents(&tokens, &forbidden);
    assert!(
        hits.is_empty(),
        "Rule 5 violated: main.rs must not directly call SQLite/PTY/ACP types: {hits:?}"
    );
    assert!(
        src.contains("grein_cli::bootstrap::run_cli"),
        "Rule 5 violated: main.rs must delegate to grein_cli::bootstrap::run_cli"
    );
}

#[test]
fn rule_6_ttc_003_terminal_tier1_hosts_no_concrete_providers() {
    let root = workspace_root();
    let tui_host = root.join("crates/grein-tui/src/terminal/host.rs");
    let tui_registry = root.join("crates/grein-tui/src/terminal/registry.rs");
    assert!(tui_host.is_file(), "missing {}", tui_host.display());
    assert!(tui_registry.is_file(), "missing {}", tui_registry.display());

    let cli_host = root.join("crates/grein-cli/src/terminal/host.rs");
    assert!(cli_host.is_file(), "missing {}", cli_host.display());
    let targets = vec![tui_host, tui_registry, cli_host];

    let forbidden = [
        "providers",
        "GenericXtermProvider",
        "AppleTerminalProvider",
        "ITerm2Provider",
        "GhosttyProvider",
        "KittyProvider",
        "WezTermProvider",
        "AlacrittyProvider",
        "TmuxProvider",
        "ZellijProvider",
        "AppleTerminalInstaller",
        "ITerm2Installer",
        "GhosttyInstaller",
        "KittyInstaller",
        "WezTermInstaller",
        "AlacrittyInstaller",
        "TmuxInstaller",
        "ZellijInstaller",
    ];

    let mut violations = Vec::new();
    for path in targets {
        let src =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let tokens = production_tokens(&src);
        for (line, ident) in find_forbidden_idents(&tokens, &forbidden) {
            violations.push(format!(
                "{}:{}: forbidden `{ident}` reference in Tier 1 terminal host/registry",
                path.display(),
                line
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "Rule 6 (TTC-003 Terminal Tier 1 Hosts No Concrete Providers) violated:\n{}",
        violations.join("\n")
    );
}

#[test]
fn scanner_self_check_distinguishes_production_vs_cfg_test_and_comments() {
    let sample = r#"
        use std::sync::Arc;

        // struct MockCommentOnly { }
        const DOC: &str = "struct FakeInString {} grein_tui::Ignore rusqlite::Ignore";

        pub struct ProductionWatch {
            #[cfg(test)]
            pub(super) state: Arc<std::sync::Mutex<Vec<String>>>,
        }

        pub fn leaked() {
            let _ = grein_tui::ThemePalette::default();
        }

        pub struct InMemoryProdLeak;

        #[cfg(test)]
        mod tests {
            use grein_tui::ThemePalette;
            use rusqlite::Connection;

            struct MockInsideCfgTest;
            struct FakeInsideCfgTest;
            struct InMemoryInsideCfgTest;
        }
    "#;

    let tokens = production_tokens(sample);
    assert_eq!(
        find_forbidden_idents(&tokens, &["grein_tui"]),
        vec![(13, "grein_tui".to_string())]
    );
    assert!(find_forbidden_idents(&tokens, &["rusqlite"]).is_empty());
    assert_eq!(
        find_test_double_structs(&tokens),
        vec![(16, "InMemoryProdLeak".to_string())]
    );
}
