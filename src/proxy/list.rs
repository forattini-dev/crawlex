use std::io::{BufRead, BufReader};
use std::path::Path;
use url::Url;

use crate::{Error, Result};

pub fn parse_line(line: &str) -> Option<Url> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    Url::parse(trimmed).ok()
}

pub fn load_from_file(path: impl AsRef<Path>) -> Result<Vec<Url>> {
    let f = std::fs::File::open(path.as_ref())
        .map_err(|e| Error::Config(format!("open proxy file: {e}")))?;
    let mut out = Vec::new();
    for line in BufReader::new(f).lines() {
        let line = line.map_err(Error::Io)?;
        if let Some(u) = parse_line(&line) {
            out.push(u);
        }
    }
    Ok(out)
}

pub fn load_from_stdin() -> Result<Vec<Url>> {
    let stdin = std::io::stdin();
    let mut out = Vec::new();
    for line in stdin.lock().lines() {
        let line = line.map_err(Error::Io)?;
        if let Some(u) = parse_line(&line) {
            out.push(u);
        }
    }
    Ok(out)
}
