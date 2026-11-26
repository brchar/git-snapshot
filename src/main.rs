//! Entry point for `git-snapshot`, wiring CLI arguments to [`snapshot::SnapshotStore`].

mod cli;
mod error;
mod git;
mod snapshot;

use clap::Parser;
use snafu::ResultExt;

use crate::cli::{Cli, Command};
use crate::error::Result;
use crate::git::GitRepo;
use crate::snapshot::{ApplyOutcome, PopOutcome, SnapshotStore};

#[snafu::report]
fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()
        .with_whatever_context(|_| "failed to read current working directory")?;
    let repo = GitRepo::discover(&cwd)?;
    let store = SnapshotStore::new(repo);

    match cli.command {
        Command::Save(args) => {
            let created = store.create(&args.into_create_request())?;
            println!(
                "Created snapshot '{}' ({})",
                created.name,
                created.oid.short()
            );
        }
        Command::Apply { name } => match store.apply(name.as_deref())? {
            ApplyOutcome::Applied { name, oid } => {
                println!("Applied snapshot '{name}' ({})", oid.short());
            }
            ApplyOutcome::AppliedWithoutIndex { name, oid } => {
                eprintln!(
                    "Warning: could not restore index state for '{name}'; applied changes to worktree only"
                );
                println!(
                    "Applied snapshot '{name}' ({}) (without index)",
                    oid.short()
                );
            }
        },
        Command::Pop { name } => match store.pop(name.as_deref())? {
            PopOutcome::Popped { name, oid } => {
                println!("Popped snapshot '{name}' ({})", oid.short());
            }
            PopOutcome::AppliedWithoutIndexKept { name, oid } => {
                eprintln!(
                    "Warning: could not restore index state for '{name}'; applied to worktree only and kept snapshot"
                );
                println!(
                    "Applied snapshot '{name}' ({}) without index (snapshot kept)",
                    oid.short()
                );
            }
        },
        Command::Drop { name } => {
            let dropped = store.drop(name.as_deref())?;
            println!(
                "Dropped snapshot '{}' ({})",
                dropped.name,
                dropped.oid.short()
            );
        }
        Command::List => {
            let entries = store.list()?;
            if let Some(name_width) = entries.iter().map(|e| e.name.as_str().len()).max() {
                for entry in entries {
                    println!(
                        "{:<width$}  {}  {}  {}",
                        entry.name.as_str(),
                        entry.oid.short(),
                        entry.relative_date,
                        entry.subject,
                        width = name_width
                    );
                }
            }
        }
    }

    Ok(())
}
