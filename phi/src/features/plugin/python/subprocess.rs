use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::Mutex,
};

use serde::Deserialize;

use super::super::types::{LoadedPyPlugin, PythonRuntimeInfo};
use super::backend::{PhiPythonRuntime, ensure_supported_version};
use crate::executor::PhiToolDefinition;
use crate::features::plugin::python::worker_template_source;
use crate::features::plugin::runtime::protocol::{PythonRuntimeRequest, PythonRuntimeResponse};
use crate::features::plugin::sdk::{PHI_PYTHON_MODULE_NAME, python_module_source};
use crate::home::PhiHomeUrl;

fn worker_script() -> String {
    let sdk_source = serde_json::to_string(python_module_source())
        .expect("embedded phi python sdk source should serialize");
    worker_template_source()
        .replace("__PHI_SDK_SOURCE__", &sdk_source)
        .replace("__PHI_MODULE_NAME__", PHI_PYTHON_MODULE_NAME)
}

pub(crate) fn load_backend() -> Result<Box<dyn PhiPythonRuntime>, String> {
    let executable = detect_python_executable()?;
    let info = probe_runtime_info(&executable)?;
    ensure_supported_version(&info.version)?;
    let worker = SubprocessWorker::spawn(&executable)?;

    Ok(Box::new(SubprocessPythonRuntime {
        info,
        worker: Mutex::new(worker),
    }))
}

struct SubprocessPythonRuntime {
    info: PythonRuntimeInfo,
    worker: Mutex<SubprocessWorker>,
}

impl PhiPythonRuntime for SubprocessPythonRuntime {
    fn runtime_info(&self) -> &PythonRuntimeInfo {
        &self.info
    }

    fn load_plugin(&self, source: &PhiHomeUrl, code: &str) -> Result<LoadedPyPlugin, String> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| "python worker mutex was poisoned".to_string())?;
        let response = worker.load_plugin(source.display(), code)?;
        Ok(LoadedPyPlugin {
            name: response.name.unwrap_or_else(|| plugin_name(source)),
            backend: self.info.backend.clone(),
            source: Some(source.display()),
        })
    }

    fn list_tools(&self) -> Result<Vec<PhiToolDefinition>, String> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| "python worker mutex was poisoned".to_string())?;
        worker.list_tools()
    }

    fn call_tool(&self, name: &str, arguments: &serde_json::Value) -> Result<String, String> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| "python worker mutex was poisoned".to_string())?;
        worker.call_tool(name, arguments)
    }

    fn run_code(&self, code: &str) -> Result<String, String> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| "python worker mutex was poisoned".to_string())?;
        let response = worker.run_code(code)?;
        Ok(response.output.unwrap_or_default())
    }
}

impl Drop for SubprocessPythonRuntime {
    fn drop(&mut self) {
        if let Ok(worker) = self.worker.get_mut() {
            let _ = worker.child.kill();
            let _ = worker.child.wait();
        }
    }
}

