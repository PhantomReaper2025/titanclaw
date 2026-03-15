//! CLI subcommand definitions for `ironclaw backup`.
//!
//! Provides backup and restore functionality for agent state:
//! - Database (libSQL/Postgres)
//! - Workspace files (~/.ironclaw/)
//! - Settings and secrets (encrypted)
//! - Memory, goals, plans tables

use std::path::PathBuf;

use clap::Subcommand;

/// Backup subcommands for exporting and importing agent state.
#[derive(Subcommand, Debug, Clone)]
pub enum BackupCommand {
    /// Create a new backup of current agent state.
    ///
    /// Saves database, workspace files, and settings to a timestamped archive.
    /// Secrets are exported in encrypted form.
    #[command(alias = "new")]
    Create {
        /// Optional name for the backup (defaults to timestamp).
        #[arg(short, long)]
        name: Option<String>,

        /// Output directory for the backup file.
        #[arg(short, long, default_value = "~/.ironclaw/backups")]
        output: PathBuf,

        /// Include full database dump (default is schema + data).
        #[arg(long)]
        full_dump: bool,

        /// Compress the backup archive (default: tar.gz).
        #[arg(long, default_value = "tar.gz")]
        format: String,
    },

    /// Restore agent state from a backup archive.
    ///
    /// Restores database, workspace files, and settings.
    /// Secrets require the same master key to decrypt.
    #[command(alias = "load")]
    Restore {
        /// Path to the backup archive file.
        #[arg(short, long)]
        file: PathBuf,

        /// Restore only specific components (db, workspace, settings, secrets).
        #[arg(short = 'C', long, value_delimiter = ',')]
        components: Vec<String>,

        /// Don't actually restore, just show what would be done.
        #[arg(long)]
        dry_run: bool,

        /// Force restore even if current state exists (will overwrite).
        #[arg(short, long)]
        force: bool,
    },

    /// List available backups.
    ///
    /// Shows backups in the default backup directory.
    #[command(alias = "ls")]
    List {
        /// Show backups in a different directory.
        #[arg(short, long)]
        dir: Option<PathBuf>,

        /// Show detailed information about each backup.
        #[arg(long)]
        verbose: bool,
    },

    /// Export a backup to a specific file path.
    ///
    /// Creates a backup and saves it to the specified location.
    /// Useful for external backup storage or transfer.
    #[command(alias = "save")]
    Export {
        /// Output file path for the backup.
        #[arg(short, long)]
        output: PathBuf,

        /// Optional name/label for the backup.
        #[arg(short, long)]
        name: Option<String>,

        /// Include secrets in the export (encrypted).
        #[arg(long, default_value = "true")]
        include_secrets: bool,
    },

    /// Import a backup from a file path.
    ///
    /// Imports a backup archive from an external location.
    /// Useful for restoring from external backup storage.
    #[command(alias = "load-file")]
    Import {
        /// Path to the backup archive to import.
        #[arg(short, long)]
        file: PathBuf,

        /// Restore after importing.
        #[arg(short, long)]
        restore: bool,

        /// Copy the backup to the default backup directory.
        #[arg(long, default_value = "true")]
        copy_to_default: bool,
    },
}

/// Run the backup command.
pub async fn run_backup_command(cmd: BackupCommand) -> anyhow::Result<()> {
    match cmd {
        BackupCommand::Create {
            name,
            output,
            full_dump,
            format,
        } => run_backup_create(name.as_deref(), &output, full_dump, &format).await,
        BackupCommand::Restore {
            file,
            components,
            dry_run,
            force,
        } => run_backup_restore(&file, &components, dry_run, force).await,
        BackupCommand::List { dir, verbose } => run_backup_list(dir.as_ref(), verbose).await,
        BackupCommand::Export {
            output,
            name,
            include_secrets,
        } => run_backup_export(&output, name.as_deref(), include_secrets).await,
        BackupCommand::Import {
            file,
            restore,
            copy_to_default,
        } => run_backup_import(&file, restore, copy_to_default).await,
    }
}

