use std::ops::Range;

/// The kind of extracted conversational anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnchorKind {
    /// A standard Markdown heading (`#`, `##`, `###`, etc.).
    Heading,
    /// A secondary titled numbered list item.
    NumberedList,
}

/// A conversational anchor parsed from an assistant Markdown turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// Human-readable title of the anchor.
    pub title: String,
    /// Normalized URL/anchor-friendly slug.
    pub slug: String,
    /// Structural nesting level (e.g. 1 for H1, 2 for H2 or secondary items).
    pub level: u32,
    /// Classification kind of the anchor.
    pub kind: AnchorKind,
    /// Zero-based line number where the anchor begins.
    pub line_offset: usize,
    /// Line boundary range for the anchor.
    pub line_range: Range<usize>,
    /// Byte range within the source markdown document.
    pub byte_range: Range<usize>,
}

/// Structural and heuristic Markdown anchor parser.
#[derive(Debug, Default, Clone)]
pub struct AnchorParser;

impl AnchorParser {
    /// Creates a new `AnchorParser` instance.
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Parses assistant Markdown content and extracts structural and heuristic anchors.
    #[must_use]
    pub fn parse(&self, markdown: &str) -> Vec<Anchor> {
        if markdown.trim().is_empty() {
            return Vec::new();
        }

        let mut anchors = Vec::new();
        let mut active_fence: Option<(char, usize)> = None;
        let mut current_byte = 0;
        let mut line_offset = 0;

        for raw_line in markdown.split_inclusive('\n') {
            let line_len = raw_line.len();
            let line_content = raw_line.trim_end_matches(['\r', '\n']);
            let byte_start = current_byte;
            let byte_end = current_byte + line_content.len();
            let byte_range = byte_start..byte_end;
            let line_range = line_offset..line_offset + 1;

            let trimmed = line_content.trim_start();

            // Check for code fence block transitions
            if let Some((fence_char, min_len)) = active_fence {
                let trimmed_close = line_content.trim();
                let count = trimmed_close
                    .chars()
                    .take_while(|&c| c == fence_char)
                    .count();
                if count >= min_len && trimmed_close[count..].trim().is_empty() {
                    active_fence = None;
                }
                current_byte += line_len;
                line_offset += 1;
                continue;
            }

            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                let fence_char = trimmed.chars().next().unwrap_or('`');
                let count = trimmed.chars().take_while(|&c| c == fence_char).count();
                if count >= 3 {
                    active_fence = Some((fence_char, count));
                    current_byte += line_len;
                    line_offset += 1;
                    continue;
                }
            }

            // 1. Markdown Headings (#, ##, ###, ####, etc.)
            if trimmed.starts_with('#') {
                let hash_count = trimmed.chars().take_while(|&c| c == '#').count();
                if (1..=6).contains(&hash_count) {
                    let after_hashes = &trimmed[hash_count..];
                    if after_hashes.is_empty()
                        || after_hashes.starts_with(' ')
                        || after_hashes.starts_with('\t')
                    {
                        let title = after_hashes.trim();
                        if !title.is_empty() {
                            let slug = slugify(title);
                            anchors.push(Anchor {
                                title: title.to_string(),
                                slug,
                                level: hash_count as u32,
                                kind: AnchorKind::Heading,
                                line_offset,
                                line_range,
                                byte_range,
                            });
                            current_byte += line_len;
                            line_offset += 1;
                            continue;
                        }
                    }
                }
            }

            // 2. Titled Numbered List Items
            let digits_len = trimmed.chars().take_while(char::is_ascii_digit).count();
            if digits_len > 0 {
                let after_digits = &trimmed[digits_len..];
                if let Some(after_dot) = after_digits.strip_prefix('.') {
                    // Byte-safe whitespace skipping: slicing happens on the remainder
                    // returned by `trim_start_matches`, never on a char count.
                    let rest =
                        after_dot.trim_start_matches(|c: char| c.is_whitespace() && c != '\n');
                    let ws_len = after_dot.len() - rest.len();
                    if ws_len > 0 {
                        if let Some(title) = extract_numbered_list_title(rest) {
                            let slug = slugify(&title);
                            anchors.push(Anchor {
                                title,
                                slug,
                                level: 2,
                                kind: AnchorKind::NumberedList,
                                line_offset,
                                line_range,
                                byte_range,
                            });
                            current_byte += line_len;
                            line_offset += 1;
                            continue;
                        }
                    }
                }
            }

            current_byte += line_len;
            line_offset += 1;
        }

        anchors
    }
}

/// Extracts title from numbered list items matching `^([0-9]+)\.\s+(\*\*[^*]+\*\*|[^:\n]+:|[^\n]{20,})`.
fn extract_numbered_list_title(rest: &str) -> Option<String> {
    // Branch 1: Bold title `**Bold Title**`
    if let Some(after_open) = rest.strip_prefix("**") {
        if let Some(end_idx) = after_open.find("**") {
            let bold = after_open[..end_idx].trim();
            if !bold.is_empty() {
                return Some(bold.to_string());
            }
        }
    }

    // Branch 2: Title ending in colon `Title:`
    if let Some(colon_pos) = rest.find(':') {
        let candidate = rest[..colon_pos].trim();
        if !candidate.is_empty() && candidate != "http" && candidate != "https" {
            return Some(candidate.to_string());
        }
    }

    // Branch 3: Descriptive item with length >= 20 characters
    let trimmed = rest.trim();
    if trimmed.chars().count() >= 20 {
        return Some(trimmed.to_string());
    }

    None
}

/// Generates an anchor slug by lowercasing (Unicode-aware), normalizing non-alphanumeric runs to `-`, and trimming dashes.
fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut prev_dash = false;

    for c in text.chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    slug
}

/// Convenience function to parse anchors using `AnchorParser::new()`.
#[must_use]
pub fn parse_anchors(markdown: &str) -> Vec<Anchor> {
    AnchorParser::new().parse(markdown)
}
