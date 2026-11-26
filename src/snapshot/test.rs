use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::{ApplyOutcome, CreateRequest, PopOutcome, SnapshotKind, SnapshotName, SnapshotStore};
use crate::error::Error;
use crate::git::GitRepo;

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command should run");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn run_git_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("git command should run");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    run_git(dir.path(), &["init", "-b", "main"]);
    run_git(dir.path(), &["config", "user.name", "Test User"]);
    run_git(dir.path(), &["config", "user.email", "test@example.com"]);
    fs::write(dir.path().join("tracked.txt"), "base\n").expect("write");
    fs::write(dir.path().join("other.txt"), "other-base\n").expect("write");
    run_git(dir.path(), &["add", "tracked.txt", "other.txt"]);
    run_git(dir.path(), &["commit", "-m", "initial commit"]);
    dir
}

fn assert_fsck_clean(dir: &Path) {
    let out = Command::new("git")
        .args(["fsck", "--strict"])
        .current_dir(dir)
        .output()
        .expect("git fsck");
    assert!(
        out.status.success(),
        "git fsck --strict failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn all_five_snapshot_kinds_are_non_destructive_and_fsck_clean() {
    let kinds = [
        (
            "both",
            SnapshotKind::Both {
                include_untracked: false,
            },
        ),
        (
            "both-u",
            SnapshotKind::Both {
                include_untracked: true,
            },
        ),
        ("index", SnapshotKind::Index),
        (
            "worktree",
            SnapshotKind::Worktree {
                include_untracked: false,
            },
        ),
        (
            "worktree-u",
            SnapshotKind::Worktree {
                include_untracked: true,
            },
        ),
    ];

    for (name, kind) in kinds {
        let dir = init_repo();
        let root = dir.path();

        fs::write(root.join("tracked.txt"), "staged\n").expect("write");
        run_git(root, &["add", "tracked.txt"]);
        fs::write(root.join("tracked.txt"), "staged\nunstaged\n").expect("write");
        fs::write(root.join("untracked file.txt"), "new-untracked\n").expect("write");

        let index_before = run_git(root, &["diff", "--cached"]);
        let worktree_before = fs::read_to_string(root.join("tracked.txt")).expect("read");

        let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));
        let created = store
            .create(&CreateRequest {
                kind,
                name: Some(name.to_string()),
                message: None,
                force: false,
            })
            .expect("create snapshot");

        assert_eq!(created.name.as_str(), name);
        assert_eq!(run_git(root, &["diff", "--cached"]), index_before);
        assert_eq!(
            fs::read_to_string(root.join("tracked.txt")).expect("read"),
            worktree_before
        );
        assert!(root.join("untracked file.txt").exists());
        assert_fsck_clean(root);
    }
}

#[test]
fn same_file_staged_and_unstaged_isolation_for_both_index_and_worktree() {
    let dir = init_repo();
    let root = dir.path();
    let base_content = "line1\nline2\nline3\nline4\nline5\nline6\n";
    let staged_content = "line1-staged\nline2\nline3\nline4\nline5\nline6\n";
    let both_content = "line1-staged\nline2\nline3\nline4\nline5\nline6-unstaged\n";
    let worktree_only_restored = "line1\nline2\nline3\nline4\nline5\nline6-unstaged\n";

    fs::write(root.join("tracked.txt"), base_content).expect("write");
    run_git(root, &["commit", "-am", "multiline base"]);

    fs::write(root.join("tracked.txt"), staged_content).expect("write");
    run_git(root, &["add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), both_content).expect("write");
    fs::write(root.join("extra.txt"), "untracked-body\n").expect("write");

    let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: true,
            },
            name: Some("snap-both".to_string()),
            message: Some("save both".to_string()),
            force: false,
        })
        .expect("both");
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Index,
            name: Some("snap-index".to_string()),
            message: None,
            force: false,
        })
        .expect("index");
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: true,
            },
            name: Some("snap-worktree".to_string()),
            message: None,
            force: false,
        })
        .expect("worktree");

    run_git(root, &["reset", "--hard", "HEAD"]);
    fs::remove_file(root.join("extra.txt")).expect("remove untracked");
    let idx_oid = store
        .list()
        .expect("list")
        .into_iter()
        .find(|s| s.name.as_str() == "snap-index")
        .expect("find")
        .oid;
    assert_eq!(
        store.apply(Some("snap-index")).expect("apply index"),
        ApplyOutcome::Applied {
            name: SnapshotName("snap-index".to_string()),
            oid: idx_oid,
        }
    );
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("read"),
        staged_content
    );
    assert!(!run_git(root, &["diff", "--cached"]).is_empty());
    assert!(run_git(root, &["diff"]).is_empty());
    assert!(!root.join("extra.txt").exists());

    run_git(root, &["reset", "--hard", "HEAD"]);
    store.apply(Some("snap-worktree")).expect("apply worktree");
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("read"),
        worktree_only_restored
    );
    assert!(run_git(root, &["diff", "--cached"]).is_empty());
    assert_eq!(
        fs::read_to_string(root.join("extra.txt")).expect("read"),
        "untracked-body\n"
    );

    run_git(root, &["reset", "--hard", "HEAD"]);
    fs::remove_file(root.join("extra.txt")).expect("remove untracked");
    store.apply(Some("snap-both")).expect("apply both");
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("read"),
        both_content
    );
    assert!(!run_git(root, &["diff", "--cached"]).is_empty());
    assert!(!run_git(root, &["diff"]).is_empty());
    assert_eq!(
        fs::read_to_string(root.join("extra.txt")).expect("read"),
        "untracked-body\n"
    );
}

