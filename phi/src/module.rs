use crate::{
    agent::{
        PhiAgentBuildContext, PhiAgentRuntime, StepCont, StepInterveneNext, StepInterveneResult,
    },
    error::{PhiAgentRuntimeError, PhiAgentRuntimeResult},
    executor::{PhiTool, ToolCallRequest, ToolCallResponse},
    expr::{PhiExprDelta, PhiStepExpr},
    message::{PhiHistory, PhiMessage},
    render::PhiProviderCall,
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PhiModuleProbeJson {
    pub name: &'static str,
    pub info: serde_json::Value,
}

// A step is the agent's atomic state transition. Runtime and modules transform the
// runtime-owned PhiStepExpr; they do not construct or edit Session values. Session is the
// external ownership/serialization boundary consumed by agent construction and produced by
// agent checkpoints or output.
//
// A module participates in two different ways:
// - intervene(runtime, cont, next): runs inside eval_step itself and may
//   either forward to `next`, rewrite the runtime-owned step state before
//   forwarding, or finish the
//   current step immediately with its own StepBounce. Because this is part of
//   the interpreter proper, intervene shares the same recoverable runtime-error
//   channel as other step evaluation logic.
// - handle(event): runs during a step and may only affect the specific event
//   payload/control exposed by that event so that candidate payloads can still
//   be rewritten before the step commits session history.
#[allow(dead_code)]
#[derive(Debug)]
pub(crate) enum PhiAgentStepEvent<'a> {
    BeforeCompactRequest {
        step: &'a crate::session::PhiAgentStep,
        expr: &'a PhiStepExpr,
        history: &'a mut PhiHistory,
        request: &'a mut PhiProviderCall,
    },
    AfterCompactResponse {
        history: &'a mut PhiHistory,
    },
    BeforeModelRequest {
        step: &'a crate::session::PhiAgentStep,
        expr: &'a PhiStepExpr,
        history: &'a mut PhiHistory,
        request: &'a mut PhiProviderCall,
    },
    AfterModelResponse {
        message: &'a mut PhiMessage,
    },
    AfterModelResponseParsed {
        messages: &'a [PhiMessage],
    },
    BeforeToolCall {
        step: &'a crate::session::PhiAgentStep,
        expr: &'a PhiStepExpr,
        request: &'a mut ToolCallRequest,
    },
    BeforeCreateNextStep {
        base_expr: &'a PhiStepExpr,
        step: &'a mut crate::session::PhiReActStep,
        delta: &'a mut PhiExprDelta,
    },
    BeforeReplaceBaseStep {
        base_expr: &'a PhiStepExpr,
        step: &'a mut crate::session::PhiReActStep,
        delta: &'a mut PhiExprDelta,
    },
    AfterToolCall {
        request: &'a ToolCallRequest,
        result: &'a mut ToolCallResponse,
    },
}

