use std::sync::Arc;

use async_trait::async_trait;

use crate::executor::{
    PhiTool, ToolCallOutput, ToolCallRequest, ToolCallResponse, ToolExecutionLimits,
};
use crate::module::PhiModule;

use super::python::PhiPythonRuntime;
use super::types::{LoadedPyPlugin, PyPluginDescriptor};
use crate::home::{PhiHome, PhiHomeUrl};

pub(crate) struct PyPluginModule {
    descriptor: PyPluginDescriptor,
    home: Arc<dyn PhiHome>,
    runtime: Arc<dyn PhiPythonRuntime>,
    discovered_plugins: Vec<PhiHomeUrl>,
    loaded_plugins: Vec<LoadedPyPlugin>,
}

impl PyPluginModule {
    pub(crate) fn new(
        descriptor: PyPluginDescriptor,
        home: Arc<dyn PhiHome>,
        runtime: Box<dyn PhiPythonRuntime>,
        discovered_plugins: Vec<PhiHomeUrl>,
    ) -> Self {
        Self {
            descriptor,
            home,
            runtime: Arc::from(runtime),
            discovered_plugins,
            loaded_plugins: Vec::new(),
        }
    }

    fn ensure_plugins_loaded(&mut self) {
        if !self.loaded_plugins.is_empty() {
            return;
        }

        for plugin in &self.discovered_plugins {
            let code = match self.home.read_file(plugin) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(code) => code,
                    Err(error) => {
                        eprintln!(
                            "phi plugin: failed to decode '{}' for {} command: {error}",
                            plugin.display(),
                            self.descriptor.command_kind
                        );
                        continue;
                    }
                },
                Err(error) => {
                    eprintln!(
                        "phi plugin: failed to read '{}' for {} command: {error}",
                        plugin.display(),
                        self.descriptor.command_kind
                    );
                    continue;
                }
            };

            match self.runtime.load_plugin(plugin, &code) {
                Ok(loaded) => self.loaded_plugins.push(loaded),
                Err(error) => eprintln!(
                    "phi plugin: failed to load '{}' for {} command: {error}",
                    plugin.display(),
                    self.descriptor.command_kind
                ),
            }
        }
    }
}

struct PythonPluginTool {
    definition: crate::executor::PhiToolDefinition,
    runtime: Arc<dyn PhiPythonRuntime>,
}

#[async_trait]
impl PhiTool for PythonPluginTool {
    fn name(&self) -> &str {
        &self.definition.name
    }

    fn description(&self) -> &str {
        &self.definition.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.definition.parameters.clone()
    }

    async fn call(
        &self,
        request: &mut ToolCallRequest,
        _limits: ToolExecutionLimits,
        _runtime: &crate::agent::PhiAgentRuntime,
    ) -> ToolCallResponse {
        let request_snapshot = request.clone();
        let output = match self
            .runtime
            .call_tool(&self.definition.name, &request.arguments)
        {
            Ok(output) => output,
            Err(error) => {
                return ToolCallResponse::failure(
                    &request_snapshot,
                    self.definition.name.clone(),
                    error,
                    serde_json::Value::Null,
                );
            }
        };
        let output = serde_json::from_str::<ToolCallOutput>(&output)
            .unwrap_or_else(|_| ToolCallOutput::success(serde_json::Value::String(output)));
        ToolCallResponse {
            id: request.id.clone(),
            call_id: request.call_id.clone(),
            name: self.definition.name.clone(),
            output,
        }
    }
}

impl PhiModule for PyPluginModule {
    type ProbInfo = ();

    fn init_context(
        &mut self,
        _context: &mut crate::agent::PhiAgentBuildContext,
    ) -> crate::error::PhiRuntimeResult<()> {
        self.ensure_plugins_loaded();
        Ok(())
    }

    fn module_tools(
        &mut self,
        _context: &crate::agent::PhiAgentBuildContext,
    ) -> Vec<Arc<dyn PhiTool>> {
        self.ensure_plugins_loaded();
        let tools = match self.runtime.list_tools() {
            Ok(tools) => tools,
            Err(error) => {
                eprintln!(
                    "phi plugin: failed to collect plugin tools for {} command: {error}",
                    self.descriptor.command_kind
                );
                return Vec::new();
            }
        };

        tools
            .into_iter()
            .map(|definition| {
                Arc::new(PythonPluginTool {
                    definition,
                    runtime: self.runtime.clone(),
                }) as Arc<dyn PhiTool>
            })
            .collect()
    }

    fn run_python_code(
        &mut self,
        code: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        self.runtime
            .run_code(code)
            .map(Some)
            .map_err(|error| -> Box<dyn std::error::Error> { error.into() })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        agent::{PhiAgentBuildContext, PhiAgentCommand},
        features::plugin::{
            PhiPythonRuntime,
            types::{LoadedPyPlugin, PyPluginDescriptor, PythonRuntimeInfo},
        },
        home::{PhiHome, PhiHomeEntry, PhiHomePath, PhiHomeUrl},
        session::Session,
    };

