--- a/crates/grein-core/src/anchor.rs
+++ b/crates/grein-core/src/anchor.rs
@@ -1,4 +1,100 @@
-use crate::model::Anchor;
+use std::ops::Range;
+
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+pub enum AnchorKind {
+    Heading,
+    NumberedItem,
+}
+
+impl AnchorKind {
+    pub fn is_heading(&self) -> bool {
         matches!(self, Self::Heading)
+    }
+
+    pub fn is_numbered_item(&self) -> bool {
+        matches!(self, Self::NumberedItem)
+    }
+}
+
+#[derive(Debug, Clone, PartialEq, Eq)]
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
+    pub fn byte_range(&self) -> Range<usize> {
+        self.position.byte_range.clone()
+    }
+
+    pub fn calculate_slug(title: &str) -> String {
+        calculate_slug(title)
+    }
+
+    pub fn slug(title: &str) -> String {
+        calculate_slug(title)
+    }
+}
+
+pub fn calculate_slug(title: &str) -> String {
+    let mut slug = String::new();
+    let mut prev_dash = false;
+    for c in title.chars() {
+        if c.is_alphanumeric() {
+            slug.extend(c.to_lowercase());
+            prev_dash = false;
+        } else if (c == ' ' || c == '-' || c == '_') && !slug.is_empty() && !prev_dash {
+            slug.push('-');
+            prev_dash = true;
+        }
+    }
+    if slug.ends_with('-') {
+        slug.pop();
+    }
+    slug
+}
+
+pub fn slugify(title: &str) -> String {
+    calculate_slug(title)
+}
 
 pub struct AnchorParser;
 
@@ -6,24 +102,56 @@
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
-            }
+        let mut byte_offset = 0;
+        let mut in_code_block = false;
+
+        for (line_idx, line) in text.split('\n').enumerate() {
+            let line_len = line.len();
+            let clean_line = line.strip_suffix('\r').unwrap_or(line);
+            let line_start = byte_offset;
+            let line_end = line_start + clean_line.len();
+            byte_offset += line_len + 1;
+
+            let trimmed = clean_line.trim();
+
+            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
+                in_code_block = !in_code_block;
+                continue;
+            }
+
+            if in_code_block {
+                continue;
+            }
+
+            let heading_info = if let Some(rest) = trimmed.strip_prefix("#### ") {
+                Some((4, rest))
+            } else if let Some(rest) = trimmed.strip_prefix("### ") {
+                Some((3, rest))
+            } else if let Some(rest) = trimmed.strip_prefix("## ") {
+                Some((2, rest))
+            } else if let Some(rest) = trimmed.strip_prefix("# ") {
+                Some((1, rest))
+            } else {
+                None
+            };
+
+            if let Some((level, raw_title)) = heading_info {
+                let title = raw_title.trim_end_matches('#').trim().to_string();
+                if !title.is_empty() {
+                    let slug = calculate_slug(&title);
+                    anchors.push(Anchor {
+                        kind: AnchorKind::Heading,
+                        level,
+                        title,
+                        slug,
+                        position: AnchorPosition {
+                            line_offset: line_idx,
+                            byte_range: line_start..line_end,
+                        },
+                    });
+                }
+            }
         }
         anchors
     }
 }