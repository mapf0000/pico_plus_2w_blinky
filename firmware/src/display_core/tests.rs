use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use firmware_exec::jobs::Controller as Jobs;

use super::{input::*, model::*, renderer::*, scene::*};

fn preset() -> Preset {
    Preset {
        action: PresetAction::Keyboard,
        name: "Keyboard test",
        layout: "US",
        launches_agent: false,
        available: true,
        program: &[],
    }
}

fn snapshot(now_ms: u64) -> Snapshot {
    Snapshot {
        now_ms,
        keyboard: KeyboardSnapshot::Ready,
        agent_present: false,
        transfer: TransferSnapshot::default(),
    }
}

#[test]
fn debounce_requires_consecutive_samples_and_emits_one_edge() {
    let mut debounce = InputDebouncer::default();
    assert!(!debounce.sample([true, false, false, false])[Button::A].pressed);
    debounce.sample([false; 4]);
    assert!(!debounce.sample([true, false, false, false])[Button::A].pressed);
    assert!(debounce.sample([true, false, false, false])[Button::A].just_pressed);
    assert!(!debounce.sample([true, false, false, false])[Button::A].just_pressed);
    assert!(!debounce.sample([false; 4])[Button::A].released);
    assert!(debounce.sample([false; 4])[Button::A].released);
    assert!(!debounce.sample([false; 4])[Button::A].released);
}

#[test]
fn menu_chord_consumes_both_release_orders_and_allows_the_next_run() {
    for a_first in [true, false] {
        let mut controller = Controller::default();
        controller.navigation.menu_open = true;
        controller.navigation.selected = Selection(0);
        let mut debounce = InputDebouncer::default();
        let catalog = [preset()];
        let mut sample = |raw: [bool; 4], expect_run: bool| {
            let mut ran = false;
            for _ in 0..2 {
                let events = debounce.sample(raw);
                let route = controller.route(events, false);
                ran |= controller.handle_input(route.page, &catalog).is_some();
            }
            assert_eq!(ran, expect_run);
        };
        sample([true, false, false, false], false);
        sample([true, false, true, false], false);
        sample([!a_first, false, a_first, false], false);
        sample([false; 4], false);
        sample([false, false, true, false], false);
        sample([false; 4], true);
    }
}

#[test]
fn stop_is_global_and_suppresses_run_without_swallowing_unrelated_releases() {
    for page in 0..PageId::ALL.len() {
        let mut controller = Controller::default();
        controller.navigation.selected = Selection(page);
        let mut events = ButtonEvents::default();
        for button in [Button::A, Button::X, Button::Y] {
            events.0[button as usize].released = true;
        }
        let route = controller.route(events, true);
        assert!(route.stop);
        assert!(!route.cycle_led);
        assert!(!route.page.contains(Button::X));
    }
}

#[test]
fn idle_y_retains_led_behavior_outside_payloads() {
    let mut controller = Controller::default();
    let mut events = ButtonEvents::default();
    events.0[Button::Y as usize].released = true;
    let route = controller.route(events, false);
    assert!(route.cycle_led);
    controller.navigation.menu_open = true;
    assert_eq!(controller.route(events, false).page, ButtonMask::default());
    controller.navigation.menu_open = false;
    controller.navigation.selected = Selection(0);
    let route = controller.route(events, false);
    assert!(!route.cycle_led);
}

#[test]
fn selections_wrap_and_empty_catalogs_are_safe() {
    let mut selected = Selection(0);
    selected.previous(0);
    selected.next(0);
    assert_eq!(selected.0, 0);
    selected.previous(5);
    assert_eq!(selected.0, 4);
    selected.next(5);
    assert_eq!(selected.0, 0);
    assert_eq!(Selection(9).window(10, 3), 7..10);
    assert!(selected.window(0, 3).is_empty());
    let mut controller = Controller::default();
    controller.navigation.selected = Selection(0);
    let mut buttons = ButtonMask::default();
    buttons.insert(Button::B);
    buttons.insert(Button::X);
    assert!(controller.handle_input(buttons, &[]).is_none());
}