/// Create a new backup.
async fn run_backup_create(
    name: Option<&str>,
    output: &PathBuf,
    full_dump: bool,
    format: &str,
) -> anyhow::Result<()> {
    let output = expand_tilde(output);
    
    println!("Creating backup...");
    if let Some(n) = name {
        println!("  Name: {}", n);
    }
    println!("  Output: {}", output.display());
    println!("  Full dump: {}", full_dump);
    println!("  Format: {}", format);

    // Stub implementation - actual backup logic would:
    // 1. Connect to database and dump tables (settings, memory, goals, plans, etc.)
    // 2. Archive workspace files from ~/.ironclaw/
    // 3. Export secrets in encrypted form
    // 4. Create manifest.json with metadata
    // 5. Package into tar.gz or zip archive
    
    println!("\nBackup creation is not yet implemented.");
    println!("This is a stub - to be implemented with:");
    println!("  - Database dump (libSQL/Postgres)");
    println!("  - Workspace file archival");
    println!("  - Encrypted secrets export");
    println!("  - Manifest with version info");

    Ok(())
}

/// Restore from a backup archive.
async fn run_backup_restore(
    file: &PathBuf,
    components: &[String],
    dry_run: bool,
    force: bool,
) -> anyhow::Result<()> {
    let file = expand_tilde(file);
    
    println!("Restoring from backup...");
    println!("  File: {}", file.display());
    if !components.is_empty() {
        println!("  Components: {}", components.join(", "));
    } else {
        println!("  Components: all");
    }
    println!("  Dry run: {}", dry_run);
    println!("  Force: {}", force);

    // Stub implementation - actual restore logic would:
    // 1. Extract archive to temp directory
    // 2. Validate manifest.json
    // 3. Restore database tables (with conflict resolution)
    // 4. Restore workspace files
    // 5. Decrypt and restore secrets (requires master key)
    
    println!("\nBackup restore is not yet implemented.");
    println!("This is a stub - to be implemented with:");
    println!("  - Archive extraction and validation");
    println!("  - Database restore (with conflict handling)");
    println!("  - Workspace file restoration");
    println!("  - Secret decryption and import");

    Ok(())
}

/// List available backups.
async fn run_backup_list(dir: Option<&PathBuf>, verbose: bool) -> anyhow::Result<()> {
    let dir = dir
        .map(expand_tilde)
        .unwrap_or_else(|| default_backup_dir());

    println!("Backups in: {}", dir.display());
    println!();

    if !dir.exists() {
        println!("  No backup directory found.");
        println!("  Create one with: ironclaw backup create");
        return Ok(());
    }

    // Stub implementation - actual listing would:
    // 1. Scan directory for .tar.gz / .zip files
    // 2. Parse manifest.json from each archive
    // 3. Display metadata (name, date, size, components)
    
    println!("  Backup listing not yet implemented.");
    println!("  This is a stub - will list:");
    println!("    - Backup files in directory");
    println!("    - Metadata (name, timestamp, size)");
    if verbose {
        println!("    - Component details");
        println!("    - Database table counts");
    }

    Ok(())
}

/// Export a backup to a specific location.
async fn run_backup_export(
    output: &PathBuf,
    name: Option<&str>,
    include_secrets: bool,
) -> anyhow::Result<()> {
    let output = expand_tilde(output);
    
    println!("Exporting backup...");
    println!("  Output: {}", output.display());
    if let Some(n) = name {
        println!("  Name: {}", n);
    }
    println!("  Include secrets: {}", include_secrets);

    // Stub implementation - same as create but with explicit output path
    println!("\nBackup export is not yet implemented.");

    Ok(())
}

/// Import a backup from a file.
async fn run_backup_import(
    file: &PathBuf,
    restore: bool,
    copy_to_default: bool,
) -> anyhow::Result<()> {
    let file = expand_tilde(file);
    
    println!("Importing backup...");
    println!("  File: {}", file.display());
    println!("  Restore after import: {}", restore);
    println!("  Copy to default directory: {}", copy_to_default);

    // Stub implementation
    println!("\nBackup import is not yet implemented.");

    Ok(())
}

/// Expand tilde (~) in path to home directory.
fn expand_tilde(path: &PathBuf) -> PathBuf {
    if path.starts_with("~") {
        if let Some(home) = dirs::home_dir() {
            let without_tilde = path.strip_prefix("~").unwrap_or(path);
            return home.join(without_tilde);
        }
    }
    path.clone()
}

/// Default backup directory: ~/.ironclaw/backups
fn default_backup_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ironclaw")
        .join("backups")
}