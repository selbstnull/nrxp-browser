//! winit → keyboard-types: то, что Servo ожидает от встраивателя.

use keyboard_types::{Code, Key, KeyState, KeyboardEvent, Location, Modifiers, NamedKey};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key as WKey, KeyLocation, ModifiersState, PhysicalKey};

pub fn modifiers(state: ModifiersState) -> Modifiers {
    let mut out = Modifiers::empty();
    out.set(Modifiers::SHIFT, state.shift_key());
    out.set(Modifiers::CONTROL, state.control_key());
    out.set(Modifiers::ALT, state.alt_key());
    out.set(Modifiers::META, state.super_key());
    out
}

pub fn keyboard_event(event: &KeyEvent, mods: ModifiersState) -> KeyboardEvent {
    // Имена `winit::keyboard::NamedKey`/`KeyCode` совпадают с W3C-именами, поэтому разбираем по Debug-строке.
    let key = match &event.logical_key {
        WKey::Character(text) => Key::Character(text.to_string()),
        WKey::Named(winit::keyboard::NamedKey::Space) => Key::Character(" ".into()),
        WKey::Named(named) => format!("{named:?}")
            .parse::<NamedKey>()
            .map(Key::Named)
            .unwrap_or(Key::Named(NamedKey::Unidentified)),
        WKey::Dead(_) => Key::Named(NamedKey::Dead),
        WKey::Unidentified(_) => Key::Named(NamedKey::Unidentified),
    };
    let code = match event.physical_key {
        PhysicalKey::Code(code) => format!("{code:?}")
            .parse::<Code>()
            .unwrap_or(Code::Unidentified),
        PhysicalKey::Unidentified(_) => Code::Unidentified,
    };
    KeyboardEvent {
        state: match event.state {
            ElementState::Pressed => KeyState::Down,
            ElementState::Released => KeyState::Up,
        },
        key,
        code,
        location: match event.location {
            KeyLocation::Standard => Location::Standard,
            KeyLocation::Left => Location::Left,
            KeyLocation::Right => Location::Right,
            KeyLocation::Numpad => Location::Numpad,
        },
        modifiers: modifiers(mods),
        repeat: event.repeat,
        is_composing: false,
    }
}
