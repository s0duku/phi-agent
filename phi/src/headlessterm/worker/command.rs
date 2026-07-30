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
pub(crate) fn build(command: TerminalCommand) -> CommandBuilder {
    match command {
        TerminalCommand::Shell { command } => return build_shell(command),
        TerminalCommand::DockerExec {
            container,
            command,
            shell,
        } => {
            let mut builder = CommandBuilder::new("docker");
            builder.args(["exec", "--interactive", "--tty"]);
            builder.arg(container);
            builder.arg(shell);
            builder.arg("-lc");
            builder.arg(command);
            return builder;
        }
    }
}

fn build_shell(command: String) -> CommandBuilder {
    #[cfg(unix)]
    {
        let shell = unix_shell();
        let mut builder = CommandBuilder::new(&shell);
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