#[test]
fn cdc_install_reports_cached_presence_then_becomes_ready_without_auto_launch() {
    for action in [PresetAction::CdcArm, PresetAction::CdcInstall] {
        let mut controller = Controller::default();
        controller.navigation.selected = Selection(0);
        let mut install = preset();
        install.action = action;
        install.launches_agent = true;
        let catalog = [install];
        let mut activate = ButtonMask::default();
        activate.insert(Button::X);
        let mut state = snapshot(1_000);
        state.agent_present = true;
        controller.update(state, None);
        assert!(controller.handle_input(activate, &catalog).is_none());
        assert_eq!(
            controller.payloads.status.text,
            "Agent recently seen; wait 25s after stop"
        );
        assert_eq!(controller.payloads.status.severity, Severity::Warning);
        let mut state = snapshot(2_000);
        state.agent_present = true;
        assert!(!controller.update(state, None));
        assert!(
            controller
                .handle_input(ButtonMask::default(), &catalog)
                .is_none()
        );
        assert!(controller.update(snapshot(26_001), None));
        assert_eq!(
            controller.payloads.status.text,
            "Agent not detected; press X to retry"
        );
        assert!(!controller.update(snapshot(26_002), None));
        assert!(
            controller
                .handle_input(ButtonMask::default(), &catalog)
                .is_none()
        );
        assert!(matches!(
            controller.handle_input(activate, &catalog),
            Some(Intent::RunPreset(0))
        ));
    }
}

#[test]
fn manual_cdc_arm_blocks_another_launch_with_visible_feedback_and_recovers() {
    let mut controller = Controller::default();
    controller.navigation.selected = Selection(0);
    let mut arm = preset();
    arm.action = PresetAction::CdcArm;
    arm.launches_agent = true;
    let mut install = preset();
    install.action = PresetAction::CdcInstall;
    install.launches_agent = true;
    let catalog = [arm, install];
    let mut activate = ButtonMask::default();
    activate.insert(Button::X);
    assert!(matches!(
        controller.handle_input(activate, &catalog),
        Some(Intent::RunPreset(0))
    ));
    controller.payloads.installation_status(
        "CDC armed; enter command within 30s",
        false,
        true,
        1_000,
    );
    controller.payloads.selected = Selection(1);
    assert!(controller.handle_input(activate, &catalog).is_none());
    assert_eq!(
        controller.payloads.status.text,
        "Installation pending; wait for completion"
    );
    assert_eq!(controller.payloads.status.severity, Severity::Warning);
    controller.payloads.installation_status(
        "CDC failed; retry or use Control-C",
        true,
        false,
        31_000,
    );
    assert!(matches!(
        controller.handle_input(activate, &catalog),
        Some(Intent::RunPreset(1))
    ));
}

#[test]
fn payload_completion_and_deadline_continue_on_hidden_pages() {
    let mut controller = Controller::default(); // System is visible.
    let handle = Jobs::new().reserve(()).unwrap();
    controller
        .payloads
        .submitted(Submission::Accepted(handle), true);
    assert!(!controller.update(
        snapshot(1_100),
        Some(Completion {
            handle,
            status: CompletionStatus::Completed,
            completed_at_ms: 1_000
        })
    ));
    assert!(controller.payloads.status.text.contains("waiting"));
    controller.update(snapshot(15_999), None);
    assert!(controller.payloads.status.text.contains("waiting"));
    controller.update(snapshot(16_000), None);
    assert_eq!(
        controller.payloads.status.text,
        "Keys sent; agent not detected"
    );
    let mut activate = ButtonMask::default();
    activate.insert(Button::X);
    controller.navigation.selected = Selection(0);
    assert!(matches!(
        controller.handle_input(activate, &[preset()]),
        Some(Intent::RunPreset(0))
    ));
}

