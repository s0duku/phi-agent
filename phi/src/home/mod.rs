pub mod command;
mod local;
mod spec;
mod sqlite;

pub use local::{LocalPhiHome, detect_local_phi_home_root};
pub use spec::{PhiHomeEntry, PhiHomePath};
pub use sqlite::SqlitePhiHome;

use std::{fmt, path::Path, sync::Arc};

use serde::{Deserialize, Serialize};

/// Errors owned by PhiHome implementations.
///
/// This type deliberately does not depend on the agent evaluator. Callers that
/// use a home operation while evaluating an agent step must convert it to
/// `PhiAgentRuntimeError::Home` at that boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhiHomeError {
    InvalidPath { detail: String },
    NotFound { detail: String },
    Read { detail: String },
}

pub type PhiHomeResult<T> = Result<T, PhiHomeError>;

impl PhiHomeError {
    pub(crate) fn invalid_path(detail: impl Into<String>) -> Self {
        Self::InvalidPath {
            detail: detail.into(),
        }
    }

    pub(crate) fn read(detail: impl Into<String>) -> Self {
        Self::Read {
            detail: detail.into(),
        }
    }

    pub(crate) fn not_found(detail: impl Into<String>) -> Self {
        Self::NotFound {
            detail: detail.into(),
        }
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }
}

impl fmt::Display for PhiHomeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = match self {
            Self::InvalidPath { detail } | Self::NotFound { detail } | Self::Read { detail } => {
                detail
            }
        };
        formatter.write_str(detail)
    }
}

impl std::error::Error for PhiHomeError {}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PhiHomeUrl {
    scheme: String,
    path: String,
}

impl PhiHomeUrl {
    pub fn display(&self) -> String {
        if self.scheme == "file" {
            self.path.clone()
        } else {
            format!("{}:{}", self.scheme, self.path)
        }
    }

    pub(crate) fn new(scheme: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            path: path.into(),
        }
    }

    pub(crate) fn scheme(&self) -> &str {
        &self.scheme
    }

    pub(crate) fn path(&self) -> &str {
        &self.path
    }
}

impl fmt::Display for PhiHomeUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

pub trait PhiHome: Send + Sync {
    fn doctor_report(&self) -> PhiHomeDoctorReport;

    fn read_file(&self, source: &PhiHomeUrl) -> PhiHomeResult<Vec<u8>>;

    fn entries(&self) -> Result<Vec<PhiHomeEntry>, Box<dyn std::error::Error>>;

    fn url_for_path(&self, path: &PhiHomePath) -> PhiHomeUrl;

    fn config(&self) -> PhiHomeUrl {
        self.url_for_path(&spec::config_path())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PhiHomeDoctorReport {
    pub kind: String,
    pub root: String,
    pub source: String,
    pub config_path: String,
    pub tmp_path: String,
}

pub fn load_home(spec: Option<&str>) -> Result<Arc<dyn PhiHome>, Box<dyn std::error::Error>> {
    let home: Arc<dyn PhiHome> =
        if let Some(path) = spec.map(str::trim).filter(|value| !value.is_empty()) {
            let path = Path::new(path);
            if path.is_dir() {
                Arc::new(LocalPhiHome::from_root(path.to_path_buf())?)
            } else if sqlite::looks_like_sqlite_home(path)? {
                Arc::new(SqlitePhiHome::from_path(path.to_path_buf())?)
            } else {
                return Err(format!(
                    "unsupported phi home spec {}; expected a directory or sqlite home file",
                    path.display()
                )
                .into());
            }
        } else {
            Arc::new(LocalPhiHome::detect()?)
        };
    Ok(home)
}

#[cfg(test)]
mod tests {
    use super::{LocalPhiHome, PhiHome, PhiHomeEntry, SqlitePhiHome, spec};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn local_and_sqlite_round_trip_through_shared_entries() {
        let local_root = unique_temp_path("phi-home-local");
        let sqlite_path = unique_temp_file("phi-home-sqlite");

        let entries = vec![PhiHomeEntry::new(
            spec::config_path(),
            b"model:\n  name: demo\n".to_vec(),
        )];

        let local = LocalPhiHome::from_entries(local_root.clone(), &entries)
            .expect("local phi home should be constructible from canonical entries");
        let local_entries = local.entries().expect("local entries should load");

        let sqlite = SqlitePhiHome::from_entries(sqlite_path.clone(), &local_entries)
            .expect("sqlite phi home should be constructible from local entries");
        let sqlite_entries = sqlite.entries().expect("sqlite entries should load");
        assert_eq!(local_entries, sqlite_entries);

        let local_again =
            LocalPhiHome::from_entries(unique_temp_path("phi-home-local-again"), &sqlite_entries)
                .expect("local phi home should rebuild from sqlite entries");
        assert_eq!(
            sqlite_entries,
            local_again
                .entries()
                .expect("rebuilt local entries should load")
        );

        fs::remove_dir_all(local_root).expect("temp local home should be removable");
        fs::remove_dir_all(local_again.root()).expect("rebuilt local home should be removable");
        fs::remove_file(sqlite_path).expect("temp sqlite home should be removable");
    }

    fn unique_temp_path(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }

    fn unique_temp_file(prefix: &str) -> std::path::PathBuf {
        unique_temp_path(prefix).with_extension("sqlite")
    }
}
