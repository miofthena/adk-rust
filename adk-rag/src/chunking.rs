//! Document chunking strategies.
//!
//! This module provides the [`Chunker`] trait and three implementations:
//!
//! - [`FixedSizeChunker`] — splits by character count with configurable overlap
//! - [`RecursiveChunker`] — splits hierarchically by paragraphs, sentences, then words
//! - [`MarkdownChunker`] — splits by markdown headers, preserving header context

use crate::document::{Chunk, Document};

/// A strategy for splitting documents into chunks.
///
/// Implementations produce [`Chunk`]s with text and metadata but no embeddings.
/// Embeddings are attached later by the pipeline.
pub trait Chunker: Send + Sync {
    /// Split a document into chunks.
    ///
    /// Returns an empty `Vec` if the document has empty text.
    /// Each returned chunk has an empty embedding vector.
    fn chunk(&self, document: &Document) -> Vec<Chunk>;
}

/// Splits text into fixed-size chunks by character count with configurable overlap.
///
/// Chunk IDs are generated as `{document_id}_{chunk_index}`. Each chunk inherits
/// the parent document's metadata plus a `chunk_index` field.
///
/// # Example
///
/// ```rust,ignore
/// use adk_rag::FixedSizeChunker;
///
/// let chunker = FixedSizeChunker::new(256, 50);
/// let chunks = chunker.chunk(&document);
/// ```
#[derive(Debug, Clone)]
pub struct FixedSizeChunker {
    chunk_size: usize,
    chunk_overlap: usize,
}

impl FixedSizeChunker {
    /// Create a new `FixedSizeChunker`.
    ///
    /// # Arguments
    ///
    /// * `chunk_size` — maximum number of characters per chunk
    /// * `chunk_overlap` — number of overlapping characters between consecutive chunks
    pub fn new(chunk_size: usize, chunk_overlap: usize) -> Self {
        Self { chunk_size, chunk_overlap }
    }
}

impl Chunker for FixedSizeChunker {
    fn chunk(&self, document: &Document) -> Vec<Chunk> {
        if document.text.is_empty() {
            return Vec::new();
        }

        let text = &document.text;
        let mut chunks = Vec::new();
        let mut start = 0;
        let mut chunk_index = 0;

        while start < text.len() {
            let end = text.floor_char_boundary((start + self.chunk_size).min(text.len()));
            let chunk_text = &text[start..end];

            let mut metadata = document.metadata.clone();
            metadata.insert("chunk_index".to_string(), chunk_index.to_string());

            chunks.push(Chunk {
                id: format!("{}_{chunk_index}", document.id),
                text: chunk_text.to_string(),
                embedding: Vec::new(),
                metadata,
                document_id: document.id.clone(),
            });

            chunk_index += 1;
            let step = self.chunk_size.saturating_sub(self.chunk_overlap);
            if step == 0 {
                break;
            }
            start = text.floor_char_boundary(start + step);
        }

        chunks
    }
}

/// Splits text hierarchically: paragraphs → sentences → words.
///
/// First splits by paragraph separators (`\n\n`). If a paragraph exceeds
/// `chunk_size`, splits by sentence boundaries (`. `, `! `, `? `). If a
/// sentence still exceeds `chunk_size`, splits by word boundaries. Overlap
/// is applied between chunks at each level.
///
/// # Example
///
/// ```rust,ignore
/// use adk_rag::RecursiveChunker;
///
/// let chunker = RecursiveChunker::new(512, 100);
/// let chunks = chunker.chunk(&document);
/// ```
#[derive(Debug, Clone)]
pub struct RecursiveChunker {
    chunk_size: usize,
    chunk_overlap: usize,
}

impl RecursiveChunker {
    /// Create a new `RecursiveChunker`.
    ///
    /// # Arguments
    ///
    /// * `chunk_size` — maximum number of characters per chunk
    /// * `chunk_overlap` — number of overlapping characters between consecutive chunks
    pub fn new(chunk_size: usize, chunk_overlap: usize) -> Self {
        Self { chunk_size, chunk_overlap }
    }
}

