//! Button debouncing and ownership. GPIO sampling belongs to the hardware adapter.

use core::ops::Index;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    A,
    B,
    X,
    Y,
}

impl Button {
    pub const ALL: [Self; 4] = [Self::A, Self::B, Self::X, Self::Y];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ButtonMask(u8);

impl ButtonMask {
    pub fn contains(self, button: Button) -> bool {
        self.0 & (1 << button as u8) != 0
    }

    pub fn insert(&mut self, button: Button) {
        self.0 |= 1 << button as u8;
    }

    pub fn remove(&mut self, button: Button) {
        self.0 &= !(1 << button as u8);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ButtonState {
    pub pressed: bool,
    pub just_pressed: bool,
    pub released: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ButtonEvents(pub [ButtonState; 4]);

impl Index<Button> for ButtonEvents {
    type Output = ButtonState;

    fn index(&self, button: Button) -> &Self::Output {
        &self.0[button as usize]
    }
}

impl ButtonEvents {
    pub fn releases(self) -> ButtonMask {
        let mut mask = ButtonMask::default();
        for button in Button::ALL {
            if self[button].released {
                mask.insert(button);
            }
        }
        mask
    }
}

#[derive(Clone, Copy, Default)]
struct DebouncedButton {
    pressed: bool,
    consecutive: u8,
}

impl DebouncedButton {
    fn sample(&mut self, raw: bool) -> ButtonState {
        let previous = self.pressed;
        if raw == previous {
            self.consecutive = 0;
        } else {
            self.consecutive += 1;
            if self.consecutive == 2 {
                self.pressed = raw;
                self.consecutive = 0;
            }
        }
        ButtonState {
            pressed: self.pressed,
            just_pressed: self.pressed && !previous,
            released: !self.pressed && previous,
        }
    }
}

#[derive(Default)]
pub struct InputDebouncer([DebouncedButton; 4]);

impl InputDebouncer {
    pub fn sample(&mut self, raw: [bool; 4]) -> ButtonEvents {
        ButtonEvents(core::array::from_fn(|index| {
            self.0[index].sample(raw[index])
        }))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection(pub usize);

impl Selection {
    pub fn previous(&mut self, len: usize) {
        self.0 = if len == 0 {
            0
        } else if self.0 == 0 || self.0 >= len {
            len - 1
        } else {
            self.0 - 1
        };
    }

    pub fn next(&mut self, len: usize) {
        self.0 = if len == 0 || self.0 >= len.saturating_sub(1) {
            0
        } else {
            self.0 + 1
        };
    }

    pub fn apply(&mut self, buttons: ButtonMask, len: usize) {
        if buttons.contains(Button::A) {
            self.previous(len);
        }
        if buttons.contains(Button::B) {
            self.next(len);
        }
    }

    /// Derive a bounded viewport without changing application state during rendering.
    pub fn window(self, len: usize, rows: usize) -> core::ops::Range<usize> {
        let rows = rows.min(len);
        let selected = self.0.min(len.saturating_sub(1));
        let start = (selected + 1)
            .saturating_sub(rows)
            .min(len.saturating_sub(rows));
        start..start + rows
    }
}

pub struct Navigation {
    pub menu_open: bool,
    pub selected: Selection,
    gesture_active: bool,
}

#[derive(Default)]
pub struct Routing {
    pub page: ButtonMask,
    pub stop: bool,
    pub cycle_led: bool,
}

impl Navigation {
    pub fn new(selected: usize) -> Self {
        Self {
            menu_open: false,
            selected: Selection(selected),
            gesture_active: false,
        }
    }

    /// Reserve the complete chord, including the sample containing its last release.
    pub fn apply(&mut self, buttons: &ButtonEvents, menu_len: usize) -> bool {
        let mut consumed = self.gesture_active;
        if !self.gesture_active && buttons[Button::A].pressed && buttons[Button::X].just_pressed {
            self.menu_open = !self.menu_open;
            self.gesture_active = true;
            consumed = true;
        }
        if self.gesture_active && !buttons[Button::A].pressed && !buttons[Button::X].pressed {
            self.gesture_active = false;
        }
        if self.menu_open && !consumed {
            self.selected.apply(buttons.releases(), menu_len);
        }
        consumed
    }

    pub fn route(
        &self,
        buttons: ButtonEvents,
        gesture_consumed: bool,
        payloads: bool,
        job_reserved: bool,
    ) -> Routing {
        let releases = buttons.releases();
        let stop = releases.contains(Button::Y) && job_reserved;
        let mut page = releases;
        if self.menu_open || gesture_consumed {
            page = ButtonMask::default();
        }
        if stop {
            // Stop wins over Run even when both releases arrive in one sample.
            page.remove(Button::X);
        }
        Routing {
            page,
            stop,
            cycle_led: releases.contains(Button::Y) && !stop && !payloads,
        }
    }
}
