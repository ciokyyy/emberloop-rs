//! Tool keys the tools read themselves: no shell export and no environment variable. The daemon
//! names the file, Settings writes it, and `websearch`/`readurl` read it at call time.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// names is the whole set the Settings UI offers, with the key each one stands for.
pub const NAMES: [(&str, &str); 3] = [
    ("brave", "BRAVE_API_KEY"),
    ("exa", "EXA_API_KEY"),
    ("jina", "JINA_API_KEY"),
];

/// variable maps a Settings key name to the name stored in the file.
pub fn variable(name: &str) -> Option<&'static str> {
    NAMES
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, variable)| *variable)
}

/// read returns every stored key; a missing file is an empty set.
pub fn read(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error),
    };
    Ok(serde_json::from_slice(&bytes).unwrap_or_default())
}

/// set stores one key, or removes it when the value is empty.
pub fn set(path: &Path, variable: &str, value: &str) -> io::Result<()> {
    let mut keys = read(path)?;
    if value.is_empty() {
        keys.remove(variable);
    } else {
        keys.insert(variable.to_string(), value.to_string());
    }
    write(path, &keys)
}

/// write replaces the file atomically with owner-only permissions, because it holds keys.
fn write(path: &Path, keys: &BTreeMap<String, String>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_vec_pretty(keys)?;
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&temporary, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_stored_once_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool-keys.json");
        set(&path, "EXA_API_KEY", "one").unwrap();
        set(&path, "JINA_API_KEY", "two").unwrap();
        set(&path, "EXA_API_KEY", "three").unwrap();
        assert_eq!(read(&path).unwrap()["EXA_API_KEY"], "three");
        assert_eq!(read(&path).unwrap()["JINA_API_KEY"], "two");
    }

    #[test]
    fn an_empty_value_removes_the_key_and_a_missing_file_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool-keys.json");
        assert!(read(&path).unwrap().is_empty());
        set(&path, "BRAVE_API_KEY", "secret").unwrap();
        set(&path, "BRAVE_API_KEY", "").unwrap();
        assert!(read(&path).unwrap().is_empty());
    }

    #[test]
    fn settings_names_map_to_their_variables() {
        assert_eq!(variable("brave"), Some("BRAVE_API_KEY"));
        assert_eq!(variable("exa"), Some("EXA_API_KEY"));
        assert_eq!(variable("jina"), Some("JINA_API_KEY"));
        assert_eq!(variable("other"), None);
    }
}
