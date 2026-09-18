use std::fs::{self, OpenOptions};
use std::io;
use std::path::PathBuf;

use super::protocol::Response;
use crate::headlessterm::job::JobHandle;

const PREFIX: &str = "phi-job-";

pub(crate) fn store(handle: &str, response: &Response) -> io::Result<()> {
    let target = path(handle)?;
    let temporary = target.with_extension(format!("tmp-{}", std::process::id()));
    let data = serde_json::to_vec(response).map_err(io::Error::other)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    std::io::Write::write_all(&mut file, &data)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, target)
}

pub(crate) fn take(handle: &str) -> io::Result<Option<Response>> {
    let target = path(handle)?;
    let data = match fs::read(&target) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let response = serde_json::from_slice(&data).map_err(io::Error::other)?;
    match fs::remove_file(&target) {
        Ok(()) => Ok(Some(response)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn path(handle: &str) -> io::Result<PathBuf> {
    if !JobHandle::is_valid(handle) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid job handle",
        ));
    }
    Ok(std::env::temp_dir().join(format!("{PREFIX}{handle}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headlessterm::worker::protocol::{Response, Status};

    #[test]
    fn stored_response_is_read_once_and_removed() {
        let handle = JobHandle::random().unwrap();
        let response = Response::Terminal {
            status: Status::Exited(7),
            output: "tail".into(),
            truncated: false,
            waited_ms: 12,
        };
        store(&handle.0, &response).unwrap();
        assert!(matches!(
            take(&handle.0).unwrap(),
            Some(Response::Terminal { .. })
        ));
        assert!(take(&handle.0).unwrap().is_none());
    }
}
