//! Snapshot domain values and use-case orchestration following Magit conventions.

use std::fmt;

use snafu::ensure;

use crate::error::{
    GitCommandFailedSnafu, InvalidSnapshotNameSnafu, NoSnapshotsSnafu, NothingToSnapshotSnafu,
    Result, SnapshotAlreadyExistsSnafu, SnapshotNotFoundSnafu, UnmergedIndexSnafu,
};
use crate::git::{GitRepo, Oid};

/// Git ref prefix under which all snapshots are stored.
pub(crate) const REF_PREFIX: &str = "refs/wip/";

/// Tracked scope and untracked-file policy for creating a snapshot.
///
/// Matches Magit's `both`, `index`, and `worktree` snapshot commands, where
/// `index` strictly excludes untracked files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotKind {
    Both { include_untracked: bool },
    Index,
    Worktree { include_untracked: bool },
}

impl SnapshotKind {
    /// Returns whether untracked files are captured.
    pub fn include_untracked(self) -> bool {
        match self {
            Self::Both { include_untracked } | Self::Worktree { include_untracked } => {
                include_untracked
            }
            Self::Index => false,
        }
    }

    /// Returns whether tracked worktree changes are captured.
    pub fn includes_worktree(self) -> bool {
        matches!(self, Self::Both { .. } | Self::Worktree { .. })
    }

    /// Returns the human-readable scope tag recorded in commit messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::Both {
                include_untracked: false,
            } => "both",
            Self::Both {
                include_untracked: true,
            } => "both+untracked",
            Self::Index => "index",
            Self::Worktree {
                include_untracked: false,
            } => "worktree",
            Self::Worktree {
                include_untracked: true,
            } => "worktree+untracked",
        }
    }
}

/// Validated snapshot identifier relative to `refs/wip/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotName(String);

impl SnapshotName {
    /// Validates `raw` against Git's ref naming rules under `refs/wip/`.
    pub(crate) fn parse(repo: &GitRepo, raw: &str) -> Result<Self> {
        ensure!(
            !raw.is_empty(),
            InvalidSnapshotNameSnafu {
                name: raw.to_string(),
            }
        );

        let full_ref = format!("{REF_PREFIX}{raw}");
        ensure!(
            repo.check_ref_format(&full_ref)?,
            InvalidSnapshotNameSnafu {
                name: raw.to_string(),
            }
        );

        Ok(Self(raw.to_string()))
    }

    /// Extracts a [`SnapshotName`] from a full `refs/wip/<name>` refname.
    pub(crate) fn from_full_ref(full_ref: &str) -> Option<Self> {
        full_ref
            .strip_prefix(REF_PREFIX)
            .filter(|name| !name.is_empty())
            .map(|name| Self(name.to_string()))
    }

    /// Returns the short snapshot name without the `refs/wip/` prefix.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the full Git ref path (`refs/wip/<name>`).
    pub fn ref_name(&self) -> String {
        format!("{REF_PREFIX}{}", self.0)
    }
}

impl fmt::Display for SnapshotName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Input request for creating a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRequest {
    pub kind: SnapshotKind,
    pub name: Option<String>,
    pub message: Option<String>,
    pub force: bool,
}

/// Result of creating a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedSnapshot {
    pub name: SnapshotName,
    pub oid: Oid,
}

/// Summary entry returned when listing snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotSummary {
    pub name: SnapshotName,
    pub oid: Oid,
    pub relative_date: String,
    pub subject: String,
}

/// Outcome of applying a snapshot using Magit's `--index` fallback policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied { name: SnapshotName, oid: Oid },
    AppliedWithoutIndex { name: SnapshotName, oid: Oid },
}

/// Outcome of popping a snapshot using Magit's preservation-on-fallback policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PopOutcome {
    Popped { name: SnapshotName, oid: Oid },
    AppliedWithoutIndexKept { name: SnapshotName, oid: Oid },
}

/// Result of dropping a snapshot ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedSnapshot {
    pub name: SnapshotName,
    pub oid: Oid,
}

/// Actor orchestrating non-destructive snapshot creation, application, popping, dropping, and listing.
#[derive(Debug, Clone)]
pub(crate) struct SnapshotStore {
    repo: GitRepo,
}

