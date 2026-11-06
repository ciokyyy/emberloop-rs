use sha2::{Digest, Sha256};

/// The full sha256 of a file snapshot, uppercase hex.
pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode_upper(digest)
}

/// The 4-hex snapshot tag: first two bytes of the file's sha256.
pub fn content_tag(bytes: &[u8]) -> String {
    let hash = content_hash(bytes);
    hash[..4].to_string()
}

/// Splits text into lines, dropping the single trailing newline a file ends with.
pub fn lines_of(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect()
}

/// Number of display lines, matching `lines_of`.
pub fn line_count(text: &str) -> usize {
    lines_of(text).len()
}
