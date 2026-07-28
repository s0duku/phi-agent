use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use anyhow::{Context, Result, bail};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("install") => install(args.collect()),
        Some("windows") => windows(args.collect()),
        Some(command) => bail!("unknown xtask command: {command}"),
        None => bail!("missing xtask command"),
    }
}

fn windows(args: Vec<String>) -> Result<()> {
    let action = args.first().context("missing windows action")?;
    if !matches!(action.as_str(), "build" | "test") {
        bail!("unknown windows action: {action}");
    }
    let target = option_value(&args[1..], "--target")?
        .unwrap_or_else(|| "x86_64-pc-windows-msvc".to_owned());
    let workspace = workspace_root()?;

    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut command = Command::new(cargo);
    command.current_dir(&workspace);
    command.arg(action);
    command.arg("--workspace");
    if action == "build" {
        command.arg("--release");
    } else {
        command.arg("--all-targets");
    }
    command.args(["--target", &target]);
    run_command(&mut command, "Windows workspace")
}

fn option_value(args: &[String], option: &str) -> Result<Option<String>> {
    match args {
        [] => Ok(None),
        [name, value] if name == option => Ok(Some(value.clone())),
        [name] if name == option => bail!("missing value for {option}"),
        _ => bail!("invalid windows options: {}", args.join(" ")),
    }
}

fn install(args: Vec<String>) -> Result<()> {
    let mut target = None::<String>;
    let mut root = None::<String>;
    let mut offline = false;

    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--target" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .context("missing value for --target")?;
                target = Some(value);
            }
            "--root" => {
                index += 1;
                let value = args
                    .get(index)
                    .cloned()
                    .context("missing value for --root")?;
                root = Some(value);
            }
            "--offline" => offline = true,
            other => bail!("unknown install option: {other}"),
        }
        index += 1;
    }

    let workspace = workspace_root()?;
    install_package(
        &workspace,
        "phi",
        target.as_deref(),
        root.as_deref(),
        offline,
    )?;
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map(PathBuf::from)
        .context("xtask workspace root is missing")
}

fn install_package(
    workspace: &Path,
    package: &str,
    target: Option<&str>,
    root: Option<&str>,
    offline: bool,
) -> Result<()> {
    println!("Installing {package}...");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let mut command = Command::new(cargo);
    command.arg("install");
    command.arg("--path");
    command.arg(workspace.join(package));
    if let Some(target) = target {
        command.arg("--target");
        command.arg(target);
    }
    if let Some(root) = root {
        command.arg("--root");
        command.arg(root);
    }
    if offline {
        command.arg("--offline");
    }
    run_command(&mut command, package)
}

fn run_command(command: &mut Command, package: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to spawn cargo install for {package}"))?;
    ensure_success(status, package)
}

fn ensure_success(status: ExitStatus, package: &str) -> Result<()> {
    if status.success() {
        return Ok(());
    }
    bail!("cargo install failed for {package} with status {status}")
}
