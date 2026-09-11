use std::{env, fs, path::Path};

use bytesize::ByteSize;
use serde::{Deserialize, Serialize};

pub const PREVIEW_STORE_LEN: usize = 253;

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub preview_width: usize,
    pub max_history_length: u64,
    pub max_store_size: ByteSize,
    pub min_store_size: ByteSize,
    pub ignore_patterns: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            preview_width: 100,
            max_history_length: 500,
            max_store_size: ByteSize::mib(10),
            min_store_size: ByteSize::b(1),
            ignore_patterns: Vec::new(),
        }
    }
}

pub fn load_config() -> Config {
    let config_dir = env::home_dir()
        .unwrap()
        .join(".config")
        .join(env!("CARGO_PKG_NAME"));
    fs::create_dir_all(&config_dir).unwrap();

    let config_file = config_dir.join("config.toml");
    if !fs::exists(&config_file).unwrap() {
        let default_config = Config::default();
        fs::write(&config_file, toml::to_string(&default_config).unwrap()).unwrap();
        return default_config;
    }

    let content = fs::read_to_string(&config_file).unwrap();
    toml::from_str(&content).unwrap_or_else(|e| {
        eprintln!("Warning: config file has errors ({e}), using defaults");
        Config::default()
    })
}

pub fn should_ignore(text: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let (valid, invalid): (Vec<&String>, Vec<&String>) =
        patterns.iter().partition(|p| regex::Regex::new(p).is_ok());
    for p in invalid {
        eprintln!("Warning: bad ignore pattern {p:?}, skipping");
    }
    if valid.is_empty() {
        return false;
    }
    match regex::RegexSet::new(valid) {
        Ok(set) => set.is_match(text),
        Err(e) => {
            eprintln!("Warning: bad ignore patterns ({e}), skipping");
            false
        }
    }
}

fn parse_path_lines(text: &str) -> Option<(Option<&str>, Vec<&str>)> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next()?;
    let (verb, mut paths) = if first == "copy" || first == "cut" {
        (Some(first), Vec::new())
    } else if is_path_like(first) {
        (None, vec![first])
    } else {
        return None;
    };
    for line in lines {
        if !is_path_like(line) {
            return None;
        }
        paths.push(line);
    }
    if paths.is_empty() {
        return None;
    }
    Some((verb, paths))
}

fn is_path_like(line: &str) -> bool {
    if line.starts_with("file://") {
        return line.len() > "file://".len();
    }

    if line.starts_with('/') {
        let path = Path::new(line);
        return path.is_file() || path.is_dir();
    }
    false
}

pub fn normalize_paths(bytes: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let (verb, paths) = parse_path_lines(text)?;
    let mut out = String::new();
    if let Some(v) = verb {
        out.push_str(v);
    }
    for line in paths {
        if !out.is_empty() {
            out.push('\n');
        }
        if line.starts_with("file://") {
            out.push_str(line);
        } else {
            out.push_str("file://");
            out.push_str(line);
        }
    }
    Some(out.into_bytes())
}

pub fn detect_mime(bytes: &[u8]) -> Option<String> {
    if let Ok(text) = std::str::from_utf8(bytes) {
        if parse_path_lines(text).is_some() {
            return Some("text/uri-list".to_string());
        }
        return Some("text/plain".to_string());
    }

    infer::get(bytes)
        .filter(|kind| kind.mime_type().starts_with("image/"))
        .map(|kind| kind.mime_type().to_string())
}

pub fn create_preview(bytes: &[u8], mime: &str) -> String {
    if mime == "text/uri-list" {
        let paths = uri_display_paths(bytes);
        if !paths.is_empty() {
            let first = match paths[0].rsplit('/').next() {
                Some(name) if !name.is_empty() => name,
                _ => paths[0].as_str(),
            };
            let slash = if Path::new(&paths[0]).is_dir() {
                "/"
            } else {
                ""
            };
            let first = middle_truncate(first, PREVIEW_STORE_LEN - 12);
            let s = if paths.len() == 1 {
                format!("[[ {first}{slash} ]]")
            } else {
                format!("[[ {first}{slash} +{} ]]", paths.len() - 1)
            };
            return s.chars().take(PREVIEW_STORE_LEN).collect();
        }
    }

    if mime == "text/plain" || mime == "text/uri-list" {
        let s = String::from_utf8_lossy(bytes);
        let s = s.trim().replace('\n', " ↵ ");
        return s.chars().take(PREVIEW_STORE_LEN).collect();
    }

    if let Some(ext) = mime.strip_prefix("image/") {
        let size = ByteSize::b(bytes.len() as u64);
        return format!("[[ image {ext} {size} ]]");
    }

    format!("[[ data {} ]]", ByteSize::b(bytes.len() as u64))
}

fn middle_truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars || max_chars <= 4 {
        return s.to_string();
    }
    let keep = max_chars - 1;
    let head = keep * 2 / 3;
    let tail = keep - head;
    let head_s: String = s.chars().take(head).collect();
    let tail_s: String = s.chars().skip(count - tail).collect();
    format!("{head_s}…{tail_s}")
}

fn uri_display_paths(bytes: &[u8]) -> Vec<String> {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Vec::new();
    };
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter(|l| *l != "copy" && *l != "cut")
        .map(|l| {
            let l = l.strip_prefix("file://").unwrap_or(l);
            let l = l.split('?').next().unwrap_or(l);
            percent_encoding::percent_decode_str(l)
                .decode_utf8_lossy()
                .into_owned()
        })
        .filter(|n| !n.is_empty())
        .collect()
}

pub fn truncate_preview(preview: &str, width: usize, is_plain_text: bool) -> String {
    if is_plain_text && preview.chars().count() > width {
        let truncated: String = preview.chars().take(width).collect();
        format!("{truncated}...")
    } else {
        preview.to_string()
    }
}
