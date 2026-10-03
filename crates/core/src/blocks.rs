//! Versioned interpreter blocks. Archives are inspected in memory, never executed.

use crate::{Visibility, valid_slug};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    io::{Cursor, Read},
};

pub const MAX_GENE_BYTES: usize = 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: usize = 8 * 1024 * 1024;
const MAX_EXPANDED_BYTES: usize = 32 * 1024 * 1024;
const MAX_SEARCH_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockKind {
    Gene,
    Isi,
    Function,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Block {
    pub id: String,
    pub kind: BlockKind,
    pub name: String,
    pub description: String,
    pub owner: String,
    pub visibility: Visibility,
    pub version: String,
    pub downloads: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockVersion {
    pub version: String,
    pub published_at: DateTime<Utc>,
}

pub fn parse_id(id: &str) -> Result<(BlockKind, &str), String> {
    let (kind, name) = id
        .split_once(':')
        .ok_or("block id must begin with gene:, isi: or function:")?;
    let kind = match kind {
        "gene" => BlockKind::Gene,
        "isi" => BlockKind::Isi,
        "function" => BlockKind::Function,
        _ => return Err("block id must begin with gene:, isi: or function:".into()),
    };
    if !valid_slug(name) {
        return Err("block name must contain 1–128 lowercase letters, digits or hyphens".into());
    }
    Ok((kind, name))
}

pub fn content_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn portable_component(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let number = stem
        .strip_prefix("COM")
        .or_else(|| stem.strip_prefix("LPT"));
    !part.is_empty()
        && part != "."
        && part != ".."
        && !part.ends_with(['.', ' '])
        && !part.contains(['<', '>', '"', '|', '?', '*'])
        && !matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        && !matches!(
            number,
            Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³")
        )
}

// zip's filename index collapses duplicates; compare it with the bounded central-directory count.
fn archive_entry_count(bytes: &[u8]) -> Result<usize, String> {
    let end = bytes
        .windows(22)
        .enumerate()
        .rev()
        .find(|(index, window)| {
            window.starts_with(b"PK\x05\x06")
                && *index + 22 + u16::from_le_bytes([window[20], window[21]]) as usize
                    == bytes.len()
        })
        .map(|(_, window)| window)
        .ok_or("invalid ZIP end record")?;
    let count = u16::from_le_bytes([end[10], end[11]]) as usize;
    if end[4..8] != [0; 4] || end[8..10] != end[10..12] || count > 256 {
        return Err("ZIP must be a single archive with at most 256 entries".into());
    }
    Ok(count)
}

/// Read only ordinary files with portable relative paths, bounded before decompression.
/// Directory entries are validated but omitted; parent directories can be recreated on extraction.
pub fn archive_files(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    if bytes.len() > MAX_ARCHIVE_BYTES {
        return Err("ZIP exceeds the 8 MiB upload limit".into());
    }
    let count = archive_entry_count(bytes)?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("invalid ZIP: {e}"))?;
    if archive.len() != count {
        return Err("ZIP contains duplicate filenames".into());
    }
    let mut files = BTreeMap::new();
    let mut paths = HashSet::new();
    let mut total = 0usize;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|e| format!("invalid ZIP entry: {e}"))?;
        let name = std::str::from_utf8(file.name_raw())
            .map_err(|_| "ZIP filenames must be UTF-8")?
            .to_owned();
        let path = name.strip_suffix('/').unwrap_or(&name);
        if path.is_empty()
            || path.contains('\\')
            || path.contains(':')
            || path.chars().any(char::is_control)
            || path.split('/').any(|part| !portable_component(part))
            || file.enclosed_name().is_none()
            || !paths.insert(path.to_lowercase())
        {
            return Err(format!("unsafe or duplicate ZIP path: {name}"));
        }
        let mode = file.unix_mode().unwrap_or(0) & 0o170000;
        if file.encrypted()
            || (mode != 0 && mode != 0o100000 && mode != 0o040000)
            || (mode == 0o040000 && !file.is_dir())
        {
            return Err(format!(
                "ZIP entry must be an unencrypted regular file or directory: {name}"
            ));
        }
        if file.is_dir() {
            continue;
        }
        let remaining = MAX_EXPANDED_BYTES - total;
        if file.size() > remaining as u64 {
            return Err("ZIP exceeds the 32 MiB expanded limit".into());
        }
        let mut content = Vec::new();
        (&mut file)
            .take(remaining as u64 + 1)
            .read_to_end(&mut content)
            .map_err(|e| format!("cannot read ZIP entry: {e}"))?;
        if content.len() > remaining {
            return Err("ZIP exceeds the 32 MiB expanded limit".into());
        }
        total += content.len();
        files.insert(name, content);
    }
    // Reject file/directory aliases on case-insensitive filesystems too.
    let names: HashSet<String> = files.keys().map(|name| name.to_lowercase()).collect();
    for name in &paths {
        for (index, _) in name.match_indices('/') {
            if names.contains(&name[..index]) {
                return Err("ZIP file overlaps a parent directory".into());
            }
        }
    }
    Ok(files)
}

