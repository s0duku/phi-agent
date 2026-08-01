use std::fs;

use clap::Parser;

use crate::{
    Cli, StepArgs,
    agent::{PhiAgentCommand, StepCommandInput},
    features::plugin::{
        PluginAvailability, PyPluginDescriptor, descriptor_from_context, python_plugin_status,
    },
    home::{LocalPhiHome, PhiHome},
    message::PhiMessage,
};

#[test]
fn step_cli_collects_trailing_plugin_args_after_double_dash() {
    let cli = Cli::try_parse_from([
        "phi",
        "step",
        "--user",
        "hello",
        "--",
        "--plugin-flag",
        "123",
        "--mode=test",
    ])
    .expect("CLI should accept trailing plugin args after --");

    let crate::Command::Step(args) = cli.command else {
        panic!("expected step command");
    };

    assert_eq!(
        args.base.plugin_args,
        vec![
            "--plugin-flag".to_string(),
            "123".to_string(),
            "--mode=test".to_string(),
        ]
    );
}

#[test]
fn plugin_descriptor_captures_build_time_command_view() {
    let args = StepArgs {
        base: crate::AgentCliArgs {
            session_path: None,
            quiet: false,
            plugin_args: vec!["--plugin-flag".to_string(), "123".to_string()],
            input_messages: vec![],
        },
        max_model_request_retries: Some(3),
        template: None,
        container: None,
    };
    let command =
        PhiAgentCommand::try_from(StepCommandInput { args: &args }).expect("command should build");

    let builder = crate::agent::PhiAgent::builder(
        crate::session::Session::from_root(
            crate::session::PhiAgentStep::request_provider(
                "ready",
                &crate::tests::support::test_model_defaults(),
            ),
            vec![PhiMessage::user("hello")],
        ),
        command,
    );

    assert_eq!(
        descriptor_from_context(builder.context()),
        PyPluginDescriptor {
            command_kind: "step".to_string(),
            plugin_args: vec!["--plugin-flag".to_string(), "123".to_string()],
        }
    );
}

#[test]
fn filesystem_home_discovers_python_plugins_sorted_by_filename() {
    let root = unique_temp_dir("phi-plugin-home");
    let plugin_dir = root.join("plugins");
    fs::create_dir_all(&plugin_dir).expect("plugin dir should be creatable");
    fs::write(plugin_dir.join("b_second.py"), "print('second')\n")
        .expect("second plugin should be writable");
    fs::write(plugin_dir.join("a_first.py"), "print('first')\n")
        .expect("first plugin should be writable");
    fs::write(plugin_dir.join("note.txt"), "ignored\n").expect("extra file should be writable");

    let home = LocalPhiHome::new(root.clone());
    let plugins = home
        .list_plugins()
        .expect("plugin discovery should succeed");

    assert_eq!(plugins.len(), 2);
    assert!(plugins[0].display().ends_with("a_first.py"));
    assert_eq!(
        String::from_utf8(home.read_file(&plugins[0]).unwrap()).unwrap(),
        "print('first')\n"
    );
    assert!(plugins[1].display().ends_with("b_second.py"));

    fs::remove_dir_all(root).expect("temp plugin home should be removable");
}

#[test]
fn python_plugin_status_reports_enabled_or_disabled_cleanly() {
    let status = python_plugin_status();
    assert_eq!(status.provider, "python");
    assert!(
        !status.build.configured_backends.is_empty(),
        "at least one python backend should be compiled in for default builds",
    );
    assert!(
        status
            .build
            .configured_backends
            .iter()
            .any(|backend| backend == "subprocess"),
        "subprocess feature should be reflected in build status",
    );
    assert_eq!(status.build.minimum_version, "3.11");
    match status.availability {
        PluginAvailability::Enabled { runtime } => {
            assert_eq!(runtime.backend, "subprocess");
            assert!(
                runtime.implementation == "cpython"
                    || runtime.implementation == "python"
                    || runtime.implementation == "pypy",
                "unexpected python implementation: {}",
                runtime.implementation
            );
            assert!(
                runtime.version.starts_with("3."),
                "unexpected Python version: {}",
                runtime.version
            );
            assert!(
                runtime
                    .library
                    .as_deref()
                    .is_some_and(|location| !location.trim().is_empty()),
                "enabled runtime should report the probed python location"
            );
        }
        PluginAvailability::Disabled { reason } => {
            assert!(
                !reason.trim().is_empty(),
                "disabled plugin availability should explain why",
            );
        }
    }
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{nanos}"))
}
