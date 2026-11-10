use grein_core::anchor::{parse_anchors, AnchorKind, AnchorParser};

#[test]
fn test_extract_markdown_headings_all_levels() {
    let markdown = "# Document Title\n\nIntro paragraph.\n\n## Architecture Overview\n\n### Component Details\n\n#### Invariants & Rules\n";
    let parser = AnchorParser::new();
    let anchors = parser.parse(markdown);

    assert_eq!(anchors.len(), 4);

    // H1
    assert_eq!(anchors[0].title, "Document Title");
    assert_eq!(anchors[0].slug, "document-title");
    assert_eq!(anchors[0].level, 1);
    assert_eq!(anchors[0].kind, AnchorKind::Heading);
    assert_eq!(anchors[0].line_offset, 0);
    assert_eq!(anchors[0].line_range.start, 0);
    assert!(anchors[0].byte_range.start < anchors[0].byte_range.end);
    assert!(&markdown[anchors[0].byte_range.clone()].contains("Document Title"));

    // H2
    assert_eq!(anchors[1].title, "Architecture Overview");
    assert_eq!(anchors[1].slug, "architecture-overview");
    assert_eq!(anchors[1].level, 2);
    assert_eq!(anchors[1].kind, AnchorKind::Heading);
    assert_eq!(anchors[1].line_offset, 4);
    assert_eq!(anchors[1].line_range.start, 4);
    assert!(&markdown[anchors[1].byte_range.clone()].contains("Architecture Overview"));

    // H3
    assert_eq!(anchors[2].title, "Component Details");
    assert_eq!(anchors[2].slug, "component-details");
    assert_eq!(anchors[2].level, 3);
    assert_eq!(anchors[2].kind, AnchorKind::Heading);
    assert_eq!(anchors[2].line_offset, 6);

    // H4
    assert_eq!(anchors[3].title, "Invariants & Rules");
    assert_eq!(anchors[3].slug, "invariants-rules");
    assert_eq!(anchors[3].level, 4);
    assert_eq!(anchors[3].kind, AnchorKind::Heading);
    assert_eq!(anchors[3].line_offset, 8);
}

#[test]
fn test_extract_titled_numbered_list_items() {
    let markdown = r#"# Execution Plan

1. **Database Schema**: Setup PostgreSQL tables and indexes.
2. Architecture Boundary: Enforce Universal Two-Tier component boundaries.
3. This is a very long descriptive numbered item with at least twenty characters explaining the implementation.
"#;

    let anchors = parse_anchors(markdown);

    assert_eq!(anchors.len(), 4);
    assert_eq!(anchors[0].kind, AnchorKind::Heading);
    assert_eq!(anchors[0].title, "Execution Plan");

    // Branch 1: **Bold Title**
    let bold_anchor = &anchors[1];
    assert_eq!(bold_anchor.kind, AnchorKind::NumberedList);
    assert!(bold_anchor.title.contains("Database Schema"));
    assert!(bold_anchor.slug.contains("database-schema"));
    assert_eq!(bold_anchor.line_offset, 2);
    assert!(bold_anchor.level >= 2);
    assert!(&markdown[bold_anchor.byte_range.clone()].contains("Database Schema"));

    // Branch 2: Title:
    let colon_anchor = &anchors[2];
    assert_eq!(colon_anchor.kind, AnchorKind::NumberedList);
    assert!(colon_anchor.title.contains("Architecture Boundary"));
    assert!(colon_anchor.slug.contains("architecture-boundary"));
    assert_eq!(colon_anchor.line_offset, 3);
    assert!(colon_anchor.level >= 2);
    assert!(&markdown[colon_anchor.byte_range.clone()].contains("Architecture Boundary"));

    // Branch 3: Length >= 20 characters
    let long_anchor = &anchors[3];
    assert_eq!(long_anchor.kind, AnchorKind::NumberedList);
    assert!(long_anchor
        .title
        .contains("very long descriptive numbered item"));
    assert_eq!(long_anchor.line_offset, 4);
    assert!(long_anchor.level >= 2);
}

#[test]
fn test_noise_filter_rejects_short_checklists() {
    let markdown = r#"# Action Items

Here is what we need to run:
1. git pull
2. cargo test
3. cargo clippy
4. done
5. ok
"#;

    let anchors = parse_anchors(markdown);

    // Only the H1 heading should survive noise filtration
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].title, "Action Items");
    assert_eq!(anchors[0].kind, AnchorKind::Heading);

    for rejected in &["git pull", "cargo test", "cargo clippy", "done", "ok"] {
        assert!(
            anchors.iter().all(|a| !a.title.contains(rejected)),
            "Short checklist item '{}' was not filtered out",
            rejected
        );
    }
}