#[test]
fn late_completion_uses_its_timestamp_and_stale_handles_are_ignored() {
    let mut jobs = Jobs::new();
    let old = jobs.reserve(()).unwrap();
    jobs.begin_release(old);
    jobs.finish(old);
    let handle = jobs.reserve(()).unwrap();
    let mut controller = Controller::default();
    controller
        .payloads
        .submitted(Submission::Accepted(handle), true);
    controller.update(
        snapshot(500),
        Some(Completion {
            handle: old,
            status: CompletionStatus::Cancelled,
            completed_at_ms: 400,
        }),
    );
    assert_eq!(controller.payloads.status.text, "Keyboard sequence queued");
    controller.update(
        snapshot(20_000),
        Some(Completion {
            handle,
            status: CompletionStatus::Completed,
            completed_at_ms: 1_000,
        }),
    );
    assert_eq!(
        controller.payloads.status.text,
        "Keys sent; agent not detected"
    );
}

#[test]
fn handshake_and_cancellation_are_terminal_without_automatic_resubmission() {
    for status in [
        CompletionStatus::Completed,
        CompletionStatus::Cancelled,
        CompletionStatus::UsbUnavailable,
    ] {
        let handle = Jobs::new().reserve(()).unwrap();
        let mut controller = Controller::default();
        controller
            .payloads
            .submitted(Submission::Accepted(handle), true);
        let mut state = snapshot(1_000);
        state.agent_present = true;
        controller.update(
            state,
            Some(Completion {
                handle,
                status,
                completed_at_ms: 900,
            }),
        );
        let text = controller.payloads.status.text.clone();
        controller.update(snapshot(30_000), None);
        assert_eq!(controller.payloads.status.text, text);
        assert_eq!(
            controller.payloads.status.severity,
            if status == CompletionStatus::Completed {
                Severity::Success
            } else {
                Severity::Warning
            }
        );
    }
}

#[test]
fn transfer_progress_is_coalesced_but_connection_and_terminal_changes_are_immediate() {
    let mut controller = Controller::default();
    let mut state = snapshot(1_000);
    state.transfer.state = TransferState::Progress;
    state.transfer.id = 1;
    assert!(controller.transfer.update(state.transfer, state.now_ms));
    let mut state = snapshot(1_050);
    state.transfer = TransferSnapshot {
        state: TransferState::Progress,
        id: 1,
        received_bytes: 100,
        ..Default::default()
    };
    assert!(!controller.transfer.update(state.transfer, state.now_ms));
    assert_eq!(controller.transfer.snapshot.received_bytes, 0);
    let mut state = snapshot(1_100);
    state.transfer = TransferSnapshot {
        state: TransferState::Progress,
        id: 1,
        received_bytes: 200,
        ..Default::default()
    };
    assert!(controller.transfer.update(state.transfer, state.now_ms));
    let mut state = snapshot(1_101);
    state.transfer = TransferSnapshot {
        state: TransferState::Finished,
        id: 1,
        received_bytes: 300,
        ..Default::default()
    };
    assert!(controller.transfer.update(state.transfer, state.now_ms));
    let mut state = snapshot(1_102);
    state.transfer = controller.transfer.snapshot;
    state.transfer.browser_connected = true;
    assert!(controller.transfer.update(state.transfer, state.now_ms));
}

struct Canvas {
    pixels: std::vec::Vec<Rgb565>,
    calls: usize,
    written: std::vec::Vec<Point>,
    fail: bool,
}

impl Canvas {
    fn new() -> Self {
        Self {
            pixels: std::vec![Rgb565::BLACK; 320 * 240],
            calls: 0,
            written: std::vec::Vec::new(),
            fail: false,
        }
    }
}

impl OriginDimensions for Canvas {
    fn size(&self) -> Size {
        Size::new(320, 240)
    }
}

impl DrawTarget for Canvas {
    type Color = Rgb565;
    type Error = ();
    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), ()> {
        if self.fail {
            return Err(());
        }
        self.calls += 1;
        for Pixel(point, color) in pixels {
            assert!(self.bounding_box().contains(point));
            self.pixels[point.y as usize * 320 + point.x as usize] = color;
            self.written.push(point);
        }
        Ok(())
    }
}

