use clap::{CommandFactory, Parser};

use crate::{Cli, banner};

#[test]
fn root_help_starts_with_banner() {
    let help = Cli::command().render_help().to_string();

    assert!(help.starts_with(banner::startup_banner()));
    assert!(help.contains(concat!("Version: ", env!("CARGO_PKG_VERSION"))));
    assert!(!help.contains("--config <FILE>"));
}

#[test]
fn config_is_shared_by_setup_consuming_commands() {
    for args in [
        vec!["phi", "run", "--config", "custom.yml", "--user", "hello"],
        vec!["phi", "yolo", "--config", "custom.yml", "--user", "hello"],
        vec!["phi", "step", "--config", "custom.yml", "--user", "hello"],
        vec!["phi", "doctor", "--config", "custom.yml"],
        vec![
            "phi",
            "session",
            "new",
            "session.json",
            "--config",
            "custom.yml",
        ],
    ] {
        Cli::try_parse_from(args).expect("setup-consuming command should accept --config");
    }

    for args in [
        vec!["phi", "home", "new", ".phi", "--config", "custom.yml"],
        vec![
            "phi",
            "headlessterm",
            "close",
            "job-handle",
            "--config",
            "custom.yml",
        ],
    ] {
        assert!(
            Cli::try_parse_from(args).is_err(),
            "command without setup must reject --config"
        );
    }

    assert!(
        Cli::try_parse_from([
            "phi",
            "session",
            "append",
            "session.json",
            "--config",
            "custom.yml",
        ])
        .is_err(),
        "session transforms that do not consume setup must reject --config"
    );
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
    assert!(help.contains("--null-executor"));
    assert!(!help.contains("--no-exec"));
    assert!(!help.contains("--template"));
    assert!(!help.contains("--tool-result"));
}

#[test]
fn agent_commands_accept_null_executor() {
    for command in ["run", "yolo", "step"] {
        Cli::try_parse_from(["phi", command, "--null-executor", "--user", "hello"])
            .unwrap_or_else(|error| panic!("{command} should accept --null-executor: {error}"));
    }
}

#[test]
fn cli_model_retry_matches_library_command_defaults_and_explicit_values() {
    let default_cli = Cli::try_parse_from(["phi", "step", "--user", "hello"])
        .expect("step should parse without a retry option");
    let crate::Command::Step(default_args) = default_cli.command else {
        panic!("expected step command");
    };
    let default_command = crate::agent::PhiAgentCommand::from_step_args(
        crate::agent::AgentCommandArgs::from(&default_args.base),
    )
    .expect("default step command should build");
    assert_eq!(default_command.max_model_request_retries(), None);
    assert_eq!(
        crate::agent::PhiAgentCommand::Step(crate::agent::PhiAgentCommand::step())
            .max_model_request_retries(),
        None
    );

    let explicit_cli = Cli::try_parse_from([
        "phi",
        "step",
        "--max-model-request-retries",
        "5",
        "--user",
        "hello",
    ])
    .expect("step should parse an explicit retry budget");
    let crate::Command::Step(explicit_args) = explicit_cli.command else {
        panic!("expected step command");
    };
    let explicit_command = crate::agent::PhiAgentCommand::from_step_args(
        crate::agent::AgentCommandArgs::from(&explicit_args.base),
    )
    .expect("explicit step command should build");
    assert_eq!(explicit_command.max_model_request_retries(), Some(5));
}

#[test]
fn null_executor_dominates_container_in_agent_command_options() {
    let cli = Cli::try_parse_from([
        "phi",
        "step",
        "--null-executor",
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

    assert!(command.null_executor());
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
    let headlessterm = command
        .find_subcommand_mut("headlessterm")
        .expect("headlessterm subcommand should exist");
    assert!(headlessterm.find_subcommand_mut("write").is_none());
    let access_help = headlessterm
        .find_subcommand_mut("access")
        .expect("headlessterm access subcommand should exist")
        .render_help()
        .to_string();
    assert!(access_help.contains("--data"));
    assert!(access_help.contains("--wait-ms"));
    assert!(access_help.contains("--write-only"));
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