impl SnapshotStore {
    /// Creates a store backed by `repo`.
    pub(crate) fn new(repo: GitRepo) -> Self {
        Self { repo }
    }

    /// Creates a non-destructive stash-like snapshot at `refs/wip/<name>`.
    pub(crate) fn create(&self, req: &CreateRequest) -> Result<CreatedSnapshot> {
        ensure!(!self.repo.has_unmerged_entries()?, UnmergedIndexSnafu);

        let head = self.repo.head_commit()?;
        let head_tree = self.repo.commit_tree_oid(head.as_str())?;
        let branch = self.repo.current_branch()?;

        let name = match &req.name {
            Some(raw) => {
                let parsed = SnapshotName::parse(&self.repo, raw)?;
                if !req.force && self.repo.resolve_ref_commit(&parsed.ref_name())?.is_some() {
                    return SnapshotAlreadyExistsSnafu { name: raw.clone() }.fail();
                }
                parsed
            }
            None => self.generate_name(branch.as_deref())?,
        };

        let trees = self
            .repo
            .capture_trees(req.kind.includes_worktree(), req.kind.include_untracked())?;

        let (b_tree, i_tree, w_tree) = match req.kind {
            SnapshotKind::Both { .. } => (
                &head_tree,
                &trees.index,
                trees.worktree.as_ref().unwrap_or(&trees.index),
            ),
            SnapshotKind::Index => (&head_tree, &trees.index, &trees.index),
            SnapshotKind::Worktree { .. } => (
                &trees.index,
                &trees.index,
                trees.worktree.as_ref().unwrap_or(&trees.index),
            ),
        };

        ensure!(
            b_tree != i_tree || i_tree != w_tree || trees.untracked.is_some(),
            NothingToSnapshotSnafu
        );

        let branch_label = branch.as_deref().unwrap_or("detached");
        let b_commit = match req.kind {
            SnapshotKind::Both { .. } | SnapshotKind::Index => head.clone(),
            SnapshotKind::Worktree { .. } => {
                if trees.index == head_tree {
                    head.clone()
                } else {
                    self.repo.commit_tree(
                        &trees.index,
                        &[&head],
                        &format!("index baseline on {branch_label}"),
                    )?
                }
            }
        };

        let i_commit =
            self.repo
                .commit_tree(i_tree, &[&b_commit], &format!("index on {branch_label}"))?;

        let u_commit = match &trees.untracked {
            Some(u_tree) => Some(self.repo.commit_tree(
                u_tree,
                &[],
                &format!("untracked files on {branch_label}"),
            )?),
            None => None,
        };

        let subject = match &req.message {
            Some(msg) => format!("Snapshot ({}) on {branch_label}: {msg}", req.kind.label()),
            None => format!(
                "Snapshot ({}) on {branch_label}: {} {}",
                req.kind.label(),
                head.short(),
                self.repo.head_subject()?
            ),
        };

        let mut parents = vec![&b_commit, &i_commit];
        if let Some(u) = &u_commit {
            parents.push(u);
        }

        let w_commit = self.repo.commit_tree(w_tree, &parents, &subject)?;
        self.repo
            .update_ref(&name.ref_name(), &w_commit, &subject)?;

        Ok(CreatedSnapshot {
            name,
            oid: w_commit,
        })
    }

    /// Applies the target (or latest) snapshot, falling back to worktree-only if `--index` conflicts.
    pub(crate) fn apply(&self, name: Option<&str>) -> Result<ApplyOutcome> {
        let (target_name, oid) = self.resolve_target(name)?;
        let first = self.repo.stash_apply(&oid, true)?;

        if first.status.success() {
            return Ok(ApplyOutcome::Applied {
                name: target_name,
                oid,
            });
        }

        if self.repo.stash_has_index_changes(&oid)? && !self.repo.has_unmerged_entries()? {
            let fallback = self.repo.stash_apply(&oid, false)?;
            if fallback.status.success() {
                return Ok(ApplyOutcome::AppliedWithoutIndex {
                    name: target_name,
                    oid,
                });
            }
            return GitCommandFailedSnafu {
                command: format!("stash apply {oid}"),
                stderr: fallback.error_text(),
            }
            .fail();
        }

        GitCommandFailedSnafu {
            command: format!("stash apply --index {oid}"),
            stderr: first.error_text(),
        }
        .fail()
    }

