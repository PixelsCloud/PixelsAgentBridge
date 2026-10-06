use crate::ui_engine::{UiBackend, UiWindow};
use pab_protocol::*;
use std::collections::BTreeMap;
use uiautomation::{
    UIAutomation, UIElement,
    patterns::*,
    types::{ControlType, ExpandCollapseState, ToggleState},
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

/// Declare this outside the engine so native registry objects are dropped before
/// CoUninitialize. Neither the backend nor its elements leave the worker thread.
pub struct UiApartment;
impl UiApartment {
    pub fn enter() -> Result<Self, &'static str> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|_| "com_initialization_failed")?;
        }
        Ok(Self)
    }
}
impl Drop for UiApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}
pub struct WindowsUi {
    automation: UIAutomation,
}
impl WindowsUi {
    pub fn new(_: &UiApartment) -> Result<Self, &'static str> {
        Ok(Self {
            automation: UIAutomation::new_direct().map_err(|_| "uia_unavailable")?,
        })
    }
    fn verify_window(&self, window: &UiWindow) -> Result<(), &'static str> {
        crate::native::verify(window.id, window.pid, &window.marker_key, window.marker)
            .map_err(|_| "stale_window")?;
        if pab_os_control::process_identity(window.pid).map_err(|_| "stale_window")?
            != window.process_identity
        {
            return Err("stale_window");
        }
        Ok(())
    }
}
fn err(_: uiautomation::Error) -> &'static str {
    "provider_failed"
}
fn optional<T>(
    v: uiautomation::Result<T>,
    field: &str,
    errors: &mut BTreeMap<String, String>,
) -> Option<T> {
    match v {
        Ok(v) => Some(v),
        Err(_) => {
            errors.insert(field.into(), "unavailable".into());
            None
        }
    }
}
fn role(t: ControlType) -> UiRole {
    match t {
        ControlType::Window => UiRole::Window,
        ControlType::Button => UiRole::Button,
        ControlType::Edit => UiRole::TextField,
        ControlType::Text => UiRole::Text,
        ControlType::CheckBox => UiRole::CheckBox,
        ControlType::RadioButton => UiRole::RadioButton,
        ControlType::List => UiRole::List,
        ControlType::ListItem => UiRole::ListItem,
        ControlType::ComboBox => UiRole::ComboBox,
        ControlType::Menu => UiRole::Menu,
        ControlType::MenuItem => UiRole::MenuItem,
        ControlType::Group => UiRole::Group,
        ControlType::Tab => UiRole::Tab,
        ControlType::TabItem => UiRole::TabItem,
        ControlType::Tree => UiRole::Tree,
        ControlType::TreeItem => UiRole::TreeItem,
        _ => UiRole::Unknown,
    }
}
fn checked(v: ToggleState) -> UiCheckState {
    match v {
        ToggleState::On => UiCheckState::On,
        ToggleState::Off => UiCheckState::Off,
        ToggleState::Indeterminate => UiCheckState::Mixed,
    }
}