#[test]
fn selected_payload_text_has_both_ink_and_background_pixels() {
    let controller = Controller::default();
    let catalog = [preset()];
    let viewport = Rectangle::new(Point::zero(), Size::new(320, 240));
    let scene = Scene::build(
        PageView::Payloads {
            model: &controller.payloads,
            presets: &catalog,
            keyboard: KeyboardSnapshot::Ready,
        },
        viewport,
        Palette::default(),
    );
    let selected = scene
        .rows
        .iter()
        .find(|row| row.text == "Keyboard test")
        .unwrap();
    assert_ne!(selected.foreground, selected.background);
    let mut canvas = Canvas::new();
    let mut renderer = Renderer::new(DisplayConfig::default());
    renderer
        .render(
            &mut canvas,
            &controller.navigation,
            PageView::Payloads {
                model: &controller.payloads,
                presets: &catalog,
                keyboard: KeyboardSnapshot::Ready,
            },
        )
        .unwrap();
    let glyph = Rectangle::new(selected.rect.top_left + Point::new(6, 3), Size::new(6, 9));
    let colors: std::vec::Vec<_> = glyph
        .points()
        .map(|point| canvas.pixels[point.y as usize * 320 + point.x as usize])
        .collect();
    assert!(colors.contains(&Rgb565::BLACK) && colors.contains(&Rgb565::WHITE));
}

#[test]
fn payload_footer_stays_visible_with_large_catalog_and_selection() {
    let mut model = PayloadModel::default();
    let catalog: std::vec::Vec<_> = (0..30).map(|_| preset()).collect();
    model.selected = Selection(29);
    model.status.set(
        "A long status message whose final words need a second row",
        Severity::Warning,
    );
    for width in [194, 320] {
        let viewport = Rectangle::new(Point::zero(), Size::new(width, 240));
        let scene = Scene::build(
            PageView::Payloads {
                model: &model,
                presets: &catalog,
                keyboard: KeyboardSnapshot::Ready,
            },
            viewport,
            Palette::default(),
        );
        assert!(
            scene
                .rows
                .iter()
                .all(|row| row.rect.intersection(&viewport) == row.rect)
        );
        assert!(
            scene
                .rows
                .iter()
                .any(|row| row.text.contains("Input layout"))
        );
        assert_eq!(scene.rows.last().unwrap().rect.top_left.y, 224);
        assert!(
            scene
                .rows
                .iter()
                .any(|row| row.background == Palette::default().yellow)
        );
        assert_eq!(
            scene
                .rows
                .iter()
                .filter(|row| row.background == Rgb565::WHITE)
                .count(),
            1
        );
    }
}

#[test]
fn unchanged_scene_draws_nothing_and_selection_only_redraws_two_rows() {
    let mut controller = Controller::default();
    controller.navigation.selected = Selection(0);
    let catalog = [preset(), preset(), preset()];
    let system = SystemSnapshot::default();
    let mut renderer = Renderer::new(DisplayConfig::default());
    let mut canvas = Canvas::new();
    renderer
        .render(
            &mut canvas,
            &controller.navigation,
            controller.view(&catalog, "AP", &system, &[]),
        )
        .unwrap();
    let calls = canvas.calls;
    assert!(
        !renderer
            .render(
                &mut canvas,
                &controller.navigation,
                controller.view(&catalog, "AP", &system, &[])
            )
            .unwrap()
    );
    assert_eq!(calls, canvas.calls);
    canvas.written.clear();
    controller.payloads.selected.next(catalog.len());
    renderer
        .render(
            &mut canvas,
            &controller.navigation,
            controller.view(&catalog, "AP", &system, &[]),
        )
        .unwrap();
    assert!(
        canvas
            .written
            .iter()
            .all(|point| (72..104).contains(&point.y))
    );
}

