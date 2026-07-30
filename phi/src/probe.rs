use serde::Serialize;

use crate::{
    module::PhiModuleProbeJson,
    session::{PhiAgentStep, Session},
};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PhiStepProbe {
    pub kind: &'static str,
    pub detail: String,
    pub is_terminal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PhiSessionProbe {
    pub step: PhiStepProbe,
    pub history_messages: usize,
    pub modules: Vec<PhiModuleProbeJson>,
}

pub fn probe_session(session: &Session, modules: Vec<PhiModuleProbeJson>) -> PhiSessionProbe {
    PhiSessionProbe {
        step: PhiStepProbe {
            kind: step_kind(session.step()),
            detail: session.step().detail().to_string(),
            is_terminal: session.step().is_terminal(),
        },
        history_messages: session.history().len(),
        modules,
    }
}

fn step_kind(step: &PhiAgentStep) -> &'static str {
    match step {
        PhiAgentStep::RequestCompact => "request_compact",
        PhiAgentStep::RequestProvider { .. } => "request_provider",
        PhiAgentStep::RequestExecutor { .. } => "request_executor",
        PhiAgentStep::Compacted => "compacted",
        PhiAgentStep::TurnEnd { .. } => "turn_end",
        PhiAgentStep::Failed { .. } => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{message::PhiMessage, session::PhiAgentStep, tests::support::test_model_defaults};

    #[test]
    fn probe_report_keeps_module_probe_json_namespaced() {
        let session = Session::from_root(
            PhiAgentStep::request_provider("ready", &test_model_defaults()),
            vec![PhiMessage::user("hello")],
        );
        let probe = probe_session(
            &session,
            vec![PhiModuleProbeJson {
                name: "custom",
                info: serde_json::json!({ "answer": 42 }),
            }],
        );

        assert_eq!(probe.step.kind, "request_provider");
        assert_eq!(probe.history_messages, 1);
        assert_eq!(probe.modules[0].name, "custom");
        assert_eq!(probe.modules[0].info["answer"], 42);
    }
}
