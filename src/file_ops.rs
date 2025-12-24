use crate::config::{FileOperation, OperationType, RateLimit};
use crate::rate_limiter::RateLimiter;
use crate::validation;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub source_path: String,
    pub destination_path: String,
    pub size: u64,
    pub hash_verified: bool,
    pub success: bool,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OperationResult {
    pub operation_name: String,
    pub source: String,
    pub destination: String,
    pub success: bool,
    pub error_message: Option<String>,
    pub hash_verified: bool,
    pub operation_type: OperationType,
    pub files_processed: usize,
    pub total_size: u64,
    pub start_time: SystemTime,
    pub end_time: SystemTime,
    pub details: Vec<String>,
    pub file_list: Vec<FileEntry>,
}

pub struct FileManager;

impl FileManager {
    pub fn execute_operations(
        operations: &[FileOperation],
        global_rate_limit: &RateLimit,
        progress_callback: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Vec<OperationResult> {
        let results = Arc::new(Mutex::new(Vec::new()));
        let total_operations = operations.len();

        let pb = if progress_callback.is_some() {
            Some(ProgressBar::new(total_operations as u64))
        } else {
            None
        };

        if let Some(pb) = &pb {
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
                    .unwrap()
                    .progress_chars("#>-"),
            );
        }

        operations.par_iter().for_each(|op| {
            let start_time = SystemTime::now();
            let result = Self::execute_single_operation(op, global_rate_limit, start_time);

            let mut results_lock = results.lock().unwrap();
            results_lock.push(result);

            if let Some(callback) = &progress_callback {
                callback(format!("Completed: {}", op.name));
            }

            if let Some(pb) = &pb {
                pb.inc(1);
            }
        });

        if let Some(pb) = &pb {
            pb.finish_with_message("All operations completed");
        }

        Arc::try_unwrap(results).unwrap().into_inner().unwrap()
    }

    fn execute_single_operation(
        operation: &FileOperation,
        global_rate_limit: &RateLimit,
        start_time: SystemTime,
    ) -> OperationResult {
        let mut details = Vec::new();
        details.push(format!("Starting operation: {}", operation.name));
        details.push(format!("  Type: {:?}", operation.operation_type));
        details.push(format!("  Source: {}", operation.origin.display()));
        details.push(format!(
            "  Destination: {}",
            operation.destination.display()
        ));

        let mut result = OperationResult {
            operation_name: operation.name.clone(),
            source: operation.origin.to_string_lossy().to_string(),
            destination: operation.destination.to_string_lossy().to_string(),
            success: false,
            error_message: None,
            hash_verified: false,
            operation_type: operation.operation_type.clone(),
            files_processed: 0,
            total_size: 0,
            start_time,
            end_time: SystemTime::now(),
            details: details.clone(),
            file_list: Vec::new(),
        };

        if !operation.origin.exists() {
            let error_msg = format!("Source '{}' does not exist", operation.origin.display());
            details.push(format!("ERROR: {}", error_msg));
            result.error_message = Some(error_msg.clone()); // Clone here
            result.end_time = SystemTime::now();
            result.details = details;
            return result;
        }

        let is_dir = operation.origin.is_dir();
        let is_file = operation.origin.is_file();

        if !is_dir && !is_file {
            let error_msg = format!(
                "Source '{}' is not a valid file or directory",
                operation.origin.display()
            );
            details.push(format!("ERROR: {}", error_msg));
            result.error_message = Some(error_msg.clone()); // Clone here
            result.end_time = SystemTime::now();
            result.details = details;
            return result;
        }

        details.push(format!(
            "  Source is a {}",
            if is_dir { "directory" } else { "file" }
        ));

        if let Some(parent) = operation.destination.parent() {
            if !parent.exists() {
                details.push(format!("  Creating parent directory: {}", parent.display()));
                if let Err(e) = fs::create_dir_all(parent) {
                    let error_msg = format!(
                        "Failed to create destination directory '{}': {}",
                        parent.display(),
                        e
                    );
                    details.push(format!("ERROR: {}", error_msg));
                    result.error_message = Some(error_msg.clone()); // Clone here
                    result.end_time = SystemTime::now();
                    result.details = details;
                    return result;
                }
                details.push("  Parent directory created successfully".to_string());
            }
        }

        match operation.operation_type {
            OperationType::Copy => {
                if is_dir {
                    result = Self::copy_directory(operation, global_rate_limit, details);
                } else {
                    result = Self::copy_file(operation, global_rate_limit, details);
                }
            }
            OperationType::Move => {
                if is_dir {
                    result = Self::move_directory(operation, details);
                } else {
                    result = Self::move_file(operation, details);
                }
            }
        }

        result.end_time = SystemTime::now();
        result
    }

    fn compute_effective_bps(op: &RateLimit, global: &RateLimit) -> Option<u64> {
        // Respect enabled flags and choose defaults/caps
        // If neither enabled, no throttling
        if !op.enabled && !global.enabled {
            return None;
        }
        // Helper to convert RateLimit to optional bps
        let to_bps = |rl: &RateLimit| -> Option<u64> {
            if !rl.enabled {
                return None;
            }
            if let Some(bps) = rl.bytes_per_second {
                Some(bps)
            } else if let Some(mb_min) = rl.megabytes_per_minute {
                Some(mb_min * 1024 * 1024 / 60)
            } else {
                None
            }
        };
        let op_bps = to_bps(op);
        let global_bps = to_bps(global);
        match (op_bps, global_bps) {
            (Some(o), Some(g)) => Some(o.min(g)),
            (Some(o), None) => Some(o),
            (None, Some(g)) => Some(g),
            (None, None) => None,
        }
    }

