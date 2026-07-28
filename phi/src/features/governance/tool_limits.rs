use std::collections::BTreeMap;

use crate::{
    error::PhiRuntimeResult,
    executor::ToolExecutionLimits,
    module::{PhiAgentStepEvent, PhiModule},
};

pub struct ToolRuntimePolicy {
    default_limits: Option<ToolExecutionLimits>,
    tool_limits: BTreeMap<String, ToolExecutionLimits>,
}

impl ToolRuntimePolicy {
    pub fn builder() -> ToolRuntimePolicyBuilder {
        ToolRuntimePolicyBuilder::default()
    }
}

impl PhiModule for ToolRuntimePolicy {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        let PhiAgentStepEvent::BeforeToolCall {
            request, limits, ..
        } = event
        else {
            return Ok(());
        };

        if let Some(configured_limits) = self
            .tool_limits
            .get(&request.name)
            .copied()
            .or(self.default_limits)
        {
            **limits = configured_limits;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct ToolRuntimePolicyBuilder {
    default_limits: Option<ToolExecutionLimits>,
    tool_limits: BTreeMap<String, ToolExecutionLimits>,
}

impl ToolRuntimePolicyBuilder {
    pub fn default_limits(mut self, limits: ToolExecutionLimits) -> Self {
        self.default_limits = Some(limits);
        self
    }

    #[allow(dead_code)]
    pub fn tool_limits(
        mut self,
        tool_name: impl Into<String>,
        limits: ToolExecutionLimits,
    ) -> Self {
        self.tool_limits.insert(tool_name.into(), limits);
        self
    }

    pub fn build(self) -> ToolRuntimePolicy {
        ToolRuntimePolicy {
            default_limits: self.default_limits,
            tool_limits: self.tool_limits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{executor::ToolCallRequest, session::Session};

    #[test]
    fn tool_runtime_attaches_limits() {
        let mut policy = ToolRuntimePolicy::builder()
            .tool_limits("bash_job", ToolExecutionLimits::new(10_000, 12_000, 1_000))
            .build();
        let session = Session::empty();
        let request = ToolCallRequest {
            id: "id".to_string(),
            call_id: None,
            name: "bash_job".to_string(),
            arguments: serde_json::json!({}),
        };

        let mut limits = ToolExecutionLimits::new(1_000, 2_000, 500);
        let mut request = request;
        let expr = session.clone().into_expr();
        let mut event = crate::module::PhiAgentStepEvent::BeforeToolCall {
            step: session.step(),
            expr: &expr,
            request: &mut request,
            limits: &mut limits,
        };

        policy.handle(&mut event).unwrap();

        assert_eq!(limits, ToolExecutionLimits::new(10_000, 12_000, 1_000));
    }
}