#[test]
fn layout_changes_preserve_status_and_style_only_changes_repaint() {
    let mut controller = Controller::default();
    controller.navigation.selected = Selection(
        PageId::ALL
            .iter()
            .position(|page| *page == PageId::Agent)
            .unwrap(),
    );
    controller.agent.status.set("Sent", Severity::Success);
    let system = SystemSnapshot::default();
    let mut renderer = Renderer::new(DisplayConfig::default());
    let mut canvas = Canvas::new();
    for menu_open in [false, true, false] {
        controller.navigation.menu_open = menu_open;
        renderer
            .render(
                &mut canvas,
                &controller.navigation,
                controller.view(&[], "AP", &system, &[]),
            )
            .unwrap();
        assert_eq!(controller.agent.status.text, "Sent");
        assert_eq!(controller.agent.status.severity, Severity::Success);
    }
    controller.agent.status.severity = Severity::Warning;
    assert!(
        renderer
            .render(
                &mut canvas,
                &controller.navigation,
                controller.view(&[], "AP", &system, &[])
            )
            .unwrap()
    );
    assert!(canvas.pixels.contains(&Palette::default().yellow));
    controller.agent.status.set("", Severity::Normal);
    renderer
        .render(
            &mut canvas,
            &controller.navigation,
            controller.view(&[], "AP", &system, &[]),
        )
        .unwrap();
    assert!(!canvas.pixels.contains(&Palette::default().yellow));
}

#[test]
fn failed_frame_is_invalidated_and_retry_repaints() {
    let controller = Controller::default();
    let system = SystemSnapshot::default();
    let mut renderer = Renderer::new(DisplayConfig::default());
    let mut canvas = Canvas::new();
    canvas.fail = true;
    assert!(
        renderer
            .render(
                &mut canvas,
                &controller.navigation,
                controller.view(&[], "AP", &system, &[])
            )
            .is_err()
    );
    assert!(renderer.needs_layout(
        canvas.bounding_box(),
        &controller.navigation,
        controller.page()
    ));
    canvas.fail = false;
    assert!(
        renderer
            .render(
                &mut canvas,
                &controller.navigation,
                controller.view(&[], "AP", &system, &[])
            )
            .unwrap()
    );
}

#[test]
fn text_truncation_is_bounded_and_normalizes_unsupported_characters() {
    assert_eq!(fit_text("abcdefghij", 5), "ab...");
    assert_eq!(fit_text("long", 2), "..");
    assert_eq!(fit_text("long", 0), "");
    assert_eq!(fit_text("aé\nb", 4), "a??b");
    assert_eq!(
        bounded_text(format_args!("{}", "x".repeat(1_000))).len(),
        TEXT_CAPACITY
    );
}

#[test]
fn layout_clips_all_pages_even_with_oversized_menu_configuration() {
    let system = SystemSnapshot::default();
    let logs = [bounded_text(format_args!("{}", "x".repeat(96)))];
    let catalog = [preset()];
    for menu_width in [100, u32::MAX] {
        let config = DisplayConfig {
            menu_width,
            content_padding: u32::MAX,
            ..Default::default()
        };
        let mut renderer = Renderer::new(config);
        let mut controller = Controller::default();
        controller.navigation.menu_open = true;
        let mut canvas = Canvas::new();
        for index in 0..PageId::ALL.len() {
            controller.navigation.selected = Selection(index);
            renderer
                .render(
                    &mut canvas,
                    &controller.navigation,
                    controller.view(&catalog, "AP", &system, &logs),
                )
                .unwrap();
        }
    }
}

#[test]
fn status_changes_do_not_repaint_the_header_or_preset_list() {
    let mut controller = Controller::default();
    controller.navigation.selected = Selection(0);
    let catalog = [preset(), preset()];
    let system = SystemSnapshot::default();
    let mut renderer = Renderer::new(DisplayConfig::default());
    let mut canvas = Canvas::new();
    renderer
        .render(
            &mut canvas,
            &controller.navigation,
            controller.view(&catalog, "AP", &system, &[]),
        )
        .unwrap();
    canvas.written.clear();
    controller
        .payloads
        .status
        .set("Cancelled", Severity::Warning);
    renderer
        .render(
            &mut canvas,
            &controller.navigation,
            controller.view(&catalog, "AP", &system, &[]),
        )
        .unwrap();
    assert!(!canvas.written.is_empty());
    assert!(
        canvas
            .written
            .iter()
            .all(|point| (212..236).contains(&point.y))
    );
}

