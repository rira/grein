use grein_core::anchor::{calculate_slug, extract_anchors, Anchor, AnchorKind, AnchorPosition};

#[test]
fn test_anchor_struct_and_position_construction() {
    let pos = AnchorPosition::new(0, 0..10);
    assert_eq!(pos.line_offset, 0);
    assert_eq!(pos.byte_range, 0..10);
    let anchor = Anchor {
        title: "Test".to_string(),
        slug: "test".to_string(),
        kind: AnchorKind::Heading(1),
        position: pos,
    };
    assert_eq!(anchor.title, "Test");
}

#[test]
fn test_extract_markdown_headings_levels_1_to_4() {
    let markdown = "# Level 1 Heading\nSome text.\n## Level 2 Heading\nText.\n### Level 3 Heading\nText.\n#### Level 4 Heading\nText.\n##### Level 5 Ignored\n###### Level 6 Ignored\n";
    let anchors = extract_anchors(markdown);
    assert_eq!(anchors.len(), 4);

    assert_eq!(anchors[0].title, "Level 1 Heading");
    assert_eq!(anchors[0].slug, "level-1-heading");
    assert_eq!(anchors[0].kind, AnchorKind::Heading(1));
    assert_eq!(anchors[0].position.line_offset, 0);

    assert_eq!(anchors[1].title, "Level 2 Heading");
    assert_eq!(anchors[1].slug, "level-2-heading");
    assert_eq!(anchors[1].kind, AnchorKind::Heading(2));
    assert_eq!(anchors[1].position.line_offset, 2);

    assert_eq!(anchors[2].title, "Level 3 Heading");
    assert_eq!(anchors[2].slug, "level-3-heading");
    assert_eq!(anchors[2].kind, AnchorKind::Heading(3));
    assert_eq!(anchors[2].position.line_offset, 4);

    assert_eq!(anchors[3].title, "Level 4 Heading");
    assert_eq!(anchors[3].slug, "level-4-heading");
    assert_eq!(anchors[3].kind, AnchorKind::Heading(4));
    assert_eq!(anchors[3].position.line_offset, 6);
}

#[test]
fn test_extract_heading_byte_range_and_cursor_positioning() {
    let markdown = "# First\nBody text\n## Second\n";
    let anchors = extract_anchors(markdown);
    assert_eq!(anchors.len(), 2);

    let first = &anchors[0];
    assert_eq!(first.position.line_offset, 0);
    assert_eq!(&markdown[first.position.byte_range.clone()], "# First");

    let second = &anchors[1];
    assert_eq!(second.position.line_offset, 2);
    let expected_start = markdown.find("## Second").unwrap();
    let expected_end = expected_start + "## Second".len();
    assert_eq!(second.position.byte_range, expected_start..expected_end);
    assert_eq!(&markdown[second.position.byte_range.clone()], "## Second");
}

#[test]
fn test_headings_inside_code_fences_are_ignored() {
    let markdown = "\
# Real Heading

```bash
# This is a bash comment
cargo build
## Not a heading either
```

```python
# Python comment
```

## Another Real Heading
";
    let anchors = extract_anchors(markdown);
    assert_eq!(anchors.len(), 2);
    assert_eq!(anchors[0].title, "Real Heading");
    assert_eq!(anchors[0].kind, AnchorKind::Heading(1));
    assert_eq!(anchors[1].title, "Another Real Heading");
    assert_eq!(anchors[1].kind, AnchorKind::Heading(2));
}

#[test]
fn test_titled_numbered_list_items_extraction() {
    let markdown = "\
1. **Database Schema**: Set up PostgreSQL migrations and tables
2. Architecture Overview: Define component boundaries and interfaces
3. Implementing the complete validation accumulator pipeline across layers
";
    let anchors = extract_anchors(markdown);
    assert_eq!(anchors.len(), 3);

    assert!(anchors[0].title.contains("Database Schema"));
    assert_eq!(anchors[0].kind, AnchorKind::NumberedItem);
    assert_eq!(anchors[0].slug, "database-schema");
    assert_eq!(anchors[0].position.line_offset, 0);

    assert!(anchors[1].title.contains("Architecture Overview"));
    assert_eq!(anchors[1].kind, AnchorKind::NumberedItem);
    assert_eq!(anchors[1].slug, "architecture-overview");
    assert_eq!(anchors[1].position.line_offset, 1);

    assert_eq!(anchors[2].kind, AnchorKind::NumberedItem);
    assert_eq!(anchors[2].position.line_offset, 2);
}

#[test]
fn test_heuristic_filters_trivial_short_checklists() {
    let markdown = "\
Here are the steps:
1. git pull
2. cargo test
3. npm run build
4. ls -la
";
    let anchors = extract_anchors(markdown);
    assert!(
        anchors.is_empty(),
        "Short checklist items must be filtered out as anchors"
    );
}

#[test]
fn test_realistic_coding_agent_turn_markdown() {
    let markdown = "\
# Summary of Changes

I have completed the implementation of the requested service.

## Implementation Details

The following components were updated:

1. **Storage Adapter**: Added durable atomic writes.
2. **Event Dispatcher**: Wired channel subscriptions.

Follow these verification commands:
1. cargo check
2. cargo test

### Verification Results

All 15 tests passed successfully.
";
    let anchors = extract_anchors(markdown);
    assert_eq!(anchors.len(), 5);

    assert_eq!(anchors[0].title, "Summary of Changes");
    assert_eq!(anchors[0].kind, AnchorKind::Heading(1));

    assert_eq!(anchors[1].title, "Implementation Details");
    assert_eq!(anchors[1].kind, AnchorKind::Heading(2));

    assert!(anchors[2].title.contains("Storage Adapter"));
    assert_eq!(anchors[2].kind, AnchorKind::NumberedItem);

    assert!(anchors[3].title.contains("Event Dispatcher"));
    assert_eq!(anchors[3].kind, AnchorKind::NumberedItem);

    assert_eq!(anchors[4].title, "Verification Results");
    assert_eq!(anchors[4].kind, AnchorKind::Heading(3));
}

#[test]
fn test_empty_and_no_anchors() {
    assert!(extract_anchors("").is_empty());
    assert!(extract_anchors("   \n\n\t  \n").is_empty());
    assert!(
        extract_anchors("Just a normal paragraph without any headings or numbered lists.")
            .is_empty()
    );
}

#[test]
fn test_slug_calculation() {
    assert_eq!(calculate_slug("Introduction"), "introduction");
    assert_eq!(calculate_slug("System Architecture"), "system-architecture");
    assert_eq!(calculate_slug("**Database Schema**:"), "database-schema");
    assert_eq!(
        calculate_slug("Features & Invariants (v2.0)"),
        "features-invariants-v20"
    );
    assert_eq!(
        calculate_slug("   Leading and Trailing Spaces   "),
        "leading-and-trailing-spaces"
    );
    assert_eq!(
        calculate_slug("Multiple   Spaces   and---Hyphens"),
        "multiple-spaces-and-hyphens"
    );
}
