use super::input::ButtonEvents;

pub struct UiState {
    pub menu_open: bool,
    pub menu_selected: usize,
    nav_block: bool,
}

impl UiState {
    pub fn new(default_menu_index: usize) -> Self {
        Self {
            menu_open: false,
            menu_selected: default_menu_index,
            nav_block: false,
        }
    }

    pub fn nav_blocked(&self) -> bool {
        self.nav_block
    }
}

pub struct UiSignals {
    pub nav_block_cleared: bool,
}

pub fn apply_input(ui: &mut UiState, buttons: &ButtonEvents, menu_len: usize) -> UiSignals {
    let nav_block_cleared = ui.nav_block && !buttons.a.pressed && !buttons.b.pressed;
    if nav_block_cleared {
        ui.nav_block = false;
    }

    let mut menu_toggled = false;
    if buttons.a.pressed && buttons.x.just_pressed {
        ui.menu_open = !ui.menu_open;
        menu_toggled = true;
        ui.nav_block = true;
    }

    if ui.menu_open && !menu_toggled && !ui.nav_block && !nav_block_cleared {
        if buttons.a.released {
            if ui.menu_selected == 0 {
                ui.menu_selected = menu_len.saturating_sub(1);
            } else {
                ui.menu_selected = ui.menu_selected.saturating_sub(1);
            }
        }
        if buttons.b.released {
            ui.menu_selected = if menu_len == 0 {
                0
            } else {
                (ui.menu_selected + 1) % menu_len
            };
        }
    }

    UiSignals { nav_block_cleared }
}