#[test]
fn shrinking_log_tail_erases_removed_rows() {
    let controller = Controller::default();
    let logs = [
        bounded_text(format_args!("first")),
        bounded_text(format_args!("second")),
        bounded_text(format_args!("third")),
    ];
    let mut renderer = Renderer::new(DisplayConfig::default());
    let mut canvas = Canvas::new();
    renderer
        .render(&mut canvas, &controller.navigation, PageView::Logs(&logs))
        .unwrap();
    renderer
        .render(
            &mut canvas,
            &controller.navigation,
            PageView::Logs(&logs[..1]),
        )
        .unwrap();
    let removed = Rectangle::new(Point::new(0, 42), Size::new(320, 24));
    assert!(
        removed
            .points()
            .all(|point| canvas.pixels[point.y as usize * 320 + point.x as usize] == Rgb565::BLACK)
    );
}

#[test]
fn unavailable_and_already_connected_launchers_emit_no_run_intent() {
    let mut controller = Controller::default();
    controller.navigation.selected = Selection(0);
    let mut catalog = [preset()];
    catalog[0].launches_agent = true;
    catalog[0].available = false;
    let mut buttons = ButtonMask::default();
    buttons.insert(Button::X);
    assert!(controller.handle_input(buttons, &catalog).is_none());
    assert!(controller.payloads.status.text.contains("missing"));
    catalog[0].available = true;
    let mut state = snapshot(1_000);
    state.agent_present = true;
    controller.update(state, None);
    assert!(controller.handle_input(buttons, &catalog).is_none());
    assert_eq!(controller.payloads.status.text, "Agent already connected");
}

#[test]
fn selection_only_menu_change_is_a_redraw_even_for_the_same_page_view() {
    let mut controller = Controller::default();
    controller.navigation.menu_open = true;
    let system = SystemSnapshot::default();
    let mut renderer = Renderer::new(DisplayConfig::default());
    let mut canvas = Canvas::new();
    renderer
        .render(
            &mut canvas,
            &controller.navigation,
            PageView::System {
                ssid: "AP",
                metrics: &system,
            },
        )
        .unwrap();
    canvas.written.clear();
    controller.navigation.selected.previous(PageId::ALL.len());
    assert!(renderer.needs_layout(
        canvas.bounding_box(),
        &controller.navigation,
        PageId::System
    ));
    assert!(
        renderer
            .render(
                &mut canvas,
                &controller.navigation,
                PageView::System {
                    ssid: "AP",
                    metrics: &system
                }
            )
            .unwrap()
    );
    assert!(canvas.written.iter().all(|point| point.x < 100));
}

#[test]
fn full_frames_cover_the_screen_once_without_background_overpainting() {
    let mut controller = Controller::default();
    let catalog = [preset(), preset()];
    let system = SystemSnapshot::default();
    let logs = [bounded_text(format_args!("log row"))];
    for screen in [
        Rectangle::new(Point::zero(), Size::new(320, 240)),
        Rectangle::new(Point::new(7, 5), Size::new(30, 24)),
    ] {
        for menu_open in [false, true] {
            controller.navigation.menu_open = menu_open;
            for index in 0..PageId::ALL.len() {
                controller.navigation.selected = Selection(index);
                let mut renderer = Renderer::new(DisplayConfig::default());
                let frame = renderer.prepare(
                    screen,
                    &controller.navigation,
                    controller.view(&catalog, "AP", &system, &logs),
                );
                let mut coverage =
                    std::vec![0u8; (screen.size.width * screen.size.height) as usize];
                for paint in frame.paints() {
                    for point in paint.bounds().points() {
                        assert!(screen.contains(point));
                        let point = point - screen.top_left;
                        coverage
                            [point.y as usize * screen.size.width as usize + point.x as usize] += 1;
                    }
                }
                assert!(
                    coverage.iter().all(|count| *count == 1),
                    "{index}, {menu_open}, {screen:?}"
                );
            }
        }
    }
}

