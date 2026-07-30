use std::io;
use std::io::{Read, Write};
use std::path::PathBuf;

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, Listener, ListenerNonblockingMode, ListenerOptions, Name,
    Stream, ToFsName, ToNsName, prelude::*,
};
use serde::{Serialize, de::DeserializeOwned};

use crate::headlessterm::job::JobHandle;

const ENDPOINT_PREFIX: &str = "phi-headlessterm-";
const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

pub(crate) fn bind(handle: &str) -> io::Result<Listener> {
    validate_handle(handle)?;
    if !GenericNamespaced::is_supported()
        && let Some(parent) = endpoint_path(&format!("{ENDPOINT_PREFIX}{handle}")).parent()
    {
        std::fs::create_dir_all(parent)?;
    }
    let name = endpoint_name(handle)?;
    ListenerOptions::new()
        .name(name)
        .nonblocking(ListenerNonblockingMode::Accept)
        .create_sync()
}

pub(crate) fn connect(handle: &str) -> io::Result<Stream> {
    validate_handle(handle)?;
    Stream::connect(endpoint_name(handle)?)
}

pub(crate) fn write_frame<W: Write>(stream: &mut W, value: &impl Serialize) -> io::Result<()> {
    let data = serde_json::to_vec(value).map_err(io::Error::other)?;
    if data.len() > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "job control message is too large",
        ));
    }

    let length = u32::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "message is too large"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&data)
}

pub(crate) fn read_frame<R: Read, T: DeserializeOwned>(stream: &mut R) -> io::Result<T> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "job control message is too large",
        ));
    }

    let mut data = vec![0; length];
    stream.read_exact(&mut data)?;
    serde_json::from_slice(&data).map_err(io::Error::other)
}

fn endpoint_name(handle: &str) -> io::Result<Name<'static>> {
    let name = format!("{ENDPOINT_PREFIX}{handle}");
    if GenericNamespaced::is_supported() {
        return name
            .to_ns_name::<GenericNamespaced>()
            .map_err(io::Error::other);
    }

    endpoint_path(&name)
        .to_string_lossy()
        .into_owned()
        .to_fs_name::<GenericFilePath>()
        .map_err(io::Error::other)
}

fn endpoint_path(name: &str) -> PathBuf {
    #[cfg(unix)]
    {
        std::env::temp_dir()
            .join(format!("phi-headlessterm-{}", unsafe { libc::geteuid() }))
            .join(name)
    }
    #[cfg(windows)]
    {
        PathBuf::from(name)
    }
}

fn validate_handle(handle: &str) -> io::Result<()> {
    if JobHandle::is_valid(handle) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid job handle",
        ))
    }
}