    fn copy_file(operation: &FileOperation, global_rate_limit: &RateLimit, mut details: Vec<String>) -> OperationResult {
        let mut result = OperationResult {
            operation_name: operation.name.clone(),
            source: operation.origin.to_string_lossy().to_string(),
            destination: operation.destination.to_string_lossy().to_string(),
            success: false,
            error_message: None,
            hash_verified: false,
            operation_type: OperationType::Copy,
            files_processed: 1,
            total_size: 0,
            start_time: SystemTime::now(),
            end_time: SystemTime::now(),
            details: details.clone(),
            file_list: Vec::new(),
        };

        let file_size = if let Ok(metadata) = std::fs::metadata(&operation.origin) {
            metadata.len()
        } else {
            0
        };
        result.total_size = file_size;
        details.push(format!("  File size: {} bytes", result.total_size));

        // Compute effective rate limit combining per-op and global (cap by min)
        let effective_bps = Self::compute_effective_bps(&operation.rate_limit, global_rate_limit);
        let mut rate_limiter = RateLimiter::new(effective_bps, None);

        if rate_limiter.is_enabled() {
            if let Some(limit) = rate_limiter.get_rate_limit() {
                details.push(format!(
                    "  Rate limiting: {} bytes/second ({:.2} MB/min)",
                    limit,
                    limit as f64 * 60.0 / (1024.0 * 1024.0)
                ));
            }
        }

        details.push("  Starting file copy...".to_string());

        // Use a custom copy function with rate limiting
        let copy_result: io::Result<u64> = if rate_limiter.is_enabled() {
            Self::copy_file_with_rate_limit(
                &operation.origin,
                &operation.destination,
                &mut rate_limiter,
            )
        } else {
            fs::copy(&operation.origin, &operation.destination)
        };

        match copy_result {
            Ok(bytes_copied) => {
                details.push(format!("  Copy completed: {} bytes copied", bytes_copied));
                result.total_size = bytes_copied;

                details.push("  Verifying file integrity...".to_string());
                match validation::verify_files_match(&operation.origin, &operation.destination) {
                    Ok(true) => {
                        details.push("  Verification successful: Files match".to_string());
                        result.success = true;
                        result.hash_verified = true;

                        result.file_list.push(FileEntry {
                            source_path: operation.origin.to_string_lossy().to_string(),
                            destination_path: operation.destination.to_string_lossy().to_string(),
                            size: bytes_copied,
                            hash_verified: true,
                            success: true,
                            error_message: None,
                        });
                    }
                    Ok(false) => {
                        let error_msg =
                            "Hash verification failed - files are different".to_string();
                        details.push(format!("ERROR: {}", error_msg));
                        result.error_message = Some(error_msg.clone());
                        let _ = fs::remove_file(&operation.destination);
                        details.push("  Cleaned up failed copy".to_string());

                        result.file_list.push(FileEntry {
                            source_path: operation.origin.to_string_lossy().to_string(),
                            destination_path: operation.destination.to_string_lossy().to_string(),
                            size: bytes_copied,
                            hash_verified: false,
                            success: false,
                            error_message: Some(error_msg),
                        });
                    }
                    Err(e) => {
                        let error_msg = format!("Verification error: {}", e);
                        details.push(format!("ERROR: {}", error_msg));
                        result.error_message = Some(error_msg.clone());
                        let _ = fs::remove_file(&operation.destination);
                        details.push("  Cleaned up failed copy".to_string());

                        result.file_list.push(FileEntry {
                            source_path: operation.origin.to_string_lossy().to_string(),
                            destination_path: operation.destination.to_string_lossy().to_string(),
                            size: bytes_copied,
                            hash_verified: false,
                            success: false,
                            error_message: Some(error_msg),
                        });
                    }
                }
            }
            Err(e) => {
                let error_msg = format!(
                    "Copy failed: {} (from {} to {})",
                    e,
                    operation.origin.display(),
                    operation.destination.display()
                );
                details.push(format!("ERROR: {}", error_msg));
                result.error_message = Some(error_msg.clone());

                result.file_list.push(FileEntry {
                    source_path: operation.origin.to_string_lossy().to_string(),
                    destination_path: operation.destination.to_string_lossy().to_string(),
                    size: file_size,
                    hash_verified: false,
                    success: false,
                    error_message: Some(error_msg),
                });

                if e.kind() == io::ErrorKind::PermissionDenied {
                    details.push("  Permission denied - check file permissions".to_string());
                } else if e.kind() == io::ErrorKind::NotFound {
                    details.push("  File not found - check path".to_string());
                } else if e.kind() == io::ErrorKind::AlreadyExists {
                    details.push("  Destination already exists".to_string());
                }
            }
        }

        result.details = details;
        result
    }

