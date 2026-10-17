#pragma once
#include "common.h"
#include <linux/input-event-codes.h>

namespace pab_linux {
// Existing wire keys are Windows VK values. Translate physical keys, preserving
// the target compositor's layout and modifier handling; never log typed data.
inline unsigned evdev_key(unsigned vk) {
    static constexpr unsigned letters[]{KEY_A,KEY_B,KEY_C,KEY_D,KEY_E,KEY_F,KEY_G,
        KEY_H,KEY_I,KEY_J,KEY_K,KEY_L,KEY_M,KEY_N,KEY_O,KEY_P,KEY_Q,KEY_R,KEY_S,
        KEY_T,KEY_U,KEY_V,KEY_W,KEY_X,KEY_Y,KEY_Z};
    static constexpr unsigned digits[]{KEY_0,KEY_1,KEY_2,KEY_3,KEY_4,KEY_5,KEY_6,KEY_7,KEY_8,KEY_9};
    static constexpr unsigned keypad[]{KEY_KP0,KEY_KP1,KEY_KP2,KEY_KP3,KEY_KP4,KEY_KP5,KEY_KP6,KEY_KP7,KEY_KP8,KEY_KP9};
    if (vk >= 0x41 && vk <= 0x5a) return letters[vk - 0x41];
    if (vk >= 0x30 && vk <= 0x39) return digits[vk - 0x30];
    if (vk >= 0x60 && vk <= 0x69) return keypad[vk - 0x60];
    if (vk >= 0x70 && vk <= 0x79) return KEY_F1 + vk - 0x70;
    if (vk >= 0x7a && vk <= 0x7b) return KEY_F11 + vk - 0x7a;
    if (vk >= 0x7c && vk <= 0x87) return KEY_F13 + vk - 0x7c;
    switch (vk) {
    case 0x08:return KEY_BACKSPACE; case 0x09:return KEY_TAB; case 0x0d:return KEY_ENTER;
    case 0x10:case 0xa0:return KEY_LEFTSHIFT; case 0xa1:return KEY_RIGHTSHIFT;
    case 0x11:case 0xa2:return KEY_LEFTCTRL; case 0xa3:return KEY_RIGHTCTRL;
    case 0x12:case 0xa4:return KEY_LEFTALT; case 0xa5:return KEY_RIGHTALT;
    case 0x13:return KEY_PAUSE; case 0x14:return KEY_CAPSLOCK; case 0x1b:return KEY_ESC;
    case 0x20:return KEY_SPACE; case 0x21:return KEY_PAGEUP; case 0x22:return KEY_PAGEDOWN;
    case 0x23:return KEY_END; case 0x24:return KEY_HOME; case 0x25:return KEY_LEFT;
    case 0x26:return KEY_UP; case 0x27:return KEY_RIGHT; case 0x28:return KEY_DOWN;
    case 0x2c:return KEY_SYSRQ; case 0x2d:return KEY_INSERT; case 0x2e:return KEY_DELETE;
    case 0x5b:return KEY_LEFTMETA; case 0x5c:return KEY_RIGHTMETA; case 0x5d:return KEY_COMPOSE;
    case 0x6a:return KEY_KPASTERISK; case 0x6b:return KEY_KPPLUS; case 0x6d:return KEY_KPMINUS;
    case 0x6e:return KEY_KPDOT; case 0x6f:return KEY_KPSLASH;
    case 0x90:return KEY_NUMLOCK; case 0x91:return KEY_SCROLLLOCK;
    case 0xba:return KEY_SEMICOLON; case 0xbb:return KEY_EQUAL; case 0xbc:return KEY_COMMA;
    case 0xbd:return KEY_MINUS; case 0xbe:return KEY_DOT; case 0xbf:return KEY_SLASH;
    case 0xc0:return KEY_GRAVE; case 0xdb:return KEY_LEFTBRACE; case 0xdc:return KEY_BACKSLASH;
    case 0xdd:return KEY_RIGHTBRACE; case 0xde:return KEY_APOSTROPHE; case 0xe2:return KEY_102ND;
    default:throw std::runtime_error("Unsupported remote desktop key");
    }
}
}