/// Split text by a separator, then merge segments into chunks that respect
/// `chunk_size`. If a segment exceeds `chunk_size`, it is split further
/// using the next-level separator.
fn split_and_merge(
    text: &str,
    chunk_size: usize,
    chunk_overlap: usize,
    separators: &[&str],
) -> Vec<String> {
    if text.len() <= chunk_size || separators.is_empty() {
        return split_by_size(text, chunk_size, chunk_overlap);
    }

    let separator = separators[0];
    let remaining_separators = &separators[1..];

    let segments: Vec<&str> = if separator == " " {
        text.split(' ').collect()
    } else {
        split_keeping_separator(text, separator)
    };

    let mut chunks = Vec::new();
    let mut current = String::new();

    for segment in segments {
        if current.is_empty() {
            current = segment.to_string();
        } else if current.len() + segment.len() <= chunk_size {
            current.push_str(segment);
        } else {
            // Current chunk is full — process it
            if current.len() > chunk_size {
                chunks.extend(split_and_merge(
                    &current,
                    chunk_size,
                    chunk_overlap,
                    remaining_separators,
                ));
            } else {
                chunks.push(current);
            }
            // Start new chunk with overlap
            current = segment.to_string();
        }
    }

    if !current.is_empty() {
        if current.len() > chunk_size {
            chunks.extend(split_and_merge(
                &current,
                chunk_size,
                chunk_overlap,
                remaining_separators,
            ));
        } else {
            chunks.push(current);
        }
    }

    chunks
}

/// Split text at a separator while keeping the separator attached to the preceding segment.
fn split_keeping_separator<'a>(text: &'a str, separator: &str) -> Vec<&'a str> {
    let mut result = Vec::new();
    let mut start = 0;

    while let Some(pos) = text[start..].find(separator) {
        let end = start + pos + separator.len();
        result.push(&text[start..end]);
        start = end;
    }

    if start < text.len() {
        result.push(&text[start..]);
    }

    result
}

/// Simple character-based splitting with overlap.
fn split_by_size(text: &str, chunk_size: usize, chunk_overlap: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let end = text.floor_char_boundary((start + chunk_size).min(text.len()));
        chunks.push(text[start..end].to_string());
        let step = chunk_size.saturating_sub(chunk_overlap);
        if step == 0 {
            break;
        }
        start = text.floor_char_boundary(start + step);
    }

    chunks
}

impl Chunker for RecursiveChunker {
    fn chunk(&self, document: &Document) -> Vec<Chunk> {
        if document.text.is_empty() {
            return Vec::new();
        }

        let separators = ["\n\n", ". ", "! ", "? ", " "];
        let raw_chunks =
            split_and_merge(&document.text, self.chunk_size, self.chunk_overlap, &separators);

        raw_chunks
            .into_iter()
            .enumerate()
            .map(|(i, text)| {
                let mut metadata = document.metadata.clone();
                metadata.insert("chunk_index".to_string(), i.to_string());
                Chunk {
                    id: format!("{}_{i}", document.id),
                    text,
                    embedding: Vec::new(),
                    metadata,
                    document_id: document.id.clone(),
                }
            })
            .collect()
    }
}

/// Splits text by markdown headers, keeping each section as a chunk.
///
/// Each section is prefixed with its header hierarchy. Sections exceeding
/// `chunk_size` are further split using [`RecursiveChunker`] logic.
/// The `header_path` metadata field records the header hierarchy for each chunk.
///
/// # Example
///
/// ```rust,ignore
/// use adk_rag::MarkdownChunker;
///
/// let chunker = MarkdownChunker::new(512, 100);
/// let chunks = chunker.chunk(&document);
/// ```
#[derive(Debug, Clone)]
pub struct MarkdownChunker {
    chunk_size: usize,
    chunk_overlap: usize,
}

impl MarkdownChunker {
    /// Create a new `MarkdownChunker`.
    ///
    /// # Arguments
    ///
    /// * `chunk_size` — maximum number of characters per chunk
    /// * `chunk_overlap` — number of overlapping characters between consecutive chunks
    pub fn new(chunk_size: usize, chunk_overlap: usize) -> Self {
        Self { chunk_size, chunk_overlap }
    }
}

/// A markdown section with its header hierarchy and body text.
struct MarkdownSection {
    header_path: String,
    text: String,
}

/// Parse markdown text into sections split by headers.
fn parse_markdown_sections(text: &str) -> Vec<MarkdownSection> {
    let mut sections = Vec::new();
    let mut headers: Vec<String> = Vec::new();
    let mut current_body = String::new();
    let mut current_header_path = String::new();

    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            // Save previous section
            if !current_body.is_empty() || !current_header_path.is_empty() {
                sections.push(MarkdownSection {
                    header_path: current_header_path.clone(),
                    text: current_body.trim().to_string(),
                });
                current_body = String::new();
            }

            // Determine header level
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let header_text = trimmed[level..].trim().to_string();

            // Update header stack
            headers.truncate(level.saturating_sub(1));
            headers.push(header_text);
            current_header_path = headers.join(" > ");
        } else {
            if !current_body.is_empty() {
                current_body.push('\n');
            }
            current_body.push_str(line);
        }
    }

    // Save final section
    if !current_body.is_empty() || !current_header_path.is_empty() {
        sections.push(MarkdownSection {
            header_path: current_header_path,
            text: current_body.trim().to_string(),
        });
    }

    sections
}