#[test]
fn tiled_flush_matches_direct_pixels_and_initializes_every_transmitted_byte() {
    use super::tile::{TILE_BYTES, Tile, tiles};
    let mut controller = Controller::default();
    let catalog = [preset(), preset()];
    let system = SystemSnapshot::default();
    let mut renderer = Renderer::new(DisplayConfig::default());
    let mut direct = Canvas::new();
    let mut tiled = Canvas::new();
    for menu_open in [false, true, false] {
        controller.navigation.menu_open = menu_open;
        for index in 0..PageId::ALL.len() {
            controller.navigation.selected = Selection(index);
            let frame = renderer.prepare(
                direct.bounding_box(),
                &controller.navigation,
                controller.view(&catalog, "AP", &system, &[]),
            );
            for paint in frame.paints() {
                paint.draw(&mut direct).unwrap();
                for area in tiles(paint.bounds()) {
                    let mut first = [0xAB; TILE_BYTES];
                    let mut second = [0xCD; TILE_BYTES];
                    let mut tile = Tile::new(area, &mut first).unwrap();
                    paint.draw(&mut tile).unwrap();
                    let mut other = Tile::new(area, &mut second).unwrap();
                    paint.draw(&mut other).unwrap();
                    assert_eq!(tile.bytes(), other.bytes());
                    let colors = tile.bytes().as_chunks::<2>().0.iter().map(|bytes| {
                        Rgb565::from(embedded_graphics::pixelcolor::raw::RawU16::new(
                            u16::from_be_bytes([bytes[0], bytes[1]]),
                        ))
                    });
                    tiled.fill_contiguous(&area, colors).unwrap();
                }
            }
            assert_eq!(direct.pixels, tiled.pixels);
            renderer.commit(frame);
        }
    }
}

#[test]
fn tile_bounds_wire_order_and_buffer_capacity_are_checked() {
    use super::tile::Tile;
    let mut buffer = [0xAA; 10];
    let bounds = Rectangle::new(Point::new(4, 7), Size::new(2, 2));
    let mut tile = Tile::new(bounds, &mut buffer).unwrap();
    tile.fill_solid(&bounds, Rgb565::WHITE).unwrap();
    tile.draw_iter([
        Pixel(Point::new(4, 7), Rgb565::RED),
        Pixel(Point::new(3, 7), Rgb565::BLACK),
    ])
    .unwrap();
    assert_eq!(tile.bytes(), &[0xF8, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
    assert_eq!(buffer[8..], [0xAA; 2]);
    assert!(
        Tile::new(
            Rectangle::new(Point::zero(), Size::new(320, 8)),
            &mut buffer
        )
        .is_none()
    );
}

#[test]
fn in_flight_frames_own_text_and_uncommitted_frames_force_retry() {
    let mut controller = Controller::default();
    controller.navigation.selected = Selection(0);
    let catalog = [preset()];
    let system = SystemSnapshot::default();
    let screen = Rectangle::new(Point::zero(), Size::new(320, 240));
    let mut renderer = Renderer::new(DisplayConfig::default());
    let old = renderer.prepare(
        screen,
        &controller.navigation,
        controller.view(&catalog, "AP", &system, &[]),
    );
    controller
        .payloads
        .status
        .set("Changed during DMA", Severity::Success);
    renderer.commit(old);
    let next = renderer.prepare(
        screen,
        &controller.navigation,
        controller.view(&catalog, "AP", &system, &[]),
    );
    assert!(next.paints().next().is_some());
    drop(next); // failed/cancelled flush must not validate the pixel cache
    assert!(renderer.needs_layout(screen, &controller.navigation, controller.page()));
    let retry = renderer.prepare(
        screen,
        &controller.navigation,
        controller.view(&catalog, "AP", &system, &[]),
    );
    assert_eq!(
        retry
            .paints()
            .map(|paint| paint.bounds().size.width * paint.bounds().size.height)
            .sum::<u32>(),
        320 * 240
    );
}

#[test]
fn pairing_scene_shows_six_digits_and_physical_confirmation_without_metrics() {
    let metrics = SystemSnapshot {
        pairing_code: Some(42),
        ..Default::default()
    };
    let scene = Scene::build(
        PageView::System {
            ssid: "Bluetooth",
            metrics: &metrics,
        },
        Rectangle::new(Point::zero(), Size::new(320, 240)),
        Palette::default(),
    );
    assert!(scene.rows.iter().any(|row| row.text == "Compare: 000042"));
    assert!(
        scene
            .rows
            .iter()
            .any(|row| row.text == "X: Confirm, Y: Reject")
    );
    assert!(!scene.rows.iter().any(|row| row.text.starts_with("Uptime:")));
}
