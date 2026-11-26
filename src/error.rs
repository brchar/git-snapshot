//! Domain error types for `git-snapshot`.

use snafu::Snafu;

/// Crate-wide result type backed by [`enum@Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Domain and infrastructure errors surfaced by snapshot operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    /// Current directory is not inside a Git worktree.
    #[snafu(display("not inside a git worktree"))]
    NotAGitWorktree,

    /// Repository HEAD does not yet point to any commit.
    #[snafu(display("cannot create snapshot: HEAD does not point to a commit yet"))]
    UnbornHead,

    /// Repository index contains unresolved merge conflicts.
    #[snafu(display("cannot create snapshot while the index contains unmerged entries"))]
    UnmergedIndex,

    /// Snapshot name fails Git ref formatting rules.
    #[snafu(display("invalid snapshot name '{name}'"))]
    InvalidSnapshotName { name: String },

    /// Target snapshot ref already exists and `--force` was not given.
    #[snafu(display("snapshot '{name}' already exists (use --force to overwrite)"))]
    SnapshotAlreadyExists { name: String },

    /// Requested snapshot ref does not exist under `refs/wip/`.
    #[snafu(display("snapshot '{name}' not found"))]
    SnapshotNotFound { name: String },

    /// No snapshots exist under `refs/wip/`.
    #[snafu(display("no snapshots found under refs/wip/"))]
    NoSnapshots,

    /// Target scope contains no changes relative to its baseline.
    #[snafu(display("no changes to snapshot"))]
    NothingToSnapshot,

    /// Invocation of an underlying `git` subprocess failed.
    #[snafu(display("git {command} failed: {stderr}"))]
    GitCommandFailed { command: String, stderr: String },

    /// Ad-hoc contextual error fallback.
    #[snafu(whatever, display("{message}"))]
    Whatever {
        message: String,
        #[snafu(source(from(Box<dyn std::error::Error + Send + Sync>, Some)))]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}
