//! Platform-independent bounded traversal and action semantics, run inside the
//! disposable worker. The backend owns native identity and containment checks.
use crate::ui_registry::{UiOwner, UiRegistry};
use pab_protocol::*;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(test)]
mod tests;

/// Internal pipe message only. Never accepted from a remote client: the helper
/// constructs this ticket from its own verified window registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiWindow {
    pub window_ref: String,
    pub id: u32,
    pub pid: u32,
    pub process_identity: String,
    pub marker_key: String,
    pub marker: u32,
}

pub trait UiBackend {
    type Element: Clone;
    fn root(&mut self, window: &UiWindow) -> Result<Self::Element, &'static str>;
    fn same(&self, a: &Self::Element, b: &Self::Element) -> bool;
    fn validate(
        &mut self,
        window: &UiWindow,
        root: &Self::Element,
        node: &Self::Element,
    ) -> Result<(), &'static str>;
    /// Returns at most max nodes, plus whether children were omitted. Native
    /// backends must enforce max before copying an unbounded provider array.
    fn children(
        &mut self,
        node: &Self::Element,
        max: usize,
    ) -> Result<(Vec<Self::Element>, bool), &'static str>;
    fn read(
        &mut self,
        node: &Self::Element,
        include_value: bool,
    ) -> Result<UiElement, &'static str>;
    fn act(&mut self, node: &Self::Element, action: &UiAction) -> Result<(), &'static str>;
}

#[derive(Clone)]
struct Target<E> {
    window: UiWindow,
    root: E,
    node: E,
}

