use crate::{DesktopMouseButton, MAX_DESKTOP_TEXT_BYTES, WindowControlAction};
use serde::{Deserialize, Serialize};

pub const MAX_DESKTOP_BATCH_ACTIONS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopAction {
    Focus {},
    Control {
        control: WindowControlAction,
    },
    TypeText {
        text: String,
    },
    /// Coordinates relative to the outer window rectangle observed at execution time.
    Click {
        x: u16,
        y: u16,
        button: DesktopMouseButton,
    },
    KeyChord {
        modifiers: Vec<DesktopModifier>,
        key: String,
    },
    Scroll {
        axis: DesktopScrollAxis,
        amount: i16,
    },
    Wait {
        ms: u32,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopModifier {
    Control,
    Alt,
    Shift,
    Meta,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopScrollAxis {
    Horizontal,
    Vertical,
}

impl DesktopAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Focus {} => "focus",
            Self::Control { .. } => "control",
            Self::TypeText { .. } => "type_text",
            Self::Click { .. } => "click",
            Self::KeyChord { .. } => "key_chord",
            Self::Scroll { .. } => "scroll",
            Self::Wait { .. } => "wait",
        }
    }
}
pub fn valid_desktop_key(key: &str) -> bool {
    (key.len() == 1
        && key
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
        || matches!(
            key,
            "enter"
                | "tab"
                | "escape"
                | "space"
                | "backspace"
                | "delete"
                | "left"
                | "right"
                | "up"
                | "down"
                | "home"
                | "end"
                | "page_up"
                | "page_down"
        )
        || key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=12).contains(&n) && key == format!("f{n}"))
}
pub fn validate_desktop_batch(
    actions: &[DesktopAction],
    timeout_ms: u32,
) -> Result<(), &'static str> {
    if actions.is_empty()
        || actions.len() > MAX_DESKTOP_BATCH_ACTIONS
        || !(100..=10_000).contains(&timeout_ms)
    {
        return Err("batch requires 1..32 actions and timeout_ms 100..10000");
    }
    let mut bytes = 0;
    let mut waits = 0;
    for action in actions {
        match action {
            DesktopAction::TypeText { text } => {
                bytes += text.len();
                if text.is_empty() || text.contains('\0') || bytes > MAX_DESKTOP_TEXT_BYTES {
                    return Err(
                        "batch text must be nonempty without NUL, at most 4096 UTF-8 bytes total",
                    );
                }
            }
            DesktopAction::KeyChord { modifiers, key }
                if modifiers.len() > 4
                    || modifiers
                        .iter()
                        .enumerate()
                        .any(|(i, m)| modifiers[..i].contains(m))
                    || !valid_desktop_key(key) =>
            {
                return Err("key_chord requires unique modifiers and a supported lowercase key");
            }
            DesktopAction::Scroll { amount, .. }
                if *amount == 0 || !(-100..=100).contains(amount) =>
            {
                return Err("scroll amount must be -100..100 and nonzero");
            }
            DesktopAction::Wait { ms } => {
                if *ms == 0 || *ms > 2000 {
                    return Err("wait ms must be 1..2000");
                }
                waits += *ms;
            }
            _ => {}
        }
    }
    if waits >= timeout_ms {
        return Err("total waits must be less than batch timeout_ms");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopBatchStep {
    pub index: u32,
    pub kind: String,
    pub state: String,
    pub action_started: bool,
    pub verification: Option<String>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopBatchReport {
    pub steps: Vec<DesktopBatchStep>,
    pub completed_steps: u32,
    pub failed_step: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;
    #[test]
    fn validates_complete_batch_before_dispatch_and_bounds_utf8_and_waits() {
        assert!(validate_desktop_batch(&[], 5000).is_err());
        assert!(validate_desktop_batch(&vec![DesktopAction::Focus {}; 33], 5000).is_err());
        assert!(validate_desktop_batch(&[DesktopAction::Wait { ms: 100 }], 100).is_err());
        assert!(validate_desktop_batch(&[DesktopAction::Wait { ms: 2001 }], 5000).is_err());
        assert!(validate_desktop_batch(&[DesktopAction::Focus {}], 10_001).is_err());
        let mut actions = vec![DesktopAction::TypeText {
            text: "中".repeat(1365),
        }];
        assert!(validate_desktop_batch(&actions, 5000).is_ok());
        actions.push(DesktopAction::TypeText { text: "ok".into() });
        assert!(validate_desktop_batch(&actions, 5000).is_err());
        for modifiers in [
            vec![DesktopModifier::Control; 2],
            vec![DesktopModifier::Alt; 5],
        ] {
            assert!(
                validate_desktop_batch(
                    &[DesktopAction::KeyChord {
                        modifiers,
                        key: "a".into()
                    }],
                    5000
                )
                .is_err()
            );
        }
        for key in ["A", "f01", "f13", "control", "中", ""] {
            assert!(!valid_desktop_key(key));
        }
        for key in ["a", "0", "f1", "f12", "page_up", "enter"] {
            assert!(valid_desktop_key(key));
        }
        assert!(
            serde_json::from_str::<DesktopAction>(r#"{"type":"focus","text":"ignored"}"#).is_err()
        );
        assert!(serde_json::from_str::<DesktopAction>(r#"{"type":"batch","actions":[]}"#).is_err());
    }
    #[test]
    fn batch_version_privacy_and_old_wire_format_are_preserved() {
        let reference = RequestId::new().to_string();
        let old = DesktopQuery::Focus {
            window_ref: reference.clone(),
        };
        assert_eq!(
            serde_json::to_value(&old).unwrap(),
            serde_json::json!({"operation":"focus","window_ref":reference})
        );
        assert_eq!(old.required_helper_version(), 1);
        assert_eq!(SystemQuery::Desktop { query: old }.required_version(), 4);
        let q = DesktopQuery::Batch {
            window_ref: reference,
            actions: vec![
                DesktopAction::TypeText {
                    text: "private秘密".into(),
                },
                DesktopAction::KeyChord {
                    modifiers: vec![],
                    key: "x".into(),
                },
            ],
            timeout_ms: 5000,
        };
        assert_eq!(q.required_helper_version(), 2);
        let system = SystemQuery::Desktop { query: q };
        assert_eq!(system.required_version(), 7);
        let saved = serde_json::to_string(&system.persistence_form()).unwrap();
        assert!(!saved.contains("private"));
        assert!(!saved.contains(r#""key":"x""#));
        assert_eq!(saved.matches("blake3:").count(), 2);
        assert_eq!(
            serde_json::from_str::<SystemQuery>(&saved).unwrap(),
            system.persistence_form()
        );
        let old_snapshot =
            serde_json::to_value(DesktopSnapshot::new("old".into(), "test")).unwrap();
        assert!(old_snapshot.get("batch").is_none());
        assert!(
            serde_json::from_value::<DesktopSnapshot>(old_snapshot)
                .unwrap()
                .batch
                .is_none()
        );
    }
}
