//! AX objects stay on the isolated worker's main thread. No unsafe Send/Sync.
use crate::ui_engine::{UiBackend, UiWindow};
use accessibility::{AXAttribute, AXUIElement};
use accessibility_sys::*;
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    boolean::CFBoolean,
    number::CFNumber,
    string::CFString,
};
use pab_protocol::*;
use std::collections::BTreeMap;

pub struct MacUi;
fn error(code: i32) -> &'static str {
    match code {
        -25211 => "accessibility_permission_required",
        -25202 => "stale_element",
        -25204 => "provider_unresponsive",
        -25205 | -25206 | -25208 => "unsupported_attribute_or_action",
        -25212 => "attribute_unavailable",
        _ => "provider_failed",
    }
}
fn mapped(e: accessibility::Error) -> &'static str {
    match e {
        accessibility::Error::Ax(code) => error(code),
        _ => "provider_type_mismatch",
    }
}
fn attr(node: &AXUIElement, key: &str) -> Result<CFType, &'static str> {
    node.attribute(&AXAttribute::new(&CFString::new(key)))
        .map_err(mapped)
}
fn text(node: &AXUIElement, key: &str) -> Result<String, &'static str> {
    attr(node, key)?
        .downcast::<CFString>()
        .map(|s| s.to_string())
        .ok_or("provider_type_mismatch")
}
fn boolean(node: &AXUIElement, key: &str) -> Result<bool, &'static str> {
    attr(node, key)?
        .downcast::<CFBoolean>()
        .map(bool::from)
        .ok_or("provider_type_mismatch")
}
fn number(node: &AXUIElement, key: &str) -> Result<i64, &'static str> {
    let value = attr(node, key)?;
    if let Some(v) = value.downcast::<CFBoolean>() {
        return Ok(i64::from(bool::from(v)));
    }
    value
        .downcast::<CFNumber>()
        .and_then(|n| n.to_i64())
        .ok_or("provider_type_mismatch")
}
fn enabled(node: &AXUIElement) -> Result<bool, &'static str> {
    match boolean(node, "AXEnabled") {
        // AppKit rows omit AXEnabled; their owning table controls availability.
        // Do not turn an arbitrary missing attribute/provider error into true.
        Err("unsupported_attribute_or_action") if text(node, "AXRole")? == "AXRow" => {
            let parent = attr(node, "AXParent")?
                .downcast::<AXUIElement>()
                .ok_or("provider_type_mismatch")?;
            parent.set_messaging_timeout(0.5).map_err(mapped)?;
            match text(&parent, "AXRole")?.as_str() {
                "AXTable" | "AXOutline" => boolean(&parent, "AXEnabled"),
                _ => Err("attribute_unavailable"),
            }
        }
        result => result,
    }
}
fn settable(node: &AXUIElement, key: &str) -> bool {
    node.is_settable(&AXAttribute::<CFType>::new(&CFString::new(key)))
        .unwrap_or(false)
}
fn set(node: &AXUIElement, key: &str, value: CFType) -> Result<(), &'static str> {
    node.set_attribute(&AXAttribute::new(&CFString::new(key)), value)
        .map_err(mapped)
}
fn optional<T>(
    v: Result<T, &'static str>,
    key: &str,
    errors: &mut BTreeMap<String, String>,
) -> Option<T> {
    v.map_err(|code| {
        errors.insert(key.into(), code.into());
    })
    .ok()
}
fn list(
    node: &AXUIElement,
    key: &str,
    max: usize,
) -> Result<(Vec<AXUIElement>, bool), &'static str> {
    let key = CFString::new(key);
    let mut count = 0;
    // SAFETY: retained AX/CFString objects and writable CFIndex. Count is checked
    // before bounded range copying, unlike the high-level children() accessor.
    let code = unsafe {
        AXUIElementGetAttributeValueCount(
            node.as_concrete_TypeRef(),
            key.as_concrete_TypeRef(),
            &mut count,
        )
    };
    if code == kAXErrorAttributeUnsupported || code == kAXErrorNoValue {
        return Ok((vec![], false));
    }
    if code != 0 {
        return Err(error(code));
    }
    if count < 0 {
        return Err("provider_type_mismatch");
    }
    let take = (count as usize).min(max);
    if take == 0 {
        return Ok((vec![], count > 0));
    }
    let mut raw = std::ptr::null();
    let code = unsafe {
        AXUIElementCopyAttributeValues(
            node.as_concrete_TypeRef(),
            key.as_concrete_TypeRef(),
            0,
            take as isize,
            &mut raw,
        )
    };
    if code != 0 {
        return Err(error(code));
    }
    if raw.is_null() {
        return Err("provider_type_mismatch");
    }
    let array: CFArray<CFType> = unsafe { CFArray::wrap_under_create_rule(raw) };
    if array.len() as usize > take {
        return Err("backend_budget_violation");
    }
    let mut entries = vec![];
    for value in array.iter() {
        let element = value
            .downcast::<AXUIElement>()
            .ok_or("provider_type_mismatch")?;
        element.set_messaging_timeout(0.5).map_err(mapped)?;
        entries.push(element);
    }
    Ok((entries, count as usize > max))
}
#[repr(C)]
#[derive(Default)]
struct Pair {
    first: f64,
    second: f64,
}
fn pair(node: &AXUIElement, key: &str, kind: u32) -> Result<Pair, &'static str> {
    let value = attr(node, key)?;
    if value.type_of() != unsafe { AXValueGetTypeID() } {
        return Err("provider_type_mismatch");
    }
    let raw = value.as_CFTypeRef() as AXValueRef;
    if unsafe { AXValueGetType(raw) } != kind {
        return Err("provider_type_mismatch");
    }
    let mut pair = Pair::default();
    if !unsafe { AXValueGetValue(raw, kind, (&mut pair as *mut Pair).cast()) } {
        return Err("provider_type_mismatch");
    }
    if !pair.first.is_finite() || !pair.second.is_finite() {
        return Err("provider_type_mismatch");
    }
    Ok(pair)
}
fn rect(node: &AXUIElement) -> Result<UiRect, &'static str> {
    let p = pair(node, "AXPosition", kAXValueTypeCGPoint)?;
    let s = pair(node, "AXSize", kAXValueTypeCGSize)?;
    if p.first < i32::MIN as f64
        || p.first > i32::MAX as f64
        || p.second < i32::MIN as f64
        || p.second > i32::MAX as f64
        || s.first < 0.0
        || s.second < 0.0
        || s.first > u32::MAX as f64
        || s.second > u32::MAX as f64
    {
        return Err("invalid_geometry");
    }
    Ok(UiRect {
        x: p.first.round() as i32,
        y: p.second.round() as i32,
        width: s.first.round() as u32,
        height: s.second.round() as u32,
    })
}
fn role(name: &str) -> UiRole {
    match name {
        "AXWindow" => UiRole::Window,
        "AXButton" => UiRole::Button,
        "AXTextField" | "AXTextArea" => UiRole::TextField,
        "AXStaticText" => UiRole::Text,
        "AXCheckBox" => UiRole::CheckBox,
        "AXRadioButton" => UiRole::RadioButton,
        "AXList" | "AXTable" => UiRole::List,
        "AXRow" => UiRole::ListItem,
        "AXComboBox" | "AXPopUpButton" => UiRole::ComboBox,
        "AXMenu" => UiRole::Menu,
        "AXMenuItem" => UiRole::MenuItem,
        "AXGroup" => UiRole::Group,
        "AXTabGroup" => UiRole::Tab,
        "AXOutline" => UiRole::Tree,
        _ => UiRole::Unknown,
    }
}
fn check(v: i64) -> Option<UiCheckState> {
    match v {
        0 => Some(UiCheckState::Off),
        1 => Some(UiCheckState::On),
        2 => Some(UiCheckState::Mixed),
        _ => None,
    }
}
fn verify_process(window: &UiWindow) -> Result<(), &'static str> {
    if !crate::native::active_console() || crate::native::login_window_active() {
        return Err("interactive_session_unavailable");
    }
    if !unsafe { AXIsProcessTrusted() } {
        return Err("accessibility_permission_required");
    }
    if pab_os_control::process_identity(window.pid).map_err(|_| "stale_window")?
        != window.process_identity
    {
        return Err("stale_window");
    }
    Ok(())
}
impl UiBackend for MacUi {
    type Element = AXUIElement;
    fn root(&mut self, ticket: &UiWindow) -> Result<AXUIElement, &'static str> {
        verify_process(ticket)?;
        let native = xcap::Window::all()
            .map_err(|_| "window_list_unavailable")?
            .into_iter()
            .find(|w| w.id().ok() == Some(ticket.id) && w.pid().ok() == Some(ticket.pid))
            .ok_or("stale_window")?;
        let title = native.title().map_err(|_| "window_list_unavailable")?;
        let expected = UiRect {
            x: native.x().map_err(|_| "invalid_geometry")?,
            y: native.y().map_err(|_| "invalid_geometry")?,
            width: native.width().map_err(|_| "invalid_geometry")?,
            height: native.height().map_err(|_| "invalid_geometry")?,
        };
        let app = AXUIElement::application(ticket.pid as i32);
        app.set_messaging_timeout(0.5).map_err(mapped)?;
        let (windows, truncated) = list(&app, "AXWindows", 256)?;
        if truncated {
            return Err("window_budget");
        }
        let mut candidates = vec![];
        for window in windows {
            if text(&window, "AXRole")? != "AXWindow" {
                return Err("interactive_window_unavailable");
            }
            if text(&window, "AXTitle").ok().as_deref() != Some(&title) {
                continue;
            }
            if let Ok(r) = rect(&window) {
                if (r.x as i64 - expected.x as i64).abs() < 2
                    && (r.y as i64 - expected.y as i64).abs() < 2
                    && r.width.abs_diff(expected.width) < 2
                    && r.height.abs_diff(expected.height) < 2
                {
                    candidates.push(window);
                }
            }
        }
        if candidates.len() != 1 {
            return Err("ambiguous_window");
        }
        verify_process(ticket)?;
        Ok(candidates.remove(0))
    }
    fn same(&self, a: &AXUIElement, b: &AXUIElement) -> bool {
        a == b
    }
    fn validate(
        &mut self,
        window: &UiWindow,
        root: &AXUIElement,
        node: &AXUIElement,
    ) -> Result<(), &'static str> {
        verify_process(window)?;
        let app = AXUIElement::application(window.pid as i32);
        app.set_messaging_timeout(0.5).map_err(mapped)?;
        let (windows, truncated) = list(&app, "AXWindows", 256)?;
        if truncated || !windows.iter().any(|e| e == root) {
            return Err("stale_window");
        }
        let mut pid = 0;
        if unsafe { AXUIElementGetPid(node.as_concrete_TypeRef(), &mut pid) } != 0
            || pid != window.pid as i32
        {
            return Err("stale_element");
        }
        let mut current = node.clone();
        for _ in 0..64 {
            if &current == root {
                return Ok(());
            }
            current = attr(&current, "AXParent")?
                .downcast::<AXUIElement>()
                .ok_or("stale_element")?;
        }
        Err("element_outside_window")
    }
    fn children(
        &mut self,
        node: &AXUIElement,
        max: usize,
    ) -> Result<(Vec<AXUIElement>, bool), &'static str> {
        list(node, "AXChildren", max)
    }
    fn read(&mut self, node: &AXUIElement, include_value: bool) -> Result<UiElement, &'static str> {
        let native_role = text(node, "AXRole")?;
        let role = role(&native_role);
        let subrole = text(node, "AXSubrole").ok();
        let protected =
            subrole.as_deref() == Some("AXSecureTextField") || native_role == "AXSecureTextField";
        let mut errors = BTreeMap::new();
        let actions = node.action_names().map_err(mapped)?;
        let press = actions.iter().any(|a| a.to_string() == "AXPress");
        let mut supported = vec![];
        if press {
            supported.push(UiActionKind::Invoke);
        }
        let writable = settable(node, "AXValue");
        let is_text = role == UiRole::TextField;
        if writable && is_text && !protected {
            supported.push(UiActionKind::SetValue);
        }
        let value = if is_text && include_value && !protected {
            optional(text(node, "AXValue"), "value", &mut errors)
        } else {
            None
        };
        let checked = if role == UiRole::CheckBox {
            optional(number(node, "AXValue"), "checked", &mut errors).and_then(check)
        } else {
            None
        };
        if role == UiRole::CheckBox && (writable || press) && checked.is_some() {
            supported.push(UiActionKind::SetChecked);
        }
        let selected = if role == UiRole::RadioButton {
            optional(number(node, "AXValue"), "selected", &mut errors).and_then(|v| match v {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            })
        } else {
            optional(boolean(node, "AXSelected"), "selected", &mut errors)
        };
        if settable(node, "AXSelected")
            || (role == UiRole::RadioButton && press && selected.is_some())
        {
            supported.push(UiActionKind::Select);
        }
        let expanded = optional(boolean(node, "AXExpanded"), "expanded", &mut errors);
        if settable(node, "AXExpanded") {
            supported.extend([UiActionKind::Expand, UiActionKind::Collapse]);
        }
        if settable(node, "AXFocused") {
            supported.push(UiActionKind::Focus);
        }
        let name = text(node, "AXTitle")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| text(node, "AXDescription").ok())
            .or_else(|| text(node, "AXLabel").ok());
        Ok(UiElement {
            element_ref: String::new(),
            parent_ref: None,
            role,
            native_role,
            name,
            identifier: optional(text(node, "AXIdentifier"), "identifier", &mut errors),
            value,
            protected,
            enabled: optional(enabled(node), "enabled", &mut errors),
            visible: None,
            offscreen: None,
            focused: optional(boolean(node, "AXFocused"), "focused", &mut errors),
            read_only: if is_text { Some(!writable) } else { None },
            checked,
            selected,
            expanded,
            bounding_rect: optional(rect(node), "bounding_rect", &mut errors),
            coordinate_space: "macos_points".into(),
            supported_actions: supported,
            field_errors: errors,
        })
    }
    fn act(&mut self, node: &AXUIElement, action: &UiAction) -> Result<(), &'static str> {
        if !enabled(node)? {
            return Err("control_not_enabled");
        }
        match action {
            UiAction::Invoke => node
                .perform_action(&CFString::new("AXPress"))
                .map_err(mapped),
            UiAction::SetValue { value } => {
                if text(node, "AXSubrole").ok().as_deref() == Some("AXSecureTextField") {
                    return Err("protected_control");
                }
                set(node, "AXValue", CFString::new(value).as_CFType())
            }
            UiAction::SetChecked { checked } => {
                let current = number(node, "AXValue")?;
                let desired = i64::from(*checked);
                if current == desired {
                    return Ok(());
                }
                if settable(node, "AXValue") {
                    set(node, "AXValue", CFNumber::from(desired).as_CFType())
                } else if current == 0 || current == 1 {
                    node.perform_action(&CFString::new("AXPress"))
                        .map_err(mapped)
                } else {
                    Err("mixed_toggle_unsupported")
                }
            }
            UiAction::Select => {
                if text(node, "AXRole")? == "AXRadioButton" {
                    match number(node, "AXValue")? {
                        1 => Ok(()),
                        0 => node
                            .perform_action(&CFString::new("AXPress"))
                            .map_err(mapped),
                        _ => Err("unsupported_action"),
                    }
                } else {
                    set(node, "AXSelected", CFBoolean::true_value().as_CFType())
                }
            }
            UiAction::Expand => set(node, "AXExpanded", CFBoolean::true_value().as_CFType()),
            UiAction::Collapse => set(node, "AXExpanded", CFBoolean::false_value().as_CFType()),
            UiAction::Focus => set(node, "AXFocused", CFBoolean::true_value().as_CFType()),
        }
    }
}
