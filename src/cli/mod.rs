//! CLI argument definitions and conversion to snapshot domain requests.

use clap::{Args, Parser, Subcommand};

use crate::snapshot::{CreateRequest, SnapshotKind};

/// Magit-style non-destructive Git snapshots stored in `refs/wip/<name>`.
#[derive(Debug, Parser)]
#[command(
    name = "git-snapshot",
    about = "Magit-style non-destructive Git snapshots stored in refs/wip/<name>"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Encapsulated options for `git-snapshot save`, hiding mutually exclusive raw scope flags.
#[derive(Debug, Args, PartialEq, Eq)]
pub struct SaveArgs {
    /// Snapshot both staged (index) and unstaged (worktree) changes (default).
    #[arg(long, group = "scope")]
    both: bool,

    /// Snapshot only staged (index) changes.
    #[arg(long, group = "scope", conflicts_with = "include_untracked")]
    index: bool,

    /// Snapshot only unstaged (worktree) changes.
    #[arg(long, group = "scope")]
    worktree: bool,

    /// Include untracked, non-ignored files in the snapshot.
    #[arg(short = 'u', long)]
    include_untracked: bool,

    /// Custom message embedded in the snapshot commit subject.
    #[arg(short = 'm', long)]
    message: Option<String>,

    /// Overwrite an existing snapshot with the same name.
    #[arg(short = 'f', long)]
    force: bool,

    /// Snapshot name under `refs/wip/` (auto-generated if omitted).
    name: Option<String>,
}

impl SaveArgs {
    /// Normalizes CLI flags into a validated domain [`CreateRequest`].
    pub fn into_create_request(self) -> CreateRequest {
        let kind = if self.index {
            SnapshotKind::Index
        } else if self.worktree {
            SnapshotKind::Worktree {
                include_untracked: self.include_untracked,
            }
        } else {
            SnapshotKind::Both {
                include_untracked: self.include_untracked,
            }
        };

        CreateRequest {
            kind,
            name: self.name,
            message: self.message,
            force: self.force,
        }
    }
}

/// Supported subcommands mirroring Magit snapshot and stash operations.
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum Command {
    /// Create a non-destructive snapshot under `refs/wip/<name>`.
    Save(SaveArgs),

    /// Apply a snapshot without removing its ref (defaults to latest).
    Apply {
        /// Snapshot name under `refs/wip/` (latest if omitted).
        name: Option<String>,
    },

    /// Apply a snapshot and drop its ref if index state was restored cleanly (defaults to latest).
    Pop {
        /// Snapshot name under `refs/wip/` (latest if omitted).
        name: Option<String>,
    },

    /// Delete a snapshot ref (defaults to latest).
    Drop {
        /// Snapshot name under `refs/wip/` (latest if omitted).
        name: Option<String>,
    },

    /// List snapshots under `refs/wip/` ordered newest first.
    List,
}

#[cfg(test)]
mod test;
