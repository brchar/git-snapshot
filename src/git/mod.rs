//! Git repository actor, RAII temporary index handle, and plumbing wrappers.

use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use snafu::ResultExt;

use crate::error::{GitCommandFailedSnafu, NotAGitWorktreeSnafu, Result, UnbornHeadSnafu};

/// Git object identifier (commit or tree hash) returned by Git commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Oid(String);

impl Oid {
    fn new(raw: impl Into<String>) -> Self {
        Self(raw.into().trim().to_string())
    }

    /// Returns the full object identifier string.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the abbreviated 7-character object identifier prefix.
    pub(crate) fn short(&self) -> &str {
        &self.0[..self.0.len().min(7)]
    }
}

impl fmt::Display for Oid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// RAII handle owning a temporary directory for isolated `GIT_INDEX_FILE` paths.
///
/// Paths returned by this handle do not pre-exist on disk because Git rejects
/// empty 0-byte index files.
struct TempIndexWorkspace {
    dir: tempfile::TempDir,
}

impl TempIndexWorkspace {
    fn new() -> Result<Self> {
        let dir = tempfile::TempDir::new()
            .with_whatever_context(|_| "failed to create temporary index directory")?;
        Ok(Self { dir })
    }

    fn worktree_index_path(&self) -> PathBuf {
        self.dir.path().join("worktree.index")
    }

    fn untracked_index_path(&self) -> PathBuf {
        self.dir.path().join("untracked.index")
    }
}

/// Captured tree object IDs representing current index, worktree, and untracked state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedTrees {
    pub(crate) index: Oid,
    pub(crate) worktree: Option<Oid>,
    pub(crate) untracked: Option<Oid>,
}

/// Raw ref metadata returned by `git for-each-ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawRefEntry {
    pub(crate) ref_name: String,
    pub(crate) oid: Oid,
    pub(crate) relative_date: String,
    pub(crate) subject: String,
}

/// Raw result of running a `git` subprocess.
pub(crate) struct GitOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

impl GitOutput {
    /// Extracts diagnostic output from stderr and stdout.
    pub(crate) fn error_text(&self) -> String {
        let stderr = String::from_utf8_lossy(&self.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&self.stdout).trim().to_string();
        match (stderr.is_empty(), stdout.is_empty()) {
            (false, true) => stderr,
            (true, false) => stdout,
            (false, false) => format!("{stderr}\n{stdout}"),
            (true, true) => String::new(),
        }
    }
}

/// Actor encapsulating repository root resolution and `git` plumbing invocations.
#[derive(Debug, Clone)]
pub(crate) struct GitRepo {
    root: PathBuf,
    index_override: Option<PathBuf>,
}

impl GitRepo {
    /// Discovers the enclosing Git worktree root starting from `cwd`.
    pub(crate) fn discover(cwd: &Path) -> Result<Self> {
        let inside = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--is-inside-work-tree"])
            .stdin(Stdio::null())
            .output()
            .with_whatever_context(|_| "failed to invoke git rev-parse")?;

        if !inside.status.success() || String::from_utf8_lossy(&inside.stdout).trim() != "true" {
            return NotAGitWorktreeSnafu.fail();
        }

        let toplevel = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(["rev-parse", "--show-toplevel"])
            .stdin(Stdio::null())
            .output()
            .with_whatever_context(|_| "failed to resolve git worktree root")?;

        if !toplevel.status.success() {
            return GitCommandFailedSnafu {
                command: "rev-parse --show-toplevel".to_string(),
                stderr: String::from_utf8_lossy(&toplevel.stderr).trim().to_string(),
            }
            .fail();
        }

        let root = PathBuf::from(String::from_utf8_lossy(&toplevel.stdout).trim());
        let index_override = std::env::var_os("GIT_INDEX_FILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|path| {
                if path.is_absolute() {
                    path
                } else {
                    cwd.join(path)
                }
            });