#[test]
fn apply_and_pop_handle_index_fallback_and_ref_preservation() {
    let dir = init_repo();
    let root = dir.path();
    fs::write(root.join("tracked.txt"), "1\n2\n3\n4\n5\n").expect("write");
    run_git(root, &["commit", "-am", "five lines"]);

    fs::write(root.join("tracked.txt"), "1-snap\n2\n3\n4\n5\n").expect("write");
    run_git(root, &["add", "tracked.txt"]);
    fs::write(root.join("other.txt"), "snap-unstaged\n").expect("write");

    let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: false,
            },
            name: Some("clean-pop".to_string()),
            message: None,
            force: false,
        })
        .expect("create clean-pop");

    run_git(root, &["reset", "--hard", "HEAD"]);
    let popped = store.pop(None).expect("pop latest");
    assert!(matches!(popped, PopOutcome::Popped { .. }));
    assert!(store.list().expect("list").is_empty());

    fs::write(root.join("tracked.txt"), "1-snap\n2\n3\n4\n5\n").expect("write");
    run_git(root, &["add", "tracked.txt"]);
    fs::write(root.join("other.txt"), "snap-unstaged\n").expect("write");
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: false,
            },
            name: Some("fallback-snap".to_string()),
            message: None,
            force: false,
        })
        .expect("create fallback-snap");

    run_git(root, &["reset", "--hard", "HEAD"]);
    fs::write(root.join("tracked.txt"), "1\n2\n3-current\n4\n5\n").expect("write");
    run_git(root, &["add", "tracked.txt"]);

    let apply_outcome = store.apply(None).expect("apply latest fallback");
    assert!(matches!(
        apply_outcome,
        ApplyOutcome::AppliedWithoutIndex { .. }
    ));
    assert_eq!(store.list().expect("list").len(), 1);
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("read"),
        "1-snap\n2\n3-current\n4\n5\n"
    );

    run_git(root, &["reset", "--hard", "HEAD"]);
    fs::write(root.join("tracked.txt"), "1\n2\n3-current\n4\n5\n").expect("write");
    run_git(root, &["add", "tracked.txt"]);

    let pop_outcome = store.pop(Some("fallback-snap")).expect("pop fallback");
    assert!(matches!(
        pop_outcome,
        PopOutcome::AppliedWithoutIndexKept { .. }
    ));
    assert_eq!(store.list().expect("list").len(), 1);
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("read"),
        "1-snap\n2\n3-current\n4\n5\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("other.txt")).expect("read"),
        "snap-unstaged\n"
    );
}

#[test]
fn pop_with_worktree_conflict_fails_and_preserves_snapshot() {
    let dir = init_repo();
    let root = dir.path();

    fs::write(root.join("tracked.txt"), "from-snapshot\n").expect("write");
    let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: false,
            },
            name: Some("conflict-snap".to_string()),
            message: None,
            force: false,
        })
        .expect("create");

    fs::write(root.join("tracked.txt"), "conflicting-local\n").expect("write");
    let err = store
        .pop(Some("conflict-snap"))
        .expect_err("should fail on worktree conflict");
    assert!(matches!(err, Error::GitCommandFailed { .. }));
    assert_eq!(store.list().expect("list").len(), 1);
}

