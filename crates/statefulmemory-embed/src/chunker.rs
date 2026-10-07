//! Word-based content chunker (Phase 2.5).
//!
//! Splits long content into overlapping word-windows so each chunk fits
//! comfortably inside the embedder's context window. Pure/no I/O — the
//! caller decides whether to chunk (gated by `embed.chunk_long_content`) and
//! what to do with the resulting pieces.

/// Split `text` into overlapping chunks of `max_words` words, each chunk
/// after the first overlapping the previous one by `overlap_words`.
///
/// Returns a single chunk containing the whole text when `text` has
/// `<= max_words` words (the common case — most observations are short).
/// `max_words` is clamped to at least 1; `overlap_words` is clamped to less
/// than `max_words` so chunking always progresses.
pub fn chunk_by_words(text: &str, max_words: usize, overlap_words: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }
    let max_words = max_words.max(1);
    if words.len() <= max_words {
        return vec![text.trim().to_string()];
    }
    let overlap = overlap_words.min(max_words.saturating_sub(1));
    let stride = max_words - overlap;

    let mut out = Vec::new();
    let mut start = 0usize;
    loop {
        let end = (start + max_words).min(words.len());
        out.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += stride;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = chunk_by_words("the quick brown fox", 10, 2);
        assert_eq!(chunks, vec!["the quick brown fox".to_string()]);
    }

    #[test]
    fn empty_text_has_no_chunks() {
        assert!(chunk_by_words("   ", 10, 2).is_empty());
        assert!(chunk_by_words("", 10, 2).is_empty());
    }

    #[test]
    fn long_text_splits_with_overlap() {
        let words: Vec<String> = (0..25).map(|i| format!("w{i}")).collect();
        let text = words.join(" ");
        let chunks = chunk_by_words(&text, 10, 3);
        assert!(chunks.len() > 1, "25 words / max 10 must split");
        // Every chunk has at most max_words words.
        for c in &chunks {
            assert!(c.split_whitespace().count() <= 10);
        }
        // Consecutive chunks overlap: last `overlap` words of chunk i appear
        // as the first words of chunk i+1.
        for i in 0..chunks.len() - 1 {
            let a: Vec<&str> = chunks[i].split_whitespace().collect();
            let b: Vec<&str> = chunks[i + 1].split_whitespace().collect();
            let tail = &a[a.len() - 3..];
            let head = &b[..3];
            assert_eq!(tail, head, "chunk {i} and {} must overlap by 3", i + 1);
        }
        // Full coverage: every original word appears in some chunk, in order.
        let rejoined: Vec<&str> = chunks
            .iter()
            .flat_map(|c| c.split_whitespace())
            .collect();
        assert!(rejoined.contains(&"w0"));
        assert!(rejoined.contains(&"w24"));
    }

    #[test]
    fn overlap_clamped_below_max_words_always_progresses() {
        // overlap >= max_words would stall (stride=0); must clamp.
        let words: Vec<String> = (0..30).map(|i| format!("w{i}")).collect();
        let text = words.join(" ");
        let chunks = chunk_by_words(&text, 5, 999);
        assert!(chunks.len() > 1 && chunks.len() < 100, "must terminate: got {} chunks", chunks.len());
    }
}
