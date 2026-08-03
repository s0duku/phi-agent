use clap::{CommandFactory, Parser};

use crate::{Cli, banner};

#[test]
fn root_help_starts_with_banner() {
    let help = Cli::command().render_help().to_string();

    assert!(help.starts_with(banner::startup_banner()));
    assert!(help.contains(concat!("Version: ", env!("CARGO_PKG_VERSION"))));
}

#[test]
fn cli_version_comes_from_the_phi_package_manifest() {
    let error = match Cli::try_parse_from(["phi", "--version"]) {
        Ok(_) => panic!("--version should exit after rendering the package version"),
        Err(error) => error,
    };
    assert_eq!(error.exit_code(), 0);
    assert!(
        error
            .to_string()
            .contains(concat!("phi ", env!("CARGO_PKG_VERSION")))
    );
}

#[test]
fn direct_subcommand_help_starts_with_banner() {
    let mut command = Cli::command();
    let help = command
        .find_subcommand_mut("run")
        .expect("run subcommand should exist")
        .render_help()
        .to_string();

    assert!(help.starts_with(banner::startup_banner()));
    assert!(help.contains("--no-exec"));
    assert!(!help.contains("--tool-result"));
}

#[test]
fn agent_commands_accept_no_exec() {
    for command in ["run", "yolo", "step"] {
        Cli::try_parse_from(["phi", command, "--no-exec", "--user", "hello"])
            .unwrap_or_else(|error| panic!("{command} should accept --no-exec: {error}"));
    }
}

#[test]
fn no_exec_dominates_container_in_agent_command_options() {
    let cli = Cli::try_parse_from([
        "phi",
        "step",
        "--no-exec",
        "--container",
        "unused-container",
        "--user",
        "hello",
    ])
    .expect("step should accept executor options");
    let crate::Command::Step(args) = cli.command else {
        panic!("expected step command");
    };
    let command = crate::agent::PhiAgentCommand::from_step_args(
        crate::agent::AgentCommandArgs::from(&args.base),
    )
    .expect("step command should build");

    assert!(command.no_exec());
    assert_eq!(command.container(), None);
}

#[test]
fn nested_home_subcommand_help_starts_with_banner() {
    let mut command = Cli::command();
    let home = command
        .find_subcommand_mut("home")
        .expect("home subcommand should exist");
    let help = home
        .find_subcommand_mut("new")
        .expect("home new subcommand should exist")
        .render_help()
        .to_string();

    assert!(help.starts_with(banner::startup_banner()));
}

#[test]
fn headlessterm_help_exposes_launch_local() {
    let mut command = Cli::command();
    let help = command
        .find_subcommand_mut("headlessterm")
        .expect("headlessterm subcommand should exist")
        .render_help()
        .to_string();

    assert!(help.contains("launch-local"));
    assert!(help.contains("Launch a detached local headlessterm worker"));
    let exec_help = command
        .find_subcommand_mut("headlessterm")
        .expect("headlessterm subcommand should exist")
        .find_subcommand_mut("exec")
        .expect("headlessterm exec subcommand should exist")
        .render_help()
        .to_string();
    assert!(exec_help.contains("--container"));
}

#[test]
fn session_help_exposes_session_transform_commands() {
    let mut command = Cli::command();
    let help = command
        .find_subcommand_mut("session")
        .expect("session subcommand should exist")
        .render_help()
        .to_string();

    assert!(help.contains("new"));
    assert!(help.contains("Create a new initialized session file"));
    assert!(help.contains("append"));
    assert!(help.contains("next"));
    assert!(help.contains("replace"));
    assert!(help.contains("tool-result"));
    assert!(help.contains("rollback"));
    assert!(help.contains("peek"));
    assert!(help.contains("Inspect a session's current eval-state and governance status as JSON"));

    let append_help = command
        .find_subcommand_mut("session")
        .unwrap()
        .find_subcommand_mut("append")
        .unwrap()
        .render_help()
        .to_string();
    assert!(append_help.contains("--user <TEXT>"));
    assert!(append_help.contains("--assistant <TEXT>"));
    assert!(!append_help.contains("--tool-result"));

    let tool_result_help = command
        .find_subcommand_mut("session")
        .unwrap()
        .find_subcommand_mut("tool-result")
        .unwrap()
        .render_help()
        .to_string();
    assert!(tool_result_help.contains("--json <JSON>"));
    assert!(tool_result_help.contains("--text <TEXT>"));
}
