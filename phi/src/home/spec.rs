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
    PhiHomePath("/config.toml".to_string())
}

pub fn templates_dir() -> PhiHomePath {
    PhiHomePath("/templates".to_string())
}

pub fn is_plugin_path(path: &PhiHomePath) -> bool {
    path.as_str().starts_with("/plugins/") && path.as_str().ends_with(".py")
}

pub fn template_candidates(name: &str) -> PhiHomeResult<Vec<PhiHomePath>> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(PhiHomeError::template("template name must not be empty"));
    }

    let relative = normalize_relative_component(trimmed)?;
    let mut paths = Vec::new();
    if relative.contains('.') {
        paths.push(PhiHomePath::new(format!("/templates/{relative}"))?);
    } else {
        for extension in ["j2", "html"] {
            paths.push(PhiHomePath::new(format!(
                "/templates/{relative}.{extension}"
            ))?);
        }
    }
    Ok(paths)
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

fn normalize_relative_component(value: &str) -> PhiHomeResult<String> {
    let path = std::path::Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(PhiHomeError::template(format!(
            "template name must be a safe relative path inside phi home templates/: {value}"
        )));
    }

    let parts = path
        .components()
        .map(|component| match component {
            std::path::Component::Normal(part) => part
                .to_str()
                .ok_or_else(|| PhiHomeError::template("template name must be valid unicode"))
                .map(str::to_string),
            _ => Err(PhiHomeError::template(format!(
                "template name must be a safe relative path inside phi home templates/: {value}"
            ))),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(parts.join("/"))
}
