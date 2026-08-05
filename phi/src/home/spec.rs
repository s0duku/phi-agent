use super::{PhiHomeError, PhiHomeResult};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PhiHomePath(String);

impl PhiHomePath {
    pub(crate) fn new(path: impl Into<String>) -> PhiHomeResult<Self> {
        let path = path.into();
        validate_home_path(&path)?;
        Ok(Self(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhiHomeEntry {
    path: PhiHomePath,
    content: Vec<u8>,
}

impl PhiHomeEntry {
    pub fn new(path: PhiHomePath, content: Vec<u8>) -> Self {
        Self { path, content }
    }

    pub fn path(&self) -> &PhiHomePath {
        &self.path
    }

    pub fn content(&self) -> &[u8] {
        &self.content
    }

    pub fn into_parts(self) -> (PhiHomePath, Vec<u8>) {
        (self.path, self.content)
    }
}

pub fn config_path() -> PhiHomePath {
    PhiHomePath("/config.yml".to_string())
}

pub(crate) fn path_from_relative_file(relative: &std::path::Path) -> PhiHomeResult<PhiHomePath> {
    let parts = relative
        .components()
        .map(|component| match component {
            std::path::Component::Normal(part) => part
                .to_str()
                .ok_or_else(|| PhiHomeError::invalid_path("phi home path is not valid unicode"))
                .map(str::to_string),
            _ => Err(PhiHomeError::invalid_path(
                "phi home path contains unsupported components",
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    PhiHomePath::new(format!("/{}", parts.join("/")))
}

fn validate_home_path(path: &str) -> PhiHomeResult<()> {
    if !path.starts_with('/') {
        return Err(PhiHomeError::invalid_path(format!(
            "phi home path must start with '/': {path}"
        )));
    }
    if path.contains("//") {
        return Err(PhiHomeError::invalid_path(format!(
            "phi home path must not contain empty components: {path}"
        )));
    }
    if path == "/" || path.ends_with('/') {
        return Err(PhiHomeError::invalid_path(format!(
            "phi home path must point to a file-like resource: {path}"
        )));
    }
    for part in path.split('/').filter(|part| !part.is_empty()) {
        if part == "." || part == ".." {
            return Err(PhiHomeError::invalid_path(format!(
                "phi home path must not contain traversal components: {path}"
            )));
        }
    }
    Ok(())
}