pub struct UiEngine<B: UiBackend> {
    backend: B,
    registry: UiRegistry<Target<B::Element>>,
}
impl<B: UiBackend> UiEngine<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            registry: UiRegistry::default(),
        }
    }
    pub fn release_owner(&mut self, owner: &UiOwner) {
        self.registry.release_owner(owner);
    }
    pub fn release_window(&mut self, window_ref: &str) {
        self.registry.release_window(window_ref);
    }

    pub fn run(
        &mut self,
        owner: &UiOwner,
        ticket: Option<&UiWindow>,
        request: &UiRequest,
    ) -> UiSnapshot {
        let started = now();
        let mut result = UiSnapshot {
            elements: vec![],
            visited_count: 0,
            truncated: false,
            stop_reason: None,
            outcome: UiOutcome::Completed,
            action_dispatched: Some(false),
            verification: UiVerification::NotApplicable,
            error_code: None,
            sampled_from_unix_ms: started,
            sampled_at_unix_ms: started,
        };
        let outcome =
            request
                .validate()
                .map_err(|_| "invalid_request")
                .and_then(|_| match request {
                    UiRequest::Query {
                        scope,
                        selector,
                        limits,
                    } => self.query(owner, ticket, scope, selector, limits, false, &mut result),
                    UiRequest::Get {
                        element_ref,
                        include_value,
                    } => {
                        let (target, parent) = self.target(owner, element_ref)?;
                        let mut e = self.read(&target, *include_value)?;
                        e.element_ref = element_ref.clone();
                        e.parent_ref = parent;
                        result.elements.push(e);
                        result.visited_count = 1;
                        Ok(())
                    }
                    UiRequest::Action {
                        element_ref,
                        action,
                        expected,
                        ..
                    } => self.action(owner, element_ref, action, expected, &mut result),
                    // Executor turns a wait into bounded samples and releases the helper
                    // input queue between them. The worker must never run a polling loop.
                    UiRequest::Wait { .. } => Err("wait_requires_executor_sampling"),
                });
        if let Err(error) = outcome {
            result.error_code = Some(error.into());
            result.outcome = if result.action_dispatched == Some(false) {
                UiOutcome::Rejected
            } else {
                UiOutcome::Unconfirmed
            };
        }
        result.sampled_at_unix_ms = now();
        result
    }

    /// One wait sample only. No sleeps, and no interpretation of failures as absence.
    pub fn sample(
        &mut self,
        owner: &UiOwner,
        ticket: Option<&UiWindow>,
        scope: &UiScope,
        selector: &UiSelector,
        condition: &UiCondition,
    ) -> UiSnapshot {
        let request = match scope {
            UiScope::Element { element_ref } => UiRequest::Get {
                element_ref: element_ref.clone(),
                include_value: matches!(condition, UiCondition::ValueEquals { .. }),
            },
            UiScope::Window { .. } => UiRequest::Query {
                scope: scope.clone(),
                selector: selector.clone(),
                limits: UiQueryLimits::default(),
            },
        };
        let mut result = self.run(owner, ticket, &request);
        if result.error_code.is_some() {
            return result;
        }
        // Value waits require explicit reads; ordinary query results omit values.
        if matches!(condition, UiCondition::ValueEquals { .. })
            && result.elements.len() == 1
            && !result.truncated
        {
            let reference = result.elements[0].element_ref.clone();
            let read = self.run(
                owner,
                None,
                &UiRequest::Get {
                    element_ref: reference,
                    include_value: true,
                },
            );
            if read.error_code.is_some() {
                return read;
            }
            result.elements = read.elements;
        }
        match evaluate_ui_condition(&result.elements, !result.truncated, condition) {
            Ok(true) => result.outcome = UiOutcome::Matched,
            Ok(false) => {}
            Err(code) => {
                result.error_code = Some(code.into());
                result.outcome = UiOutcome::Rejected;
            }
        }
        result.sampled_at_unix_ms = now();
        result
    }

    fn target(
        &mut self,
        owner: &UiOwner,
        reference: &str,
    ) -> Result<(Target<B::Element>, Option<String>), &'static str> {
        let entry = self.registry.get(reference, owner, Instant::now())?;
        Ok((entry.element.clone(), entry.parent_ref.clone()))
    }
    fn read(
        &mut self,
        target: &Target<B::Element>,
        value: bool,
    ) -> Result<UiElement, &'static str> {
        self.backend
            .validate(&target.window, &target.root, &target.node)?;
        let mut e = self.backend.read(&target.node, value)?;
        e.redact_protected();
        bound_fields(&mut e);
        if !value {
            e.value = None;
        }
        Ok(e)
    }
    fn query(
        &mut self,
        owner: &UiOwner,
        ticket: Option<&UiWindow>,
        scope: &UiScope,
        selector: &UiSelector,
        limits: &UiQueryLimits,
        include_value: bool,
        result: &mut UiSnapshot,
    ) -> Result<(), &'static str> {
        let first = match scope {
            UiScope::Window { window_ref } => {
                let window = ticket
                    .filter(|w| &w.window_ref == window_ref)
                    .ok_or("stale_window")?;
                let root = self.backend.root(window)?;
                Target {
                    window: window.clone(),
                    root: root.clone(),
                    node: root,
                }
            }
            UiScope::Element { element_ref } => self.target(owner, element_ref)?.0,
        };
        let start = Instant::now();
        let mut stack = vec![(first, 0u8, None)];
        let mut visited: Vec<B::Element> = vec![];
        while let Some((target, depth, parent_ref)) = stack.pop() {
            if start.elapsed() >= Duration::from_millis(limits.timeout_ms.into()) {
                truncate(result, "time_budget");
                break;
            }
            if result.visited_count >= limits.max_visited {
                truncate(result, "visit_budget");
                break;
            }
            if visited.iter().any(|e| self.backend.same(e, &target.node)) {
                continue;
            }
            visited.push(target.node.clone());
            result.visited_count += 1;
            let mut e = self.read(&target, include_value)?;
            let mut child_parent = parent_ref.clone();
            if e.matches(selector) {
                let backend = &self.backend;
                let reference = self.registry.insert(
                    owner.clone(),
                    target.window.window_ref.clone(),
                    parent_ref.clone(),
                    target.clone(),
                    Instant::now(),
                    |a, b| backend.same(&a.node, &b.node),
                )?;
                e.element_ref = reference.clone();
                e.parent_ref = parent_ref;
                // Only point to an actual returned ancestor; unreturned parent refs
                // would otherwise be unusable and consume the whole registry budget.
                child_parent = Some(reference);
                result.elements.push(e);
                if serde_json::to_vec(result)
                    .map_err(|_| "serialization_failed")?
                    .len()
                    > MAX_UI_REPLY_BYTES - 1024
                {
                    result.elements.pop();
                    truncate(result, "byte_budget");
                    break;
                }
                if result.elements.len() >= limits.limit as usize {
                    truncate(result, "result_budget");
                    break;
                }
            }
            let remaining =
                ((limits.max_visited - result.visited_count) as usize).saturating_sub(stack.len());
            let (children, omitted) = self.backend.children(&target.node, remaining.min(2000))?;
            if omitted {
                truncate(result, "visit_budget");
            }
            if depth >= limits.max_depth {
                if !children.is_empty() {
                    truncate(result, "depth_budget");
                }
                continue;
            }
            for node in children.into_iter().rev() {
                stack.push((
                    Target {
                        window: target.window.clone(),
                        root: target.root.clone(),
                        node,
                    },
                    depth + 1,
                    child_parent.clone(),
                ));
            }
            if stack.len() > limits.max_visited as usize {
                return Err("backend_budget_violation");
            }
        }
        Ok(())
    }
    fn action(
        &mut self,
        owner: &UiOwner,
        reference: &str,
        action: &UiAction,
        expected: &UiExpected,
        result: &mut UiSnapshot,
    ) -> Result<(), &'static str> {
        let (target, parent) = self.target(owner, reference)?;
        let with_value = expected.value.is_some() || matches!(action, UiAction::SetValue { .. });
        let before = self.read(&target, with_value)?;
        result.visited_count = 1;
        before.check_expected(expected)?;
        if before.enabled != Some(true) {
            return Err("control_not_enabled");
        }
        let kind = match action {
            UiAction::Invoke => UiActionKind::Invoke,
            UiAction::SetValue { .. } => UiActionKind::SetValue,
            UiAction::SetChecked { .. } => UiActionKind::SetChecked,
            UiAction::Select => UiActionKind::Select,
            UiAction::Expand => UiActionKind::Expand,
            UiAction::Collapse => UiActionKind::Collapse,
            UiAction::Focus => UiActionKind::Focus,
        };
        if !before.supported_actions.contains(&kind) {
            return Err("unsupported_action");
        }
        if matches!(action, UiAction::SetValue { .. })
            && (before.protected || before.read_only != Some(false))
        {
            return Err("value_not_writable");
        }
        self.backend
            .validate(&target.window, &target.root, &target.node)?;
        // An idempotent desired state may already hold. No toggle is dispatched.
        let satisfied = verification_matches(&before, action);
        if satisfied != Some(true) {
            result.action_dispatched = Some(true);
            self.backend.act(&target.node, action)?;
        }
        result.verification = UiVerification::NativeReturned;
        match self.read(&target, with_value) {
            Ok(mut after) => {
                result.verification = match verification_matches(&after, action) {
                    Some(true) => UiVerification::Matched,
                    Some(false) => UiVerification::Mismatched,
                    None if matches!(action, UiAction::Invoke) => UiVerification::NativeReturned,
                    None => UiVerification::Unavailable,
                };
                after.element_ref = reference.into();
                after.parent_ref = parent;
                // Do not return typed content from actions; explicit get is separate.
                after.value = None;
                result.elements.push(after);
            }
            Err(_) => result.verification = UiVerification::Unavailable,
        }
        Ok(())
    }
}
fn verification_matches(e: &UiElement, action: &UiAction) -> Option<bool> {
    match action {
        UiAction::Invoke => None,
        UiAction::SetValue { value } => {
            if e.field_errors.contains_key("value") {
                None
            } else {
                e.value.as_ref().map(|v| v == value)
            }
        }
        UiAction::SetChecked { checked } => e.checked.map(|v| {
            v == if *checked {
                UiCheckState::On
            } else {
                UiCheckState::Off
            }
        }),
        UiAction::Select => e.selected,
        UiAction::Expand => e.expanded,
        UiAction::Collapse => e.expanded.map(|v| !v),
        UiAction::Focus => e.focused,
    }
}
fn bound_fields(e: &mut UiElement) {
    fn bound(text: &mut String, max: usize) -> bool {
        if text.len() <= max {
            return false;
        }
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        true
    }
    for (name, field, max) in [
        ("name", &mut e.name, 1024),
        ("identifier", &mut e.identifier, 512),
        ("value", &mut e.value, MAX_UI_TEXT_BYTES),
    ] {
        if let Some(text) = field
            && bound(text, max)
        {
            e.field_errors.insert(name.into(), "truncated".into());
        }
    }
    bound(&mut e.native_role, 128);
    bound(&mut e.coordinate_space, 64);
    e.field_errors = std::mem::take(&mut e.field_errors)
        .into_iter()
        .take(24)
        .map(|(mut key, mut value)| {
            bound(&mut key, 64);
            bound(&mut value, 128);
            (key, value)
        })
        .collect();
}
fn truncate(result: &mut UiSnapshot, reason: &str) {
    result.truncated = true;
    result.stop_reason.get_or_insert_with(|| reason.into());
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
