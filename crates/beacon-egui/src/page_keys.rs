//! egui's keyboard events as the engine's `KeyDown` / `KeyUp` / `TextInput`.
//!
//! The contract is the one GTK and the Mac shell follow: a press that types a character is one
//! `KeyDown` carrying that character, a press that types nothing is a `KeyDown` with the key's
//! web name (`Backspace`, `ArrowLeft`), and `TextInput` is for text that is not a single press:
//! a committed composition, several characters at once, a paste. egui-winit reports a typing
//! press twice, as a `Key` and then as its `Text`; the `Text` is the one sent.

use egui::{Event, Key};
use gosub_engine::events::{Modifiers, TabCommand};

/// What the page gets for this frame's `events`, in order.
pub fn commands(events: &[Event]) -> Vec<TabCommand> {
    let mut out = Vec::new();
    for event in events {
        match event {
            Event::Key {
                key, pressed, modifiers, ..
            } => {
                let chord = modifiers.ctrl || modifiers.alt || modifiers.command || modifiers.mac_cmd;
                // Without a modifier a character key is typing, and its `Text` follows. With one
                // there is no `Text` (Ctrl+A types nothing), so the key itself is the press.
                let name = match named(*key) {
                    Some(name) => name.to_string(),
                    None if chord || !pressed => match character(*key, modifiers.shift) {
                        Some(c) => c,
                        None => continue,
                    },
                    None => continue,
                };
                let modifiers = engine_modifiers(*modifiers);
                out.push(if *pressed {
                    TabCommand::KeyDown {
                        key: name.clone(),
                        code: name,
                        modifiers,
                    }
                } else {
                    TabCommand::KeyUp {
                        key: name.clone(),
                        code: name,
                        modifiers,
                    }
                });
            }
            Event::Text(text) if text.chars().count() == 1 => out.push(TabCommand::KeyDown {
                key: text.clone(),
                code: text.clone(),
                modifiers: Modifiers::empty(),
            }),
            Event::Text(text) | Event::Paste(text) | Event::Ime(egui::ImeEvent::Commit(text)) if !text.is_empty() => {
                out.push(TabCommand::TextInput { text: text.clone() })
            }
            // egui-winit turns these chords into events of their own and drops the key; the
            // engine copies and cuts on the chord.
            Event::Copy => out.push(chord('c')),
            Event::Cut => out.push(chord('x')),
            _ => {}
        }
    }
    out
}

/// Ctrl and `c`, as a keyboard sends it.
pub fn chord(c: char) -> TabCommand {
    TabCommand::KeyDown {
        key: c.to_string(),
        code: c.to_string(),
        modifiers: Modifiers::CONTROL,
    }
}

/// The web name of a key that types nothing.
fn named(key: Key) -> Option<&'static str> {
    Some(match key {
        Key::Enter => "Enter",
        Key::Tab => "Tab",
        Key::Escape => "Escape",
        Key::Backspace => "Backspace",
        Key::Delete => "Delete",
        Key::ArrowUp => "ArrowUp",
        Key::ArrowDown => "ArrowDown",
        Key::ArrowLeft => "ArrowLeft",
        Key::ArrowRight => "ArrowRight",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::Home => "Home",
        Key::End => "End",
        _ => return None,
    })
}

/// The character a letter, digit or Space stands for, for a chord or a release. egui names keys
/// by their unshifted symbol, so only letters can be shifted here.
fn character(key: Key, shift: bool) -> Option<String> {
    if key == Key::Space {
        return Some(" ".into());
    }
    let mut chars = key.symbol_or_name().chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return None;
    };
    if !c.is_ascii_alphanumeric() {
        return None;
    }
    Some(if shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() }.to_string())
}

fn engine_modifiers(m: egui::Modifiers) -> Modifiers {
    let mut out = Modifiers::empty();
    if m.shift {
        out |= Modifiers::SHIFT;
    }
    if m.ctrl {
        out |= Modifiers::CONTROL;
    }
    if m.alt {
        out |= Modifiers::ALT;
    }
    if m.mac_cmd {
        out |= Modifiers::META;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, pressed: bool, modifiers: egui::Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    fn down(key: &str, modifiers: Modifiers) -> TabCommand {
        TabCommand::KeyDown {
            key: key.into(),
            code: key.into(),
            modifiers,
        }
    }

    fn up(key: &str, modifiers: Modifiers) -> TabCommand {
        TabCommand::KeyUp {
            key: key.into(),
            code: key.into(),
            modifiers,
        }
    }

    // TabCommand has no PartialEq; its Debug form is compared instead.
    fn same(got: Vec<TabCommand>, want: Vec<TabCommand>) {
        assert_eq!(format!("{got:?}"), format!("{want:?}"));
    }

    #[test]
    fn a_typed_letter_is_one_key_down_with_its_character() {
        let shift = egui::Modifiers::SHIFT;
        same(
            commands(&[key(Key::A, true, shift), Event::Text("A".into()), key(Key::A, false, shift)]),
            vec![down("A", Modifiers::empty()), up("A", Modifiers::SHIFT)],
        );
    }

    #[test]
    fn a_key_that_types_nothing_goes_by_its_web_name() {
        let none = egui::Modifiers::NONE;
        same(
            commands(&[
                key(Key::Backspace, true, none),
                key(Key::Backspace, false, none),
                key(Key::ArrowLeft, true, egui::Modifiers::SHIFT),
                key(Key::Enter, true, none),
            ]),
            vec![
                down("Backspace", Modifiers::empty()),
                up("Backspace", Modifiers::empty()),
                down("ArrowLeft", Modifiers::SHIFT),
                down("Enter", Modifiers::empty()),
            ],
        );
    }

    #[test]
    fn space_is_typed_once() {
        let none = egui::Modifiers::NONE;
        same(
            commands(&[key(Key::Space, true, none), Event::Text(" ".into()), key(Key::Space, false, none)]),
            vec![down(" ", Modifiers::empty()), up(" ", Modifiers::empty())],
        );
    }

    #[test]
    fn a_chord_is_its_key_with_the_modifier() {
        same(
            commands(&[key(Key::A, true, egui::Modifiers::CTRL), Event::Copy, Event::Cut]),
            vec![down("a", Modifiers::CONTROL), chord('c'), chord('x')],
        );
    }

    #[test]
    fn several_characters_at_once_are_text() {
        same(
            commands(&[
                Event::Text("hé".into()),
                Event::Paste("pasted".into()),
                Event::Ime(egui::ImeEvent::Commit("日本".into())),
                Event::Paste(String::new()),
            ]),
            vec![
                TabCommand::TextInput { text: "hé".into() },
                TabCommand::TextInput { text: "pasted".into() },
                TabCommand::TextInput { text: "日本".into() },
            ],
        );
    }
}
