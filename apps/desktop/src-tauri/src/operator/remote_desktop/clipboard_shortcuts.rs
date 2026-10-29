use pab_protocol::RemoteDesktopInput;
#[derive(Default)]
pub(crate) struct Shortcuts {
    held: std::collections::HashSet<u16>,
    swallowed: std::collections::HashSet<u16>,
}
pub(crate) struct PasteChord {
    key: u16,
    modifiers: Vec<u16>,
}
impl Shortcuts {
    pub(crate) fn swallow(&mut self, event: &RemoteDesktopInput) -> bool {
        if let RemoteDesktopInput::Key { virtual_key, down } = event {
            if self.swallowed.contains(virtual_key) {
                if !down {
                    self.swallowed.remove(virtual_key);
                }
                return true;
            }
        }
        false
    }
    pub(crate) fn begin(&mut self, event: &RemoteDesktopInput) -> PasteChord {
        let RemoteDesktopInput::Key { virtual_key, .. } = event else {
            unreachable!()
        };
        self.swallowed.insert(*virtual_key);
        PasteChord {
            key: *virtual_key,
            modifiers: self
                .held
                .iter()
                .copied()
                .filter(|k| is_modifier(*k))
                .collect(),
        }
    }
    pub(crate) fn replay(&self, chord: PasteChord) -> Vec<RemoteDesktopInput> {
        let added: Vec<_> = chord
            .modifiers
            .into_iter()
            .filter(|k| !self.held.contains(k))
            .collect();
        added
            .iter()
            .map(|k| RemoteDesktopInput::Key {
                virtual_key: *k,
                down: true,
            })
            .chain([
                RemoteDesktopInput::Key {
                    virtual_key: chord.key,
                    down: true,
                },
                RemoteDesktopInput::Key {
                    virtual_key: chord.key,
                    down: false,
                },
            ])
            .chain(added.iter().rev().map(|k| RemoteDesktopInput::Key {
                virtual_key: *k,
                down: false,
            }))
            .collect()
    }
    pub(crate) fn cancels_paste(event: &RemoteDesktopInput) -> bool {
        match event {
            RemoteDesktopInput::Key { down, .. } => *down,
            RemoteDesktopInput::Move { .. } => false,
            _ => true,
        }
    }
    pub(crate) fn observe(&mut self, event: &RemoteDesktopInput) -> (bool, bool) {
        match event {
            RemoteDesktopInput::ReleaseAll => {
                self.held.clear();
                self.swallowed.clear();
                (false, false)
            }
            RemoteDesktopInput::Key { virtual_key, down } => {
                let shortcut = [0x11, 0xa2, 0xa3, 0x5b, 0x5c]
                    .iter()
                    .any(|key| self.held.contains(key));
                let shift = [0x10, 0xa0, 0xa1].iter().any(|key| self.held.contains(key));
                let paste = *down
                    && !self.held.contains(virtual_key)
                    && ((*virtual_key == 0x56 && shortcut) || (*virtual_key == 0x2d && shift));
                let copy = !*down && shortcut && matches!(*virtual_key, 0x43 | 0x58);
                if *down {
                    self.held.insert(*virtual_key);
                } else {
                    self.held.remove(virtual_key);
                }
                (paste, copy)
            }
            _ => (false, false),
        }
    }
}

fn is_modifier(key: u16) -> bool {
    matches!(key,0x10..=0x12|0x5b|0x5c|0xa0..=0xa5)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(virtual_key: u16, down: bool) -> RemoteDesktopInput {
        RemoteDesktopInput::Key { virtual_key, down }
    }
    #[test]
    fn delayed_paste_swallows_repeat_and_releases_only_synthesized_modifiers() {
        let mut keys = Shortcuts::default();
        keys.observe(&key(0xa2, true));
        assert!(keys.observe(&key(0x56, true)).0);
        let chord = keys.begin(&key(0x56, true));
        assert!(!keys.observe(&key(0x56, true)).0);
        assert!(keys.swallow(&key(0x56, true)));
        keys.observe(&key(0x56, false));
        assert!(keys.swallow(&key(0x56, false)));
        keys.observe(&key(0xa2, false));
        assert_eq!(
            keys.replay(chord),
            vec![
                key(0xa2, true),
                key(0x56, true),
                key(0x56, false),
                key(0xa2, false)
            ]
        );
        keys.observe(&key(0xa2, true));
        keys.observe(&key(0x56, true));
        let chord = keys.begin(&key(0x56, true));
        assert_eq!(keys.replay(chord), vec![key(0x56, true), key(0x56, false)]);
    }
    #[test]
    fn shift_insert_and_cancellation_never_replay_into_new_target() {
        let mut keys = Shortcuts::default();
        keys.observe(&key(0xa0, true));
        assert!(keys.observe(&key(0x2d, true)).0);
        let chord = keys.begin(&key(0x2d, true));
        assert!(!Shortcuts::cancels_paste(&RemoteDesktopInput::Move {
            x: 10,
            y: 20
        }));
        assert!(!Shortcuts::cancels_paste(&key(0xa0, false)));
        assert!(Shortcuts::cancels_paste(&key(65, true)));
        assert!(Shortcuts::cancels_paste(&key(0xa0, true)));
        assert!(Shortcuts::cancels_paste(&RemoteDesktopInput::ReleaseAll));
        assert!(Shortcuts::cancels_paste(&RemoteDesktopInput::Wheel {
            delta: 1
        }));
        drop(chord);
        assert!(keys.swallow(&key(0x2d, false)));
    }
    #[test]
    fn focus_reset_does_not_swallow_the_next_paste_when_key_up_was_lost() {
        let mut keys = Shortcuts::default();
        keys.observe(&key(0x11, true));
        keys.observe(&key(0x56, true));
        let _cancelled = keys.begin(&key(0x56, true));
        keys.observe(&RemoteDesktopInput::ReleaseAll);
        keys.observe(&key(0x11, true));
        assert!(keys.observe(&key(0x56, true)).0);
        assert!(!keys.swallow(&key(0x56, true)));
    }
}
