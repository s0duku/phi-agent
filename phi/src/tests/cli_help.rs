use clap::CommandFactory;

use crate::{Cli, banner};

#[test]
fn root_help_starts_with_banner() {
    let help = Cli::command().render_help().to_string();

    assert!(help.starts_with(banner::startup_banner()));
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
}
