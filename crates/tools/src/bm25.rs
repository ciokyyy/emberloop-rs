//! BM25 ranking shared by `ctx_search` (spill chunks) and `skill_search` (skill documents).
//! The caller owns the documents; this module only tokenizes and scores them. Query terms are
//! OR-ed, term frequency saturates, and long documents are normalized, as in Go's FTS5 search.

/// Lowercased alphanumeric words of `text`.
// shortcut: no Porter stemming or persistent index; use FTS5 when per-call scans become too slow.
pub(crate) fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The distinct words of `query`, sorted.
pub(crate) fn query_terms(query: &str) -> Vec<String> {
    let mut terms = tokens(query);
    terms.sort();
    terms.dedup();
    terms
}

/// BM25 score of each document, given as its tokens, for the sorted, distinct `terms`.
pub(crate) fn scores(docs: &[Vec<String>], terms: &[String]) -> Vec<f64> {
    let count = docs.len() as f64;
    if count == 0.0 {
        return Vec::new();
    }
    let average = (docs.iter().map(Vec::len).sum::<usize>() as f64 / count).max(1.0);
    let weights: Vec<f64> = terms
        .iter()
        .map(|term| {
            let frequency = docs.iter().filter(|words| words.contains(term)).count() as f64;
            ((count - frequency + 0.5) / (frequency + 0.5))
                .ln()
                .max(1e-6)
        })
        .collect();
    docs.iter()
        .map(|words| {
            let normalization = 1.2 * (0.25 + 0.75 * words.len() as f64 / average);
            terms
                .iter()
                .zip(&weights)
                .map(|(term, weight)| {
                    let frequency = words.iter().filter(|word| *word == term).count() as f64;
                    weight * frequency * 2.2 / (frequency + normalization)
                })
                .sum::<f64>()
        })
        .collect()
}
