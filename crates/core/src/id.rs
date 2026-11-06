use std::time::{SystemTime, UNIX_EPOCH};

pub fn generate_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rand_val: u32 = (now ^ (now >> 32)) as u32;
    format!(
        "{}_{:x}{:04x}",
        prefix,
        (now & 0xFFFFFFFF) as u32,
        rand_val & 0xFFFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_id() {
        let id1 = generate_id("ses");
        let id2 = generate_id("ses");
        assert!(id1.starts_with("ses_"));
        assert_ne!(id1, id2);
    }
}