        Ok(Self {
            root,
            index_override,
        })
    }

    /// Validates whether `full_ref` satisfies `git check-ref-format`.
    pub(crate) fn check_ref_format(&self, full_ref: &str) -> Result<bool> {
        let out = self.run(&["check-ref-format", full_ref], None, None)?;
        Ok(out.status.success())
    }

    /// Resolves `HEAD^{commit}`, returning [`crate::error::Error::UnbornHead`] if unborn.
    pub(crate) fn head_commit(&self) -> Result<Oid> {
        let out = self.run(
            &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"],
            None,
            None,
        )?;
        if !out.status.success() {
            return UnbornHeadSnafu.fail();
        }
        Ok(Oid::new(String::from_utf8_lossy(&out.stdout)))
    }

    /// Resolves the tree object ID associated with `rev`.
    pub(crate) fn commit_tree_oid(&self, rev: &str) -> Result<Oid> {
        let target = format!("{rev}^{{tree}}");
        let raw = self.run_stdout_trimmed(&["rev-parse", "--verify", &target], None)?;
        Ok(Oid::new(raw))
    }

    /// Returns the commit subject line of `HEAD`.
    pub(crate) fn head_subject(&self) -> Result<String> {
        self.run_stdout_trimmed(&["log", "-1", "--format=%s", "HEAD"], None)
    }

    /// Returns the checked-out short branch name, or `None` when `HEAD` is detached.
    pub(crate) fn current_branch(&self) -> Result<Option<String>> {
        let out = self.run(&["symbolic-ref", "--short", "-q", "HEAD"], None, None)?;
        if !out.status.success() {
            return Ok(None);
        }
        let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if branch.is_empty() {
            Ok(None)
        } else {
            Ok(Some(branch))
        }
    }

    /// Reports whether the active index contains unmerged conflict stages.
    pub(crate) fn has_unmerged_entries(&self) -> Result<bool> {
        let out = self.run_checked(&["ls-files", "--unmerged", "-z"], None, None)?;
        Ok(!out.is_empty())
    }

    /// Captures index, optional tracked-worktree, and optional untracked trees without altering repository state.
    pub(crate) fn capture_trees(
        &self,
        include_worktree: bool,
        include_untracked: bool,
    ) -> Result<CapturedTrees> {
        let index = self.write_index_tree()?;
        if !include_worktree && !include_untracked {
            return Ok(CapturedTrees {
                index,
                worktree: None,
                untracked: None,
            });
        }

        let workspace = TempIndexWorkspace::new()?;
        let worktree = if include_worktree {
            Some(self.write_worktree_tree(&workspace, &index)?)
        } else {
            None
        };
        let untracked = if include_untracked {
            self.write_untracked_tree(&workspace)?
        } else {
            None
        };

        Ok(CapturedTrees {
            index,
            worktree,
            untracked,
        })
    }

    /// Creates a commit object directly from `tree`, `parents`, and `message`.
    pub(crate) fn commit_tree(&self, tree: &Oid, parents: &[&Oid], message: &str) -> Result<Oid> {
        let mut args = vec!["commit-tree", tree.as_str()];
        for parent in parents {
            args.push("-p");
            args.push(parent.as_str());
        }
        args.push("-m");
        args.push(message);

        let raw = self.run_stdout_trimmed(&args, None)?;
        Ok(Oid::new(raw))
    }

    /// Resolves `full_ref` to a commit OID, returning `None` if the ref does not exist.
    pub(crate) fn resolve_ref_commit(&self, full_ref: &str) -> Result<Option<Oid>> {
        let target = format!("{full_ref}^{{commit}}");
        let out = self.run(&["rev-parse", "--verify", "--quiet", &target], None, None)?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(Oid::new(String::from_utf8_lossy(&out.stdout))))
    }

    /// Updates or creates `full_ref` pointing to `new_oid`, preserving its reflog.
    pub(crate) fn update_ref(&self, full_ref: &str, new_oid: &Oid, reflog_msg: &str) -> Result<()> {
        self.run_checked(
            &[
                "update-ref",
                "--create-reflog",
                "-m",
                reflog_msg,
                full_ref,
                new_oid.as_str(),
            ],
            None,
            None,
        )?;
        Ok(())
    }

    /// Atomically deletes `full_ref` if it still points to `expected_oid`.
    pub(crate) fn delete_ref(&self, full_ref: &str, expected_oid: &Oid) -> Result<()> {
        self.run_checked(
            &["update-ref", "-d", full_ref, expected_oid.as_str()],
            None,
            None,
        )?;
        Ok(())
    }

    /// Lists refs under `prefix` sorted by committer date descending.
    pub(crate) fn list_refs(&self, prefix: &str) -> Result<Vec<RawRefEntry>> {
        let out = self.run_checked(
            &[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname)%00%(objectname)%00%(committerdate:relative)%00%(subject)",
                prefix,
            ],
            None,
            None,
        )?;

        let mut entries = Vec::new();
        for line in out.split(|&byte| byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split(|&byte| byte == 0);
            let (Some(ref_name), Some(oid), Some(relative_date), Some(subject)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            entries.push(RawRefEntry {
                ref_name: String::from_utf8_lossy(ref_name).into_owned(),
                oid: Oid::new(String::from_utf8_lossy(oid)),
                relative_date: String::from_utf8_lossy(relative_date).into_owned(),
                subject: String::from_utf8_lossy(subject).into_owned(),
            });
        }

        Ok(entries)
    }

    /// Executes `git stash apply` on `oid`, returning raw output for fallback inspection.
    pub(crate) fn stash_apply(&self, oid: &Oid, restore_index: bool) -> Result<GitOutput> {
        if restore_index {
            self.run(&["stash", "apply", "--index", oid.as_str()], None, None)
        } else {
            self.run(&["stash", "apply", oid.as_str()], None, None)
        }
    }

    /// Reports whether `oid` is a stash-like commit recording staged changes (`^1^{tree} != ^2^{tree}`).
    ///
    /// Returns `Ok(false)` when `^1` or `^2` does not exist so caller-facing errors from
    /// `git stash apply` are preserved on non-stash commits.
    pub(crate) fn stash_has_index_changes(&self, oid: &Oid) -> Result<bool> {
        let Some(base_tree) = self.try_tree_oid(&format!("{oid}^1"))? else {
            return Ok(false);
        };
        let Some(index_tree) = self.try_tree_oid(&format!("{oid}^2"))? else {
            return Ok(false);
        };
        Ok(base_tree != index_tree)
    }

    fn write_index_tree(&self) -> Result<Oid> {
        let raw = self.run_stdout_trimmed(&["write-tree"], None)?;
        Ok(Oid::new(raw))
    }

    fn write_worktree_tree(
        &self,
        workspace: &TempIndexWorkspace,
        fallback_tree: &Oid,
    ) -> Result<Oid> {
        let real_index = self.resolve_index_path()?;
        let temp_index = workspace.worktree_index_path();

        if real_index.exists() {
            std::fs::copy(&real_index, &temp_index).with_whatever_context(|_| {
                format!(
                    "failed to copy index {} to {}",
                    real_index.display(),
                    temp_index.display()
                )
            })?;
        } else {
            self.run_checked(
                &["read-tree", fallback_tree.as_str()],
                Some(&temp_index),
                None,
            )?;
        }

        self.run_checked(&["add", "-u"], Some(&temp_index), None)?;
        let raw = self.run_stdout_trimmed(&["write-tree"], Some(&temp_index))?;
        Ok(Oid::new(raw))
    }

    fn write_untracked_tree(&self, workspace: &TempIndexWorkspace) -> Result<Option<Oid>> {
        let files = self.run_checked(
            &["ls-files", "--others", "--exclude-standard", "-z"],
            None,
            None,
        )?;
        if files.is_empty() {
            return Ok(None);
        }

        let temp_index = workspace.untracked_index_path();
        self.run_checked(
            &["update-index", "--add", "-z", "--stdin"],
            Some(&temp_index),
            Some(&files),
        )?;
        let raw = self.run_stdout_trimmed(&["write-tree"], Some(&temp_index))?;
        Ok(Some(Oid::new(raw)))
    }

    fn try_tree_oid(&self, rev: &str) -> Result<Option<Oid>> {
        let target = format!("{rev}^{{tree}}");
        let out = self.run(&["rev-parse", "--verify", "--quiet", &target], None, None)?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(Some(Oid::new(String::from_utf8_lossy(&out.stdout))))
    }

    fn resolve_index_path(&self) -> Result<PathBuf> {
        if let Some(override_path) = &self.index_override {
            return Ok(override_path.clone());
        }
        let raw = self.run_stdout_trimmed(&["rev-parse", "--git-path", "index"], None)?;
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(self.root.join(path))
        }
    }

    fn run(
        &self,
        args: &[&str],
        env_index: Option<&Path>,
        stdin_bytes: Option<&[u8]>,
    ) -> Result<GitOutput> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.root);

        if let Some(index_path) = env_index {
            cmd.env("GIT_INDEX_FILE", index_path);
        } else if let Some(index_path) = &self.index_override {
            cmd.env("GIT_INDEX_FILE", index_path);
        }

        if let Some(input) = stdin_bytes {
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = cmd
                .spawn()
                .with_whatever_context(|_| format!("failed to spawn git {}", args.join(" ")))?;
            if let Some(mut stdin) = child.stdin.take() {
                match stdin.write_all(input) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {}
                    Err(err) => {
                        let _ = child.wait();
                        return Err(err).with_whatever_context(|_| {
                            format!("failed to write stdin to git {}", args.join(" "))
                        });
                    }
                }
            }
            let output = child
                .wait_with_output()
                .with_whatever_context(|_| format!("failed to wait for git {}", args.join(" ")))?;
            Ok(GitOutput {
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
            })
        } else {
            let output = cmd
                .stdin(Stdio::null())
                .output()
                .with_whatever_context(|_| format!("failed to execute git {}", args.join(" ")))?;
            Ok(GitOutput {
                status: output.status,
                stdout: output.stdout,
                stderr: output.stderr,
            })
        }
    }

    fn run_checked(
        &self,
        args: &[&str],
        env_index: Option<&Path>,
        stdin_bytes: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        let out = self.run(args, env_index, stdin_bytes)?;
        if !out.status.success() {
            return GitCommandFailedSnafu {
                command: args.join(" "),
                stderr: out.error_text(),
            }
            .fail();
        }
        Ok(out.stdout)
    }

    fn run_stdout_trimmed(&self, args: &[&str], env_index: Option<&Path>) -> Result<String> {
        let out = self.run_checked(args, env_index, None)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }
}

#[cfg(test)]
mod test;
