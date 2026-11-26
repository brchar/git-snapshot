use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::{GitRepo, TempIndexWorkspace};
use crate::error::Error;

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git command should execute");
    assert!(status.success(), "git {:?} failed", args);
}

fn init_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    run_git(dir.path(), &["init", "-b", "main"]);
    run_git(dir.path(), &["config", "user.name", "Test User"]);
    run_git(dir.path(), &["config", "user.email", "test@example.com"]);
    fs::write(dir.path().join("README.md"), "init\n").expect("write");
    run_git(dir.path(), &["add", "README.md"]);
    run_git(dir.path(), &["commit", "-m", "initial commit"]);
    dir
}

#[test]
fn discover_fails_outside_git_worktree() {
    let dir = TempDir::new().expect("tempdir");
    let err = GitRepo::discover(dir.path()).expect_err("should fail outside git");
    assert!(matches!(err, Error::NotAGitWorktree));
}

#[test]
fn discover_from_subdirectory_resolves_worktree_root() {
    let dir = init_repo();
    let subdir = dir.path().join("nested/inner");
    fs::create_dir_all(&subdir).expect("create subdir");
    fs::write(dir.path().join("README.md"), "modified-from-root\n").expect("write");

    let repo = GitRepo::discover(&subdir).expect("discover from subdir");
    let head = repo.head_commit().expect("head commit");
    let head_tree = repo.commit_tree_oid(head.as_str()).expect("head tree");
    let trees = repo
        .capture_trees(true, false)
        .expect("capture worktree tree");
    assert_ne!(trees.worktree.expect("worktree tree"), head_tree);
}

#[test]
fn temp_index_workspace_cleans_up_on_drop() {
    let dir = {
        let workspace = TempIndexWorkspace::new().expect("workspace");
        let worktree_path = workspace.worktree_index_path();
        assert!(!worktree_path.exists());
        worktree_path.parent().expect("parent dir").to_path_buf()
    };
    assert!(!dir.exists());
}

#[test]
fn capture_trees_handles_spaces_in_untracked_filenames() {
    let dir = init_repo();
    let file_path = dir.path().join("hello world.txt");
    fs::write(&file_path, "spaced content\n").expect("write");

    let repo = GitRepo::discover(dir.path()).expect("discover");
    let trees = repo
        .capture_trees(false, true)
        .expect("capture untracked tree");
    let tree_oid = trees.untracked.expect("should contain untracked file");

    let ls_tree = Command::new("git")
        .args(["ls-tree", "--name-only", tree_oid.as_str()])
        .current_dir(dir.path())
        .output()
        .expect("ls-tree");
    assert!(ls_tree.status.success());
    let listed = String::from_utf8_lossy(&ls_tree.stdout);
    assert_eq!(listed.trim(), "hello world.txt");
}

#[test]
fn stash_has_index_changes_returns_false_for_non_stash_commit() {
    let dir = init_repo();
    let repo = GitRepo::discover(dir.path()).expect("discover");
    let head = repo.head_commit().expect("head");
    assert!(
        !repo
            .stash_has_index_changes(&head)
            .expect("non-stash commit should not error")
    );
}
