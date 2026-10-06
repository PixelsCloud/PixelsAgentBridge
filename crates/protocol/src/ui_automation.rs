//! Bounded, window-scoped accessibility requests. References are opaque; the
//! helper must bind them to the authenticated connection and native object.
use crate::RequestId;
use serde::{Deserialize, Serialize};

pub const MAX_UI_TEXT_BYTES: usize = 4096;
pub const MAX_UI_SELECTOR_BYTES: usize = 512;
pub const MAX_UI_REPLY_BYTES: usize = 32 * 1024;
pub const MAX_UI_VISITED: u32 = 2000;

/// A value is only populated on explicit reads of non-protected controls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiElement {
    pub element_ref: String,
    pub parent_ref: Option<String>,
    pub role: UiRole,
    pub native_role: String,
    pub name: Option<String>,
    pub identifier: Option<String>,
    pub value: Option<String>,
    pub protected: bool,
    pub enabled: Option<bool>,
    pub visible: Option<bool>,
    pub offscreen: Option<bool>,
    pub focused: Option<bool>,
    pub read_only: Option<bool>,
    pub checked: Option<UiCheckState>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
    pub bounding_rect: Option<UiRect>,
    pub coordinate_space: String,
    pub supported_actions: Vec<UiActionKind>,
    pub field_errors: std::collections::BTreeMap<String, String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiActionKind {
    Invoke,
    SetValue,
    SetChecked,
    Select,
    Expand,
    Collapse,
    Focus,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiOutcome {
    Completed,
    Matched,
    TimedOut,
    Cancelled,
    Rejected,
    Unconfirmed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiVerification {
    NotApplicable,
    NativeReturned,
    Matched,
    Mismatched,
    Unavailable,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSnapshot {
    pub elements: Vec<UiElement>,
    pub visited_count: u32,
    pub truncated: bool,
    pub stop_reason: Option<String>,
    pub outcome: UiOutcome,
    /// None means a worker died before it could report the dispatch boundary.
    pub action_dispatched: Option<bool>,
    pub verification: UiVerification,
    pub error_code: Option<String>,
    pub sampled_from_unix_ms: u64,
    pub sampled_at_unix_ms: u64,
}

impl UiElement {
    /// Do not permit a platform adapter to leak a secure control's value.
    pub fn redact_protected(&mut self) {
        if self.protected {
            self.value = None;
            self.supported_actions
                .retain(|a| *a != UiActionKind::SetValue);
        }
    }
    pub fn matches(&self, selector: &UiSelector) -> bool {
        selector.role.is_none_or(|role| role == self.role)
            && selector
                .name
                .as_ref()
                .is_none_or(|v| self.name.as_ref() == Some(v))
            && selector
                .name_contains
                .as_ref()
                .is_none_or(|v| self.name.as_ref().is_some_and(|n| n.contains(v)))
            && selector
                .identifier
                .as_ref()
                .is_none_or(|v| self.identifier.as_ref() == Some(v))
    }
    pub fn check_expected(&self, expected: &UiExpected) -> Result<(), &'static str> {
        if expected.enabled.is_some_and(|v| self.enabled != Some(v))
            || expected
                .read_only
                .is_some_and(|v| self.read_only != Some(v))
            || expected.checked.is_some_and(|v| self.checked != Some(v))
            || expected.selected.is_some_and(|v| self.selected != Some(v))
            || expected
                .value
                .as_ref()
                .is_some_and(|v| self.protected || self.value.as_ref() != Some(v))
        {
            return Err("precondition_failed");
        }
        Ok(())
    }
}

/// Only a complete, successful traversal can prove absence. Unknown attributes
/// never satisfy boolean conditions by being treated as false.
pub fn evaluate_ui_condition(
    elements: &[UiElement],
    complete: bool,
    condition: &UiCondition,
) -> Result<bool, &'static str> {
    match condition {
        UiCondition::Exists => return Ok(!elements.is_empty()),
        UiCondition::Absent => return Ok(complete && elements.is_empty()),
        _ => {}
    }
    if elements.len() > 1 {
        return Err("ambiguous");
    }
    // A truncated traversal cannot prove that a single match is unique.
    if !complete {
        return Ok(false);
    }
    let Some(e) = elements.first() else {
        return Ok(false);
    };
    Ok(match condition {
        UiCondition::Enabled { value } => e.enabled == Some(*value),
        UiCondition::ValueEquals { value } => !e.protected && e.value.as_ref() == Some(value),
        UiCondition::Checked { value } => e.checked == Some(*value),
        UiCondition::Selected { value } => e.selected == Some(*value),
        _ => unreachable!(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiScope {
    Window { window_ref: String },
    Element { element_ref: String },
}
impl UiScope {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Window { window_ref } => reference(window_ref),
            Self::Element { element_ref } => reference(element_ref),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRole {
    Window,
    Button,
    TextField,
    Text,
    CheckBox,
    RadioButton,
    List,
    ListItem,
    ComboBox,
    Menu,
    MenuItem,
    Group,
    Tab,
    TabItem,
    Tree,
    TreeItem,
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiSelector {
    pub role: Option<UiRole>,
    pub name: Option<String>,
    pub name_contains: Option<String>,
    pub identifier: Option<String>,
}
impl UiSelector {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.is_some() && self.name_contains.is_some() {
            return Err("name and name_contains are mutually exclusive");
        }
        for text in [&self.name, &self.name_contains, &self.identifier]
            .into_iter()
            .flatten()
        {
            bounded_text(text, MAX_UI_SELECTOR_BYTES)?;
        }
        if self.name_contains.as_deref() == Some("") || self.identifier.as_deref() == Some("") {
            return Err("contains and identifier cannot be empty");
        }
        Ok(())
    }
    fn redact(&mut self) {
        for value in [
            &mut self.name,
            &mut self.name_contains,
            &mut self.identifier,
        ]
        .into_iter()
        .flatten()
        {
            *value = digest(value);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiQueryLimits {
    pub limit: u16,
    pub max_depth: u8,
    pub max_visited: u32,
    pub timeout_ms: u32,
}
impl Default for UiQueryLimits {
    fn default() -> Self {
        Self {
            limit: 100,
            max_depth: 6,
            max_visited: MAX_UI_VISITED,
            timeout_ms: 3000,
        }
    }
}
impl UiQueryLimits {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(1..=500).contains(&self.limit)
            || !(1..=12).contains(&self.max_depth)
            || self.max_visited < u32::from(self.limit)
            || self.max_visited > MAX_UI_VISITED
            || !(100..=10_000).contains(&self.timeout_ms)
        {
            return Err(
                "UI query budget invalid: limit 1..500, depth 1..12, visited limit..2000, timeout 100..10000 ms",
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiCheckState {
    Off,
    On,
    Mixed,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiExpected {
    pub enabled: Option<bool>,
    pub read_only: Option<bool>,
    pub checked: Option<UiCheckState>,
    pub selected: Option<bool>,
    pub value: Option<String>,
}
impl UiExpected {
    pub fn validate(&self) -> Result<(), &'static str> {
        if let Some(value) = &self.value {
            bounded_text(value, MAX_UI_TEXT_BYTES)?;
        }
        Ok(())
    }
    fn redact(&mut self) {
        if let Some(value) = &mut self.value {
            *value = digest(value);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiAction {
    Invoke,
    SetValue { value: String },
    SetChecked { checked: bool },
    Select,
    Expand,
    Collapse,
    Focus,
}
impl UiAction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Invoke => "invoke",
            Self::SetValue { .. } => "set_value",
            Self::SetChecked { .. } => "set_checked",
            Self::Select => "select",
            Self::Expand => "expand",
            Self::Collapse => "collapse",
            Self::Focus => "focus",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiCondition {
    Exists,
    Absent,
    Enabled { value: bool },
    ValueEquals { value: String },
    Checked { value: UiCheckState },
    Selected { value: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiRequest {
    Query {
        scope: UiScope,
        #[serde(default)]
        selector: UiSelector,
        #[serde(default)]
        limits: UiQueryLimits,
    },
    Get {
        element_ref: String,
        #[serde(default)]
        include_value: bool,
    },
    Action {
        element_ref: String,
        action: UiAction,
        #[serde(default)]
        expected: UiExpected,
        #[serde(default = "action_timeout")]
        timeout_ms: u32,
    },
    Wait {
        scope: UiScope,
        #[serde(default)]
        selector: UiSelector,
        condition: UiCondition,
        #[serde(default = "action_timeout")]
        timeout_ms: u32,
        #[serde(default = "poll_interval")]
        poll_ms: u32,
    },
}
fn action_timeout() -> u32 {
    5000
}
fn poll_interval() -> u32 {
    250
}
impl UiRequest {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Query { .. } => "ui_query",
            Self::Get { .. } => "ui_get",
            Self::Action { .. } => "ui_action",
            Self::Wait { .. } => "ui_wait",
        }
    }
    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Action { .. })
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Query {
                scope,
                selector,
                limits,
            } => {
                scope.validate()?;
                selector.validate()?;
                limits.validate()?;
            }
            Self::Get { element_ref, .. } => reference(element_ref)?,
            Self::Action {
                element_ref,
                action,
                expected,
                timeout_ms,
            } => {
                reference(element_ref)?;
                expected.validate()?;
                if !(100..=10_000).contains(timeout_ms) {
                    return Err("UI action timeout must be 100..10000 ms");
                }
                if let UiAction::SetValue { value } = action {
                    bounded_text(value, MAX_UI_TEXT_BYTES)?;
                }
            }
            Self::Wait {
                scope,
                selector,
                condition,
                timeout_ms,
                poll_ms,
            } => {
                scope.validate()?;
                selector.validate()?;
                if !(100..=30_000).contains(timeout_ms)
                    || !(100..=1000).contains(poll_ms)
                    || poll_ms > timeout_ms
                {
                    return Err(
                        "UI wait timeout 100..30000 ms, poll 100..1000 ms and no greater than timeout",
                    );
                }
                if matches!(scope, UiScope::Element { .. }) && selector != &UiSelector::default() {
                    return Err("element-bound wait cannot replace its target using a selector");
                }
                if let UiCondition::ValueEquals { value } = condition {
                    bounded_text(value, MAX_UI_TEXT_BYTES)?;
                }
            }
        }
        Ok(())
    }
    /// Preserve deduplication identity without persisting selectors or typed text.
    /// Validate the original request, never this storage-only representation.
    pub fn persistence_form(&self) -> Self {
        let mut query = self.clone();
        match &mut query {
            Self::Query { selector, .. } => selector.redact(),
            Self::Wait {
                selector,
                condition,
                ..
            } => {
                selector.redact();
                if let UiCondition::ValueEquals { value } = condition {
                    *value = digest(value);
                }
            }
            Self::Action {
                action, expected, ..
            } => {
                expected.redact();
                if let UiAction::SetValue { value } = action {
                    *value = digest(value);
                }
            }
            Self::Get { .. } => {}
        }
        query
    }
}
fn reference(value: &str) -> Result<(), &'static str> {
    value
        .parse::<RequestId>()
        .map(|_| ())
        .map_err(|_| "UI reference must be an opaque UUID from this connection")
}
fn bounded_text(value: &str, max: usize) -> Result<(), &'static str> {
    if value.len() > max || value.contains('\0') {
        Err("UI text exceeds UTF-8 byte budget or contains NUL")
    } else {
        Ok(())
    }
}
fn digest(value: &str) -> String {
    format!("blake3:{}", blake3::hash(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn element() -> UiElement {
        UiElement {
            element_ref: RequestId::new().to_string(),
            parent_ref: None,
            role: UiRole::TextField,
            native_role: "AXTextField".into(),
            name: Some("Fixture input".into()),
            identifier: None,
            value: Some("fixture".into()),
            protected: false,
            enabled: Some(true),
            visible: None,
            offscreen: None,
            focused: None,
            read_only: None,
            checked: None,
            selected: None,
            expanded: None,
            bounding_rect: None,
            coordinate_space: "macos_points".into(),
            supported_actions: vec![UiActionKind::SetValue],
            field_errors: Default::default(),
        }
    }
    #[test]
    fn absence_requires_complete_query_and_unknown_is_not_false() {
        assert!(!evaluate_ui_condition(&[], false, &UiCondition::Absent).unwrap());
        assert!(evaluate_ui_condition(&[], true, &UiCondition::Absent).unwrap());
        let e = element();
        assert!(
            !evaluate_ui_condition(&[e.clone()], true, &UiCondition::Selected { value: false })
                .unwrap()
        );
        assert_eq!(
            evaluate_ui_condition(
                &[e.clone(), e.clone()],
                true,
                &UiCondition::Enabled { value: true }
            ),
            Err("ambiguous")
        );
        assert!(
            !evaluate_ui_condition(&[e], false, &UiCondition::Enabled { value: true }).unwrap()
        );
    }
    #[test]
    fn secure_values_cannot_be_read_set_or_used_to_satisfy_preconditions() {
        let mut e = element();
        e.protected = true;
        let expected = UiExpected {
            value: Some("fixture".into()),
            ..Default::default()
        };
        assert_eq!(e.check_expected(&expected), Err("precondition_failed"));
        assert!(
            !evaluate_ui_condition(
                &[e.clone()],
                true,
                &UiCondition::ValueEquals {
                    value: "fixture".into()
                }
            )
            .unwrap()
        );
        e.redact_protected();
        assert!(e.value.is_none());
        assert!(e.supported_actions.is_empty());
    }
    #[test]
    fn expectations_fail_closed_on_missing_attributes_and_names_match_exactly() {
        let e = element();
        assert!(e.check_expected(&UiExpected::default()).is_ok());
        assert!(
            e.check_expected(&UiExpected {
                read_only: Some(false),
                ..Default::default()
            })
            .is_err()
        );
        assert!(!e.matches(&UiSelector {
            name: Some("input".into()),
            ..Default::default()
        }));
        assert!(e.matches(&UiSelector {
            name_contains: Some("input".into()),
            ..Default::default()
        }));
    }
    #[test]
    fn request_is_strict_and_does_not_accept_native_handles_or_unbounded_scopes() {
        for value in [
            json!({"operation":"query","scope":{"type":"window","window_ref":"123"}}),
            json!({"operation":"get","element_ref":RequestId::new().to_string(),"pid":42}),
            json!({"operation":"action","element_ref":RequestId::new().to_string(),"action":{"type":"click","x":1,"y":1}}),
            json!({"operation":"query","scope":{"type":"desktop"}}),
        ] {
            assert!(
                serde_json::from_value::<UiRequest>(value).map_or(true, |r| r.validate().is_err())
            );
        }
    }
    #[test]
    fn unicode_budget_allows_empty_value_but_rejects_nul_and_overflow() {
        let mut q = UiRequest::Action {
            element_ref: RequestId::new().to_string(),
            action: UiAction::SetValue {
                value: String::new(),
            },
            expected: UiExpected::default(),
            timeout_ms: 5000,
        };
        assert!(q.validate().is_ok());
        for (text, valid) in [
            ("中".repeat(1365), true),
            ("中".repeat(1366), false),
            ("a\0b".into(), false),
        ] {
            if let UiRequest::Action { action, .. } = &mut q {
                *action = UiAction::SetValue { value: text };
            }
            assert_eq!(q.validate().is_ok(), valid);
        }
    }
    #[test]
    fn query_limits_bound_tree_cost_and_selector_is_unambiguous() {
        assert!(UiQueryLimits::default().validate().is_ok());
        for limits in [
            UiQueryLimits {
                limit: 501,
                ..Default::default()
            },
            UiQueryLimits {
                max_depth: 13,
                ..Default::default()
            },
            UiQueryLimits {
                max_visited: 99,
                ..Default::default()
            },
            UiQueryLimits {
                timeout_ms: 10_001,
                ..Default::default()
            },
        ] {
            assert!(limits.validate().is_err());
        }
        assert!(
            UiSelector {
                name: Some("x".into()),
                name_contains: Some("x".into()),
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn storage_redacts_values_but_still_distinguishes_different_requests() {
        let q = UiRequest::Action {
            element_ref: RequestId::new().to_string(),
            action: UiAction::SetValue {
                value: "private-输入".into(),
            },
            expected: UiExpected {
                value: Some("previous-private".into()),
                ..Default::default()
            },
            timeout_ms: 5000,
        };
        let saved = serde_json::to_string(&q.persistence_form()).unwrap();
        assert!(!saved.contains("private"));
        assert!(saved.contains("blake3:"));
        assert_eq!(
            serde_json::from_str::<UiRequest>(&saved).unwrap(),
            q.persistence_form()
        );
        let mut other = q.clone();
        if let UiRequest::Action { action, .. } = &mut other {
            *action = UiAction::SetValue {
                value: "other".into(),
            };
        }
        assert_ne!(q.persistence_form(), other.persistence_form());
    }
    #[test]
    fn waits_are_bounded_and_element_wait_cannot_retarget() {
        let mut q = UiRequest::Wait {
            scope: UiScope::Element {
                element_ref: RequestId::new().to_string(),
            },
            selector: UiSelector::default(),
            condition: UiCondition::Exists,
            timeout_ms: 5000,
            poll_ms: 250,
        };
        assert!(q.validate().is_ok());
        assert!(!q.is_mutation());
        if let UiRequest::Wait { selector, .. } = &mut q {
            selector.name = Some("replacement".into());
        }
        assert!(q.validate().is_err());
    }
}
