use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn run_git(dir: &Path, args: &[&str], envs: &[(&str, &str)]) {
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

fn run_cli(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    let bin = env!("CARGO_BIN_EXE_git-snapshot");
    let mut cmd = Command::new(bin);
    cmd.args(args).current_dir(dir);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().expect("run git-snapshot binary")
}

fn init_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    run_git(root, &["init", "-b", "main"], &[]);
    run_git(root, &["config", "user.name", "Test User"], &[]);
    run_git(root, &["config", "user.email", "test@example.com"], &[]);
    fs::write(root.join("tracked.txt"), "1\n2\n3\n4\n5\n").expect("write");
    run_git(root, &["add", "tracked.txt"], &[]);
    run_git(root, &["commit", "-m", "initial"], &[]);
    dir
}

#[test]
fn cli_binary_honors_relative_git_index_file_override() {
    let dir = init_repo();
    let root = dir.path();

    let custom_index = "custom.index";
    run_git(
        root,
        &["read-tree", "HEAD"],
        &[("GIT_INDEX_FILE", custom_index)],
    );
    fs::write(root.join("tracked.txt"), "custom-index-staged\n").expect("write");
    run_git(
        root,
        &["add", "tracked.txt"],
        &[("GIT_INDEX_FILE", custom_index)],
    );
    fs::write(
        root.join("tracked.txt"),
        "custom-index-staged\nworktree-edit\n",
    )
    .expect("write");

    let save_out = run_cli(
        root,
        &["save", "custom-idx-snap"],
        &[("GIT_INDEX_FILE", custom_index)],
    );
    assert!(
        save_out.status.success(),
        "save failed: {}",
        String::from_utf8_lossy(&save_out.stderr)
    );

    run_git(root, &["reset", "--hard", "HEAD"], &[]);
    let apply_out = run_cli(root, &["apply", "custom-idx-snap"], &[]);
    assert!(
        apply_out.status.success(),
        "apply failed: {}",
        String::from_utf8_lossy(&apply_out.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("tracked.txt")).expect("read"),
        "custom-index-staged\nworktree-edit\n"
    );
}

#[test]
fn cli_binary_save_scopes_list_pop_fallback_and_drop_end_to_end() {
    let dir = init_repo();
    let root = dir.path();

    let invalid = run_cli(root, &["save", "--index", "-u"], &[]);
    assert!(!invalid.status.success());

    fs::write(root.join("tracked.txt"), "1-staged\n2\n3\n4\n5\n").expect("write");
    run_git(root, &["add", "tracked.txt"], &[]);
    fs::write(root.join("untracked.txt"), "untracked-body\n").expect("write");

    let save_idx = run_cli(root, &["save", "--index", "idx-snap"], &[]);
    assert!(
        save_idx.status.success(),
        "save --index failed: {}",
        String::from_utf8_lossy(&save_idx.stderr)
    );

    fs::write(root.join("tracked.txt"), "1-staged\n2\n3\n4\n5-wt\n").expect("write");
    let save_wt = run_cli(root, &["save", "--worktree", "-u", "wt-snap"], &[]);
    assert!(
        save_wt.status.success(),
        "save --worktree -u failed: {}",
        String::from_utf8_lossy(&save_wt.stderr)
    );

    let list_out = run_cli(root, &["list"], &[]);
    assert!(list_out.status.success());
    let list_stdout = String::from_utf8_lossy(&list_out.stdout);
    assert!(list_stdout.contains("idx-snap"));
    assert!(list_stdout.contains("wt-snap"));

    run_git(root, &["reset", "--hard", "HEAD"], &[]);
    fs::remove_file(root.join("untracked.txt")).expect("remove untracked");
    let pop_wt = run_cli(root, &["pop", "wt-snap"], &[]);
    assert!(
        pop_wt.status.success(),
        "pop wt-snap failed: {}",
        String::from_utf8_lossy(&pop_wt.stderr)
    );
    assert!(String::from_utf8_lossy(&pop_wt.stdout).contains("Popped snapshot 'wt-snap'"));
    assert_eq!(
        fs::read_to_string(root.join("untracked.txt")).expect("read"),
        "untracked-body\n"
    );

    run_git(root, &["reset", "--hard", "HEAD"], &[]);
    fs::write(root.join("tracked.txt"), "1\n2\n3-conflict\n4\n5\n").expect("write");
    run_git(root, &["add", "tracked.txt"], &[]);
    let pop_fallback = run_cli(root, &["pop", "idx-snap"], &[]);
    assert!(pop_fallback.status.success());
    assert!(String::from_utf8_lossy(&pop_fallback.stderr).contains("kept snapshot"));
    assert!(
        String::from_utf8_lossy(&pop_fallback.stdout).contains("without index (snapshot kept)")
    );

    let drop_out = run_cli(root, &["drop"], &[]);
    assert!(
        drop_out.status.success(),
        "drop failed: {}",
        String::from_utf8_lossy(&drop_out.stderr)
    );
    assert!(String::from_utf8_lossy(&drop_out.stdout).contains("Dropped snapshot 'idx-snap'"));

    let empty_list = run_cli(root, &["list"], &[]);
    assert!(empty_list.status.success());
    assert!(empty_list.stdout.is_empty());
}