    /// Applies the target (or latest) snapshot and drops its ref only if index restoration succeeded.
    pub(crate) fn pop(&self, name: Option<&str>) -> Result<PopOutcome> {
        match self.apply(name)? {
            ApplyOutcome::Applied { name, oid } => {
                self.repo.delete_ref(&name.ref_name(), &oid)?;
                Ok(PopOutcome::Popped { name, oid })
            }
            ApplyOutcome::AppliedWithoutIndex { name, oid } => {
                Ok(PopOutcome::AppliedWithoutIndexKept { name, oid })
            }
        }
    }

    /// Drops the target (or latest) snapshot ref.
    pub(crate) fn drop(&self, name: Option<&str>) -> Result<DroppedSnapshot> {
        let (target_name, oid) = self.resolve_target(name)?;
        self.repo.delete_ref(&target_name.ref_name(), &oid)?;
        Ok(DroppedSnapshot {
            name: target_name,
            oid,
        })
    }

    /// Lists all snapshots under `refs/wip/`, newest first.
    pub(crate) fn list(&self) -> Result<Vec<SnapshotSummary>> {
        let raw_entries = self.repo.list_refs(REF_PREFIX)?;
        let summaries = raw_entries
            .into_iter()
            .filter_map(|entry| {
                let name = SnapshotName::from_full_ref(&entry.ref_name)?;
                Some(SnapshotSummary {
                    name,
                    oid: entry.oid,
                    relative_date: entry.relative_date,
                    subject: entry.subject,
                })
            })
            .collect();
        Ok(summaries)
    }

    fn resolve_target(&self, name: Option<&str>) -> Result<(SnapshotName, Oid)> {
        match name {
            Some(raw) => {
                let parsed = SnapshotName::parse(&self.repo, raw)?;
                let oid = self
                    .repo
                    .resolve_ref_commit(&parsed.ref_name())?
                    .ok_or_else(|| {
                        SnapshotNotFoundSnafu {
                            name: raw.to_string(),
                        }
                        .build()
                    })?;
                Ok((parsed, oid))
            }
            None => {
                let latest = self
                    .list()?
                    .into_iter()
                    .next()
                    .ok_or_else(|| NoSnapshotsSnafu.build())?;
                Ok((latest.name, latest.oid))
            }
        }
    }

    fn generate_name(&self, branch: Option<&str>) -> Result<SnapshotName> {
        let prefix = match branch {
            Some(raw) => Self::sanitize_branch_prefix(raw),
            None => "detached".to_string(),
        };
        let timestamp = jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string();
        let base = format!("{prefix}-{timestamp}");

        let base_name = SnapshotName::parse(&self.repo, &base)?;
        if self
            .repo
            .resolve_ref_commit(&base_name.ref_name())?
            .is_none()
        {
            return Ok(base_name);
        }

        let mut suffix = 1u64;
        loop {
            let candidate = format!("{base}-{suffix}");
            let name = SnapshotName::parse(&self.repo, &candidate)?;
            if self.repo.resolve_ref_commit(&name.ref_name())?.is_none() {
                return Ok(name);
            }
            let Some(next) = suffix.checked_add(1) else {
                snafu::whatever!("exhausted snapshot name suffixes");
            };
            suffix = next;
        }
    }

    fn sanitize_branch_prefix(branch: &str) -> String {
        let mut sanitized = String::with_capacity(branch.len());
        let mut prev_dash = false;

        for ch in branch.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                sanitized.push(ch);
                prev_dash = ch == '-';
            } else if !prev_dash {
                sanitized.push('-');
                prev_dash = true;
            }
        }

        let trimmed = sanitized.trim_matches('-');
        if trimmed.is_empty() {
            "snapshot".to_string()
        } else {
            trimmed.to_string()
        }
    }
}

#[cfg(test)]
mod test;
