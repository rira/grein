use crate::model::Anchor;

pub struct AnchorParser;

impl AnchorParser {
    /// Extracts primary (Markdown headings) and secondary (titled numbered items) anchors.
    pub fn parse(text: &str) -> Vec<Anchor> {
        let mut anchors = Vec::new();
        for (line_idx, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if let Some(heading) = trimmed.strip_prefix("#### ") {
                anchors.push(Anchor {
                    level: 4,
                    title: heading.to_string(),
                    slug: heading.to_lowercase().replace(' ', "-"),
                    line_number: line_idx + 1,
                });
            } else if let Some(heading) = trimmed.strip_prefix("### ") {
                anchors.push(Anchor {
                    level: 3,
                    title: heading.to_string(),
                    slug: heading.to_lowercase().replace(' ', "-"),
                    line_number: line_idx + 1,
                });
            }
        }
        anchors
    }
}
