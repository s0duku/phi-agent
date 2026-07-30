use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

#[cfg(test)]
use crate::error::PhiRuntimeError;
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
    #[serde(default, skip_serializing_if = "PhiStoreDelta::is_empty")]
    store: PhiStoreDelta,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
pub(crate) struct PhiStoreDelta {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    bindings: BTreeMap<String, PhiStoreBinding>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PhiStoreBinding {
    Set { value: Value },
    Unset,
}

pub(crate) trait PhiStoreKey {
    type Value: Serialize + DeserializeOwned;
    const NAME: &'static str;
}

impl PhiExprDelta {
    pub(crate) fn is_empty(&self) -> bool {
        self.history.is_empty() && self.store.is_empty()
    }

    pub(crate) fn history(&self) -> &PhiHistory {
        &self.history
    }

    pub(crate) fn push_message(&mut self, message: PhiMessage) {
        self.history.push(message);
    }

    pub(crate) fn bind<T>(&mut self, name: &str, value: T)
    where
        T: Serialize,
    {
        self.store.bind(name, value);
    }

    pub(crate) fn unbind(&mut self, name: &str) {
        self.store.unbind(name);
    }

    pub(crate) fn lookup<T>(&self, name: &str) -> DeltaLookup<T>
    where
        T: DeserializeOwned,
    {
        self.store.lookup(name)
    }
}

impl From<PhiHistory> for PhiExprDelta {
    fn from(history: PhiHistory) -> Self {
        Self {
            history,
            store: PhiStoreDelta::default(),
        }
    }
}

impl From<Vec<PhiMessage>> for PhiExprDelta {
    fn from(messages: Vec<PhiMessage>) -> Self {
        Self::from(PhiHistory::from_messages(messages))
    }
}

impl PhiStoreDelta {
    pub(crate) fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub(crate) fn bind<T>(&mut self, name: &str, value: T)
    where
        T: Serialize,
    {
        let value = serde_json::to_value(value).expect("stored value should serialize");
        self.bindings
            .insert(name.to_string(), PhiStoreBinding::Set { value });
    }

    pub(crate) fn unbind(&mut self, name: &str) {
        self.bindings
            .insert(name.to_string(), PhiStoreBinding::Unset);
    }

    pub(crate) fn lookup<T>(&self, name: &str) -> DeltaLookup<T>
    where
        T: DeserializeOwned,
    {
        match self.bindings.get(name) {
            Some(PhiStoreBinding::Set { value }) => match serde_json::from_value(value.clone()) {
                Ok(value) => DeltaLookup::Value(value),
                Err(_) => DeltaLookup::Unset,
            },
            Some(PhiStoreBinding::Unset) => DeltaLookup::Unset,
            None => DeltaLookup::Missing,
        }
    }
}

pub(crate) enum DeltaLookup<T> {
    Missing,
    Unset,
    Value(T),
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
    pub(crate) fn commit<H>(self, step: PhiAgentStep, delta: H) -> Self
    where
        H: Into<PhiExprDelta>,
    {
        Self::branch(self, step, delta)
    }

    #[must_use]
    #[cfg(test)]
    pub(crate) fn branch_failed(self, error: PhiRuntimeError) -> Self {
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
        matches!(self.step, PhiAgentStep::Compacted)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_store<T>(mut self, name: &str, value: T) -> Self
    where
        T: Serialize,
    {
        self.delta.bind(name, value);
        self
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn without_store(mut self, name: &str) -> Self {
        self.delta.unbind(name);
        self
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn with_key<K>(self, value: K::Value) -> Self
    where
        K: PhiStoreKey,
    {
        self.with_store(K::NAME, value)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn without_key<K>(self) -> Self
    where
        K: PhiStoreKey,
    {
        self.without_store(K::NAME)
    }

    pub(crate) fn lookup<T>(&self, name: &str) -> Option<T>
    where
        T: DeserializeOwned,
    {
        match self.delta.lookup(name) {
            DeltaLookup::Value(value) => Some(value),
            DeltaLookup::Unset => None,
            DeltaLookup::Missing => self.expr.as_deref().and_then(|expr| expr.lookup(name)),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn lookup_key<K>(&self) -> Option<K::Value>
    where
        K: PhiStoreKey,
    {
        self.lookup(K::NAME)
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
        let mut messages = if matches!(step, PhiAgentStep::Compacted) {
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
    fn store_writes_into_current_delta_store() {
        let expr = PhiStepExpr::empty_root()
            .with_store("first", 1)
            .with_store("second", 2);

        assert_eq!(expr.delta().store.bindings.len(), 2);
        assert_eq!(expr.lookup::<i32>("first"), Some(1));
        assert_eq!(expr.lookup::<i32>("second"), Some(2));
    }

    #[test]
    fn store_only_mutates_current_delta_store() {
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

        expr = expr.with_store("answer", "world");

        assert_eq!(
            expr.expr()
                .expect("base expr should exist")
                .delta()
                .store
                .bindings
                .len(),
            0
        );
        assert_eq!(expr.delta().history(), &PhiHistory::default());
        assert_eq!(expr.lookup::<String>("answer"), Some(String::from("world")));
        assert_eq!(
            expr.history(),
            PhiHistory::from_messages(vec![PhiMessage::user("earlier")])
        );
    }

    #[test]
    fn lookup_prefers_current_delta_before_parent_expr() {
        let base = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: {
                let mut delta = PhiExprDelta::default();
                delta.bind("name", "base");
                delta.bind("count", 1);
                delta
            },
            expr: None,
        };
        let mut expr = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: PhiExprDelta::default(),
            expr: Some(Arc::new(base)),
        };

        expr = expr.with_store("name", "current");

        assert_eq!(expr.lookup::<String>("name"), Some(String::from("current")));
        assert_eq!(expr.lookup::<i32>("count"), Some(1));
        assert_eq!(expr.lookup::<bool>("missing"), None);
    }

    #[test]
    fn unstore_blocks_parent_lookup() {
        let base = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: {
                let mut delta = PhiExprDelta::default();
                delta.bind("retry_state", 3);
                delta
            },
            expr: None,
        };
        let mut expr = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: PhiExprDelta::default(),
            expr: Some(Arc::new(base)),
        };

        expr = expr.without_store("retry_state");

        assert_eq!(expr.lookup::<i32>("retry_state"), None);
    }

    #[test]
    fn incompatible_current_store_binding_does_not_fall_back_to_parent() {
        let base = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: {
                let mut delta = PhiExprDelta::default();
                delta.bind("count", 7);
                delta
            },
            expr: None,
        };
        let mut expr = PhiStepExpr {
            step: serde_default_request_provider_step(),
            delta: PhiExprDelta::default(),
            expr: Some(Arc::new(base)),
        };

        expr = expr.with_store("count", "wrong-type");

        assert_eq!(expr.lookup::<i32>("count"), None);
    }

    #[test]
    fn typed_store_keys_round_trip_and_support_unbind() {
        struct RetryStateKey;

        impl PhiStoreKey for RetryStateKey {
            type Value = usize;
            const NAME: &'static str = "retry_state";
        }

        let expr = PhiStepExpr::empty_root().with_key::<RetryStateKey>(2);
        assert_eq!(expr.lookup_key::<RetryStateKey>(), Some(2));

        let expr = expr.without_key::<RetryStateKey>();
        assert_eq!(expr.lookup_key::<RetryStateKey>(), None);
    }
}