#[cfg(unix)]
fn executable_permissions(created: u32, archived: u32) -> u32 {
    created | ((created & 0o444) >> 2 & archived & 0o111)
}

/// Extract into an empty staging directory, preserving executable bits and the caller's umask.
/// The caller can rename the staging directory only after this succeeds.
pub fn extract_archive(bytes: &[u8], target: &std::path::Path) -> Result<(), String> {
    use std::fs;
    let files = archive_files(bytes)?;
    match fs::symlink_metadata(target) {
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.is_symlink()
                && fs::read_dir(target)
                    .map_err(|e| e.to_string())?
                    .next()
                    .is_none() => {}
        Ok(_) => return Err("archive destination must be an empty directory".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(target).map_err(|e| e.to_string())?
        }
        Err(e) => return Err(e.to_string()),
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = std::str::from_utf8(entry.name_raw()).map_err(|e| e.to_string())?;
        let path = target.join(name);
        if entry.is_dir() {
            fs::create_dir_all(path).map_err(|e| e.to_string())?;
            continue;
        }
        fs::create_dir_all(path.parent().ok_or("invalid archive path")?)
            .map_err(|e| e.to_string())?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut file, &files[name]).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = file.metadata().map_err(|e| e.to_string())?.permissions();
            permissions.set_mode(executable_permissions(
                permissions.mode(),
                entry.unix_mode().unwrap_or(0),
            ));
            file.set_permissions(permissions)
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Validate an immutable payload and return bounded text suitable for search indexing.
pub fn validate_payload(id: &str, bytes: &[u8]) -> Result<String, String> {
    let (kind, name) = parse_id(id)?;
    if kind == BlockKind::Gene {
        if bytes.len() > MAX_GENE_BYTES {
            return Err("gene exceeds the 1 MiB limit".into());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| "gene must be UTF-8 Markdown")?;
        if text.trim().is_empty() || text.contains('\0') {
            return Err("gene must contain Markdown text".into());
        }
        return Ok(text.to_owned());
    }
    let files = archive_files(bytes)?;
    let (filename, key) = if kind == BlockKind::Isi {
        ("isi.yaml", "isi")
    } else {
        ("function.yaml", "functions")
    };
    let yaml = files
        .get(filename)
        .ok_or_else(|| format!("ZIP must contain {filename} at its root"))?;
    let value: serde_yaml::Value =
        serde_yaml::from_slice(yaml).map_err(|e| format!("invalid {filename}: {e}"))?;
    let root = value
        .as_mapping()
        .ok_or_else(|| format!("{filename} must contain a mapping"))?;
    let entries = root
        .get(serde_yaml::Value::String(key.into()))
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or_else(|| format!("{filename} must contain a {key}: mapping"))?;
    if entries.len() != 1 {
        return Err(format!("{filename} must contain exactly one {key} block"));
    }
    let (block_name, definition) = entries.iter().next().unwrap();
    let block_name = block_name
        .as_str()
        .filter(|name| !name.is_empty())
        .ok_or("block name must be a nonempty string")?;
    if !definition.is_mapping() {
        return Err("block definition must be a mapping".into());
    }
    if kind == BlockKind::Isi && block_name != name {
        return Err("ISI name in isi.yaml must match its published id".into());
    }
    let other = if kind == BlockKind::Isi {
        "functions"
    } else {
        "isi"
    };
    if root.contains_key(serde_yaml::Value::String(other.into())) {
        return Err(format!("{filename} cannot also define {other}"));
    }
    let mut text = String::new();
    for (path, bytes) in files {
        if let Ok(content) = std::str::from_utf8(&bytes) {
            if content.contains('\0') {
                continue;
            }
            text.push_str(&path);
            text.push('\n');
            let available = MAX_SEARCH_BYTES.saturating_sub(text.len());
            let end = content.floor_char_boundary(available.min(content.len()));
            text.push_str(&content[..end]);
            if text.len() >= MAX_SEARCH_BYTES {
                break;
            }
            text.push('\n');
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn zip(files: &[(&str, &str)]) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (path, text) in files {
            archive
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(text.as_bytes()).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }
    #[test]
    fn validates_genes_and_single_named_blocks_with_safe_supporting_files() {
        assert_eq!(
            content_hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(validate_payload("gene:creative", b"# Be creative").is_ok());
        assert!(validate_payload("gene:creative", b" ").is_err());
        assert!(parse_id("gene:../escape").is_err());
        let good = zip(&[
            (
                "isi.yaml",
                "isi:\n  worker: {dna: {assemble: [prompts/default.md]}}",
            ),
            ("prompts/default.md", "Work carefully"),
        ]);
        assert!(
            validate_payload("isi:worker", &good)
                .unwrap()
                .contains("Work carefully")
        );
        assert!(validate_payload("isi:wrong", &good).is_err());
        assert!(
            validate_payload(
                "function:greet",
                &zip(&[(
                    "function.yaml",
                    "functions:\n  greet: {params: [name], do: []}"
                )])
            )
            .is_ok()
        );
        for yaml in [
            "isi: {}",
            "isi: {one: {}, two: {}}",
            "isi: {worker: []}",
            "isi: {worker: {}, worker: {}}",
            "isi: {worker: {}}\nfunctions: {bad: {}}",
        ] {
            assert!(
                validate_payload("isi:worker", &zip(&[("isi.yaml", yaml)])).is_err(),
                "{yaml}"
            );
        }
        for path in [
            "../escape",
            "/absolute",
            "a/../escape",
            "a\\escape",
            "C:escape",
            "a//b",
            "a./file",
            "a /file",
            "CON.txt",
            "con/file",
            "com1",
            "LPT9",
            "a?b",
        ] {
            assert!(archive_files(&zip(&[(path, "bad")])).is_err(), "{path}");
        }
        assert!(archive_files(&zip(&[("FILE", "a"), ("file", "b")])).is_err());
        let mut duplicate = zip(&[("one", "a"), ("two", "b")]);
        for index in 0..duplicate.len() - 2 {
            if &duplicate[index..index + 3] == b"two" {
                duplicate[index..index + 3].copy_from_slice(b"one");
            }
        }
        assert!(archive_files(&duplicate).unwrap_err().contains("duplicate"));
        assert!(archive_files(&zip(&[("file", "a"), ("file/child", "b")])).is_err());
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .add_symlink(
                "link",
                "/etc/passwd",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        assert!(archive_files(&archive.finish().unwrap().into_inner()).is_err());
        assert!(archive_files(&zip(&[("large", &"x".repeat(MAX_EXPANDED_BYTES + 1))])).is_err());
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("output");
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .add_directory("empty/", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive
            .start_file(
                "scripts/run.sh",
                zip::write::SimpleFileOptions::default().unix_permissions(0o755),
            )
            .unwrap();
        archive.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        let bytes = archive.finish().unwrap().into_inner();
        extract_archive(&bytes, &target).unwrap();
        assert!(target.join("empty").is_dir());
        assert_eq!(
            std::fs::read(target.join("scripts/run.sh")).unwrap(),
            b"#!/bin/sh\nexit 0\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                std::fs::metadata(target.join("scripts/run.sh"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o100,
                0
            );
            assert_eq!(executable_permissions(0o600, 0o755), 0o700);
            assert_eq!(executable_permissions(0o640, 0o744), 0o740);
            assert_eq!(executable_permissions(0o600, 0o644), 0o600);
        }
        assert!(extract_archive(&bytes, &target).is_err());
    }
}
