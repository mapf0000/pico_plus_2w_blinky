use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::DrawTarget;

use super::page_daemon::DaemonPageState;
use super::page_logs::LogsPageState;
use super::page_payloads::PayloadsPageState;
use super::page_system::{SystemMetrics, SystemPageState};
use super::{DisplayConfig, DisplayPalette};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PageId {
    Payloads,
    Daemon,
    System,
    Logs,
}

pub struct MenuItem {
    pub id: PageId,
    pub label: &'static str,
}

pub const MENU_ITEMS: [MenuItem; 4] = [
    MenuItem {
        id: PageId::Payloads,
        label: "Payloads",
    },
    MenuItem {
        id: PageId::Daemon,
        label: "Host Agent",
    },
    MenuItem {
        id: PageId::System,
        label: "System",
    },
    MenuItem {
        id: PageId::Logs,
        label: "Logs",
    },
];

pub fn page_for_index(idx: usize) -> PageId {
    MENU_ITEMS
        .get(idx)
        .map(|item| item.id)
        .unwrap_or(PageId::System)
}

pub fn default_menu_index() -> usize {
    MENU_ITEMS
        .iter()
        .position(|item| matches!(item.id, PageId::Payloads))
        .unwrap_or(0)
}

pub struct PageInput {
    pub a_released: bool,
    pub b_released: bool,
    pub x_released: bool,
    pub y_released: bool,
    pub menu_open: bool,
    pub nav_blocked: bool,
    pub nav_block_cleared: bool,
}

impl PageInput {
    pub fn actions_allowed(&self) -> bool {
        !self.nav_blocked && !self.nav_block_cleared
    }
}

pub struct PageContext<'a> {
    pub palette: &'a DisplayPalette,
    pub idle_bg: Rgb565,
}

pub struct PageRenderArgs<'a, D: DrawTarget<Color = Rgb565>> {
    pub disp: &'a mut D,
    pub line_x: i32,
    pub y_pos: i32,
    pub clear_w: u32,
    pub content_width: u32,
    pub content_height: u32,
    pub bg_color: Rgb565,
    pub config: &'a DisplayConfig,
    pub title_style: &'a MonoTextStyle<'a, Rgb565>,
    pub body_style: &'a MonoTextStyle<'a, Rgb565>,
}

pub enum PageRenderData<'a> {
    None,
    System {
        ap_ssid: &'a str,
        metrics: SystemMetrics<'a>,
    },
    Logs {
        log_gen: u32,
    },
}

pub trait Page {
    fn on_reset(&mut self);
    fn handle_input(&mut self, input: &PageInput, ctx: &PageContext) -> bool;
    fn on_tick(&mut self, _ctx: &PageContext) -> bool {
        false
    }
    fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        args: PageRenderArgs<'_, D>,
        data: PageRenderData<'_>,
    );
}

pub struct PageRegistry {
    payloads: PayloadsPageState,
    daemon: DaemonPageState,
    system: SystemPageState,
    logs: LogsPageState,
}

impl PageRegistry {
    pub fn new() -> Self {
        Self {
            payloads: PayloadsPageState::new(),
            daemon: DaemonPageState::new(),
            system: SystemPageState::new(),
            logs: LogsPageState::new(),
        }
    }

    pub fn menu_items(&self) -> &'static [MenuItem] {
        &MENU_ITEMS
    }

    pub fn menu_len(&self) -> usize {
        MENU_ITEMS.len()
    }

    pub fn page_for_index(&self, idx: usize) -> PageId {
        page_for_index(idx)
    }

    pub fn default_menu_index(&self) -> usize {
        default_menu_index()
    }

    pub fn logs_last_gen(&self) -> u32 {
        self.logs.last_gen
    }

    pub fn reset_all(&mut self) {
        self.payloads.on_reset();
        self.daemon.on_reset();
        self.system.on_reset();
        self.logs.on_reset();
    }

    pub fn handle_input(&mut self, id: PageId, input: &PageInput, ctx: &PageContext) -> bool {
        match id {
            PageId::Payloads => self.payloads.handle_input(input, ctx),
            PageId::Daemon => self.daemon.handle_input(input, ctx),
            PageId::System => self.system.handle_input(input, ctx),
            PageId::Logs => self.logs.handle_input(input, ctx),
        }
    }

    pub fn on_tick(&mut self, id: PageId, ctx: &PageContext) -> bool {
        match id {
            PageId::Payloads => self.payloads.on_tick(ctx),
            PageId::Daemon => self.daemon.on_tick(ctx),
            PageId::System => self.system.on_tick(ctx),
            PageId::Logs => self.logs.on_tick(ctx),
        }
    }

    pub fn render<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        id: PageId,
        args: PageRenderArgs<'_, D>,
        data: PageRenderData<'_>,
    ) {
        match id {
            PageId::Payloads => self.payloads.render(args, data),
            PageId::Daemon => self.daemon.render(args, data),
            PageId::System => self.system.render(args, data),
            PageId::Logs => self.logs.render(args, data),
        }
    }
}