#[test]
fn test_ignores_headings_and_lists_inside_code_blocks() {
    let markdown = r#"# Real Section Heading

```rust
// This should NOT be an anchor
# Simulated Heading
1. **Fake Title**: Do not parse
2. cargo test
```

~~~markdown
### Another Fake Heading
1. Also ignored inside tilde block
~~~

## Second Real Heading
"#;

    let parser = AnchorParser::new();
    let anchors = parser.parse(markdown);

    assert_eq!(anchors.len(), 2);
    assert_eq!(anchors[0].title, "Real Section Heading");
    assert_eq!(anchors[0].kind, AnchorKind::Heading);
    assert_eq!(anchors[0].line_offset, 0);

    assert_eq!(anchors[1].title, "Second Real Heading");
    assert_eq!(anchors[1].kind, AnchorKind::Heading);
    assert_eq!(anchors[1].line_offset, 14);
}

#[test]
fn test_byte_ranges_and_line_boundaries_for_cursor_positioning() {
    let markdown = "Line 0\n# Header One\nLine 2\n\n## Header Two\nLine 5";
    let anchors = parse_anchors(markdown);

    assert_eq!(anchors.len(), 2);

    let h1 = &anchors[0];
    assert_eq!(h1.title, "Header One");
    assert_eq!(h1.line_offset, 1);
    assert_eq!(h1.line_range.start, 1);
    assert!(h1.byte_range.start < h1.byte_range.end);
    assert_eq!(&markdown[h1.byte_range.clone()], "# Header One");

    let h2 = &anchors[1];
    assert_eq!(h2.title, "Header Two");
    assert_eq!(h2.line_offset, 4);
    assert_eq!(h2.line_range.start, 4);
    assert!(h2.byte_range.start < h2.byte_range.end);
    assert_eq!(&markdown[h2.byte_range.clone()], "## Header Two");
}

#[test]
fn test_slug_generation_and_normalization() {
    let markdown = "### Complex: Title (With Punctuation & Symbols)!";
    let anchors = parse_anchors(markdown);

    assert_eq!(anchors.len(), 1);
    assert_eq!(
        anchors[0].title,
        "Complex: Title (With Punctuation & Symbols)!"
    );
    assert_eq!(anchors[0].slug, "complex-title-with-punctuation-symbols");
}

#[test]
fn test_empty_and_no_anchor_markdown() {
    assert!(parse_anchors("").is_empty());
    assert!(parse_anchors("   \n\t\n  ").is_empty());

    let plain_text = "This is a simple paragraph with no headings or numbered lists.\nJust plain narrative text.";
    assert!(parse_anchors(plain_text).is_empty());
}

#[test]
fn test_coding_agent_markdown_turn_comprehensive() {
    let markdown = r#"# Architectural Analysis & Implementation Plan

We need to implement the structural and heuristic anchor extraction engine.

## 1. Domain Invariants
We must adhere to ADR 0031 and ADR 0007:
1. **Zero-Null Policy**: Null values are strictly eradicated from the codebase.
2. **Universal Two-Tier Architecture**: Tier 1 domain host and Tier 2 pluggable leaf edge providers.
3. Component Interfaces: All components must expose explicit public interface contracts.

## 2. Noise Filtering Implementation
Here is how we reject noise in checklists:
```rust
fn check_noise() {
    // # Not a real heading
    1. git pull
}
```

Verification checklist:
1. git fetch
2. cargo check
3. cargo test

### Verification & Completion
All invariants verified successfully.
"#;

    let parser = AnchorParser::new();
    let anchors = parser.parse(markdown);

    assert_eq!(anchors.len(), 7);

    assert_eq!(
        anchors[0].title,
        "Architectural Analysis & Implementation Plan"
    );
    assert_eq!(anchors[0].kind, AnchorKind::Heading);
    assert_eq!(anchors[0].level, 1);

    assert_eq!(anchors[1].title, "1. Domain Invariants");
    assert_eq!(anchors[1].kind, AnchorKind::Heading);
    assert_eq!(anchors[1].level, 2);

    assert_eq!(anchors[2].kind, AnchorKind::NumberedList);
    assert!(anchors[2].title.contains("Zero-Null Policy"));

    assert_eq!(anchors[3].kind, AnchorKind::NumberedList);
    assert!(anchors[3].title.contains("Universal Two-Tier Architecture"));

    assert_eq!(anchors[4].kind, AnchorKind::NumberedList);
    assert!(anchors[4].title.contains("Component Interfaces"));

    assert_eq!(anchors[5].title, "2. Noise Filtering Implementation");
    assert_eq!(anchors[5].kind, AnchorKind::Heading);
    assert_eq!(anchors[5].level, 2);

    assert_eq!(anchors[6].title, "Verification & Completion");
    assert_eq!(anchors[6].kind, AnchorKind::Heading);
    assert_eq!(anchors[6].level, 3);

    // Verify noise filter rejected the short checklist at the end
    assert!(anchors.iter().all(|a| !a.title.contains("git fetch")));
    assert!(anchors.iter().all(|a| !a.title.contains("cargo check")));
    assert!(anchors.iter().all(|a| !a.title.contains("cargo test")));
}
