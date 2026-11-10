use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnchorKind {
    Heading(u8),
    NumberedItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnchorPosition {
    pub line_offset: usize,
    pub byte_range: Range<usize>,
}

impl AnchorPosition {
    pub fn new(line_offset: usize, byte_range: Range<usize>) -> Self {
        Self {
            line_offset,
            byte_range,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub title: String,
    pub slug: String,
    pub kind: AnchorKind,
    pub position: AnchorPosition,
}

pub fn calculate_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;

    for ch in title.trim().chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                slug.push(lower);
            }
            prev_dash = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }

    while slug.ends_with('-') {
        slug.pop();
    }

    slug
}

pub fn extract_anchors(markdown: &str) -> Vec<Anchor> {
    let mut anchors = Vec::new();
    let mut in_code_block = false;
    let mut byte_offset = 0;

    for (line_offset, line) in markdown.split('\n').enumerate() {
        let line_len = line.len();
        let line_content = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line_content.trim();

        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            byte_offset += line_len + 1;
            continue;
        }

        if in_code_block || trimmed.is_empty() {
            byte_offset += line_len + 1;
            continue;
        }

        if trimmed.starts_with('#') {
            let hash_count = trimmed.chars().take_while(|&c| c == '#').count();
            let after_hashes = &trimmed[hash_count..];

            if (1..=4).contains(&hash_count)
                && (after_hashes.starts_with(' ') || after_hashes.starts_with('\t'))
            {
                let heading_text = after_hashes.trim().trim_end_matches('#').trim();
                if !heading_text.is_empty() {
                    let title = heading_text.to_string();
                    let slug = calculate_slug(&title);
                    let line_start = byte_offset;
                    let line_end = byte_offset + line_content.len();

                    anchors.push(Anchor {
                        title,
                        slug,
                        kind: AnchorKind::Heading(hash_count as u8),
                        position: AnchorPosition {
                            line_offset,
                            byte_range: line_start..line_end,
                        },
                    });

                    byte_offset += line_len + 1;
                    continue;
                }
            }
        }

        let digit_count = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
        if digit_count > 0 {
            let after_digits = &trimmed[digit_count..];
            if let Some(after_dot) = after_digits.strip_prefix('.') {
                if after_dot.starts_with(' ') || after_dot.starts_with('\t') {
                    let content = after_dot.trim_start();
                    let matched_title = if let Some(rest) = content.strip_prefix("**") {
                        rest.find("**").map(|end| rest[..end].trim())
                    } else if let Some(colon_pos) = content.find(':') {
                        Some(content[..colon_pos].trim())
                    } else if content.len() >= 20 {
                        Some(content.trim())
                    } else {
                        None
                    };

                    if let Some(raw_title) = matched_title {
                        let title = raw_title
                            .trim_matches('*')
                            .trim_end_matches(':')
                            .trim()
                            .to_string();

                        let is_trivial = (title.starts_with("git ")
                            || title.starts_with("cargo ")
                            || title.starts_with("npm ")
                            || title == "ls -la")
                            && !content.starts_with("**")
                            && !content.contains(':');

                        if !title.is_empty() && !is_trivial {
                            let slug = calculate_slug(&title);
                            let line_start = byte_offset;
                            let line_end = byte_offset + line_content.len();

                            anchors.push(Anchor {
                                title,
                                slug,
                                kind: AnchorKind::NumberedItem,
                                position: AnchorPosition {
                                    line_offset,
                                    byte_range: line_start..line_end,
                                },
                            });
                        }
                    }
                }
            }
        }

        byte_offset += line_len + 1;
    }

    anchors
}

pub struct AnchorParser;

impl AnchorParser {
    pub fn parse(text: &str) -> Vec<Anchor> {
        extract_anchors(text)
    }
}
