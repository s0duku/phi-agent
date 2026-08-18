use std::process::Command;

use portable_pty::CommandBuilder;

use crate::headlessterm::job::TerminalCommand;

#[cfg(unix)]
const EXIT_SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

/// Build the temporary process that owns one job's terminal session.
///
/// On Unix, portable-pty makes this process the PTY session leader. Keeping it
/// alive briefly after the user shell exits gives asynchronously launched
/// programs time to establish `nohup` handling before termination of the
/// terminal session sends SIGHUP to its foreground process group.
pub(crate) fn build(command: TerminalCommand) -> Result<CommandBuilder, String> {
    let cwd = current_working_directory()?;
    match command {
        TerminalCommand::Shell { command } => return Ok(build_shell(command, &cwd)),
        TerminalCommand::DockerExec {
            container,
            command,
            shell,
        } => {
            let cli = resolve_container_cli()?;
            validate_running_container(cli, &container)?;
            let mut builder = CommandBuilder::new(cli.executable());
            builder.args(["exec", "--interactive", "--tty"]);
            builder.arg(container);
            builder.arg(shell);
            builder.arg("-lc");
            builder.arg(command);
            return Ok(builder);
        }
        TerminalCommand::CustomRunner {
            program,
            args,
            command,
        } => {
            if program.trim().is_empty() {
                return Err("custom runner program cannot be empty".to_owned());
            }
            let mut builder = CommandBuilder::new(program);
            builder.cwd(&cwd);
            builder.args(args);
            builder.arg(command);
            return Ok(builder);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContainerCli {
    Docker,
    Podman,
}

impl ContainerCli {
    fn executable(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

fn resolve_container_cli() -> Result<ContainerCli, String> {
    let docker_version = probe_version("docker");
    let podman_available = docker_version
        .as_deref()
        .is_none_or(|version| version.to_ascii_lowercase().contains("podman"))
        && probe_version("podman").is_some();
    choose_container_cli(docker_version.as_deref(), podman_available)
        .ok_or_else(|| "neither docker nor podman is available for container execution".to_owned())
}

fn probe_version(executable: &str) -> Option<String> {
    let output = Command::new(executable).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let mut version = String::from_utf8_lossy(&output.stdout).into_owned();
    version.push_str(&String::from_utf8_lossy(&output.stderr));
    Some(version)
}

fn validate_running_container(cli: ContainerCli, container: &str) -> Result<(), String> {
    let output = Command::new(cli.executable())
        .args([
            "container",
            "inspect",
            "--format",
            "{{.State.Running}}",
            container,
        ])
        .output()
        .map_err(|error| {
            format!(
                "unable to inspect container {container:?} with {}: {error}",
                cli.executable()
            )
        })?;
    inspect_result(
        cli,
        container,
        output.status.success(),
        &output.stdout,
        &output.stderr,
    )
}

fn inspect_result(
    cli: ContainerCli,
    container: &str,
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<(), String> {
    let state = String::from_utf8_lossy(stdout);
    if success && state.trim().eq_ignore_ascii_case("true") {
        return Ok(());
    }
    if success {
        return Err(format!("container {container:?} is not running"));
    }

    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    let detail = if detail.is_empty() {
        String::from_utf8_lossy(stdout).trim().to_owned()
    } else {
        detail.to_owned()
    };
    if detail.is_empty() {
        Err(format!(
            "{} could not inspect container {container:?}",
            cli.executable()
        ))
    } else {
        Err(detail)
    }
}

fn choose_container_cli(
    docker_version: Option<&str>,
    podman_available: bool,
) -> Option<ContainerCli> {
    match docker_version {
        Some(version) if version.to_ascii_lowercase().contains("podman") => {
            podman_available.then_some(ContainerCli::Podman)
        }
        Some(_) => Some(ContainerCli::Docker),
        None if podman_available => Some(ContainerCli::Podman),
        None => None,
    }
}

fn current_working_directory() -> Result<std::path::PathBuf, String> {
    std::env::current_dir()
        .map_err(|error| format!("unable to determine current working directory: {error}"))
}

fn build_shell(command: String, cwd: &std::path::Path) -> CommandBuilder {
    #[cfg(unix)]
    {
        let shell = unix_shell();
        let mut builder = CommandBuilder::new(&shell);
        builder.cwd(&cwd);
        builder.arg("-c");
        builder.arg(
            "\"$1\" -c \"$2\"\n\
             status=$?\n\
             sleep \"$3\"\n\
             exit \"$status\"",
        );
        builder.arg("phi-session-supervisor");
        builder.arg(shell);
        builder.arg(command);
        builder.arg(EXIT_SETTLE.as_secs_f64().to_string());
        builder
    }

    #[cfg(windows)]
    {
        let mut builder = CommandBuilder::new("powershell.exe");
        builder.cwd(&cwd);
        builder.arg("-NoLogo");
        builder.arg("-NoProfile");
        builder.arg("-Command");
        builder.arg(command);
        builder
    }
}

#[cfg(unix)]
fn unix_shell() -> String {
    std::path::Path::new("/bin/bash")
        .is_file()
        .then(|| "/bin/bash".to_owned())
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join("bash"))
                    .find(|candidate| candidate.is_file())
                    .map(|path| path.to_string_lossy().into_owned())
            })
        })
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{ContainerCli, build, choose_container_cli, inspect_result};
    use crate::headlessterm::TerminalCommand;

    #[test]
    fn real_docker_uses_docker_cli() {
        assert_eq!(
            choose_container_cli(Some("Docker version 27.0.0"), true),
            Some(ContainerCli::Docker)
        );
    }

    #[test]
    fn podman_docker_compatibility_wrapper_uses_podman_directly() {
        assert_eq!(
            choose_container_cli(Some("podman version 4.9.3"), true),
            Some(ContainerCli::Podman)
        );
    }

    #[test]
    fn podman_is_used_when_docker_is_unavailable() {
        assert_eq!(choose_container_cli(None, true), Some(ContainerCli::Podman));
    }

    #[test]
    fn podman_wrapper_without_podman_is_unavailable() {
        assert_eq!(
            choose_container_cli(Some("podman version 4.9.3"), false),
            None
        );
    }

    #[test]
    fn missing_container_clis_are_reported() {
        assert_eq!(choose_container_cli(None, false), None);
    }

    #[test]
    fn local_command_builders_use_the_current_directory() {
        let cwd = std::env::current_dir().unwrap();

        let shell = build(TerminalCommand::shell("pwd")).unwrap();
        assert_eq!(shell.get_cwd().and_then(|dir| dir.to_str()), cwd.to_str());

        let runner = build(TerminalCommand::custom_runner(
            "runner",
            vec!["--flag".into()],
            "pwd",
        ))
        .unwrap();
        assert_eq!(runner.get_cwd().and_then(|dir| dir.to_str()), cwd.to_str());
    }

    #[test]
    fn running_container_passes_preflight() {
        assert_eq!(
            inspect_result(ContainerCli::Docker, "dev", true, b"true\n", b""),
            Ok(())
        );
    }

    #[test]
    fn stopped_container_fails_preflight() {
        assert_eq!(
            inspect_result(ContainerCli::Podman, "dev", true, b"false\n", b""),
            Err("container \"dev\" is not running".to_owned())
        );
    }

    #[test]
    fn missing_container_preserves_cli_error() {
        assert_eq!(
            inspect_result(
                ContainerCli::Podman,
                "missing",
                false,
                b"",
                b"Error: no container with name or ID missing found\n",
            ),
            Err("Error: no container with name or ID missing found".to_owned())
        );
    }
}