#[test]
fn list_orders_newest_first_and_default_targets_latest_snapshot() {
    let dir = init_repo();
    let root = dir.path();
    let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));
    assert!(store.list().expect("empty list").is_empty());

    fs::write(root.join("tracked.txt"), "first\n").expect("write");
    run_git_env(
        root,
        &["commit", "--allow-empty", "-m", "helper"],
        &[("GIT_COMMITTER_DATE", "2025-01-01T00:00:00Z")],
    );
    run_git(root, &["reset", "--soft", "HEAD~1"]);

    fs::write(root.join("tracked.txt"), "first-change\n").expect("write");
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: false,
            },
            name: Some("older".to_string()),
            message: Some("older msg".to_string()),
            force: false,
        })
        .expect("older");

    let older_oid = store
        .list()
        .expect("list")
        .into_iter()
        .find(|s| s.name.as_str() == "older")
        .expect("older exists")
        .oid;
    let backdated = Command::new("git")
        .args([
            "commit-tree",
            &format!("{older_oid}^{{tree}}"),
            "-p",
            &format!("{older_oid}^1"),
            "-p",
            &format!("{older_oid}^2"),
            "-m",
            "older backdated",
        ])
        .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
        .current_dir(root)
        .output()
        .expect("commit-tree");
    assert!(backdated.status.success());
    let backdated_oid = String::from_utf8_lossy(&backdated.stdout)
        .trim()
        .to_string();
    run_git(root, &["update-ref", "refs/wip/older", &backdated_oid]);

    fs::write(root.join("tracked.txt"), "newer-change\n").expect("write");
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: false,
            },
            name: Some("newer".to_string()),
            message: Some("newer msg".to_string()),
            force: false,
        })
        .expect("newer");

    let entries = store.list().expect("list");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name.as_str(), "newer");
    assert_eq!(entries[1].name.as_str(), "older");

    let dropped = store.drop(None).expect("drop latest");
    assert_eq!(dropped.name.as_str(), "newer");
    let remaining = store.list().expect("list after drop");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name.as_str(), "older");
}

#[test]
fn tracked_deletion_and_untracked_only_worktree_snapshot_restore_cleanly() {
    let dir = init_repo();
    let root = dir.path();
    let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));

    fs::write(root.join("only-untracked.txt"), "hello\n").expect("write");
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: true,
            },
            name: Some("untracked-only".to_string()),
            message: None,
            force: false,
        })
        .expect("create untracked-only");
    assert_fsck_clean(root);

    fs::remove_file(root.join("only-untracked.txt")).expect("remove");
    store
        .apply(Some("untracked-only"))
        .expect("apply untracked-only");
    assert_eq!(
        fs::read_to_string(root.join("only-untracked.txt")).expect("read"),
        "hello\n"
    );

    fs::remove_file(root.join("only-untracked.txt")).expect("remove");
    fs::remove_file(root.join("other.txt")).expect("delete tracked file");
    store
        .create(&CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: false,
            },
            name: Some("deleted-tracked".to_string()),
            message: None,
            force: false,
        })
        .expect("create deleted-tracked");
    assert_fsck_clean(root);

    run_git(root, &["checkout", "--", "other.txt"]);
    assert!(root.join("other.txt").exists());
    store
        .apply(Some("deleted-tracked"))
        .expect("apply deleted-tracked");
    assert!(!root.join("other.txt").exists());
}

#[test]
fn missing_and_empty_targets_and_non_stash_refs_report_expected_errors() {
    let dir = init_repo();
    let root = dir.path();
    let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));

    assert!(matches!(store.apply(None), Err(Error::NoSnapshots)));
    assert!(matches!(store.pop(None), Err(Error::NoSnapshots)));
    assert!(matches!(store.drop(None), Err(Error::NoSnapshots)));
    assert!(matches!(
        store.apply(Some("missing")),
        Err(Error::SnapshotNotFound { .. })
    ));

    let head = run_git(root, &["rev-parse", "HEAD"]);
    run_git(root, &["update-ref", "refs/wip/non-stash", head.trim()]);
    let err = store
        .apply(Some("non-stash"))
        .expect_err("non-stash commit should fail");
    assert!(matches!(
        err,
        Error::GitCommandFailed { ref command, .. } if command.starts_with("stash apply --index")
    ));
}

