use clap::Parser;

use super::{Cli, Command};
use crate::snapshot::{CreateRequest, SnapshotKind};

fn parse_save(args: &[&str]) -> CreateRequest {
    let cli = Cli::try_parse_from(args).expect("save command should parse");
    match cli.command {
        Command::Save(save_args) => save_args.into_create_request(),
        other => panic!("expected Command::Save, got {other:?}"),
    }
}

#[test]
fn parses_save_scopes_and_flags() {
    assert_eq!(
        parse_save(&["git-snapshot", "save"]),
        CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: false,
            },
            name: None,
            message: None,
            force: false,
        }
    );

    assert_eq!(
        parse_save(&[
            "git-snapshot",
            "save",
            "--both",
            "-u",
            "-f",
            "-m",
            "wip",
            "feat/a",
        ]),
        CreateRequest {
            kind: SnapshotKind::Both {
                include_untracked: true,
            },
            name: Some("feat/a".to_string()),
            message: Some("wip".to_string()),
            force: true,
        }
    );

    assert_eq!(
        parse_save(&["git-snapshot", "save", "--index", "idx-only"]),
        CreateRequest {
            kind: SnapshotKind::Index,
            name: Some("idx-only".to_string()),
            message: None,
            force: false,
        }
    );

    assert_eq!(
        parse_save(&["git-snapshot", "save", "--worktree", "--include-untracked"]),
        CreateRequest {
            kind: SnapshotKind::Worktree {
                include_untracked: true,
            },
            name: None,
            message: None,
            force: false,
        }
    );
}

#[test]
fn rejects_conflicting_save_options() {
    for args in [
        &["git-snapshot", "save", "--index", "-u"][..],
        &["git-snapshot", "save", "--index", "--worktree"][..],
        &["git-snapshot", "save", "--both", "--index"][..],
        &["git-snapshot", "save", "--both", "--worktree"][..],
    ] {
        let err = Cli::try_parse_from(args).expect_err("conflicting options must fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }
}

#[test]
fn parses_apply_pop_drop_and_list() {
    let apply = Cli::try_parse_from(["git-snapshot", "apply"]).expect("apply");
    assert_eq!(apply.command, Command::Apply { name: None });

    let pop = Cli::try_parse_from(["git-snapshot", "pop", "snap1"]).expect("pop");
    assert_eq!(
        pop.command,
        Command::Pop {
            name: Some("snap1".to_string()),
        }
    );

    let drop = Cli::try_parse_from(["git-snapshot", "drop"]).expect("drop");
    assert_eq!(drop.command, Command::Drop { name: None });

    let list = Cli::try_parse_from(["git-snapshot", "list"]).expect("list");
    assert_eq!(list.command, Command::List);
}