    // NEW: Copy file with rate limiting
    fn copy_file_with_rate_limit(
        source: &Path,
        destination: &Path,
        rate_limiter: &mut RateLimiter,
    ) -> io::Result<u64> {
        use std::io::{Read, Write};

        let mut source_file = fs::File::open(source)?;
        let mut dest_file = fs::File::create(destination)?;

        let metadata = source_file.metadata()?;
        let total_size = metadata.len();
        let mut total_copied = 0;

        // Emit initial progress at 0%
        if total_size > 0 {
            println!("  Progress: 0% (0.00 KB/s)");
        }

        // Use a buffer for chunked copying
        let buffer_size = 64 * 1024; // 64KB chunks
        let mut buffer = vec![0u8; buffer_size];

        loop {
            let bytes_read = source_file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }

            dest_file.write_all(&buffer[..bytes_read])?;
            total_copied += bytes_read as u64;

            // Apply rate limiting for this chunk
            rate_limiter.throttle_chunk(bytes_read, total_size);

            // Report progress every 10% or for files under 10MB
            if total_size > 0 {
                let before = (total_copied.saturating_sub(bytes_read as u64)) * 100 / total_size;
                let after = (total_copied * 100 / total_size).min(99); // avoid 100% inside loop
                if after > before || total_size < 10 * 1024 * 1024 {
                    let rate = rate_limiter.get_current_rate();
                    println!("  Progress: {}% ({:.2} KB/s)", after, rate / 1024.0);
                }
            }
        }

        // Finalize at 100%
        if total_size > 0 {
            let rate = rate_limiter.get_current_rate();
            println!("  Progress: 100% ({:.2} KB/s)", rate / 1024.0);
        }