impl UiBackend for WindowsUi {
    type Element = UIElement;
    fn root(&mut self, window: &UiWindow) -> Result<UIElement, &'static str> {
        self.verify_window(window)?;
        let root = self
            .automation
            .element_from_handle((window.id as i32 as isize).into())
            .map_err(err)?;
        if root.get_process_id().map_err(err)? != window.pid {
            return Err("stale_window");
        }
        Ok(root)
    }
    fn same(&self, a: &UIElement, b: &UIElement) -> bool {
        self.automation.compare_elements(a, b).unwrap_or(false)
    }
    fn validate(
        &mut self,
        window: &UiWindow,
        root: &UIElement,
        node: &UIElement,
    ) -> Result<(), &'static str> {
        self.verify_window(window)?;
        let current = self
            .automation
            .element_from_handle((window.id as i32 as isize).into())
            .map_err(err)?;
        if !self.same(root, &current) || node.get_process_id().map_err(err)? != window.pid {
            return Err("stale_element");
        }
        let walker = self.automation.get_control_view_walker().map_err(err)?;
        let mut current = node.clone();
        for _ in 0..64 {
            if self.same(&current, root) {
                return Ok(());
            }
            current = walker.get_parent(&current).map_err(|_| "stale_element")?;
        }
        Err("element_outside_window")
    }
    fn children(
        &mut self,
        node: &UIElement,
        max: usize,
    ) -> Result<(Vec<UIElement>, bool), &'static str> {
        let walker = self.automation.get_control_view_walker().map_err(err)?;
        let mut next = walk_child(&walker, node, false)?;
        let mut children = vec![];
        loop {
            let Some(child) = next else {
                return Ok((children, false));
            };
            if children.len() >= max {
                return Ok((children, true));
            }
            if children.iter().any(|old| self.same(old, &child)) {
                return Err("provider_cycle");
            }
            next = walk_child(&walker, &child, true)?;
            children.push(child);
        }
    }

    fn read(&mut self, node: &UIElement, include_value: bool) -> Result<UiElement, &'static str> {
        let t = node.get_control_type().map_err(err)?;
        let mut errors = BTreeMap::new();
        let protected = optional(node.is_password(), "protected", &mut errors).unwrap_or(true);
        let mut actions = vec![];
        if node.get_pattern::<UIInvokePattern>().is_ok() {
            actions.push(UiActionKind::Invoke);
        }
        let value_pattern = node.get_pattern::<UIValuePattern>().ok();
        let read_only = value_pattern
            .as_ref()
            .and_then(|p| optional(p.is_readonly(), "read_only", &mut errors));
        if value_pattern.is_some() && read_only == Some(false) && !protected {
            actions.push(UiActionKind::SetValue);
        }
        let value = if include_value && !protected {
            value_pattern
                .as_ref()
                .and_then(|p| optional(p.get_value(), "value", &mut errors))
        } else {
            None
        };
        let toggle = node.get_pattern::<UITogglePattern>().ok();
        let checked = toggle
            .as_ref()
            .and_then(|p| optional(p.get_toggle_state(), "checked", &mut errors))
            .map(checked);
        if toggle.is_some() {
            actions.push(UiActionKind::SetChecked);
        }
        let selection = node.get_pattern::<UISelectionItemPattern>().ok();
        let selected = selection
            .as_ref()
            .and_then(|p| optional(p.is_selected(), "selected", &mut errors));
        if selection.is_some() {
            actions.push(UiActionKind::Select);
        }
        let expansion = node.get_pattern::<UIExpandCollapsePattern>().ok();
        let expanded = expansion
            .as_ref()
            .and_then(|p| optional(p.get_state(), "expanded", &mut errors))
            .and_then(|v| match v {
                ExpandCollapseState::Expanded => Some(true),
                ExpandCollapseState::Collapsed => Some(false),
                _ => None,
            });
        if expanded.is_some() {
            actions.extend([UiActionKind::Expand, UiActionKind::Collapse]);
        }
        if node.is_keyboard_focusable().unwrap_or(false) {
            actions.push(UiActionKind::Focus);
        }
        let rect =
            optional(node.get_bounding_rectangle(), "bounding_rect", &mut errors).map(|r| UiRect {
                x: r.get_left(),
                y: r.get_top(),
                width: r.get_right().saturating_sub(r.get_left()).max(0) as u32,
                height: r.get_bottom().saturating_sub(r.get_top()).max(0) as u32,
            });
        Ok(UiElement {
            element_ref: String::new(),
            parent_ref: None,
            role: role(t),
            native_role: format!("{t:?}"),
            name: optional(node.get_name(), "name", &mut errors),
            identifier: optional(node.get_automation_id(), "identifier", &mut errors),
            value,
            protected,
            enabled: optional(node.is_enabled(), "enabled", &mut errors),
            visible: None,
            offscreen: optional(node.is_offscreen(), "offscreen", &mut errors),
            focused: optional(node.has_keyboard_focus(), "focused", &mut errors),
            read_only,
            checked,
            selected,
            expanded,
            bounding_rect: rect,
            coordinate_space: "windows_physical_pixels".into(),
            supported_actions: actions,
            field_errors: errors,
        })
    }
    fn act(&mut self, node: &UIElement, action: &UiAction) -> Result<(), &'static str> {
        if !node.is_enabled().map_err(err)? {
            return Err("control_not_enabled");
        }
        match action {
            UiAction::Invoke => node
                .get_pattern::<UIInvokePattern>()
                .map_err(err)?
                .invoke()
                .map_err(err),
            UiAction::SetValue { value } => {
                if node.is_password().map_err(err)? {
                    return Err("protected_control");
                }
                let p = node.get_pattern::<UIValuePattern>().map_err(err)?;
                if p.is_readonly().map_err(err)? {
                    return Err("value_not_writable");
                }
                p.set_value(value).map_err(err)
            }
            UiAction::SetChecked { checked: desired } => {
                let p = node.get_pattern::<UITogglePattern>().map_err(err)?;
                let current = checked(p.get_toggle_state().map_err(err)?);
                let desired = if *desired {
                    UiCheckState::On
                } else {
                    UiCheckState::Off
                };
                if current == desired {
                    return Ok(());
                }
                // Mixed-state cycles are provider-defined. Never blindly cycle.
                if current == UiCheckState::Mixed {
                    return Err("mixed_toggle_unsupported");
                }
                p.toggle().map_err(err)
            }
            UiAction::Select => node
                .get_pattern::<UISelectionItemPattern>()
                .map_err(err)?
                .select()
                .map_err(err),
            UiAction::Expand => node
                .get_pattern::<UIExpandCollapsePattern>()
                .map_err(err)?
                .expand()
                .map_err(err),
            UiAction::Collapse => node
                .get_pattern::<UIExpandCollapsePattern>()
                .map_err(err)?
                .collapse()
                .map_err(err),
            UiAction::Focus => node.set_focus().map_err(err),
        }
    }
}

// The high-level windows wrapper maps successful NULL interface outputs into an
// error. Inspect the official COM HRESULT and nullable output separately, so an
// empty child list never swallows an actual provider failure.
fn walk_child(
    walker: &uiautomation::core::UITreeWalker,
    node: &UIElement,
    sibling: bool,
) -> Result<Option<UIElement>, &'static str> {
    use windows::{
        Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTreeWalker},
        core::Interface,
    };
    let walker: &IUIAutomationTreeWalker = walker.as_ref();
    let node: &IUIAutomationElement = node.as_ref();
    let mut output = std::ptr::null_mut();
    // SAFETY: both COM interfaces are retained on this MTA thread. Output is a
    // writable nullable interface pointer; successful nonnull output owns +1 ref.
    let status = unsafe {
        let vtable = Interface::vtable(walker);
        let call = if sibling {
            vtable.GetNextSiblingElement
        } else {
            vtable.GetFirstChildElement
        };
        call(
            Interface::as_raw(walker),
            Interface::as_raw(node),
            &mut output,
        )
    };
    let result = if output.is_null() {
        None
    } else {
        Some(unsafe { IUIAutomationElement::from_raw(output) })
    };
    status.ok().map_err(|_| "provider_failed")?;
    Ok(result.map(UIElement::from))
}
