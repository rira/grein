--- a/crates/grein-core/src/anchor.rs
+++ b/crates/grein-core/src/anchor.rs
@@ -1,29 +1,185 @@
-use crate::model::Anchor;
+use std::ops::Range;
+
+#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
+pub enum AnchorKind {
+    Heading,
+    NumberedList,
+}
+
+impl AnchorKind {
+    pub fn is_heading(&self) -> bool {
+        matches!(self, Self::Heading)
+    }
+
+    pub fn is_numbered_list(&self) -> bool {
+        matches!(self, Self::NumberedList)
+    }
+}
+
+#[derive(Debug, Clone, PartialEq, Eq, Hash)]
+pub struct AnchorPosition {
+    pub line_offset: usize,
+    pub byte_range: Range<usize>,
+}
+
+impl AnchorPosition {
+    pub fn new(line_offset: usize, byte_range: Range<usize>) -> Self {
+        Self {
+            line_offset,
+            byte_range,
+        }
+    }
+}
+
+#[derive(Debug, Clone, PartialEq, Eq)]
+pub struct Anchor {
+    pub kind: AnchorKind,
+    pub level: usize,
+    pub title: String,
+    pub slug: String,
+    pub position: AnchorPosition,
+}
+
+impl Anchor {
+    pub fn new(
+        kind: AnchorKind,
+        level: usize,
+        title: impl Into<String>,
+        slug: impl Into<String>,
+        position: AnchorPosition,
+    ) -> Self {
+        Self {
+            kind,
+            level,
+            title: title.into(),
+            slug: slug.into(),
+            position,
+        }
+    }
+
+    pub fn line_offset(&self) -> usize {
+        self.position.line_offset
+    }
+
+    pub fn line_number(&self) -> usize {
+        self.position.line_offset + 1
+    }
+
+    pub fn byte_range(&self) -> &Range<usize> {
+        &self.position.byte_range
+    }
+
+    pub fn calculate_slug(title: &str) -> String {
+        calculate_slug(title)
+    }
+
+    pub fn slug_for(title: &str) -> String {
+        calculate_slug(title)
+    }
+}
+
+/// Computes a URL-friendly slug from an anchor title.
+pub fn calculate_slug(title: &str) -> String {
+    let mut slug = String::new();
+    let mut prev_dash = false;
+
+    for c in title.chars() {
+        if c.is_alphanumeric() {
+            slug.push(c.to_ascii_lowercase());
+            prev_dash = false;
+        } else if (c == ' ' || c == '-' || c == '_') && !prev_dash && !slug.is_empty() {
+            slug.push('-');
+            prev_dash = true;
+        }
+    }
+
+    if slug.ends_with('-') {
+        slug.pop();
+    }
+
+    if slug.is_empty() {
+        title.to_lowercase().replace(' ', "-")
+    } else {
+        slug
+    }
+}
 
 pub struct AnchorParser;
 
 impl AnchorParser {
     /// Extracts primary (Markdown headings) and secondary (titled numbered items) anchors.
     pub fn parse(text: &str) -> Vec<Anchor> {
         let mut anchors = Vec::new();
-        for (line_idx, line) in text.lines().enumerate() {
-            let trimmed = line.trim();
-            if let Some(heading) = trimmed.strip_prefix("#### ") {
-                anchors.push(Anchor {
-                    level: 4,
-                    title: heading.to_string(),
-                    slug: heading.to_lowercase().replace(' ', "-"),
-                    line_number: line_idx + 1,
-                });
-            } else if let Some(heading) = trimmed.strip_prefix("### ") {
-                anchors.push(Anchor {
-                    level: 3,
-                    title: heading.to_string(),
-                    slug: heading.to_lowercase().replace(' ', "-"),
-                    line_number: line_idx + 1,
-                });
+        let mut byte_offset = 0;
+        let mut in_code_block = false;
+
+        for (line_idx, line) in text.split_inclusive('\n').enumerate() {
+            let line_len = line.len();
+            let line_trimmed = line.trim_end_matches(&['\r', '\n'][..]);
+            let trimmed = line_trimmed.trim_start();
+            let leading_spaces = line_trimmed.len() - trimmed.len();
+
+            // Ignore fenced code blocks to prevent false positive anchors from comments
+            if (trimmed.starts_with("```") || trimmed.starts_with("~~~")) && leading_spaces <= 3 {
+                in_code_block = !in_code_block;
+                byte_offset += line_len;
+                continue;
+            }
+
+            if in_code_block {
+                byte_offset += line_len;
+                continue;
+            }
+
+            // Headings allow up to 3 leading spaces in Markdown
+            if leading_spaces <= 3 {
+                let heading_opt = if let Some(heading) = trimmed.strip_prefix("#### ") {
+                    Some((4, heading))
+                } else if let Some(heading) = trimmed.strip_prefix("### ") {
+                    Some((3, heading))
+                } else if let Some(heading) = trimmed.strip_prefix("## ") {
+                    Some((2, heading))
+                } else if let Some(heading) = trimmed.strip_prefix("# ") {
+                    Some((1, heading))
+                } else {
+                    None
+                };
+
+                if let Some((level, raw_heading)) = heading_opt {
+                    let title = Self::clean_heading_title(raw_heading);
+                    let slug = calculate_slug(&title);
+                    let start_byte = byte_offset;
+                    let end_byte = byte_offset + line_trimmed.len();
+
+                    anchors.push(Anchor {
+                        kind: AnchorKind::Heading,
+                        level,
+                        title,
+                        slug,
+                        position: AnchorPosition {
+                            line_offset: line_idx,
+                            byte_range: start_byte..end_byte,
+                        },
+                    });
+                }
             }
+
+            byte_offset += line_len;
         }
         anchors
     }
+
+    fn clean_heading_title(raw: &str) -> String {
+        let trimmed = raw.trim();
+        if let Some(stripped) = trimmed.strip_suffix('#') {
+            let stripped = stripped.trim_end_matches('#');
+            if stripped.ends_with(' ') {
+                stripped.trim().to_string()
+            } else {
+                trimmed.to_string()
+            }
+        } else {
+            trimmed.to_string()
+        }
+    }
 }