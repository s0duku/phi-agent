use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::error::PhiRuntimeError;

use super::{
    PhiHome, PhiHomeDoctorReport, PhiHomeEntry, PhiHomePath, PhiHomeUrl,
    spec::{self, path_from_relative_file},
};

pub struct LocalPhiHome {
    root: PathBuf,
    source: LocalPhiHomeSource,
}

#[derive(Clone, Copy)]
enum LocalPhiHomeSource {
    Explicit,
    CwdDotPhi,
    UserHomeFallback,
}

impl LocalPhiHome {
    pub fn detect() -> Result<Self, Box<dyn std::error::Error>> {
        let (root, source) = detect_local_phi_home_root_with_source()?;
        Self::from_root_with_source(root, source)
    }

    pub fn from_root(root: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        Self::from_root_with_source(root, LocalPhiHomeSource::Explicit)
    }

    pub fn from_entries(
        root: PathBuf,
        entries: &[PhiHomeEntry],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(&root)?;
        for entry in entries {
            let path = root.join(entry.path().as_str().trim_start_matches('/'));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, entry.content())?;
        }
        Self::from_root(root)
    }

    fn from_root_with_source(
        root: PathBuf,
        source: LocalPhiHomeSource,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(&root)?;
        Ok(Self { root, source })
    }

    #[cfg(test)]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            source: LocalPhiHomeSource::Explicit,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl PhiHome for LocalPhiHome {
    fn doctor_report(&self) -> PhiHomeDoctorReport {
        PhiHomeDoctorReport {
            kind: "local".to_string(),
            root: self.root.display().to_string(),
            source: self.source.label().to_string(),
            config_path: self.root.join("config.toml").display().to_string(),
            plugins_path: self.root.join("plugins").display().to_string(),
            tmp_path: self.root.join("tmp").display().to_string(),
        }
    }

    fn read_file(&self, source: &PhiHomeUrl) -> crate::error::PhiRuntimeResult<Vec<u8>> {
        let path = url_to_path(source)?;
        fs::read(&path).map_err(|error| {
            PhiRuntimeError::tool_execution(format!(
                "failed to read phi home file {}: {error}",
                path.display()
            ))
        })
    }

    fn entries(&self) -> Result<Vec<PhiHomeEntry>, Box<dyn std::error::Error>> {
        let mut entries = Vec::new();
        collect_entries(&self.root, &self.root, &mut entries)?;
        entries.sort_by(|left, right| left.path().as_str().cmp(right.path().as_str()));
        Ok(entries)
    }

    fn url_for_path(&self, path: &PhiHomePath) -> PhiHomeUrl {
        PhiHomeUrl::new(
            "file",
            self.root
                .join(path.as_str().trim_start_matches('/'))
                .display()
                .to_string(),
        )
    }
}

pub fn detect_local_phi_home_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(detect_local_phi_home_root_with_source_from(env::current_dir()?)?.0)
}

fn detect_local_phi_home_root_with_source()
-> Result<(PathBuf, LocalPhiHomeSource), Box<dyn std::error::Error>> {
    detect_local_phi_home_root_with_source_from(env::current_dir()?)
}

fn detect_local_phi_home_root_with_source_from(
    cwd: PathBuf,
) -> Result<(PathBuf, LocalPhiHomeSource), Box<dyn std::error::Error>> {
    let cwd_root = cwd.join(".phi");
    if cwd_root.exists() {
        return Ok((cwd_root, LocalPhiHomeSource::CwdDotPhi));
    }
    Ok((
        platform_user_home_dir()?.join(".phi"),
        LocalPhiHomeSource::UserHomeFallback,
    ))
}

fn platform_user_home_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(target_os = "windows")]
    {
        if let Some(path) = env::var_os("USERPROFILE").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(path));
        }

        let home_drive = env::var_os("HOMEDRIVE").filter(|value| !value.is_empty());
        let home_path = env::var_os("HOMEPATH").filter(|value| !value.is_empty());
        if let (Some(drive), Some(path)) = (home_drive, home_path) {
            return Ok(PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )));
        }

        return Err(
            "could not determine phi home: neither cwd/.phi nor USERPROFILE/HOMEDRIVE+HOMEPATH is available"
                .into(),
        );
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Some(path) = env::var_os("HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(path));
        }
        Err("could not determine phi home: neither cwd/.phi nor HOME is available".into())
    }
}