    use super::PyPluginModule;

    struct MockBackend;

    struct MockHome;

    impl PhiHome for MockHome {
        fn doctor_report(&self) -> crate::home::PhiHomeDoctorReport {
            crate::home::PhiHomeDoctorReport {
                kind: "mock".to_string(),
                root: "/mock".to_string(),
                source: "test".to_string(),
                config_path: "/mock/config.toml".to_string(),
                plugins_path: "/mock/plugins".to_string(),
                tmp_path: "/mock/tmp".to_string(),
            }
        }

        fn config(&self) -> Result<crate::config::PhiConfig, Box<dyn std::error::Error>> {
            Ok(crate::config::PhiConfig::default())
        }

        fn list_plugins(&self) -> Result<Vec<PhiHomeUrl>, Box<dyn std::error::Error>> {
            Ok(Vec::new())
        }

        fn read_template(&self, _name: &str) -> crate::error::PhiRuntimeResult<String> {
            unreachable!("templates are not used in plugin module tests")
        }

        fn read_file(&self, source: &PhiHomeUrl) -> crate::error::PhiRuntimeResult<Vec<u8>> {
            if source.display().contains("broken.py") {
                Ok(b"   ".to_vec())
            } else if source.display().contains("printy.py") {
                Ok(b"print('hello from plugin')".to_vec())
            } else if source.display().contains("stateful.py") {
                Ok(b"shared_answer = 41".to_vec())
            } else if source.display().contains("tooly.py") {
                Ok(br#"import phi

@phi.tool(description="adds a suffix")
def adder(value: str):
    return {"result": value + "!"}
"#
                .to_vec())
            } else if source.display().contains("explode.py") {
                Ok(br#"import phi

@phi.tool(description="raises a python exception")
def explode(name: str):
    raise ValueError(f"bad name: {name}")
"#
                .to_vec())
            } else if source.display().contains("badreturn.py") {
                Ok(br#"import phi

@phi.tool(description="returns a value that cannot be json serialized")
def badreturn():
    return object()
"#
                .to_vec())
            } else {
                Ok(b"print('ok')".to_vec())
            }
        }

        fn entries(&self) -> Result<Vec<PhiHomeEntry>, Box<dyn std::error::Error>> {
            Ok(Vec::new())
        }

        fn url_for_path(&self, path: &PhiHomePath) -> PhiHomeUrl {
            PhiHomeUrl::new("mock", path.as_str())
        }
    }

    impl PhiPythonRuntime for MockBackend {
        fn runtime_info(&self) -> &PythonRuntimeInfo {
            static INFO: std::sync::OnceLock<PythonRuntimeInfo> = std::sync::OnceLock::new();
            INFO.get_or_init(|| PythonRuntimeInfo {
                backend: "mock".to_string(),
                version: "3.12.0".to_string(),
                implementation: "cpython".to_string(),
                library: None,
            })
        }

        fn load_plugin(&self, source: &PhiHomeUrl, code: &str) -> Result<LoadedPyPlugin, String> {
            if code.trim().is_empty() {
                return Err(format!("plugin '{}' is empty", source.display()));
            }

            Ok(LoadedPyPlugin {
                name: source.display(),
                backend: "mock".to_string(),
                source: Some(source.display()),
            })
        }

        fn run_code(&self, code: &str) -> Result<String, String> {
            Ok(format!("mock:{code}"))
        }

