use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub fn calculate_sha256(file_path: &Path) -> anyhow::Result<String> {
    let mut file = File::open(file_path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

pub fn verify_files_match(src: &Path, dst: &Path) -> anyhow::Result<bool> {
    if !src.exists() || !dst.exists() {
        return Ok(false);
    }

    let src_hash = calculate_sha256(src)?;
    let dst_hash = calculate_sha256(dst)?;

    Ok(src_hash == dst_hash)
}

pub fn verify_file_integrity(file_path: &Path, expected_hash: &str) -> anyhow::Result<bool> {
    if !file_path.exists() {
        return Ok(false);
    }

    let actual_hash = calculate_sha256(file_path)?;
    Ok(actual_hash == expected_hash)
}
