//! Saves prompt attachments under the Project, so the agent can read a file it was sent instead of
//! carrying it in context.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use ct_codecs::{Base64, Decoder};

use crate::routes::{PromptFileDto, PromptImageDto};

/// Project-relative directory holding every Session's attachments.
pub const DIRECTORY: &str = ".emberloop/files";
const GITIGNORE_ENTRY: &str = "files/\n";

/// Writes every attachment under `<root>/.emberloop/files/<session>` and answers the Project-relative
/// path of each non-image file, in payload order. An image is saved too but not named: it is already
/// in context.
pub fn save(
    root: &Path,
    session_id: &str,
    images: &[PromptImageDto],
    files: &[PromptFileDto],
) -> io::Result<Vec<String>> {
    if images.is_empty() && files.is_empty() {
        return Ok(Vec::new());
    }
    let session = sanitize(session_id);
    let directory = root.join(DIRECTORY).join(&session);
    fs::create_dir_all(&directory)?;
    ignore_attachments(root)?;
    for image in images {
        let bytes = decode(&image.data_base64)?;
        write_unique(&directory, image.name.as_deref().unwrap_or("image"), &bytes)?;
    }
    let mut paths = Vec::with_capacity(files.len());
    for file in files {
        let bytes = decode(&file.data_base64)?;
        let path = write_unique(&directory, &file.name, &bytes)?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        paths.push(format!("{DIRECTORY}/{session}/{name}"));
    }
    Ok(paths)
}

/// The prompt text with one line per saved file after the user's own text.
pub fn with_saved_files(text: &str, paths: &[String]) -> String {
    let mut text = text.to_string();
    for path in paths {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str("Attached file saved at ");
        text.push_str(path);
    }
    text
}

/// The browser's own name reduced to a basename that cannot leave the directory it is joined to.
fn sanitize(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    if base.is_empty() || base == "." || base == ".." {
        return "file".into();
    }
    base
}

fn decode(data: &str) -> io::Result<Vec<u8>> {
    Base64::decode_to_vec(data, None).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the attachment is not valid base64",
        )
    })
}

/// `create_new` doubles as the existence test, so two prompts racing for one name never overwrite.
fn write_unique(directory: &Path, raw: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let name = sanitize(raw);
    let (stem, extension) = split_extension(&name);
    for attempt in 1u32.. {
        let candidate = if attempt == 1 {
            name.clone()
        } else {
            format!("{stem}-{attempt}{extension}")
        };
        let path = directory.join(candidate);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(bytes)?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!("`u32::MAX` attachments of one name in one Session is not a real case")
}

fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(index) if index > 0 => (&name[..index], &name[index..]),
        _ => (name, ""),
    }
}

/// Keeps attachments out of the Project's `git status` without touching its own `.gitignore`.
fn ignore_attachments(root: &Path) -> io::Result<()> {
    let path = root.join(".emberloop/.gitignore");
    let current = fs::read_to_string(&path).unwrap_or_default();
    if current
        .lines()
        .any(|line| line.trim() == GITIGNORE_ENTRY.trim())
    {
        return Ok(());
    }
    let mut text = current;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(GITIGNORE_ENTRY);
    fs::write(&path, text)
}
