use std::io;

use portable_pty::CommandBuilder;

pub(crate) fn build_shell_command(command: &str) -> CommandBuilder {
    let mut builder = CommandBuilder::new("powershell.exe");
    builder.arg("-NoLogo");
    builder.arg("-NoProfile");
    builder.arg("-Command");
    builder.arg(command);
    builder
}

pub(crate) fn poll_process_status(
    child: &mut (dyn portable_pty::Child + Send + Sync),
) -> io::Result<Option<i8>> {
    child
        .try_wait()
        .map(|status| status.map(|status| status.exit_code() as u8 as i8))
}

pub(crate) fn kill_process(
    child: &mut (dyn portable_pty::Child + Send + Sync),
    _force: bool,
) -> io::Result<()> {
    child.kill()
}