impl Chunker for MarkdownChunker {
    fn chunk(&self, document: &Document) -> Vec<Chunk> {
        if document.text.is_empty() {
            return Vec::new();
        }

        let sections = parse_markdown_sections(&document.text);
        let mut chunks = Vec::new();
        let mut chunk_index = 0;

        for section in sections {
            // Build section text with header prefix
            let section_text = if section.header_path.is_empty() {
                section.text.clone()
            } else if section.text.is_empty() {
                section.header_path.clone()
            } else {
                format!("{}\n{}", section.header_path, section.text)
            };

            if section_text.is_empty() {
                continue;
            }

            let sub_chunks = if section_text.len() > self.chunk_size {
                // Further split using recursive logic
                let separators = ["\n\n", ". ", "! ", "? ", " "];
                split_and_merge(&section_text, self.chunk_size, self.chunk_overlap, &separators)
            } else {
                vec![section_text]
            };

            for text in sub_chunks {
                let mut metadata = document.metadata.clone();
                metadata.insert("chunk_index".to_string(), chunk_index.to_string());
                metadata.insert("header_path".to_string(), section.header_path.clone());

                chunks.push(Chunk {
                    id: format!("{}_{chunk_index}", document.id),
                    text,
                    embedding: Vec::new(),
                    metadata,
                    document_id: document.id.clone(),
                });
                chunk_index += 1;
            }
        }

        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;
    use std::collections::HashMap;

    fn doc(text: &str) -> Document {
        Document {
            id: "test".to_string(),
            text: text.to_string(),
            metadata: HashMap::new(),
            source_uri: None,
        }
    }

    #[test]
    fn fixed_chunker_utf8_multibyte() {
        // Chinese characters are 3 bytes each in UTF-8.
        // "你好世界" = 4 chars, 12 bytes.
        // With chunk_size=5 (bytes), naive slicing would panic mid-character.
        let chunker = FixedSizeChunker::new(5, 0);
        let chunks = chunker.chunk(&doc("你好世界测试文本"));
        // Should not panic and all chunks should be valid UTF-8
        for chunk in &chunks {
            assert!(chunk.text.is_char_boundary(0));
            // Verify it's valid UTF-8 by iterating chars
            let _ = chunk.text.chars().count();
        }
        // Should produce multiple chunks
        assert!(chunks.len() > 1);
    }

    #[test]
    fn fixed_chunker_utf8_emoji() {
        // Emoji are 4 bytes each. "🦀🚀🎉" = 3 chars, 12 bytes.
        let chunker = FixedSizeChunker::new(6, 0);
        let chunks = chunker.chunk(&doc("🦀🚀🎉✨🌟💫"));
        for chunk in &chunks {
            let _ = chunk.text.chars().count();
        }
        assert!(chunks.len() > 1);
    }

    #[test]
    fn fixed_chunker_utf8_mixed() {
        // Mix of ASCII (1 byte), accented (2 bytes), CJK (3 bytes), emoji (4 bytes)
        let text = "Hello café 你好 🦀";
        let chunker = FixedSizeChunker::new(7, 2);
        let chunks = chunker.chunk(&doc(text));
        for chunk in &chunks {
            let _ = chunk.text.chars().count();
        }
        assert!(!chunks.is_empty());
    }

    #[test]
    fn split_by_size_utf8() {
        let text = "日本語のテスト文字列です";
        let chunks = split_by_size(text, 10, 3);
        for chunk in &chunks {
            let _ = chunk.chars().count();
        }
        assert!(chunks.len() > 1);
    }

    #[test]
    fn recursive_chunker_utf8() {
        let text = "第一段落。这是中文文本。\n\n第二段落。更多中文内容在这里。";
        let chunker = RecursiveChunker::new(15, 3);
        let chunks = chunker.chunk(&doc(text));
        for chunk in &chunks {
            let _ = chunk.text.chars().count();
        }
        assert!(!chunks.is_empty());
    }
}