struct SubprocessWorker {
    child: Child,
    stdin: BufWriter<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl SubprocessWorker {
    fn spawn(executable: &Path) -> Result<Self, String> {
        let worker_script = worker_script();
        let mut child = Command::new(executable)
            .args(["-u", "-c", &worker_script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| {
                format!(
                    "failed to spawn python worker '{}': {error}",
                    executable.display()
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "python worker did not expose stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "python worker did not expose stdout".to_string())?;

        let mut worker = Self {
            child,
            stdin: BufWriter::new(stdin),
            stdout: BufReader::new(stdout),
        };
        worker.ping()?;
        Ok(worker)
    }

    fn ping(&mut self) -> Result<(), String> {
        let response = self.request(&PythonRuntimeRequest::Ping)?;
        if response.ok {
            Ok(())
        } else {
            Err(response
                .error
                .unwrap_or_else(|| "python worker ping failed".to_string()))
        }
    }

    fn load_plugin(&mut self, source: String, code: &str) -> Result<PythonRuntimeResponse, String> {
        let response = self.request(&PythonRuntimeRequest::LoadPlugin {
            source,
            code: code.to_string(),
        })?;
        if response.ok {
            Ok(response)
        } else {
            Err(response
                .error
                .unwrap_or_else(|| "python worker rejected plugin load".to_string()))
        }
    }

    fn list_tools(&mut self) -> Result<Vec<PhiToolDefinition>, String> {
        let response = self.request(&PythonRuntimeRequest::ListTools)?;
        if response.ok {
            Ok(response.tools.unwrap_or_default())
        } else {
            Err(response
                .error
                .unwrap_or_else(|| "python worker failed to list tools".to_string()))
        }
    }

    fn call_tool(&mut self, name: &str, arguments: &serde_json::Value) -> Result<String, String> {
        let response = self.request(&PythonRuntimeRequest::CallTool {
            name: name.to_string(),
            arguments: arguments.clone(),
        })?;
        if response.ok {
            Ok(response.output.unwrap_or_default())
        } else {
            Err(response
                .error
                .unwrap_or_else(|| format!("python worker tool call failed for {name}")))
        }
    }

    fn run_code(&mut self, code: &str) -> Result<PythonRuntimeResponse, String> {
        let response = self.request(&PythonRuntimeRequest::RunCode {
            code: code.to_string(),
        })?;
        if response.ok {
            Ok(response)
        } else {
            Err(response
                .error
                .unwrap_or_else(|| "python worker code execution failed".to_string()))
        }
    }

    fn request(&mut self, request: &PythonRuntimeRequest) -> Result<PythonRuntimeResponse, String> {
        if let Some(status) = self
            .child
            .try_wait()
            .map_err(|error| format!("failed to poll python worker status: {error}"))?
        {
            return Err(format!("python worker exited early with status {status}"));
        }

        let payload = serde_json::to_string(request)
            .map_err(|error| format!("failed to encode python worker request: {error}"))?;
        self.stdin
            .write_all(payload.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|error| format!("failed to write python worker request: {error}"))?;

        let mut line = String::new();
        let bytes = self
            .stdout
            .read_line(&mut line)
            .map_err(|error| format!("failed to read python worker response: {error}"))?;
        if bytes == 0 {
            return Err("python worker closed stdout unexpectedly".to_string());
        }

        serde_json::from_str::<PythonRuntimeResponse>(line.trim_end())
            .map_err(|error| format!("failed to decode python worker response: {error}"))
    }
}

fn detect_python_executable() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("PHI_PYTHON").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let candidates: &[(&str, &[&str])] = if cfg!(windows) {
        &[
            ("python.exe", &[]),
            ("python3.exe", &[]),
            ("python", &[]),
            ("py", &["-3"]),
        ]
    } else {
        &[("python3", &[]), ("python", &[])]
    };

    let mut failures = Vec::new();
    for (program, prefix_args) in candidates {
        let mut command = Command::new(program);
        command.args(*prefix_args);
        command.args(["-c", "import sys; print(sys.executable)"]);
        match command.output() {
            Ok(output) if output.status.success() => {
                let path = String::from_utf8(output.stdout).map_err(|error| {
                    format!("python executable probe returned non-UTF-8 data: {error}")
                })?;
                let trimmed = path.trim();
                if !trimmed.is_empty() {
                    return Ok(PathBuf::from(trimmed));
                }
                failures.push(format!(
                    "{program}: probe returned an empty executable path"
                ));
            }
            Ok(output) => failures.push(format!(
                "{program}: probe exited with status {}",
                output.status
            )),
            Err(error) => failures.push(format!("{program}: {error}")),
        }
    }

    Err(format!(
        "no python executable found; tried {}",
        failures.join(", ")
    ))
}

fn probe_runtime_info(executable: &Path) -> Result<PythonRuntimeInfo, String> {
    let output = Command::new(executable)
        .args([
            "-c",
            "import json,platform,sys; print(json.dumps({'version': sys.version, 'implementation': platform.python_implementation().lower(), 'executable': sys.executable}))",
        ])
        .output()
        .map_err(|error| {
            format!(
                "failed to probe python runtime '{}': {error}",
                executable.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "python runtime probe for '{}' exited with status {}",
            executable.display(),
            output.status
        ));
    }

    let payload = String::from_utf8(output.stdout)
        .map_err(|error| format!("python runtime probe returned non-UTF-8 data: {error}"))?;
    let probe: RuntimeProbe = serde_json::from_str(payload.trim())
        .map_err(|error| format!("failed to decode python runtime probe: {error}"))?;

    Ok(PythonRuntimeInfo {
        backend: "subprocess".to_string(),
        version: probe.version.trim().to_string(),
        implementation: probe.implementation,
        library: Some(probe.executable),
    })
}

#[derive(Deserialize)]
struct RuntimeProbe {
    version: String,
    implementation: String,
    executable: String,
}

fn plugin_name(source: &PhiHomeUrl) -> String {
    Path::new(&source.display())
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("plugin")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{plugin_name, worker_script};
    use crate::home::PhiHomeUrl;

    #[test]
    fn plugin_name_falls_back_to_path_stem() {
        assert_eq!(
            plugin_name(&PhiHomeUrl::file_for_test("/tmp/example.py")),
            "example"
        );
    }

    #[test]
    fn worker_script_bootstraps_phi_sdk_module() {
        let script = worker_script();
        assert!(script.contains("install_phi_sdk"));
        assert!(script.contains("sys.modules[\"phi\"]"));
    }
}
