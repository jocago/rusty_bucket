mod config;
mod file_ops;
mod rate_limiter;
mod ui;
mod validation;

use clap::{Arg, Command};
use std::fs;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let matches = Command::new("File Manager")
        .version("1.0")
        .author("Your Name")
        .about("Manages file operations with validation")
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Sets a custom config file")
                .default_value("config.yaml"),
        )
        .arg(
            Arg::new("batch")
                .short('b')
                .long("batch")
                .help("Run in batch mode (no UI)")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .help("Show verbose output")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("report-dir")
                .short('r')
                .long("report-dir")
                .value_name("DIRECTORY")
                .help("Directory to save detailed reports")
                .default_value("."),
        )
        .get_matches();

    let config_path = matches.get_one::<String>("config").unwrap();
    let batch_mode = matches.get_flag("batch");
    let verbose = matches.get_flag("verbose");
    let report_dir = matches.get_one::<String>("report-dir").unwrap();

    let config = match config::Config::load_from_file(config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            if verbose {
                println!("Failed to load config from '{}': {}", config_path, e);
            }
            println!("Config file not found or invalid. Creating default config...");
            let default_config = create_default_config();
            if let Err(e) = default_config.save_to_file(config_path) {
                println!("Warning: Could not save default config: {}", e);
            } else {
                println!("Default config created at '{}'", config_path);
            }
            default_config
        }
    };

    if verbose {
        println!("Loaded configuration:");
        println!("  Config file: {}", config_path);
        println!("  Operations configured: {}", config.operations.len());
        for (i, op) in config.operations.iter().enumerate() {
            println!("  Operation {}: {}", i + 1, op.name);
            println!("    From: {}", op.origin.display());
            println!("    To: {}", op.destination.display());
            println!("    Type: {:?}", op.operation_type);
        }
        println!();
    }

    if batch_mode {
        run_batch_mode(&config, verbose, report_dir)
    } else {
        run_ui_mode(&config, report_dir)
    }
}

