use embassy_rp::gpio::Input;

#[derive(Clone, Copy, Default)]
pub struct ButtonState {
    pub pressed: bool,
    pub just_pressed: bool,
    pub released: bool,
}

#[derive(Clone, Copy, Default)]
pub struct ButtonEvents {
    pub a: ButtonState,
    pub b: ButtonState,
    pub x: ButtonState,
    pub y: ButtonState,
}

pub struct InputDebouncer {
    a_state: bool,
    b_state: bool,
    x_state: bool,
    y_state: bool,
    a_count: u8,
    b_count: u8,
    x_count: u8,
    y_count: u8,
    prev_a: bool,
    prev_b: bool,
    prev_x: bool,
    prev_y: bool,
}

impl InputDebouncer {
    pub fn new() -> Self {
        Self {
            a_state: false,
            b_state: false,
            x_state: false,
            y_state: false,
            a_count: 0,
            b_count: 0,
            x_count: 0,
            y_count: 0,
            prev_a: false,
            prev_b: false,
            prev_x: false,
            prev_y: false,
        }
    }

    pub fn sample(
        &mut self,
        btn_a: &Input<'_>,
        btn_b: &Input<'_>,
        btn_x: &Input<'_>,
        btn_y: &Input<'_>,
    ) -> ButtonEvents {
        let a_pressed = debounce_button(btn_a.is_low(), &mut self.a_state, &mut self.a_count);
        let b_pressed = debounce_button(btn_b.is_low(), &mut self.b_state, &mut self.b_count);
        let x_pressed = debounce_button(btn_x.is_low(), &mut self.x_state, &mut self.x_count);
        let y_pressed = debounce_button(btn_y.is_low(), &mut self.y_state, &mut self.y_count);

        let events = ButtonEvents {
            a: button_events(a_pressed, self.prev_a),
            b: button_events(b_pressed, self.prev_b),
            x: button_events(x_pressed, self.prev_x),
            y: button_events(y_pressed, self.prev_y),
        };

        self.prev_a = a_pressed;
        self.prev_b = b_pressed;
        self.prev_x = x_pressed;
        self.prev_y = y_pressed;

        events
    }
}

#[inline]
fn debounce_button(raw: bool, state: &mut bool, counter: &mut u8) -> bool {
    if raw == *state {
        *counter = 0;
    } else {
        *counter = counter.saturating_add(1);
        if *counter >= 2 {
            *state = raw;
            *counter = 0;
        }
    }
    *state
}

#[inline]
fn button_events(pressed: bool, prev: bool) -> ButtonState {
    ButtonState {
        pressed,
        just_pressed: pressed && !prev,
        released: !pressed && prev,
    }
}
