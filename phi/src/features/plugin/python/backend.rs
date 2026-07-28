use crate::executor::PhiToolDefinition;
use crate::home::PhiHomeUrl;

use super::super::types::{LoadedPyPlugin, PythonRuntimeInfo};

pub(crate) const MINIMUM_PYTHON_MAJOR: u32 = 3;
pub(crate) const MINIMUM_PYTHON_MINOR: u32 = 11;

pub trait PhiPythonRuntime: Send + Sync {
    fn runtime_info(&self) -> &PythonRuntimeInfo;

    fn load_plugin(&self, source: &PhiHomeUrl, code: &str) -> Result<LoadedPyPlugin, String>;

    fn list_tools(&self) -> Result<Vec<PhiToolDefinition>, String>;

    fn call_tool(&self, name: &str, arguments: &serde_json::Value) -> Result<String, String>;

    fn run_code(&self, code: &str) -> Result<String, String>;
}

pub(crate) fn minimum_version_string() -> String {
    format!("{MINIMUM_PYTHON_MAJOR}.{MINIMUM_PYTHON_MINOR}")
}

pub(crate) fn ensure_supported_version(version: &str) -> Result<(), String> {
    let Some((major, minor)) = parse_python_version(version) else {
        return Err(format!("could not parse Python version from '{version}'"));
    };

    if (major, minor) < (MINIMUM_PYTHON_MAJOR, MINIMUM_PYTHON_MINOR) {
        return Err(format!(
            "python {major}.{minor} is below the required minimum version {}",
            minimum_version_string()
        ));
    }

    Ok(())
}

fn parse_python_version(version: &str) -> Option<(u32, u32)> {
    let mut parts = version.split_whitespace().next()?.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::{MINIMUM_PYTHON_MAJOR, MINIMUM_PYTHON_MINOR, parse_python_version};

    #[test]
    fn parses_cpython_version_prefix() {
        assert_eq!(
            parse_python_version("3.12.4 (main, Jun 1 2026, 00:00:00)"),
            Some((3, 12))
        );
        assert_eq!(parse_python_version("3.11.9"), Some((3, 11)));
    }

    #[test]
    fn rejects_unparseable_versions() {
        assert_eq!(parse_python_version("cpython"), None);
        assert_eq!(parse_python_version(""), None);
    }

    #[test]
    fn minimum_version_contract_stays_at_python_311() {
        assert_eq!((MINIMUM_PYTHON_MAJOR, MINIMUM_PYTHON_MINOR), (3, 11));
    }
}
