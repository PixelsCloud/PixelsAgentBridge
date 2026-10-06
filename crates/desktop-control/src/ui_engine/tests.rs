use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

struct Fake {
    state: Arc<Mutex<State>>,
}
#[derive(Default)]
struct State {
    calls: usize,
    value: String,
    dead: bool,
    fail_after_effect: bool,
    protected: bool,
    node_count: Option<u32>,
    cycle: bool,
    permission_denied: bool,
    ignore_action: bool,
}
impl UiBackend for Fake {
    type Element = u32;
    fn root(&mut self, _: &UiWindow) -> Result<u32, &'static str> {
        Ok(0)
    }
    fn same(&self, a: &u32, b: &u32) -> bool {
        a == b
    }
    fn validate(&mut self, _: &UiWindow, _: &u32, _: &u32) -> Result<(), &'static str> {
        let state = self.state.lock().unwrap();
        if state.permission_denied {
            return Err("accessibility_permission_required");
        }
        if state.dead {
            Err("stale_element")
        } else {
            Ok(())
        }
    }
    fn children(&mut self, node: &u32, max: usize) -> Result<(Vec<u32>, bool), &'static str> {
        let state = self.state.lock().unwrap();
        let children = if *node == 0 {
            (1..=state.node_count.unwrap_or(2)).collect::<Vec<_>>()
        } else if state.cycle {
            vec![0]
        } else {
            vec![]
        };
        let omitted = children.len() > max;
        Ok((children.into_iter().take(max).collect(), omitted))
    }
    fn read(&mut self, node: &u32, value: bool) -> Result<UiElement, &'static str> {
        let s = self.state.lock().unwrap();
        Ok(UiElement {
            element_ref: String::new(),
            parent_ref: None,
            role: if *node == 0 {
                UiRole::Window
            } else {
                UiRole::TextField
            },
            native_role: "test".into(),
            name: Some(if *node == 0 { "root" } else { "duplicate" }.into()),
            identifier: None,
            value: value.then(|| s.value.clone()),
            protected: s.protected,
            enabled: Some(true),
            read_only: Some(false),
            visible: None,
            offscreen: None,
            focused: None,
            checked: None,
            selected: None,
            expanded: None,
            bounding_rect: None,
            coordinate_space: "test".into(),
            supported_actions: vec![UiActionKind::Invoke, UiActionKind::SetValue],
            field_errors: BTreeMap::new(),
        })
    }
    fn act(&mut self, _: &u32, action: &UiAction) -> Result<(), &'static str> {
        let mut s = self.state.lock().unwrap();
        s.calls += 1;
        if let UiAction::SetValue { value } = action
            && !s.ignore_action
        {
            s.value = value.clone();
        }
        if s.fail_after_effect {
            Err("provider_failed")
        } else {
            Ok(())
        }
    }
}
fn fixture() -> (UiEngine<Fake>, UiOwner, UiWindow, Arc<Mutex<State>>) {
    let state = Arc::new(Mutex::new(State::default()));
    let engine = UiEngine::new(Fake {
        state: state.clone(),
    });
    let owner = UiOwner {
        connection: RequestId::new(),
        helper_instance: "helper".into(),
    };
    let ticket = UiWindow {
        window_ref: RequestId::new().to_string(),
        id: 1,
        pid: 2,
        process_identity: "test".into(),
        marker_key: "test".into(),
        marker: 3,
    };
    (engine, owner, ticket, state)
}
fn query(ticket: &UiWindow) -> UiRequest {
    UiRequest::Query {
        scope: UiScope::Window {
            window_ref: ticket.window_ref.clone(),
        },
        selector: UiSelector::default(),
        limits: UiQueryLimits::default(),
    }
}
fn action(reference: String, value: &str) -> UiRequest {
    UiRequest::Action {
        element_ref: reference,
        action: UiAction::SetValue {
            value: value.into(),
        },
        expected: UiExpected::default(),
        timeout_ms: 5000,
    }
}
#[test]
fn query_is_scoped_reports_duplicates_and_value_is_opt_in() {
    let (mut engine, owner, ticket, state) = fixture();
    state.lock().unwrap().value = "private".into();
    assert_eq!(
        engine
            .run(&owner, None, &query(&ticket))
            .error_code
            .as_deref(),
        Some("stale_window")
    );
    let result = engine.run(&owner, Some(&ticket), &query(&ticket));
    assert_eq!(result.elements.len(), 3);
    assert!(!result.truncated);
    assert!(result.elements.iter().all(|e| e.value.is_none()));
    let child = &result.elements[1];
    assert_eq!(
        child.parent_ref.as_ref(),
        Some(&result.elements[0].element_ref)
    );
    let read = engine.run(
        &owner,
        None,
        &UiRequest::Get {
            element_ref: child.element_ref.clone(),
            include_value: true,
        },
    );
    assert_eq!(read.elements[0].value.as_deref(), Some("private"));
    let sample = engine.sample(
        &owner,
        Some(&ticket),
        &UiScope::Window {
            window_ref: ticket.window_ref.clone(),
        },
        &UiSelector {
            name: Some("duplicate".into()),
            ..Default::default()
        },
        &UiCondition::Enabled { value: true },
    );
    assert_eq!(sample.error_code.as_deref(), Some("ambiguous"));
}
#[test]
fn preconditions_and_dead_targets_prevent_dispatch_and_actions_do_not_echo_text() {
    let (mut engine, owner, ticket, state) = fixture();
    let reference = engine.run(&owner, Some(&ticket), &query(&ticket)).elements[1]
        .element_ref
        .clone();
    let mut request = action(reference.clone(), "PAB 中文🙂");
    if let UiRequest::Action { expected, .. } = &mut request {
        expected.value = Some("wrong".into());
    }
    let rejected = engine.run(&owner, None, &request);
    assert_eq!(rejected.error_code.as_deref(), Some("precondition_failed"));
    assert_eq!(state.lock().unwrap().calls, 0);
    let success = engine.run(&owner, None, &action(reference.clone(), "PAB 中文🙂"));
    assert_eq!(success.verification, UiVerification::Matched);
    assert_eq!(state.lock().unwrap().calls, 1);
    assert!(!serde_json::to_string(&success).unwrap().contains("PAB"));
    let same = engine.run(&owner, None, &action(reference.clone(), "PAB 中文🙂"));
    assert_eq!(same.action_dispatched, Some(false));
    assert_eq!(state.lock().unwrap().calls, 1);
    state.lock().unwrap().dead = true;
    assert_eq!(
        engine
            .run(&owner, None, &action(reference, "other"))
            .error_code
            .as_deref(),
        Some("stale_element")
    );
    assert_eq!(state.lock().unwrap().calls, 1);
}
#[test]
fn error_after_side_effect_is_unconfirmed_and_is_never_replayed() {
    let (mut engine, owner, ticket, state) = fixture();
    let reference = engine.run(&owner, Some(&ticket), &query(&ticket)).elements[1]
        .element_ref
        .clone();
    state.lock().unwrap().fail_after_effect = true;
    let result = engine.run(&owner, None, &action(reference, "effect"));
    assert_eq!(result.action_dispatched, Some(true));
    assert_eq!(result.outcome, UiOutcome::Unconfirmed);
    assert_eq!(state.lock().unwrap().calls, 1);
    assert_eq!(state.lock().unwrap().value, "effect");
}
#[test]
fn secure_fields_and_wrong_owner_are_rejected_without_native_action() {
    let (mut engine, owner, ticket, state) = fixture();
    state.lock().unwrap().protected = true;
    let reference = engine.run(&owner, Some(&ticket), &query(&ticket)).elements[1]
        .element_ref
        .clone();
    let result = engine.run(&owner, None, &action(reference.clone(), "secret"));
    assert_eq!(result.error_code.as_deref(), Some("unsupported_action"));
    let stranger = UiOwner {
        connection: RequestId::new(),
        ..owner.clone()
    };
    assert_eq!(
        engine
            .run(&stranger, None, &action(reference, "secret"))
            .error_code
            .as_deref(),
        Some("stale_element")
    );
    assert_eq!(state.lock().unwrap().calls, 0);
}
#[test]
fn exact_reference_wait_does_not_match_a_descendant_and_query_limits_are_explicit() {
    let (mut engine, owner, ticket, _) = fixture();
    let mut request = query(&ticket);
    if let UiRequest::Query { limits, .. } = &mut request {
        limits.limit = 1;
    }
    let result = engine.run(&owner, Some(&ticket), &request);
    assert!(result.truncated);
    assert_eq!(result.stop_reason.as_deref(), Some("result_budget"));
    let sample = engine.sample(
        &owner,
        None,
        &UiScope::Element {
            element_ref: result.elements[0].element_ref.clone(),
        },
        &UiSelector::default(),
        &UiCondition::Exists,
    );
    assert_eq!(sample.outcome, UiOutcome::Matched);
    assert_eq!(sample.elements.len(), 1);
}
#[test]
fn truncated_values_do_not_satisfy_expected_value_or_verification() {
    let (mut engine, owner, ticket, state) = fixture();
    state.lock().unwrap().value = "a".repeat(MAX_UI_TEXT_BYTES + 1);
    let reference = engine.run(&owner, Some(&ticket), &query(&ticket)).elements[1]
        .element_ref
        .clone();
    let read = engine.run(
        &owner,
        None,
        &UiRequest::Get {
            element_ref: reference.clone(),
            include_value: true,
        },
    );
    assert_eq!(
        read.elements[0]
            .field_errors
            .get("value")
            .map(String::as_str),
        Some("truncated")
    );
    let sample = engine.sample(
        &owner,
        None,
        &UiScope::Element {
            element_ref: reference,
        },
        &UiSelector::default(),
        &UiCondition::ValueEquals {
            value: "a".repeat(MAX_UI_TEXT_BYTES),
        },
    );
    assert_ne!(sample.outcome, UiOutcome::Matched);
}
#[test]
fn wide_trees_cycles_and_byte_budgets_remain_bounded_and_cannot_prove_absence() {
    let (mut engine, owner, ticket, state) = fixture();
    state.lock().unwrap().node_count = Some(500);
    state.lock().unwrap().cycle = true;
    let mut request = query(&ticket);
    if let UiRequest::Query { limits, .. } = &mut request {
        limits.limit = 500;
    }
    let result = engine.run(&owner, Some(&ticket), &request);
    assert!(result.truncated);
    assert_eq!(result.stop_reason.as_deref(), Some("byte_budget"));
    assert!(serde_json::to_vec(&result).unwrap().len() < MAX_UI_REPLY_BYTES);
    if let UiRequest::Query { selector, .. } = &mut request {
        selector.name = Some("absent".into());
    }
    let scan = engine.run(&owner, Some(&ticket), &request);
    assert!(!scan.truncated);
    assert_eq!(scan.visited_count, 501);
    if let UiRequest::Query { limits, .. } = &mut request {
        limits.max_visited = 100;
        limits.limit = 100;
    }
    let partial = engine.run(&owner, Some(&ticket), &request);
    assert!(partial.truncated);
    assert!(partial.visited_count <= 100);
    assert_ne!(
        evaluate_ui_condition(&partial.elements, false, &UiCondition::Absent),
        Ok(true)
    );
}
#[test]
fn permission_revocation_cache_release_and_native_success_without_effect_are_distinct() {
    let (mut engine, owner, ticket, state) = fixture();
    let reference = engine.run(&owner, Some(&ticket), &query(&ticket)).elements[1]
        .element_ref
        .clone();
    state.lock().unwrap().permission_denied = true;
    let denied = engine.run(&owner, None, &action(reference.clone(), "value"));
    assert_eq!(
        denied.error_code.as_deref(),
        Some("accessibility_permission_required")
    );
    assert_eq!(denied.action_dispatched, Some(false));
    state.lock().unwrap().permission_denied = false;
    state.lock().unwrap().ignore_action = true;
    let no_effect = engine.run(&owner, None, &action(reference.clone(), "value"));
    assert_eq!(no_effect.action_dispatched, Some(true));
    assert_eq!(no_effect.verification, UiVerification::Mismatched);
    engine.release_owner(&owner);
    assert_eq!(
        engine
            .run(&owner, None, &action(reference, "value"))
            .error_code
            .as_deref(),
        Some("stale_element")
    );
    let reference = engine.run(&owner, Some(&ticket), &query(&ticket)).elements[1]
        .element_ref
        .clone();
    engine.release_window(&ticket.window_ref);
    assert_eq!(
        engine
            .run(&owner, None, &action(reference, "value"))
            .error_code
            .as_deref(),
        Some("stale_element")
    );
    assert_eq!(state.lock().unwrap().calls, 1);
}
