// Raw input events -> held movement keys, mouse look and one-shot commands. The platform layer maps nothing itself
// (pathon-platform: "raw input only"), so the key names are what winit reports: characters as typed and `Named(...)`.

use std::collections::HashSet;

use pathon::platform::{ElementState, InputEvent, MouseButton};

use super::camera::MoveInput;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    /// Number keys: the n-th camera of the file (0 is the first).
    SwitchCamera(usize),
    ExposureUp,
    ExposureDown,
    AmbientUp,
    AmbientDown,
    ToggleShadows,
    ToggleAutoExposure,
    ToggleBloom,
    PrintInfo,
    Exit,
}

#[derive(Debug, Default)]
pub struct Controls {
    held: HashSet<String>,
    looking: bool,
    last_cursor: Option<(f64, f64)>,
    look_delta: (f32, f32),
    wheel: f32,
}

fn command_for(key: &str) -> Option<Command> {
    match key {
        "+" | "=" => Some(Command::ExposureUp),
        "-" | "_" => Some(Command::ExposureDown),
        "]" => Some(Command::AmbientUp),
        "[" => Some(Command::AmbientDown),
        "t" => Some(Command::ToggleShadows),
        "x" => Some(Command::ToggleAutoExposure),
        "b" => Some(Command::ToggleBloom),
        "i" => Some(Command::PrintInfo),
        "Named(Escape)" => Some(Command::Exit),
        digit => {
            let number: usize = digit.parse().ok().filter(|number| (1..=9).contains(number))?;
            Some(Command::SwitchCamera(number - 1))
        }
    }
}

impl Controls {
    pub fn handle(&mut self, event: &InputEvent) -> Option<Command> {
        match event {
            InputEvent::Key { name, state } => {
                let key = name.to_lowercase();
                match state {
                    ElementState::Pressed => {
                        let command = command_for(&key).or_else(|| command_for(name));
                        self.held.insert(key);
                        command
                    }
                    ElementState::Released => {
                        self.held.remove(&key);
                        None
                    }
                }
            }
            InputEvent::MouseButton {
                button: MouseButton::Right | MouseButton::Left,
                state,
            } => {
                self.looking = *state == ElementState::Pressed;
                self.last_cursor = None;
                None
            }
            InputEvent::CursorMoved { position } => {
                if let Some((x, y)) = self.last_cursor.filter(|_| self.looking) {
                    #[expect(clippy::cast_possible_truncation, reason = "a mouse delta in pixels fits f32 easily")]
                    {
                        self.look_delta.0 += (position.x - x) as f32;
                        self.look_delta.1 += (position.y - y) as f32;
                    }
                }
                self.last_cursor = Some((position.x, position.y));
                None
            }
            InputEvent::MouseWheel { delta_y, .. } => {
                #[expect(clippy::cast_possible_truncation, reason = "wheel ticks fit f32 easily")]
                {
                    self.wheel += *delta_y as f32;
                }
                None
            }
            InputEvent::Focused(false) => {
                self.held.clear();
                self.looking = false;
                None
            }
            InputEvent::MouseButton { .. } | InputEvent::Focused(true) => None,
        }
    }

    fn axis(&self, positive: &[&str], negative: &[&str]) -> f32 {
        let held = |keys: &[&str]| keys.iter().any(|key| self.held.contains(*key));
        f32::from(u8::from(held(positive))) - f32::from(u8::from(held(negative)))
    }

    pub fn movement(&self) -> MoveInput {
        MoveInput {
            forward: self.axis(&["w", "named(arrowup)"], &["s", "named(arrowdown)"]),
            right: self.axis(&["d", "named(arrowright)"], &["a", "named(arrowleft)"]),
            up: self.axis(&["e", " "], &["q", "c"]),
            fast: self.held.contains("named(shift)"),
        }
    }

    pub fn take_look(&mut self) -> (f32, f32) {
        std::mem::take(&mut self.look_delta)
    }

    pub fn take_wheel(&mut self) -> f32 {
        std::mem::take(&mut self.wheel)
    }
}

#[cfg(test)]
mod tests {
    use pathon::platform::PhysicalPosition;

    use super::*;

    fn key(name: &str, state: ElementState) -> InputEvent {
        InputEvent::Key {
            name: name.to_owned(),
            state,
        }
    }

    #[test]
    fn held_keys_drive_the_movement_until_released() {
        let mut controls = Controls::default();
        controls.handle(&key("w", ElementState::Pressed));
        controls.handle(&key("Named(Shift)", ElementState::Pressed));
        assert_eq!(controls.movement().forward, 1.0);
        assert!(controls.movement().fast);
        controls.handle(&key("w", ElementState::Released));
        assert_eq!(controls.movement().forward, 0.0);
    }

    #[test]
    fn number_keys_switch_cameras_and_plus_raises_the_exposure() {
        let mut controls = Controls::default();
        assert_eq!(
            controls.handle(&key("3", ElementState::Pressed)),
            Some(Command::SwitchCamera(2))
        );
        assert_eq!(
            controls.handle(&key("+", ElementState::Pressed)),
            Some(Command::ExposureUp)
        );
        assert_eq!(
            controls.handle(&key("Named(Escape)", ElementState::Pressed)),
            Some(Command::Exit)
        );
    }

    #[test]
    fn the_mouse_looks_only_while_a_button_is_down() {
        let mut controls = Controls::default();
        let moved = |x| InputEvent::CursorMoved {
            position: PhysicalPosition { x, y: 0.0 },
        };
        controls.handle(&moved(10.0));
        controls.handle(&moved(20.0));
        assert_eq!(controls.take_look(), (0.0, 0.0));
        controls.handle(&InputEvent::MouseButton {
            button: MouseButton::Right,
            state: ElementState::Pressed,
        });
        controls.handle(&moved(30.0));
        controls.handle(&moved(45.0));
        assert_eq!(controls.take_look(), (15.0, 0.0));
    }
}
