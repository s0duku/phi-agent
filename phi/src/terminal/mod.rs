/*
Headless terminal module
*/
mod transport;
mod launcher;
mod exec;

use std::todo;


//use crate::container::{JobHandle, JobStatus};





pub struct PhiJobHandle(pub String);

impl PhiJobHandle {
    const LETTER_COUNT: usize = 8;
    pub(crate) const ENCODED_LENGTH: usize = Self::LETTER_COUNT + 1;
    const SEPARATOR_INDEX: usize = Self::LETTER_COUNT / 2;
    const UNBIASED_BYTE_LIMIT: u8 = 26 * (u8::MAX / 26);

    pub(crate) fn random() -> Result<Self, String> {
        let mut handle = String::with_capacity(Self::ENCODED_LENGTH);
        let mut random = [0_u8; 16];
        while handle.len() < Self::ENCODED_LENGTH {
            getrandom::fill(&mut random).map_err(|error| error.to_string())?;
            for byte in random {
                if byte >= Self::UNBIASED_BYTE_LIMIT {
                    continue;
                }
                if handle.len() == Self::SEPARATOR_INDEX {
                    handle.push('-');
                }
                handle.push(char::from(b'a' + byte % 26));
                if handle.len() == Self::ENCODED_LENGTH {
                    break;
                }
            }
        }
        Ok(Self(handle))
    }


    pub(crate) fn is_valid(value: &str) -> bool {
        value.len() == Self::ENCODED_LENGTH
            && value.bytes().enumerate().all(|(index, byte)| {
                if index == Self::SEPARATOR_INDEX {
                    byte == b'-'
                } else {
                    byte.is_ascii_lowercase()
                }
            })
    }


    pub(crate) fn as_str_ref(&self) -> &str {
        &self.0
    }

}


pub enum PhiJobStatus {
    Running,
    Exited(i8),
    NoExist
}

pub struct PhiJobInfo {
    status: PhiJobStatus,
    output: String,
    truncated: bool,
    waited: std::time::Duration
}


pub enum PhiJobAccess {
    /// Write input without waiting for or acquiring an output delta.
    Write { data: String },
    /// Write input and acquire the output delta since the previous interaction.
    /// `try_wait` is the maximum duration to wait for the job to exit or for terminal
    /// output activity to settle. Output activity is used as a heuristic that
    /// meaningful new output is ready, so the request returns after the activity
    /// is followed by the configured quiet period. With no output activity, it
    /// waits for the full duration.
    Interact {
        data: String,
        try_wait: std::time::Duration,
    }
    
}


pub enum PhiJobAccessResult {
    Written(PhiJobStatus),
    Interacted(PhiJobInfo),
}


pub struct PhiHeadlessTerminal {
    driver:String
}


impl PhiHeadlessTerminal {

    pub async fn exec_job(
        cmd:&str,
        try_wait:std::time::Duration,
        expiration: std::time::Duration,
    ) -> Result<(Option<PhiJobHandle>, PhiJobInfo), String>{
        todo!()
    }

    pub async fn access_job(
        handle: PhiJobHandle, 
        access: PhiJobAccess,
    ) -> Result<PhiJobAccessResult, String> {
        todo!()
    }

    pub async fn close_job(
        handle: PhiJobHandle
    ) -> Result<PhiJobInfo, String> {
        todo!()
    }

}

