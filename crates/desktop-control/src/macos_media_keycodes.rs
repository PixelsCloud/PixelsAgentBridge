//! Physical VK -> public macOS virtual key codes for the remote media channel.
//! Do not translate through Unicode/current IME: layout lookup can choose a
//! keypad key (e.g. `1` -> keypad 1) and lose Shift punctuation or composition.
pub(crate) fn native_key(value: u16) -> Option<u16> {
    const LETTERS: [u16; 26] = [
        0, 11, 8, 2, 14, 3, 5, 4, 34, 38, 40, 37, 46, 45, 31, 35, 12, 15, 1, 17, 32, 9, 13, 7, 16,
        6,
    ];
    const DIGITS: [u16; 10] = [29, 18, 19, 20, 21, 23, 22, 26, 28, 25];
    const KEYPAD: [u16; 10] = [82, 83, 84, 85, 86, 87, 88, 89, 91, 92];
    const FUNCTIONS: [u16; 20] = [
        122, 120, 99, 118, 96, 97, 98, 100, 101, 109, 103, 111, 105, 107, 113, 106, 64, 79, 80, 90,
    ];
    Some(match value {
        65..=90 => LETTERS[usize::from(value - 65)],
        48..=57 => DIGITS[usize::from(value - 48)],
        96..=105 => KEYPAD[usize::from(value - 96)],
        112..=131 => FUNCTIONS[usize::from(value - 112)],
        8 => 51,
        9 => 48,
        13 => 36,
        16 | 160 => 56,
        161 => 60,
        17 | 162 => 59,
        163 => 62,
        18 | 164 => 58,
        165 => 61,
        20 => 57,
        27 => 53,
        32 => 49,
        33 => 116,
        34 => 121,
        35 => 119,
        36 => 115,
        37 => 123,
        38 => 126,
        39 => 124,
        40 => 125,
        45 => 114,
        46 => 117,
        91 | 93 | 224 => 55,
        92 => 54,
        106 => 67,
        107 => 69,
        109 => 78,
        110 => 65,
        111 => 75,
        144 => 71,
        186 => 41,
        187 => 24,
        188 => 43,
        189 => 27,
        190 => 47,
        191 => 44,
        192 => 50,
        219 => 33,
        220 => 42,
        221 => 30,
        222 => 39,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::native_key;

    #[test]
    fn shifted_printable_keys_keep_their_physical_positions() {
        assert_eq!(native_key(49), Some(18)); // top-row 1, not keypad 1
        assert_eq!(native_key(97), Some(83));
        for (vk, mac) in [(186, 41), (191, 44), (187, 24), (222, 39)] {
            assert_eq!(native_key(vk), Some(mac));
        }
        let top: Vec<_> = (48..=57).map(|v| native_key(v).unwrap()).collect();
        let pad: Vec<_> = (96..=105).map(|v| native_key(v).unwrap()).collect();
        assert!(top.iter().all(|code| !pad.contains(code)));
    }

    #[test]
    fn remote_ime_letters_and_sided_modifiers_are_layout_independent() {
        for (vk, mac) in [(78, 45), (73, 34), (72, 4), (65, 0), (79, 31)] {
            assert_eq!(native_key(vk), Some(mac));
        }
        for (left, right) in [(160, 161), (162, 163), (164, 165), (91, 92)] {
            assert_ne!(native_key(left), native_key(right));
            assert!(native_key(left).is_some() && native_key(right).is_some());
        }
        assert_eq!(native_key(0), None);
        assert_eq!(native_key(255), None);
    }
}