fn run_batch_mode(config: &config::Config, verbose: bool, report_dir: &str) -> anyhow::Result<()> {
    println!("Starting batch operations...");

    if verbose {
        println!("Operations to execute:");
        for (i, op) in config.operations.iter().enumerate() {
            println!(
                "  {}. {}: {} -> {} ({})",
                i + 1,
                op.name,
                op.origin.display(),
                op.destination.display(),
                match op.operation_type {
                    config::OperationType::Copy => "Copy",
                    config::OperationType::Move => "Move",
                }
            );

            if op.origin.exists() {
                if op.origin.is_dir() {
                    println!("    Source is a directory");
                    let mut file_count = 0;
                    let mut total_size = 0;
                    if let Ok(entries) = std::fs::read_dir(&op.origin) {
                        for entry in entries.flatten() {
                            if let Ok(metadata) = entry.metadata() {
                                if metadata.is_file() {
                                    file_count += 1;
                                    total_size += metadata.len();
                                }
                            }
                        }
                    }
                    println!("    Contains approximately {} files", file_count);
                    println!("    Total size: {} bytes", total_size);
                } else if op.origin.is_file() {
                    println!("    Source is a file");
                    if let Ok(metadata) = std::fs::metadata(&op.origin) {
                        println!("    Size: {} bytes", metadata.len());
                        println!("    Permissions: {:?}", metadata.permissions());
                    }
                } else {
                    println!("    Source exists but is not a regular file or directory");
                }
            } else {
                println!("    WARNING: Source does not exist!");
            }

            if op.destination.exists() {
                println!("    Destination already exists");
            } else {
                println!("    Destination will be created");
            }

            if let Some(parent) = op.destination.parent() {
                if parent.exists() {
                    if let Ok(metadata) = std::fs::metadata(parent) {
                        println!(
                            "    Destination parent directory permissions: {:?}",
                            metadata.permissions()
                        );
                    }
                } else {
                    println!("    Destination parent directory does not exist, will be created");
                }
            }
        }
        println!();
    }

    let results = file_ops::FileManager::execute_operations(&config.operations, None);

    let summary_report = file_ops::FileManager::generate_report(&results);
    println!("{}", summary_report);

    let report_path = PathBuf::from(report_dir);
    if !report_path.exists() {
        if let Err(e) = fs::create_dir_all(&report_path) {
            println!(
                "Warning: Could not create report directory '{}': {}",
                report_dir, e
            );
            println!("Saving report to current directory instead.");
        }
    }

    match file_ops::FileManager::generate_detailed_report(&results, &report_path) {
        Ok(detailed_report) => {
            let lines: Vec<&str> = detailed_report.lines().collect();
            let display_lines = lines.len().min(50);
            for line in lines.iter().take(display_lines) {
                println!("{}", line);
            }
            if lines.len() > display_lines {
                println!("... (full report saved to file)");
            }
        }
        Err(e) => {
            println!("Warning: Could not generate detailed report: {}", e);
        }
    }

    println!("\nSaving operation reports to destination folders:");
    match file_ops::FileManager::save_operation_reports_to_destinations(&results) {
        Ok(saved_paths) => {
            for path in saved_paths {
                println!("  {}", path);
            }
        }
        Err(e) => {
            println!("Warning: Could not save operation reports: {}", e);
        }
    }

    println!("\nSaving file list reports:");
    match file_ops::FileManager::save_file_list_reports(&results) {
        Ok(saved_paths) => {
            for path in saved_paths {
                println!("  {}", path);
            }
        }
        Err(e) => {
            println!("Warning: Could not save file list reports: {}", e);
        }
    }

    let summary_filename = report_path.join("operation_summary.txt");
    if let Err(e) = std::fs::write(&summary_filename, &summary_report) {
        println!(
            "Warning: Could not save summary report to '{}': {}",
            summary_filename.display(),
            e
        );
    } else {
        println!("\nSummary report saved to {}", summary_filename.display());
    }

    let successful = results.iter().filter(|r| r.success).count();
    let total = results.len();

    if successful == total {
        println!("\n✓ All operations completed successfully!");
    } else {
        println!("\n⚠ {}/{} operations failed.", total - successful, total);
        println!("Check the reports for detailed error information.");

        println!("\nDetailed error information:");
        for (i, result) in results.iter().enumerate().filter(|(_, r)| !r.success) {
            println!("  Operation {}: {}", i + 1, result.operation_name);
            if let Some(err) = &result.error_message {
                println!("    Error: {}", err);

                if verbose {
                    println!("    Operation details:");
                    for detail in &result.details {
                        println!("      {}", detail);
                    }
                }
            }
        }
    }

    Ok(())
}

fn run_ui_mode(config: &config::Config, report_dir: &str) -> anyhow::Result<()> {
    ui::run_app(config.clone(), report_dir)
}

fn create_default_config() -> config::Config {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    config::Config {
        operations: vec![
            config::FileOperation {
                name: "Example Copy".to_string(),
                origin: current_dir.join("example_source.txt"),
                destination: current_dir.join("example_destination.txt"),
                operation_type: config::OperationType::Copy,
                rate_limit: config::RateLimit::default(),
            },
            config::FileOperation {
                name: "Example Move".to_string(),
                origin: current_dir.join("example_to_move.txt"),
                destination: current_dir.join("archive/example_moved.txt"),
                operation_type: config::OperationType::Move,
                rate_limit: config::RateLimit::default(),
            },
            config::FileOperation {
                name: "Backup Documents".to_string(),
                origin: current_dir.join("documents"),
                destination: current_dir.join("backup/documents"),
                operation_type: config::OperationType::Copy,
                rate_limit: config::RateLimit::default(),
            },
        ],
        global_rate_limit: config::RateLimit::default(),
    }
}
