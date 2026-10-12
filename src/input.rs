//! One-line text fields (the rename field). GPUI has no text input; these take keys
//! while focused: typing appends, Backspace deletes, Cmd-Backspace clears. No cursor movement.

use gpui::Keystroke;

#[derive(Debug, PartialEq, Eq)]
pub enum Edit {
    /// The text changed.
    Changed,
    /// Enter; `shift` held.
    Submit {
        shift: bool,
    },
    /// Escape.
    Cancel,
    Up,
    Down,
    /// Not for the field (a Cmd shortcut, an unmapped key).
    Ignored,
}

/// Applies one key press to `text`.
pub fn apply(text: &mut String, keystroke: &Keystroke) -> Edit {
    let modifiers = &keystroke.modifiers;
    match keystroke.key.as_str() {
        "escape" => Edit::Cancel,
        "enter" => Edit::Submit {
            shift: modifiers.shift,
        },
        "up" => Edit::Up,
        "down" => Edit::Down,
        "backspace" if modifiers.platform => {
            text.clear();
            Edit::Changed
        }
        "backspace" => {
            text.pop();
            Edit::Changed
        }
        _ if modifiers.platform || modifiers.control => Edit::Ignored,
        _ => match keystroke.key_char.as_deref() {
            Some(typed) if !typed.chars().any(char::is_control) => {
                text.push_str(typed);
                Edit::Changed
            }
            _ => Edit::Ignored,
        },
    }
}

#[cfg(test)]
mod tests {
    use gpui::Modifiers;

    use super::*;

    fn key(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: key.into(),
            key_char: key_char.map(String::from),
        }
    }

    #[test]
    fn keys_edit_one_line() {
        let none = Modifiers::default();
        let cmd = Modifiers {
            platform: true,
            ..Default::default()
        };
        let mut text = String::new();
        assert_eq!(apply(&mut text, &key("a", Some("a"), none)), Edit::Changed);
        assert_eq!(apply(&mut text, &key("b", Some("B"), none)), Edit::Changed);
        assert_eq!(text, "aB");
        assert_eq!(
            apply(&mut text, &key("backspace", None, none)),
            Edit::Changed
        );
        assert_eq!(text, "a");
        assert_eq!(apply(&mut text, &key("v", Some("v"), cmd)), Edit::Ignored);
        assert_eq!(
            apply(&mut text, &key("backspace", None, cmd)),
            Edit::Changed
        );
        assert_eq!(text, "");
        assert_eq!(
            apply(
                &mut text,
                &key(
                    "enter",
                    None,
                    Modifiers {
                        shift: true,
                        ..Default::default()
                    }
                )
            ),
            Edit::Submit { shift: true }
        );
        assert_eq!(apply(&mut text, &key("escape", None, none)), Edit::Cancel);
    }
}