        dest_file.sync_all()?;
        Ok(total_copied)
    }

    fn copy_directory(operation: &FileOperation, global_rate_limit: &RateLimit, mut details: Vec<String>) -> OperationResult {
        let mut result = OperationResult {
            operation_name: operation.name.clone(),
            source: operation.origin.to_string_lossy().to_string(),
            destination: operation.destination.to_string_lossy().to_string(),
            success: false,
            error_message: None,
            hash_verified: true,
            operation_type: OperationType::Copy,
            files_processed: 0,
            total_size: 0,
            start_time: SystemTime::now(),
            end_time: SystemTime::now(),
            details: details.clone(),
            file_list: Vec::new(),
        };

        let mut all_successful = true;
        let mut error_messages = Vec::new();

        details.push("  Starting directory copy...".to_string());

        // Prepare a shared rate limiter for the whole directory copy
        let effective_bps = Self::compute_effective_bps(&operation.rate_limit, global_rate_limit);
        let mut dir_rate_limiter = RateLimiter::new(effective_bps, None);
        if dir_rate_limiter.is_enabled() {
            if let Some(limit) = dir_rate_limiter.get_rate_limit() {
                details.push(format!(
                    "  Directory rate limiting: {} bytes/second ({:.2} MB/min)",
                    limit,
                    limit as f64 * 60.0 / (1024.0 * 1024.0)
                ));
            }
        }

        if let Err(e) = fs::create_dir_all(&operation.destination) {
            let error_msg = format!("Failed to create destination directory: {}", e);
            details.push(format!("ERROR: {}", error_msg));
            result.error_message = Some(error_msg.clone()); // Clone here
            result.details = details;
            return result;
        }
        details.push("  Destination directory created".to_string());

        for entry in WalkDir::new(&operation.origin) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    error_messages.push(format!("Error reading directory entry: {}", e));
                    details.push(format!("WARNING: Error reading entry: {}", e));
                    all_successful = false;
                    continue;
                }
            };

            let source_path = entry.path();

            if source_path == operation.origin {
                continue;
            }

            let relative_path = match source_path.strip_prefix(&operation.origin) {
                Ok(p) => p,
                Err(_) => {
                    let msg = format!("Failed to get relative path for: {}", source_path.display());
                    error_messages.push(msg.clone());
                    details.push(format!("WARNING: {}", msg));
                    continue;
                }
            };

            let dest_path = operation.destination.join(relative_path);

            if entry.file_type().is_dir() {
                if let Err(e) = fs::create_dir_all(&dest_path) {
                    let msg = format!("Failed to create directory {}: {}", dest_path.display(), e);
                    error_messages.push(msg.clone());
                    details.push(format!("ERROR creating directory: {}", msg));
                    all_successful = false;
                } else {
                    details.push(format!("  Created directory: {}", dest_path.display()));
                }
            } else if entry.file_type().is_file() {
                result.files_processed += 1;

                let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                result.total_size += file_size;

                details.push(format!(
                    "  Copying file {}/{}: {}",
                    result.files_processed,
                    "?",
                    source_path.display()
                ));

                let copy_res: io::Result<u64> = if dir_rate_limiter.is_enabled() {
                    Self::copy_file_with_rate_limit(source_path, &dest_path, &mut dir_rate_limiter)
                } else {
                    fs::copy(source_path, &dest_path)
                };

                match copy_res {
                    Ok(bytes_copied) => {
                        details.push(format!("    Copied {} bytes", bytes_copied));

                        match validation::verify_files_match(source_path, &dest_path) {
                            Ok(true) => {
                                details.push("    Verification successful".to_string());

                                result.file_list.push(FileEntry {
                                    source_path: source_path.to_string_lossy().to_string(),
                                    destination_path: dest_path.to_string_lossy().to_string(),
                                    size: bytes_copied,
                                    hash_verified: true,
                                    success: true,
                                    error_message: None,
                                });
                            }
                            Ok(false) => {
                                let msg = format!(
                                    "Hash verification failed for: {}",
                                    source_path.display()
                                );
                                error_messages.push(msg.clone());
                                details.push(format!("ERROR: {}", msg));
                                all_successful = false;
                                result.hash_verified = false;
                                let _ = fs::remove_file(&dest_path);
                                details.push("    Cleaned up failed copy".to_string());

                                result.file_list.push(FileEntry {
                                    source_path: source_path.to_string_lossy().to_string(),
                                    destination_path: dest_path.to_string_lossy().to_string(),
                                    size: bytes_copied,
                                    hash_verified: false,
                                    success: false,
                                    error_message: Some("Hash verification failed".to_string()),
                                });
                            }
                            Err(e) => {
                                let msg = format!(
                                    "Verification error for {}: {}",
                                    source_path.display(),
                                    e
                                );
                                error_messages.push(msg.clone());
                                details.push(format!("ERROR: {}", msg));
                                all_successful = false;
                                result.hash_verified = false;
                                let _ = fs::remove_file(&dest_path);
                                details.push("    Cleaned up failed copy".to_string());

                                result.file_list.push(FileEntry {
                                    source_path: source_path.to_string_lossy().to_string(),
                                    destination_path: dest_path.to_string_lossy().to_string(),
                                    size: bytes_copied,
                                    hash_verified: false,
                                    success: false,
                                    error_message: Some(format!("Verification error: {}", e)),
                                });
                            }
                        }
                    }
                    Err(e) => {
                        let msg = format!(
                            "Failed to copy {} to {}: {}",
                            source_path.display(),
                            dest_path.display(),
                            e
                        );
                        error_messages.push(msg.clone());
                        details.push(format!("ERROR: {}", msg));
                        all_successful = false;

                        result.file_list.push(FileEntry {
                            source_path: source_path.to_string_lossy().to_string(),
                            destination_path: dest_path.to_string_lossy().to_string(),
                            size: file_size,
                            hash_verified: false,
                            success: false,
                            error_message: Some(msg),
                        });
                    }
                }
            }
        }

        details.push(format!(
            "  Total files processed: {}",
            result.files_processed
        ));
        details.push(format!("  Total size: {} bytes", result.total_size));

        result.success = all_successful;
        if !error_messages.is_empty() {
            result.error_message = Some(error_messages.join("; "));
        }
        result.details = details;

        result
    }

    fn move_file(operation: &FileOperation, mut details: Vec<String>) -> OperationResult {
        let mut result = OperationResult {
            operation_name: operation.name.clone(),
            source: operation.origin.to_string_lossy().to_string(),
            destination: operation.destination.to_string_lossy().to_string(),
            success: false,
            error_message: None,
            hash_verified: true,
            operation_type: OperationType::Move,
            files_processed: 1,
            total_size: 0,
            start_time: SystemTime::now(),
            end_time: SystemTime::now(),
            details: details.clone(),
            file_list: Vec::new(),
        };

        let file_size = if let Ok(metadata) = std::fs::metadata(&operation.origin) {
            metadata.len()
        } else {
            0
        };
        result.total_size = file_size;
        details.push(format!("  File size: {} bytes", result.total_size));

        details.push("  Starting file move...".to_string());

        if operation.destination.exists() {
            details.push("  WARNING: Destination already exists".to_string());

            match fs::remove_file(&operation.destination) {
                Ok(_) => {
                    details.push("  Removed existing destination file".to_string());
                }
                Err(e) => {
                    let error_msg = format!(
                        "Cannot move: destination exists and cannot be removed: {}",
                        e
                    );
                    details.push(format!("ERROR: {}", error_msg));
                    result.error_message = Some(error_msg.clone()); // Clone here
                    result.details = details;
                    return result;
                }
            }
        }

        match fs::rename(&operation.origin, &operation.destination) {
            Ok(_) => {
                details.push("  Move operation completed".to_string());
                result.success = operation.destination.exists();
                if result.success {
                    details.push("  Verification: Destination exists".to_string());

                    result.file_list.push(FileEntry {
                        source_path: operation.origin.to_string_lossy().to_string(),
                        destination_path: operation.destination.to_string_lossy().to_string(),
                        size: file_size,
                        hash_verified: true,
                        success: true,
                        error_message: None,
                    });
                } else {
                    let error_msg = "Destination file doesn't exist after move".to_string();
                    details.push(format!("ERROR: {}", error_msg));
                    result.error_message = Some(error_msg.clone()); // Clone here
                }
            }
            Err(e) => {
                let error_msg = format!(
                    "Move failed: {} (from {} to {})",
                    e,
                    operation.origin.display(),
                    operation.destination.display()
                );
                details.push(format!("ERROR: {}", error_msg));
                result.error_message = Some(error_msg.clone()); // Clone here

                result.file_list.push(FileEntry {
                    source_path: operation.origin.to_string_lossy().to_string(),
                    destination_path: operation.destination.to_string_lossy().to_string(),
                    size: file_size,
                    hash_verified: false,
                    success: false,
                    error_message: Some(error_msg), // Use the original
                });

                if e.kind() == io::ErrorKind::PermissionDenied {
                    details.push("  Permission denied - check file permissions".to_string());
                } else if e.kind() == io::ErrorKind::CrossesDevices {
                    details.push("  Cannot move across devices - use copy instead".to_string());
                } else if e.kind() == io::ErrorKind::NotFound {
                    details.push("  Source not found - check path".to_string());
                }
            }
        }

        result.details = details;
        result
    }

    fn move_directory(operation: &FileOperation, mut details: Vec<String>) -> OperationResult {
        let mut result = OperationResult {
            operation_name: operation.name.clone(),
            source: operation.origin.to_string_lossy().to_string(),
            destination: operation.destination.to_string_lossy().to_string(),
            success: false,
            error_message: None,
            hash_verified: true,
            operation_type: OperationType::Move,
            files_processed: 0,
            total_size: 0,
            start_time: SystemTime::now(),
            end_time: SystemTime::now(),
            details: details.clone(),
            file_list: Vec::new(),
        };

        details.push("  Starting directory move...".to_string());

        if operation.destination.exists() {
            details.push("  WARNING: Destination already exists".to_string());

            if operation.origin.canonicalize().ok() == operation.destination.canonicalize().ok() {
                let error_msg = "Source and destination are the same directory".to_string();
                details.push(format!("ERROR: {}", error_msg));
                result.error_message = Some(error_msg.clone()); // Clone here
                result.details = details;
                return result;
            }

            match fs::remove_dir_all(&operation.destination) {
                Ok(_) => {
                    details.push("  Removed existing destination directory".to_string());
                }
                Err(e) => {
                    let error_msg = format!(
                        "Cannot move: destination exists and cannot be removed: {}",
                        e
                    );
                    details.push(format!("ERROR: {}", error_msg));
                    result.error_message = Some(error_msg.clone()); // Clone here
                    result.details = details;
                    return result;
                }
            }
        }

        match fs::rename(&operation.origin, &operation.destination) {
            Ok(_) => {
                details.push("  Move operation completed".to_string());
                result.success = operation.destination.exists();
                if result.success {
                    details.push("  Verification: Destination exists".to_string());
                    for entry in WalkDir::new(&operation.destination) {
                        if let Ok(entry) = entry {
                            if entry.file_type().is_file() {
                                result.files_processed += 1;
                                if let Ok(metadata) = entry.metadata() {
                                    result.total_size += metadata.len();

                                    let source_path = entry.path();
                                    let relative_path = source_path
                                        .strip_prefix(&operation.destination)
                                        .ok()
                                        .map(|p| p.to_string_lossy().to_string())
                                        .unwrap_or_else(|| {
                                            source_path.to_string_lossy().to_string()
                                        });

                                    let original_source = operation.origin.join(&relative_path);

                                    result.file_list.push(FileEntry {
                                        source_path: original_source.to_string_lossy().to_string(),
                                        destination_path: source_path.to_string_lossy().to_string(),
                                        size: metadata.len(),
                                        hash_verified: true,
                                        success: true,
                                        error_message: None,
                                    });
                                }
                            }
                        }
                    }
                    details.push(format!("  Files moved: {}", result.files_processed));
                    details.push(format!("  Total size: {} bytes", result.total_size));
                } else {
                    let error_msg = "Destination directory doesn't exist after move".to_string();
                    details.push(format!("ERROR: {}", error_msg));
                    result.error_message = Some(error_msg.clone()); // Clone here
                }
            }
            Err(e) => {
                let error_msg = format!(
                    "Move failed: {} (from {} to {})",
                    e,
                    operation.origin.display(),
                    operation.destination.display()
                );
                details.push(format!("ERROR: {}", error_msg));
                result.error_message = Some(error_msg.clone()); // Clone here

                if e.kind() == io::ErrorKind::PermissionDenied {
                    details.push("  Permission denied - check directory permissions".to_string());
                } else if e.kind() == io::ErrorKind::CrossesDevices {
                    details.push("  Cannot move across devices - use copy instead".to_string());
                } else if e.kind() == io::ErrorKind::NotFound {
                    details.push("  Source not found - check path".to_string());
                } else if e.kind() == io::ErrorKind::InvalidInput {
                    details.push(
                        "  Invalid operation - check if destination is a subdirectory of source"
                            .to_string(),
                    );
                }
            }
        }

        result.details = details;
        result
    }

    pub fn generate_report(results: &[OperationResult]) -> String {
        let mut report = String::new();
        report.push_str("File Operation Report\n");
        report.push_str("=====================\n\n");

        let successful: Vec<_> = results.iter().filter(|r| r.success).collect();
        let failed: Vec<_> = results.iter().filter(|r| !r.success).collect();

        report.push_str(&format!("Total Operations: {}\n", results.len()));
        report.push_str(&format!("Successful: {}\n", successful.len()));
        report.push_str(&format!("Failed: {}\n\n", failed.len()));

        let total_files: usize = results.iter().map(|r| r.files_processed).sum();
        let total_size: u64 = results.iter().map(|r| r.total_size).sum();

        report.push_str(&format!("Total Files Processed: {}\n", total_files));
        report.push_str(&format!(
            "Total Data Size: {} bytes ({:.2} MB)\n\n",
            total_size,
            total_size as f64 / (1024.0 * 1024.0)
        ));

        if !successful.is_empty() {
            report.push_str("Successful Operations:\n");
            for result in successful {
                report.push_str(&format!(
                    "  ✓ {}: {} -> {}\n",
                    result.operation_name, result.source, result.destination
                ));
                report.push_str(&format!(
                    "    Files: {}, Size: {} bytes, Verified: {}\n",
                    result.files_processed,
                    result.total_size,
                    if result.hash_verified { "✓" } else { "✗" }
                ));
            }
            report.push_str("\n");
        }

        if !failed.is_empty() {
            report.push_str("Failed Operations:\n");
            for result in failed {
                report.push_str(&format!(
                    "  ✗ {}: {} -> {}\n",
                    result.operation_name, result.source, result.destination
                ));
                if let Some(err) = &result.error_message {
                    if err.len() > 80 {
                        let chunks: Vec<&str> = err.split(';').collect();
                        for (i, chunk) in chunks.iter().enumerate() {
                            if i == 0 {
                                report.push_str(&format!("    Error: {}\n", chunk.trim()));
                            } else {
                                report.push_str(&format!("           {}\n", chunk.trim()));
                            }
                        }
                    } else {
                        report.push_str(&format!("    Error: {}\n", err));
                    }
                }
                report.push_str(&format!(
                    "    Files Processed: {}, Size: {} bytes\n",
                    result.files_processed, result.total_size
                ));
            }
        }

        report
    }

    pub fn generate_detailed_report(
        results: &[OperationResult],
        destination_dir: &Path,
    ) -> anyhow::Result<String> {
        use chrono::{DateTime, Local};
        use std::env;

        let now: DateTime<Local> = Local::now();
        let hostname = whoami::fallible::hostname().unwrap_or_else(|_| "unknown".to_string());
        let username = whoami::username();
        let platform = whoami::platform();

        let mut report = String::new();
        report.push_str(&"=".repeat(80));
        report.push('\n');
        report.push_str("                   DETAILED FILE OPERATION REPORT\n");
        report.push_str(&"=".repeat(80));
        report.push_str("\n\n");

        report.push_str("SYSTEM INFORMATION\n");
        report.push_str(&"-".repeat(40));
        report.push('\n');
        report.push_str(&format!(
            "Report Generated: {}\n",
            now.format("%Y-%m-%d %H:%M:%S")
        ));
        report.push_str(&format!("System: {}\n", platform));
        report.push_str(&format!("Hostname: {}\n", hostname));
        report.push_str(&format!("Username: {}\n", username));
        report.push_str(&format!(
            "Current Directory: {}\n",
            env::current_dir()?.display()
        ));
        report.push_str(&format!(
            "Report Directory: {}\n",
            destination_dir.display()
        ));
        report.push('\n');

        report.push_str("OPERATION SUMMARY\n");
        report.push_str(&"-".repeat(40));
        report.push('\n');

        let successful: Vec<_> = results.iter().filter(|r| r.success).collect();
        let failed: Vec<_> = results.iter().filter(|r| !r.success).collect();
        let total_files: usize = results.iter().map(|r| r.files_processed).sum();
        let total_size: u64 = results.iter().map(|r| r.total_size).sum();
        let total_duration: u128 = results
            .iter()
            .map(|r| {
                r.end_time
                    .duration_since(r.start_time)
                    .unwrap_or_default()
                    .as_millis()
            })
            .sum();

        report.push_str(&format!("Total Operations: {}\n", results.len()));
        report.push_str(&format!(
            "Successful: {} ({}%)\n",
            successful.len(),
            (successful.len() as f32 / results.len() as f32 * 100.0) as u32
        ));
        report.push_str(&format!(
            "Failed: {} ({}%)\n",
            failed.len(),
            (failed.len() as f32 / results.len() as f32 * 100.0) as u32
        ));
        report.push_str(&format!("Total Files Processed: {}\n", total_files));
        report.push_str(&format!(
            "Total Data Size: {} bytes ({:.2} MB)\n",
            total_size,
            total_size as f64 / (1024.0 * 1024.0)
        ));
        report.push_str(&format!("Total Duration: {} ms\n", total_duration));
        report.push('\n');

        report.push_str("DETAILED RESULTS\n");
        report.push_str(&"-".repeat(40));
        report.push('\n');

        for (i, result) in results.iter().enumerate() {
            report.push_str(&format!("\n{}. {}\n", i + 1, result.operation_name));
            report.push_str(&"=".repeat(result.operation_name.len() + 3));
            report.push('\n');

            report.push_str(&format!(
                "   Status: {}\n",
                if result.success { "SUCCESS" } else { "FAILED" }
            ));
            report.push_str(&format!("   Type: {:?}\n", result.operation_type));
            report.push_str(&format!("   Source: {}\n", result.source));
            report.push_str(&format!("   Destination: {}\n", result.destination));

            let duration = result
                .end_time
                .duration_since(result.start_time)
                .unwrap_or_default()
                .as_millis();
            report.push_str(&format!("   Duration: {} ms\n", duration));
            report.push_str(&format!(
                "   Files: {}, Size: {} bytes\n",
                result.files_processed, result.total_size
            ));
            report.push_str(&format!(
                "   Hash Verified: {}\n",
                if result.hash_verified { "Yes" } else { "No" }
            ));

            if let Some(err) = &result.error_message {
                report.push_str(&format!("   Error: {}\n", err));
            }

            if !result.file_list.is_empty() {
                report.push_str("\n   File List:\n");
                for (file_idx, file_entry) in result.file_list.iter().enumerate() {
                    let status = if file_entry.success { "✓" } else { "✗" };
                    report.push_str(&format!(
                        "     {}. {} {} -> {}\n",
                        file_idx + 1,
                        status,
                        file_entry.source_path,
                        file_entry.destination_path
                    ));
                }
            }

            report.push_str("\n   Operation Log:\n");
            for detail in &result.details {
                report.push_str(&format!("     {}\n", detail));
            }

            report.push('\n');
        }

        let timestamp = now.format("%Y%m%d_%H%M%S");
        let report_filename =
            destination_dir.join(format!("file_operations_report_{}.txt", timestamp));

        match std::fs::write(&report_filename, &report) {
            Ok(_) => {
                report.push('\n');
                report.push_str("REPORT FILE\n");
                report.push_str(&"-".repeat(40));
                report.push('\n');
                report.push_str(&format!(
                    "Detailed report saved to: {}\n",
                    report_filename.display()
                ));
            }
            Err(e) => {
                report.push_str(&format!("\nWARNING: Could not save report file: {}\n", e));
            }
        }

        Ok(report)
    }

    pub fn save_operation_reports_to_destinations(
        results: &[OperationResult],
    ) -> anyhow::Result<Vec<String>> {
        use chrono::{DateTime, Local};
        use std::fs;

        let now: DateTime<Local> = Local::now();
        let mut saved_paths = Vec::new();

        for (i, result) in results.iter().enumerate() {
            let dest_path = Path::new(&result.destination);
            let report_dir = if dest_path.is_dir() {
                dest_path.to_path_buf()
            } else {
                dest_path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| Path::new(".").to_path_buf())
            };

            if !report_dir.exists() {
                if let Err(e) = fs::create_dir_all(&report_dir) {
                    saved_paths.push(format!(
                        "✗ Could not create directory for operation {}: {}",
                        i + 1,
                        e
                    ));
                    continue;
                }
            }

            let mut operation_report = String::new();
            operation_report.push_str(&"=".repeat(80));
            operation_report.push('\n');
            operation_report.push_str(&format!(
                "            OPERATION REPORT: {}\n",
                result.operation_name
            ));
            operation_report.push_str(&"=".repeat(80));
            operation_report.push_str("\n\n");

            operation_report.push_str("OPERATION DETAILS\n");
            operation_report.push_str(&"-".repeat(40));
            operation_report.push('\n');

            operation_report.push_str(&format!("Name: {}\n", result.operation_name));
            operation_report.push_str(&format!(
                "Status: {}\n",
                if result.success { "SUCCESS" } else { "FAILED" }
            ));
            operation_report.push_str(&format!("Type: {:?}\n", result.operation_type));
            operation_report.push_str(&format!("Source: {}\n", result.source));
            operation_report.push_str(&format!("Destination: {}\n", result.destination));

            let duration = result
                .end_time
                .duration_since(result.start_time)
                .unwrap_or_default()
                .as_millis();
            operation_report.push_str(&format!("Duration: {} ms\n", duration));
            operation_report.push_str(&format!("Files Processed: {}\n", result.files_processed));
            operation_report.push_str(&format!("Total Size: {} bytes\n", result.total_size));
            operation_report.push_str(&format!(
                "Hash Verified: {}\n",
                if result.hash_verified { "Yes" } else { "No" }
            ));

            if let Some(err) = &result.error_message {
                operation_report.push_str(&format!("Error: {}\n", err));
            }

            if !result.file_list.is_empty() {
                operation_report.push_str("\nFILE LIST:\n");
                operation_report.push_str(&"-".repeat(40));
                operation_report.push('\n');

                for (file_idx, file_entry) in result.file_list.iter().enumerate() {
                    let status = if file_entry.success { "✓" } else { "✗" };
                    operation_report.push_str(&format!(
                        "{}. {} {} -> {}\n",
                        file_idx + 1,
                        status,
                        file_entry.source_path,
                        file_entry.destination_path
                    ));
                }
            }

            operation_report.push('\n');
            operation_report.push_str("SYSTEM INFORMATION\n");
            operation_report.push_str(&"-".repeat(40));
            operation_report.push('\n');

            operation_report.push_str(&format!(
                "Report Generated: {}\n",
                now.format("%Y-%m-%d %H:%M:%S")
            ));
            operation_report.push_str(&format!(
                "Hostname: {}\n",
                whoami::fallible::hostname().unwrap_or_else(|_| "unknown".to_string())
            ));
            operation_report.push_str(&format!("Username: {}\n", whoami::username()));
            operation_report.push_str(&format!("System: {}\n", whoami::platform()));

            operation_report.push('\n');
            operation_report.push_str("OPERATION LOG\n");
            operation_report.push_str(&"-".repeat(40));
            operation_report.push('\n');

            for detail in &result.details {
                operation_report.push_str(&format!("{}\n", detail));
            }

            let timestamp = now.format("%Y%m%d_%H%M%S");
            let report_filename = report_dir.join(format!(
                "operation_{}_{}_{}.txt",
                i + 1,
                result.operation_name.replace(" ", "_").to_lowercase(),
                timestamp
            ));

            match fs::write(&report_filename, &operation_report) {
                Ok(_) => {
                    saved_paths.push(format!("✓ Report saved to: {}", report_filename.display()));
                }
                Err(e) => {
                    saved_paths.push(format!(
                        "✗ Failed to save report for {}: {}",
                        result.operation_name, e
                    ));
                }
            }
        }

        Ok(saved_paths)
    }

    pub fn generate_file_list_report(results: &[OperationResult]) -> String {
        let mut report = String::new();
        report.push_str("FILE LIST REPORT\n");
        report.push_str(&"=".repeat(80));
        report.push_str("\n\n");

        let total_files: usize = results.iter().map(|r| r.file_list.len()).sum();
        report.push_str(&format!("Total Files Listed: {}\n\n", total_files));

        for (op_idx, result) in results.iter().enumerate() {
            report.push_str(&format!(
                "Operation {}: {}\n",
                op_idx + 1,
                result.operation_name
            ));
            report.push_str(&format!(
                "Type: {:?}, Status: {}\n",
                result.operation_type,
                if result.success { "SUCCESS" } else { "FAILED" }
            ));
            report.push_str(&format!("Source: {}\n", result.source));
            report.push_str(&format!("Destination: {}\n", result.destination));

            if !result.file_list.is_empty() {
                report.push_str("\nFiles:\n");
                report.push_str(&"-".repeat(40));
                report.push('\n');

                for (file_idx, file_entry) in result.file_list.iter().enumerate() {
                    let status = if file_entry.success { "✓" } else { "✗" };
                    let verified = if file_entry.hash_verified {
                        "✓"
                    } else {
                        "✗"
                    };

                    report.push_str(&format!(
                        "{}. {} {} -> {}\n",
                        file_idx + 1,
                        status,
                        file_entry.source_path,
                        file_entry.destination_path
                    ));
                    report.push_str(&format!(
                        "   Size: {} bytes, Verified: {}\n",
                        file_entry.size, verified
                    ));

                    if let Some(err) = &file_entry.error_message {
                        report.push_str(&format!("   Error: {}\n", err));
                    }
                    report.push('\n');
                }
            } else {
                report.push_str("\nNo files processed in this operation.\n");
            }

            report.push_str(&"=".repeat(80));
            report.push_str("\n\n");
        }

        report
    }

    pub fn save_file_list_reports(results: &[OperationResult]) -> anyhow::Result<Vec<String>> {
        use chrono::{DateTime, Local};
        use std::fs;

        let now: DateTime<Local> = Local::now();
        let mut saved_paths = Vec::new();

        let overall_report = Self::generate_file_list_report(results);
        let timestamp = now.format("%Y%m%d_%H%M%S");
        let overall_filename = format!("file_list_report_{}.txt", timestamp);

        if let Err(e) = fs::write(&overall_filename, &overall_report) {
            saved_paths.push(format!("✗ Failed to save overall file list report: {}", e));
        } else {
            saved_paths.push(format!(
                "✓ Overall file list report saved to: {}",
                overall_filename
            ));
        }

        for (i, result) in results.iter().enumerate() {
            if !result.file_list.is_empty() {
                let dest_path = Path::new(&result.destination);
                let report_dir = if dest_path.is_dir() {
                    dest_path.to_path_buf()
                } else {
                    dest_path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| Path::new(".").to_path_buf())
                };

                if !report_dir.exists() {
                    if let Err(e) = fs::create_dir_all(&report_dir) {
                        saved_paths.push(format!(
                            "✗ Could not create directory for operation {}: {}",
                            i + 1,
                            e
                        ));
                        continue;
                    }
                }

                let mut operation_file_report = String::new();
                operation_file_report.push_str(&format!("FILE LIST: {}\n", result.operation_name));
                operation_file_report.push_str(&"=".repeat(80));
                operation_file_report.push_str("\n\n");

                operation_file_report.push_str(&format!("Operation: {}\n", result.operation_name));
                operation_file_report.push_str(&format!("Type: {:?}\n", result.operation_type));
                operation_file_report.push_str(&format!(
                    "Status: {}\n",
                    if result.success { "SUCCESS" } else { "FAILED" }
                ));
                operation_file_report.push_str(&format!("Source: {}\n", result.source));
                operation_file_report.push_str(&format!("Destination: {}\n", result.destination));
                operation_file_report
                    .push_str(&format!("Total Files: {}\n", result.file_list.len()));
                operation_file_report
                    .push_str(&format!("Total Size: {} bytes\n\n", result.total_size));

                operation_file_report.push_str("FILE DETAILS:\n");
                operation_file_report.push_str(&"-".repeat(40));
                operation_file_report.push('\n');

                for (file_idx, file_entry) in result.file_list.iter().enumerate() {
                    let status = if file_entry.success {
                        "SUCCESS"
                    } else {
                        "FAILED"
                    };
                    let verified = if file_entry.hash_verified {
                        "VERIFIED"
                    } else {
                        "NOT VERIFIED"
                    };

                    operation_file_report.push_str(&format!("\n{}. {}\n", file_idx + 1, status));
                    operation_file_report
                        .push_str(&format!("   Source: {}\n", file_entry.source_path));
                    operation_file_report.push_str(&format!(
                        "   Destination: {}\n",
                        file_entry.destination_path
                    ));
                    operation_file_report
                        .push_str(&format!("   Size: {} bytes\n", file_entry.size));
                    operation_file_report.push_str(&format!("   Status: {}\n", verified));

                    if let Some(err) = &file_entry.error_message {
                        operation_file_report.push_str(&format!("   Error: {}\n", err));
                    }
                }

                let operation_filename = report_dir.join(format!(
                    "file_list_{}_{}.txt",
                    result.operation_name.replace(" ", "_").to_lowercase(),
                    timestamp
                ));

                match fs::write(&operation_filename, &operation_file_report) {
                    Ok(_) => {
                        saved_paths.push(format!(
                            "✓ File list for '{}' saved to: {}",
                            result.operation_name,
                            operation_filename.display()
                        ));
                    }
                    Err(e) => {
                        saved_paths.push(format!(
                            "✗ Failed to save file list for '{}': {}",
                            result.operation_name, e
                        ));
                    }
                }
            }
        }

        Ok(saved_paths)
    }
}
