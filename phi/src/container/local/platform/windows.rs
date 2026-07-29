use std::io;

pub(crate) fn disable_pty_echo(_master: &dyn portable_pty::MasterPty) -> io::Result<()> {
    // ConPTY does not expose Unix-style line-discipline echo through
    // portable-pty; applications remain responsible for their own rendering.
    Ok(())
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
