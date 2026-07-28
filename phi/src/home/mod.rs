pub mod command;
mod local;
mod spec;
mod sqlite;

pub use local::{LocalPhiHome, detect_local_phi_home_root};
pub use spec::{PhiHomeEntry, PhiHomePath};
pub use sqlite::SqlitePhiHome;

use std::{fmt, path::Path, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{
    config::{PhiConfig, ambient_config},
    error::{PhiRuntimeError, PhiRuntimeResult},
};

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

    #[cfg(test)]
    pub(crate) fn file_for_test(path: &str) -> Self {
        Self::new("file", path)
    }
}

impl fmt::Display for PhiHomeUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

pub trait PhiHome: Send + Sync {
    fn doctor_report(&self) -> PhiHomeDoctorReport;

    fn read_file(&self, source: &PhiHomeUrl) -> PhiRuntimeResult<Vec<u8>>;

    fn entries(&self) -> Result<Vec<PhiHomeEntry>, Box<dyn std::error::Error>>;

    fn url_for_path(&self, path: &PhiHomePath) -> PhiHomeUrl;

    fn config(&self) -> Result<PhiConfig, Box<dyn std::error::Error>> {
        let mut config = match self.read_file(&self.url_for_path(&spec::config_path())) {
            Ok(bytes) => parse_config_bytes(&bytes)?,
            Err(_) => PhiConfig::default(),
        };
        config.extend(&ambient_config());
        Ok(config)
    }

    fn list_plugins(&self) -> Result<Vec<PhiHomeUrl>, Box<dyn std::error::Error>> {
        let mut plugins = self
            .entries()?
            .into_iter()
            .filter(|entry| spec::is_plugin_path(entry.path()))
            .map(|entry| self.url_for_path(entry.path()))
            .collect::<Vec<_>>();
        plugins.sort_by(|left, right| left.display().cmp(&right.display()));
        Ok(plugins)
    }

    fn read_template(&self, name: &str) -> PhiRuntimeResult<String> {
        for path in spec::template_candidates(name)? {
            let url = self.url_for_path(&path);
            if let Ok(bytes) = self.read_file(&url) {
                return String::from_utf8(bytes).map_err(|error| {
                    PhiRuntimeError::session(format!(
                        "failed to decode phi template {} as UTF-8: {error}",
                        path.as_str()
                    ))
                });
            }
        }

        Err(PhiRuntimeError::session(format!(
            "template not found: {} (searched under {})",
            name.trim(),
            spec::templates_dir().as_str()
        )))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PhiHomeDoctorReport {
    pub kind: String,
    pub root: String,
    pub source: String,
    pub config_path: String,
    pub plugins_path: String,
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

fn parse_config_bytes(bytes: &[u8]) -> Result<PhiConfig, Box<dyn std::error::Error>> {
    let contents = std::str::from_utf8(bytes)?;
    let value = contents.parse::<toml::Value>()?;
    let table = value
        .as_table()
        .ok_or_else(|| "phi home config.toml must contain a top-level table".to_string())?;
    let mut config = std::collections::BTreeMap::new();

    for (key, value) in table {
        let encoded = match value {
            toml::Value::String(text) => text.clone(),
            toml::Value::Integer(number) => number.to_string(),
            toml::Value::Float(number) => number.to_string(),
            toml::Value::Boolean(flag) => flag.to_string(),
            toml::Value::Datetime(datetime) => datetime.to_string(),
            toml::Value::Array(_) | toml::Value::Table(_) => {
                return Err(format!(
                    "phi home config key '{key}' must be a scalar value, not {}",
                    value.type_str()
                )
                .into());
            }
        };
        config.insert(key.clone(), encoded);
    }

    Ok(PhiConfig::new(config))
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

        let entries = vec![
            PhiHomeEntry::new(spec::config_path(), b"PHI_MODEL = \"demo\"\n".to_vec()),
            PhiHomeEntry::new(
                spec::PhiHomePath::new("/plugins/hello.py").expect("plugin path should resolve"),
                b"print('hi')\n".to_vec(),
            ),
            PhiHomeEntry::new(
                spec::template_candidates("hello.html")
                    .expect("template path should resolve")
                    .into_iter()
                    .next()
                    .expect("template path should exist"),
                b"<message role=\"user\">hello</message>\n".to_vec(),
            ),
        ];

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