        fn list_tools(&self) -> Result<Vec<crate::executor::PhiToolDefinition>, String> {
            Ok(vec![crate::executor::PhiToolDefinition {
                name: "mock_tool".to_string(),
                description: "mock plugin tool".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "value": { "type": "string" }
                    }
                }),
            }])
        }

        fn call_tool(&self, name: &str, arguments: &serde_json::Value) -> Result<String, String> {
            Ok(format!("tool:{name}:{arguments}"))
        }
    }

    fn test_agent_runtime(home: Arc<MockHome>) -> crate::agent::PhiAgent {
        crate::agent::PhiAgent::builder(
            Session::empty(),
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        )
        .with_home(home)
        .with_client(crate::tests::support::stub_client(Vec::new()))
        .build()
        .expect("test agent should build")
    }

    #[test]
    fn init_context_ignores_failed_plugin_loads() {
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "step".to_string(),
                plugin_args: vec!["--plugin-mode".to_string()],
            },
            Arc::new(MockHome),
            Box::new(MockBackend),
            vec![
                PhiHomeUrl::file_for_test("/tmp/broken.py"),
                PhiHomeUrl::file_for_test("/tmp/ok.py"),
            ],
        );

        let mut context = PhiAgentBuildContext::new(
            Session::empty(),
            PhiAgentCommand::Step(
                PhiAgentCommand::step().with_plugin_args(vec!["--plugin-mode".to_string()]),
            ),
        );

        crate::module::PhiModule::init_context(&mut middleware, &mut context)
            .expect("plugin load failures should be warnings, not runtime errors");
        assert_eq!(middleware.loaded_plugins.len(), 1);
        assert!(middleware.loaded_plugins[0].name.contains("ok.py"));
    }

    #[test]
    fn init_context_tolerates_plugin_stdout_during_load() {
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "step".to_string(),
                plugin_args: vec![],
            },
            Arc::new(MockHome),
            Box::new(MockBackend),
            vec![PhiHomeUrl::file_for_test("/tmp/printy.py")],
        );

        let mut context = PhiAgentBuildContext::new(
            Session::empty(),
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        );

        crate::module::PhiModule::init_context(&mut middleware, &mut context)
            .expect("plugin stdout should be redirected away from the protocol stream");
        assert_eq!(middleware.loaded_plugins.len(), 1);
        assert!(middleware.loaded_plugins[0].name.contains("printy.py"));
    }

    #[test]
    fn middleware_routes_python_code_into_runtime() {
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "py".to_string(),
                plugin_args: vec![],
            },
            Arc::new(MockHome),
            Box::new(MockBackend),
            vec![],
        );

        let output = crate::module::PhiModule::run_python_code(&mut middleware, "print(1)")
            .expect("python code dispatch should succeed");
        assert_eq!(output.as_deref(), Some("mock:print(1)"));
    }

    #[test]
    fn python_code_shares_worker_context_with_loaded_plugins() {
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "step".to_string(),
                plugin_args: vec![],
            },
            Arc::new(MockHome),
            crate::features::plugin::python::load_backend()
                .expect("python backend should load in tests"),
            vec![PhiHomeUrl::file_for_test("/tmp/stateful.py")],
        );

        let mut context = PhiAgentBuildContext::new(
            Session::empty(),
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        );
        crate::module::PhiModule::init_context(&mut middleware, &mut context)
            .expect("plugin init should succeed");

        let output =
            crate::module::PhiModule::run_python_code(&mut middleware, "print(shared_answer + 1)")
                .expect("python code should execute");
        assert_eq!(output.as_deref(), Some("42\n"));
    }

    #[test]
    fn python_runtime_bootstraps_phi_sdk_module() {
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "py".to_string(),
                plugin_args: vec![],
            },
            Arc::new(MockHome),
            crate::features::plugin::python::load_backend()
                .expect("python backend should load in tests"),
            vec![],
        );

        let output = crate::module::PhiModule::run_python_code(
            &mut middleware,
            "import phi\nprint(phi.sdk_version())",
        )
        .expect("python code should execute");
        assert_eq!(output.as_deref(), Some("0.1.0\n"));
    }

    fn build_executor_with_module_tools(
        middleware: &mut PyPluginModule,
        context: &PhiAgentBuildContext,
    ) -> crate::error::PhiRuntimeResult<crate::executor::PhiExecutor> {
        crate::executor::PhiExecutor::from_tools(
            crate::executor::builtins::default_tools()
                .into_iter()
                .chain(crate::module::PhiModule::module_tools(middleware, context))
                .collect(),
        )
    }

    #[test]
    fn module_tools_register_plugin_tools() {
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "step".to_string(),
                plugin_args: vec![],
            },
            Arc::new(MockHome),
            Box::new(MockBackend),
            vec![],
        );
        let context = PhiAgentBuildContext::new(
            Session::empty(),
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        );
        let executor = build_executor_with_module_tools(&mut middleware, &context)
            .expect("plugin tool construction should succeed");

        assert!(executor.tool("mock_tool").is_some());
    }

    #[tokio::test]
    async fn subprocess_runtime_registers_and_executes_python_tools() {
        let home = Arc::new(MockHome);
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "step".to_string(),
                plugin_args: vec![],
            },
            home.clone(),
            crate::features::plugin::python::load_backend()
                .expect("python backend should load in tests"),
            vec![PhiHomeUrl::file_for_test("/tmp/tooly.py")],
        );
        let mut context = PhiAgentBuildContext::new(
            Session::empty(),
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        );
        crate::module::PhiModule::init_context(&mut middleware, &mut context)
            .expect("plugin init should succeed");
        let executor = build_executor_with_module_tools(&mut middleware, &context)
            .expect("plugin tool construction should succeed");

        let definitions = executor.definitions();
        let definition = definitions
            .iter()
            .find(|definition| definition.name == "adder")
            .expect("adder tool should be registered");
        assert_eq!(definition.description, "adds a suffix");
        assert_eq!(
            definition.parameters,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "additionalProperties": false,
                "required": ["value"]
            })
        );

        let agent = test_agent_runtime(home.clone());
        let (_request, response) = executor
            .call_tool(
                crate::executor::ToolCallRequest {
                    id: "call_1".to_string(),
                    call_id: Some("call_1".to_string()),
                    name: "adder".to_string(),
                    arguments: serde_json::json!({ "value": "hello" }),
                },
                crate::executor::ToolExecutionLimits::new(1_000, 1024, 256),
                agent.runtime(),
            )
            .await
            .expect("python tool should execute through executor");

        assert_eq!(response.name, "adder");
        assert!(response.output.is_ok());
        assert_eq!(response.output.error(), None);
        assert_eq!(
            *response.output.as_value(),
            serde_json::json!({
                "result": "hello!"
            })
        );
    }

    #[tokio::test]
    async fn subprocess_runtime_returns_structured_tool_error_for_python_exception() {
        let home = Arc::new(MockHome);
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "step".to_string(),
                plugin_args: vec![],
            },
            home.clone(),
            crate::features::plugin::python::load_backend()
                .expect("python backend should load in tests"),
            vec![PhiHomeUrl::file_for_test("/tmp/explode.py")],
        );
        let mut context = PhiAgentBuildContext::new(
            Session::empty(),
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        );
        crate::module::PhiModule::init_context(&mut middleware, &mut context)
            .expect("plugin init should succeed");
        let executor = build_executor_with_module_tools(&mut middleware, &context)
            .expect("plugin tool construction should succeed");

        let agent = test_agent_runtime(home.clone());
        let (_request, response) = executor
            .call_tool(
                crate::executor::ToolCallRequest {
                    id: "call_2".to_string(),
                    call_id: Some("call_2".to_string()),
                    name: "explode".to_string(),
                    arguments: serde_json::json!({ "name": "alice" }),
                },
                crate::executor::ToolExecutionLimits::new(1_000, 1024, 256),
                agent.runtime(),
            )
            .await
            .expect("python exception should still return a tool result");

        assert!(!response.output.is_ok());
        assert_eq!(response.output.error(), Some("bad name: alice"));
        let payload = response.output.as_value();
        assert_eq!(payload["type"], serde_json::json!("ValueError"));
        assert_eq!(payload["message"], serde_json::json!("bad name: alice"));
        assert!(
            payload.get("traceback").is_none(),
            "tool errors should not leak full traceback strings into model-visible payloads"
        );
        assert_eq!(
            payload["location"]["source"],
            serde_json::json!("explode.py")
        );
        assert_eq!(
            payload["location"]["function"],
            serde_json::json!("explode")
        );
        assert_eq!(payload["location"]["line"], serde_json::json!(5));
    }

    #[tokio::test]
    async fn subprocess_runtime_returns_structured_tool_error_for_non_serializable_output() {
        let home = Arc::new(MockHome);
        let mut middleware = PyPluginModule::new(
            PyPluginDescriptor {
                command_kind: "step".to_string(),
                plugin_args: vec![],
            },
            home.clone(),
            crate::features::plugin::python::load_backend()
                .expect("python backend should load in tests"),
            vec![PhiHomeUrl::file_for_test("/tmp/badreturn.py")],
        );
        let mut context = PhiAgentBuildContext::new(
            Session::empty(),
            PhiAgentCommand::Step(PhiAgentCommand::step()),
        );
        crate::module::PhiModule::init_context(&mut middleware, &mut context)
            .expect("plugin init should succeed");
        let executor = build_executor_with_module_tools(&mut middleware, &context)
            .expect("plugin tool construction should succeed");

        let agent = test_agent_runtime(home.clone());
        let (_request, response) = executor
            .call_tool(
                crate::executor::ToolCallRequest {
                    id: "call_3".to_string(),
                    call_id: Some("call_3".to_string()),
                    name: "badreturn".to_string(),
                    arguments: serde_json::json!({}),
                },
                crate::executor::ToolExecutionLimits::new(1_000, 1024, 256),
                agent.runtime(),
            )
            .await
            .expect("non-serializable python output should still return a tool result");

        assert!(!response.output.is_ok());
        assert!(
            response.output.error().is_some_and(
                |error| error.contains("Object of type object is not JSON serializable")
            )
        );
        let payload = response.output.as_value();
        assert_eq!(payload["type"], serde_json::json!("TypeError"));
        assert!(payload["message"].as_str().is_some_and(|message| {
            message.contains("Object of type object is not JSON serializable")
        }));
        assert!(
            payload.get("traceback").is_none(),
            "tool errors should not leak full traceback strings into model-visible payloads"
        );
    }
}