fn collect_entries(
    root: &Path,
    current: &Path,
    entries: &mut Vec<PhiHomeEntry>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !current.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_entries(root, &path, entries)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("phi home path escaped root {}: {error}", root.display()))?;
        let home_path = path_from_relative_file(relative)?;
        if !is_managed_home_path(&home_path) {
            continue;
        }
        entries.push(PhiHomeEntry::new(home_path, fs::read(path)?));
    }
    Ok(())
}

fn is_managed_home_path(path: &PhiHomePath) -> bool {
    path == &spec::config_path()
        || spec::is_plugin_path(path)
        || path.as_str().starts_with("/templates/")
}

fn url_to_path(url: &PhiHomeUrl) -> crate::error::PhiRuntimeResult<PathBuf> {
    if url.scheme() != "file" {
        return Err(PhiRuntimeError::tool_execution(format!(
            "local phi home only supports file urls, got {}",
            url.scheme()
        )));
    }
    Ok(PathBuf::from(url.path()))
}

impl LocalPhiHomeSource {
    fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::CwdDotPhi => "cwd_dot_phi",
            Self::UserHomeFallback => "user_home_fallback",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LocalPhiHome;
    use crate::{
        home::{PhiHome, PhiHomeEntry, spec},
        tests::support::env_lock,
    };
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn detect_prefers_existing_cwd_dot_phi_over_user_home() {
        let _lock = env_lock();
        let cwd = unique_temp_dir("phi-home-cwd");
        fs::create_dir_all(cwd.join(".phi")).expect("cwd .phi should be creatable");
        let previous_home = std::env::var_os("HOME");

        unsafe {
            std::env::set_var("HOME", "/tmp/phi-home-user-ignored");
        }

        let detected = super::detect_local_phi_home_root_with_source_from(cwd.clone())
            .expect("local phi home should resolve");
        assert_eq!(detected.0, cwd.join(".phi"));

        unsafe {
            restore_env("HOME", previous_home);
        }
        fs::remove_dir_all(cwd).expect("temp cwd should be removable");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn detect_falls_back_to_user_home_when_cwd_dot_phi_is_missing() {
        let _lock = env_lock();
        let cwd = unique_temp_dir("phi-home-cwd-fallback");
        let previous_home = std::env::var_os("HOME");

        unsafe {
            std::env::set_var("HOME", "/tmp/phi-home-user");
        }

        let detected = super::detect_local_phi_home_root_with_source_from(cwd.clone())
            .expect("local phi home should resolve");
        assert_eq!(
            detected.0,
            std::path::PathBuf::from("/tmp/phi-home-user/.phi")
        );

        unsafe {
            restore_env("HOME", previous_home);
        }
    }

    #[test]
    fn from_entries_round_trips_managed_home_files() {
        let root = unique_temp_dir("phi-home-entries");
        let entries = vec![
            PhiHomeEntry::new(spec::config_path(), b"PHI_MODEL = \"demo\"\n".to_vec()),
            PhiHomeEntry::new(
                spec::template_candidates("hello.html")
                    .expect("template path should resolve")
                    .into_iter()
                    .next()
                    .expect("template path should exist"),
                b"<message role=\"user\">hello</message>\n".to_vec(),
            ),
            PhiHomeEntry::new(
                spec::PhiHomePath::new("/plugins/hello.py").expect("plugin path should resolve"),
                b"print('hi')\n".to_vec(),
            ),
        ];

        let home = LocalPhiHome::from_entries(root.clone(), &entries)
            .expect("local phi home should be constructible from canonical entries");
        let mut expected = entries.clone();
        expected.sort_by(|left, right| left.path().as_str().cmp(right.path().as_str()));
        assert_eq!(home.entries().expect("entries should load back"), expected);

        fs::remove_dir_all(root).expect("temp home should be removable");
    }

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}"))
    }

    unsafe fn restore_env(name: &str, value: Option<std::ffi::OsString>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }
}
