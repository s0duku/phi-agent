use tokio::sync::mpsc::UnboundedSender;

use crate::{
    error::PhiRuntimeResult,
    message::PhiMessage,
    module::{PhiAgentCommitEvent, PhiAgentStepEvent, PhiModule},
};

pub struct ChannelModule {
    sender: UnboundedSender<PhiMessage>,
}

impl ChannelModule {
    pub fn new(sender: UnboundedSender<PhiMessage>) -> Self {
        Self { sender }
    }
}

impl PhiModule for ChannelModule {
    type ProbInfo = ();

    fn handle(&mut self, event: &mut PhiAgentStepEvent<'_>) -> PhiRuntimeResult<()> {
        let _ = event;
        Ok(())
    }

    fn observe(&mut self, event: &PhiAgentCommitEvent<'_>) {
        if let PhiAgentCommitEvent::MessageCommitted { message, .. } = event {
            let _ = self.sender.send((*message).clone());
        }
    }
}
