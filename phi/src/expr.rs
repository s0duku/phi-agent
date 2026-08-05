use std::{collections::BTreeMap, marker::PhantomData, sync::Arc};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

#[cfg(test)]
use crate::error::PhiAgentRuntimeError;
use crate::{
    message::{PhiHistory, PhiMessage},
    session::PhiAgentStep,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PhiStepExpr {
    #[serde(default = "crate::session::serde_default_request_provider_step")]
    step: PhiAgentStep,
    #[serde(default, skip_serializing_if = "PhiExprDelta::is_empty")]
    delta: PhiExprDelta,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expr: Option<Arc<PhiStepExpr>>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct PhiExprDelta {
    #[serde(default, skip_serializing_if = "PhiHistory::is_empty")]
    history: PhiHistory,
    #[serde(default, skip_serializing_if = "PhiVariableEffects::is_empty")]
    effects: PhiVariableEffects,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(transparent)]
struct PhiVariableEffects {
    effects: BTreeMap<String, PhiVariableEffect>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum PhiVariableEffect {
    Store { value: Value },
    Remove,
}

/// A serialized variable name coupled to the Rust type stored under that name.
pub(crate) struct PhiVariable<T> {
    name: &'static str,
    value: PhantomData<fn() -> T>,
}

impl<T> PhiVariable<T> {
    pub(crate) const fn new(name: &'static str) -> Self {
        Self {
            name,
            value: PhantomData,
        }
    }
}

impl<T> Clone for PhiVariable<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for PhiVariable<T> {}

impl PhiExprDelta {
    pub(crate) fn is_empty(&self) -> bool {
        self.history.is_empty() && self.effects.is_empty()
    }

    pub(crate) fn history(&self) -> &PhiHistory {
        &self.history
    }

    pub(crate) fn push_message(&mut self, message: PhiMessage) {
        self.history.push(message);
    }

    /// Composes deltas in evaluation order; effects in `next` take precedence.
    pub(crate) fn then(mut self, next: Self) -> Self {
        for message in next.history.into_messages() {
            self.history.push(message);
        }
        self.effects.then(next.effects);
        self
    }

    pub(crate) fn store<T>(&mut self, variable: PhiVariable<T>, value: T)
    where
        T: Serialize,
    {
        self.effects.store(variable.name, value);
    }

    pub(crate) fn remove<T>(&mut self, variable: PhiVariable<T>) {
        self.effects.remove(variable.name);
    }

    fn lookup_impl(&self, name: &str) -> EffectLookup<'_> {
        self.effects.lookup_impl(name)
    }

    pub(crate) fn affects<T>(&self, variable: PhiVariable<T>) -> bool {
        self.effects.affects(variable.name)
    }
}

impl From<PhiHistory> for PhiExprDelta {
    fn from(history: PhiHistory) -> Self {
        Self {
            history,
            effects: PhiVariableEffects::default(),
        }
    }
}

impl From<Vec<PhiMessage>> for PhiExprDelta {
    fn from(messages: Vec<PhiMessage>) -> Self {
        Self::from(PhiHistory::from_messages(messages))
    }
}

impl PhiVariableEffects {
    fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    fn then(&mut self, next: Self) {
        self.effects.extend(next.effects);
    }

    fn store<T>(&mut self, name: &str, value: T)
    where
        T: Serialize,
    {
        let value = serde_json::to_value(value).expect("stored value should serialize");
        self.effects
            .insert(name.to_string(), PhiVariableEffect::Store { value });
    }

    fn remove(&mut self, name: &str) {
        self.effects
            .insert(name.to_string(), PhiVariableEffect::Remove);
    }

    fn lookup_impl(&self, name: &str) -> EffectLookup<'_> {
        match self.effects.get(name) {
            Some(PhiVariableEffect::Store { value }) => EffectLookup::Stored(value),
            Some(PhiVariableEffect::Remove) => EffectLookup::Removed,
            None => EffectLookup::Missing,
        }
    }

    fn affects(&self, name: &str) -> bool {
        self.effects.contains_key(name)
    }
}

enum EffectLookup<'a> {
    Missing,
    Removed,
    Stored(&'a Value),
}

impl PhiStepExpr {
    pub(crate) fn new<H>(step: PhiAgentStep, delta: H) -> Self
    where
        H: Into<PhiExprDelta>,
    {
        Self {
            step,
            delta: delta.into(),
            expr: None,
        }
    }

    pub(crate) fn branch<H>(expr: Self, step: PhiAgentStep, delta: H) -> Self
    where
        H: Into<PhiExprDelta>,
    {
        Self {
            step,
            delta: delta.into(),
            expr: Some(Arc::new(expr)),
        }
    }

    /// Applies the frame transformation used by the CreateNextStep bounce.
    pub(crate) fn create_next_step(self, step: PhiAgentStep, delta: PhiExprDelta) -> Self {
        Self::branch(self, step, delta)
    }

    /// Applies the frame transformation used by the ReplaceBaseStep bounce.
    pub(crate) fn replace_base_step(self, step: PhiAgentStep, current_delta: PhiExprDelta) -> Self {
        let delta = self.delta.clone().then(current_delta);
        self.replace_base_step_with_delta(step, delta)
    }

    pub(crate) fn replace_base_step_with_delta(
        self,
        step: PhiAgentStep,
        delta: PhiExprDelta,
    ) -> Self {
        let parent = self.expr.clone();
        match parent {
            Some(parent) => Self {
                step,
                delta,
                expr: Some(parent),
            },
            None => Self::new(step, delta),
        }
    }

    pub(crate) fn step(&self) -> &PhiAgentStep {
        &self.step
    }

    pub(crate) fn expr(&self) -> Option<&PhiStepExpr> {
        self.expr.as_deref()
    }

    #[allow(dead_code)]
    pub(crate) fn into_expr(self) -> Option<Arc<PhiStepExpr>> {
        self.expr
    }

    pub(crate) fn delta(&self) -> &PhiExprDelta {
        &self.delta
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn store<T>(mut self, variable: PhiVariable<T>, value: T) -> Self
    where
        T: Serialize,
    {
        self.delta.store(variable, value);
        self
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn remove<T>(mut self, variable: PhiVariable<T>) -> Self {
        self.delta.remove(variable);
        self
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn commit<H>(self, step: PhiAgentStep, delta: H) -> Self
    where
        H: Into<PhiExprDelta>,
    {
        Self::branch(self, step, delta)
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn branch_failed(self, error: PhiAgentRuntimeError) -> Self {
        Self::branch(self, PhiAgentStep::failed(error), PhiExprDelta::default())
    }

    pub(crate) fn find_ancestor(
        &self,
        predicate: impl Fn(&PhiAgentStep) -> bool,
    ) -> Option<&PhiStepExpr> {
        let mut current = self.expr();
        while let Some(expr) = current {
            if predicate(expr.step()) {
                return Some(expr);
            }
            current = expr.expr();
        }
        None
    }

    pub(crate) fn is_history_barrier(&self) -> bool {
        matches!(
            self.step,
            PhiAgentStep::ReAct(crate::session::PhiReActStep::Compacted)
        )
    }

    pub(crate) fn lookup<T>(&self, variable: PhiVariable<T>) -> Option<T>
    where
        T: DeserializeOwned,
    {
        match self.delta.lookup_impl(variable.name) {
            EffectLookup::Stored(value) => serde_json::from_value(value.clone()).ok(),
            EffectLookup::Removed => None,
            EffectLookup::Missing => self.expr.as_deref().and_then(|expr| expr.lookup(variable)),
        }
    }

    pub(crate) fn history(&self) -> PhiHistory {
        let mut messages = if self.is_history_barrier() {
            Vec::new()
        } else if let Some(expr) = &self.expr {
            expr.history().into_messages()
        } else {
            Vec::new()
        };
        messages.extend(self.delta.history().clone().into_messages());
        PhiHistory::from_messages(messages)
    }

    pub(crate) fn into_history(self) -> PhiHistory {
        let Self { step, delta, expr } = self;
        let mut messages = if matches!(
            step,
            PhiAgentStep::ReAct(crate::session::PhiReActStep::Compacted)
        ) {
            Vec::new()
        } else if let Some(expr) = expr {
            match Arc::try_unwrap(expr) {
                Ok(expr) => expr.into_history(),
                Err(expr) => expr.history(),
            }
            .into_messages()
        } else {
            Vec::new()
        };
        messages.extend(delta.history.into_messages());
        PhiHistory::from_messages(messages)
    }
}

impl PhiStepExpr {
    pub(crate) fn empty_root() -> Self {
        Self::new(
            crate::session::serde_default_request_provider_step(),
            PhiHistory::default(),
        )
    }
}

#[cfg(test)]
mod step_expr_tests {
    use super::*;
    use crate::message::PhiMessage;
    use crate::session::serde_default_request_provider_step;

    const FIRST: PhiVariable<i32> = PhiVariable::new("first");
    const SECOND: PhiVariable<i32> = PhiVariable::new("second");
    const ANSWER: PhiVariable<String> = PhiVariable::new("answer");
    const NAME: PhiVariable<String> = PhiVariable::new("name");
    const COUNT: PhiVariable<i32> = PhiVariable::new("count");
    const MISSING: PhiVariable<bool> = PhiVariable::new("missing");
    const RETRY_STATE: PhiVariable<i32> = PhiVariable::new("retry_state");
    const RETRY_STATE_USIZE: PhiVariable<usize> = PhiVariable::new("retry_state");
    const KEPT: PhiVariable<i32> = PhiVariable::new("kept");
    const OVERRIDDEN: PhiVariable<String> = PhiVariable::new("overridden");

    #[test]
    fn clone_shares_parent_expr() {
        let expr = PhiStepExpr::branch(
            PhiStepExpr::empty_root(),
            serde_default_request_provider_step(),
            PhiExprDelta::default(),
        );

        let cloned = expr.clone();

        assert!(Arc::ptr_eq(
            expr.expr.as_ref().expect("parent expr should exist"),
            cloned.expr.as_ref().expect("cloned parent should exist"),
        ));
    }

    #[test]
    fn store_records_effects_in_the_current_delta() {
        let expr = PhiStepExpr::empty_root().store(FIRST, 1).store(SECOND, 2);

        assert_eq!(expr.delta().effects.effects.len(), 2);
        assert_eq!(expr.lookup(FIRST), Some(1));
        assert_eq!(expr.lookup(SECOND), Some(2));
    }

    #[test]
    fn store_only_affects_the_current_delta() {
        let base = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: PhiExprDelta::from(vec![PhiMessage::user("earlier")]),
            expr: None,
        };
        let mut expr = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: PhiExprDelta::default(),
            expr: Some(Arc::new(base)),
        };

        expr = expr.store(ANSWER, String::from("world"));

        assert_eq!(
            expr.expr()
                .expect("base expr should exist")
                .delta()
                .effects
                .effects
                .len(),
            0
        );
        assert_eq!(expr.delta().history(), &PhiHistory::default());
        assert_eq!(expr.lookup(ANSWER), Some(String::from("world")));
        assert_eq!(
            expr.history(),
            PhiHistory::from_messages(vec![PhiMessage::user("earlier")])
        );
    }

    #[test]
    fn lookup_prefers_current_frame_effect_before_parent_expr() {
        let base = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: {
                let mut delta = PhiExprDelta::default();
                delta.store(NAME, String::from("base"));
                delta.store(COUNT, 1);
                delta
            },
            expr: None,
        };
        let mut expr = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: PhiExprDelta::default(),
            expr: Some(Arc::new(base)),
        };

        expr = expr.store(NAME, String::from("current"));

        assert_eq!(expr.lookup(NAME), Some(String::from("current")));
        assert_eq!(expr.lookup(COUNT), Some(1));
        assert_eq!(expr.lookup(MISSING), None);
    }

    #[test]
    fn remove_blocks_parent_lookup() {
        let base = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: {
                let mut delta = PhiExprDelta::default();
                delta.store(RETRY_STATE, 3);
                delta
            },
            expr: None,
        };
        let mut expr = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: PhiExprDelta::default(),
            expr: Some(Arc::new(base)),
        };

        expr = expr.remove(RETRY_STATE);

        assert_eq!(expr.lookup(RETRY_STATE), None);
    }

    #[test]
    fn incompatible_current_store_effect_does_not_fall_back_to_parent() {
        let base = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: {
                let mut delta = PhiExprDelta::default();
                delta.store(COUNT, 7);
                delta
            },
            expr: None,
        };
        let mut expr = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: PhiExprDelta::default(),
            expr: Some(Arc::new(base)),
        };

        expr.delta.effects.store("count", "wrong-type");

        assert_eq!(expr.lookup(COUNT), None);
    }

    #[test]
    fn store_round_trips_and_remove_hides_parent_value() {
        let expr = PhiStepExpr::empty_root().store(RETRY_STATE_USIZE, 2);
        assert_eq!(expr.lookup(RETRY_STATE_USIZE), Some(2));

        let expr = PhiStepExpr::branch(
            expr,
            serde_default_request_provider_step(),
            PhiExprDelta::default(),
        )
        .remove(RETRY_STATE_USIZE);
        assert_eq!(expr.lookup(RETRY_STATE_USIZE), None);
    }

    #[test]
    fn delta_then_appends_history_and_applies_later_variable_effects() {
        let mut base = PhiExprDelta::from(vec![PhiMessage::user("base")]);
        base.store(KEPT, 1);
        base.store(OVERRIDDEN, String::from("base"));

        let mut current = PhiExprDelta::from(vec![PhiMessage::assistant("current")]);
        current.store(OVERRIDDEN, String::from("current"));
        current.remove(KEPT);
        base = base.then(current);

        assert_eq!(
            base.history(),
            &PhiHistory::from_messages(vec![
                PhiMessage::user("base"),
                PhiMessage::assistant("current"),
            ])
        );
        assert!(matches!(base.lookup_impl("kept"), EffectLookup::Removed));
        assert!(matches!(
            base.lookup_impl("overridden"),
            EffectLookup::Stored(value) if value == "current"
        ));
    }
}