#[test]
fn generate_name_sanitizes_branch_and_disambiguates_same_second_collisions() {
    let dir = init_repo();
    let root = dir.path();
    run_git(root, &["checkout", "-b", "feat/foo.bar"]);
    fs::write(root.join("tracked.txt"), "dirty\n").expect("write");

    let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));
    let first = store
        .create(&CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: false,
            },
            name: None,
            message: None,
            force: false,
        })
        .expect("first auto");
    let second = store
        .create(&CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: false,
            },
            name: None,
            message: None,
            force: false,
        })
        .expect("second auto");

    assert!(first.name.as_str().starts_with("feat-foo-bar-"));
    assert_ne!(first.name, second.name);
    assert!(second.name.as_str().starts_with("feat-foo-bar-"));
}

#[test]
fn edge_cases_empty_duplicate_force_invalid_detached_unborn_and_unmerged() {
    let dir = init_repo();
    let root = dir.path();
    let store = SnapshotStore::new(GitRepo::discover(root).expect("discover"));

    let empty_err = store
        .create(&CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: false,
            },
            name: None,
            message: None,
            force: false,
        })
        .expect_err("clean tree should fail");
    assert!(matches!(empty_err, Error::NothingToSnapshot));

    fs::write(root.join("tracked.txt"), "staged-then-reverted\n").expect("write");
    run_git(root, &["add", "tracked.txt"]);
    fs::write(root.join("tracked.txt"), "base\n").expect("revert worktree to base");
    let reverted_both = store
        .create(&CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: false,
            },
            name: None,
            message: None,
            force: false,
        })
        .expect("staged-then-reverted should still create snapshot");
    assert!(reverted_both.name.as_str().starts_with("main-"));

    let dup_err = store
        .create(&CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: false,
            },
            name: Some(reverted_both.name.as_str().to_string()),
            message: None,
            force: false,
        })
        .expect_err("duplicate without force should fail");
    assert!(matches!(dup_err, Error::SnapshotAlreadyExists { .. }));

    let forced = store
        .create(&CreateRequest {
            kind: SnapshotKind::Index,
            name: Some(reverted_both.name.as_str().to_string()),
            message: Some("overwritten".to_string()),
            force: true,
        })
        .expect("force overwrite should succeed");
    assert_eq!(forced.name, reverted_both.name);

    let invalid_err = store
        .create(&CreateRequest {
            kind: SnapshotKind::Index,
            name: Some("bad..name".to_string()),
            message: None,
            force: false,
        })
        .expect_err("invalid ref name");
    assert!(matches!(invalid_err, Error::InvalidSnapshotName { .. }));

    run_git(root, &["checkout", "--detach", "HEAD"]);
    let detached = store
        .create(&CreateRequest {
            kind: SnapshotKind::Index,
            name: None,
            message: None,
            force: false,
        })
        .expect("detached head snapshot");
    assert!(detached.name.as_str().starts_with("detached-"));

    let unborn_dir = TempDir::new().expect("unborn dir");
    run_git(unborn_dir.path(), &["init", "-b", "main"]);
    let unborn_store = SnapshotStore::new(GitRepo::discover(unborn_dir.path()).expect("discover"));
    let unborn_err = unborn_store
        .create(&CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: true,
            },
            name: None,
            message: None,
            force: false,
        })
        .expect_err("unborn head should fail");
    assert!(matches!(unborn_err, Error::UnbornHead));

    let conflict_dir = init_repo();
    let croot = conflict_dir.path();
    run_git(croot, &["checkout", "-b", "side"]);
    fs::write(croot.join("tracked.txt"), "side\n").expect("write");
    run_git(croot, &["commit", "-am", "side"]);
    run_git(croot, &["checkout", "main"]);
    fs::write(croot.join("tracked.txt"), "main\n").expect("write");
    run_git(croot, &["commit", "-am", "main"]);
    let merge_out = Command::new("git")
        .args(["merge", "side"])
        .current_dir(croot)
        .output()
        .expect("merge");
    assert!(!merge_out.status.success());

    let conflict_store = SnapshotStore::new(GitRepo::discover(croot).expect("discover"));
    let unmerged_err = conflict_store
        .create(&CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: false,
            },
            name: None,
            message: None,
            force: false,
        })
        .expect_err("unmerged index should fail");
    assert!(matches!(unmerged_err, Error::UnmergedIndex));
}