#[derive(Debug)]
pub(crate) enum PhiAgentCommitEvent<'a> {
    ModelResponseCommitted { message: &'a PhiMessage },
    MessageCommitted { message: &'a PhiMessage },
    WarningEmitted { message: &'a str },
    StepFailed { error: &'a PhiAgentRuntimeError },
}

pub(crate) trait PhiModule: Send + Sync {
    type ProbInfo: Serialize;

    fn probe_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn probe(&self, _runtime: &PhiAgentRuntime) -> Option<Self::ProbInfo> {
        None
    }

    fn init_context(&mut self, _context: &mut PhiAgentBuildContext) -> PhiAgentRuntimeResult<()> {
        Ok(())
    }

    fn module_tools(&mut self, _context: &PhiAgentBuildContext) -> Vec<Arc<dyn PhiTool>> {
        Vec::new()
    }

    fn intervene(
        &mut self,
        runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> StepInterveneResult {
        next.call(runtime, cont)
    }

    // Event handling is intentionally payload-scoped: modules should shape
    // the current atomic transition through the event's mutable payload, rather
    // than arbitrarily rewriting unrelated session fields here.
    //
    // Design goal: "prepare first, commit last" inside a step so history
    // updates stay coherent even when modules rewrites or rejects candidate
    // payloads.
    fn handle(&mut self, _event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        Ok(())
    }

    // Commit observation is intentionally infallible: once a step publishes
    // session history, observers may react to what happened but must not pull
    // the step back onto an error path and leave behind a partially committed
    // session.
    fn observe(&mut self, _event: &PhiAgentCommitEvent<'_>) {}

    // Some modules own runtime-specific execution environments that sit
    // outside the agent step loop but still belong to the same initialized
    // lifecycle. Returning Some(output) means the module handled the code
    // execution; returning None means "not supported here".
    fn run_python_code(
        &mut self,
        _code: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        Ok(None)
    }
}

pub(crate) trait DynPhiModule: Send + Sync {
    // Probe is observational and is not part of runtime step evaluation.
    // PhiAgentRuntimeResult is reserved for errors that may become a Failed
    // agent step; probe info therefore serializes directly at this boundary.
    fn probe_json(&self, runtime: &PhiAgentRuntime) -> Option<PhiModuleProbeJson>;

    fn init_context(&mut self, context: &mut PhiAgentBuildContext) -> PhiAgentRuntimeResult<()>;

    fn module_tools(&mut self, context: &PhiAgentBuildContext) -> Vec<Arc<dyn PhiTool>>;

    fn intervene(
        &mut self,
        runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> StepInterveneResult;

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()>;

    fn observe(&mut self, event: &PhiAgentCommitEvent<'_>);

    fn run_python_code(&mut self, code: &str)
    -> Result<Option<String>, Box<dyn std::error::Error>>;
}

impl<T> DynPhiModule for T
where
    T: PhiModule,
{
    fn probe_json(&self, runtime: &PhiAgentRuntime) -> Option<PhiModuleProbeJson> {
        self.probe(runtime).map(|info| PhiModuleProbeJson {
            name: self.probe_name(),
            info: serde_json::to_value(info).expect("module probe info should serialize to JSON"),
        })
    }

    fn init_context(&mut self, context: &mut PhiAgentBuildContext) -> PhiAgentRuntimeResult<()> {
        PhiModule::init_context(self, context)
    }

    fn module_tools(&mut self, context: &PhiAgentBuildContext) -> Vec<Arc<dyn PhiTool>> {
        PhiModule::module_tools(self, context)
    }

    fn intervene(
        &mut self,
        runtime: PhiAgentRuntime,
        cont: StepCont,
        next: StepInterveneNext,
    ) -> StepInterveneResult {
        PhiModule::intervene(self, runtime, cont, next)
    }

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiAgentRuntimeResult<()> {
        PhiModule::handle(self, event)
    }

    fn observe(&mut self, event: &PhiAgentCommitEvent<'_>) {
        PhiModule::observe(self, event)
    }

    fn run_python_code(
        &mut self,
        code: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        PhiModule::run_python_code(self, code)
    }
}

#[derive(Default)]
pub(crate) struct PhiModuleLayout {
    init: Vec<Box<dyn DynPhiModule>>,
    governance: Vec<Box<dyn DynPhiModule>>,
    observer: Vec<Box<dyn DynPhiModule>>,
    extension: Vec<Box<dyn DynPhiModule>>,
    recovery: Vec<Box<dyn DynPhiModule>>,
}

impl PhiModuleLayout {
    pub(crate) fn push_init(&mut self, module: Box<dyn DynPhiModule>) {
        self.init.push(module);
    }

    pub(crate) fn push_governance(&mut self, module: Box<dyn DynPhiModule>) {
        self.governance.push(module);
    }

    pub(crate) fn push_observer(&mut self, module: Box<dyn DynPhiModule>) {
        self.observer.push(module);
    }

    pub(crate) fn push_extension(&mut self, module: Box<dyn DynPhiModule>) {
        self.extension.push(module);
    }

    pub(crate) fn push_recovery(&mut self, module: Box<dyn DynPhiModule>) {
        self.recovery.push(module);
    }

    pub(crate) fn extend(&mut self, mut other: Self) {
        self.init.append(&mut other.init);
        self.governance.append(&mut other.governance);
        self.observer.append(&mut other.observer);
        self.extension.append(&mut other.extension);
        self.recovery.append(&mut other.recovery);
    }

    pub(crate) fn into_modules(self) -> Vec<Box<dyn DynPhiModule>> {
        // This is the single flattening point for module ordering. Build code
        // cannot accidentally interleave segments once modules have been placed
        // into their semantic bucket.
        let Self {
            init,
            governance,
            observer,
            extension,
            recovery,
        } = self;
        let mut modules = Vec::with_capacity(
            init.len() + governance.len() + observer.len() + extension.len() + recovery.len(),
        );
        modules.extend(init);
        modules.extend(governance);
        modules.extend(observer);
        modules.extend(extension);
        modules.extend(recovery);
        modules
    }
}

#[derive(Default)]
pub(crate) struct PhiModuleChain {
    modules: Vec<Box<dyn DynPhiModule>>,
}

impl PhiModuleChain {
    pub(crate) fn new(modules: Vec<Box<dyn DynPhiModule>>) -> Self {
        Self { modules }
    }

    pub(crate) fn len(&self) -> usize {
        self.modules.len()
    }

    pub(crate) fn take_module(&mut self, index: usize) -> Box<dyn DynPhiModule> {
        std::mem::replace(&mut self.modules[index], Box::new(NoopModule))
    }

    pub(crate) fn restore_module(&mut self, index: usize, module: Box<dyn DynPhiModule>) {
        self.modules[index] = module;
    }

    pub(crate) fn probe_json(&self, runtime: &PhiAgentRuntime) -> Vec<PhiModuleProbeJson> {
        let mut output = Vec::new();
        for module in &self.modules {
            if let Some(probe) = module.probe_json(runtime) {
                output.push(probe);
            }
        }
        output
    }

    pub(crate) fn handle(
        &mut self,
        event: &mut PhiAgentStepEvent<'_>,
    ) -> PhiAgentRuntimeResult<()> {
        for module in &mut self.modules {
            module.handle(event)?;
        }
        Ok(())
    }

    pub(crate) fn observe(&mut self, event: &PhiAgentCommitEvent<'_>) {
        for module in &mut self.modules {
            module.observe(event);
        }
    }

    pub(crate) fn run_python_code(
        &mut self,
        code: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        for module in &mut self.modules {
            if let Some(output) = module.run_python_code(code)? {
                return Ok(Some(output));
            }
        }
        Ok(None)
    }
}

pub(crate) fn init_context_modules(
    modules: &mut [Box<dyn DynPhiModule>],
    context: &mut PhiAgentBuildContext,
) -> PhiAgentRuntimeResult<()> {
    for module in modules {
        module.init_context(context)?;
    }
    Ok(())
}

pub(crate) fn module_tools(
    modules: &mut [Box<dyn DynPhiModule>],
    context: &PhiAgentBuildContext,
    tools: &mut Vec<Arc<dyn PhiTool>>,
) {
    for module in modules {
        tools.extend(module.module_tools(context));
    }
}

pub(crate) struct NoopModule;

impl PhiModule for NoopModule {
    type ProbInfo = ();
}
