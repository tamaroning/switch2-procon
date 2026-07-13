//! GUI for switch2-procon.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2, ViewportCommand};
use notify_rust::Notification;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use switch2_procon::{
    AppState, Command, ConnectionPhase, SessionHandle, Stick, VigemStatus, format_buttons,
    format_xinput,
};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};

const VIGEM_URL: &str = VigemStatus::RELEASES_URL;
/// Secondary/hint text on dark background.
const MUTED: Color32 = Color32::from_rgb(190, 190, 195);
const TEXT: Color32 = Color32::from_rgb(235, 235, 240);
const STICK_RING: Color32 = Color32::from_rgb(140, 140, 150);
const STICK_CROSS: Color32 = Color32::from_rgb(100, 100, 110);

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.widgets.noninteractive.fg_stroke.color = TEXT;
    visuals.widgets.inactive.fg_stroke.color = Color32::from_rgb(200, 200, 205);
    visuals.widgets.hovered.fg_stroke.color = Color32::WHITE;
    visuals.widgets.active.fg_stroke.color = Color32::WHITE;
    visuals.widgets.open.fg_stroke.color = Color32::WHITE;
    ctx.set_visuals(visuals);
}

fn make_tray_icon() -> Icon {
    // 32x32 simple blue square with a white crosshair-ish mark.
    let size = 32u32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let i = ((y * size + x) * 4) as usize;
            let edge = x < 2 || y < 2 || x >= size - 2 || y >= size - 2;
            let cross = (14..=17).contains(&x) || (14..=17).contains(&y);
            if edge || cross {
                rgba[i] = 240;
                rgba[i + 1] = 240;
                rgba[i + 2] = 245;
                rgba[i + 3] = 255;
            } else {
                rgba[i] = 40;
                rgba[i + 1] = 90;
                rgba[i + 2] = 180;
                rgba[i + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, size, size).expect("tray icon")
}

fn toast(summary: &str, body: &str) {
    let _ = Notification::new()
        .summary(summary)
        .body(body)
        .appname("switch2-procon")
        .show();
}

fn draw_stick(ui: &mut egui::Ui, label: &str, stick: Stick) {
    ui.vertical(|ui| {
        ui.label(label);
        let (resp, painter) = ui.allocate_painter(Vec2::splat(96.0), Sense::hover());
        let rect = resp.rect;
        let center = rect.center();
        let radius = rect.width() * 0.42;
        painter.circle_stroke(center, radius, Stroke::new(1.0_f32, STICK_RING));
        painter.line_segment(
            [
                Pos2::new(center.x - radius, center.y),
                Pos2::new(center.x + radius, center.y),
            ],
            Stroke::new(1.0_f32, STICK_CROSS),
        );
        painter.line_segment(
            [
                Pos2::new(center.x, center.y - radius),
                Pos2::new(center.x, center.y + radius),
            ],
            Stroke::new(1.0_f32, STICK_CROSS),
        );
        // Screen Y grows downward; stick +Y is up.
        let tip = Pos2::new(center.x + stick.x * radius, center.y - stick.y * radius);
        painter.circle_filled(tip, 6.0, Color32::from_rgb(80, 160, 255));
        painter.rect_stroke(
            Rect::from_center_size(center, Vec2::splat(radius * 2.0)),
            2.0,
            Stroke::new(1.0_f32, STICK_RING),
            egui::StrokeKind::Outside,
        );
    });
}

struct UiApp {
    session: SessionHandle,
    auto_connect_done: bool,
    auto_addr: Option<String>,
    tray: Option<TrayIcon>,
    menu_show: MenuItem,
    menu_disconnect: MenuItem,
    menu_quit: MenuItem,
    window_visible: Arc<AtomicBool>,
    last_phase: ConnectionPhase,
    last_error: Option<String>,
}

impl UiApp {
    fn new(cc: &eframe::CreationContext<'_>, auto_addr: Option<String>) -> Self {
        let session = SessionHandle::spawn();
        session.send(Command::StartScan);

        let menu_show = MenuItem::new("Show", true, None);
        let menu_disconnect = MenuItem::new("Disconnect", true, None);
        let menu_quit = MenuItem::new("Quit", true, None);
        let tray_menu = Menu::new();
        let _ = tray_menu.append(&menu_show);
        let _ = tray_menu.append(&menu_disconnect);
        let _ = tray_menu.append(&PredefinedMenuItem::separator());
        let _ = tray_menu.append(&menu_quit);

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(tray_menu))
            .with_tooltip("switch2-procon")
            .with_icon(make_tray_icon())
            .build()
            .ok();

        let window_visible = Arc::new(AtomicBool::new(true));
        apply_theme(&cc.egui_ctx);

        Self {
            session,
            auto_connect_done: false,
            auto_addr,
            tray,
            menu_show,
            menu_disconnect,
            menu_quit,
            window_visible,
            last_phase: ConnectionPhase::Idle,
            last_error: None,
        }
    }

    fn snapshot(&self) -> AppState {
        self.session
            .state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    fn poll_tray(&mut self, ctx: &egui::Context) {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } | TrayIconEvent::Click { .. } = event {
                self.show_window(ctx);
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.menu_show.id() {
                self.show_window(ctx);
            } else if event.id == self.menu_disconnect.id() {
                self.session.send(Command::Disconnect);
            } else if event.id == self.menu_quit.id() {
                self.session.send(Command::Quit);
                ctx.send_viewport_cmd(ViewportCommand::Close);
                // Force exit even if close-to-tray is active.
                std::process::exit(0);
            }
        }
    }

    fn show_window(&self, ctx: &egui::Context) {
        self.window_visible.store(true, Ordering::SeqCst);
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }

    fn hide_window(&self, ctx: &egui::Context) {
        self.window_visible.store(false, Ordering::SeqCst);
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }

    fn maybe_toast(&mut self, state: &AppState) {
        if state.phase != self.last_phase {
            match state.phase {
                ConnectionPhase::Active => {
                    let addr = state
                        .selected_addr
                        .clone()
                        .unwrap_or_else(|| "controller".into());
                    toast("Connected", &format!("Linked to {addr}"));
                }
                ConnectionPhase::Idle if self.last_phase == ConnectionPhase::Active => {
                    toast("Disconnected", "Controller session ended");
                }
                ConnectionPhase::Error => {
                    let msg = state
                        .last_error
                        .clone()
                        .unwrap_or_else(|| "Unknown error".into());
                    toast("Error", &msg);
                }
                _ => {}
            }
            self.last_phase = state.phase;
        }
        if state.last_error != self.last_error {
            if let (Some(err), ConnectionPhase::Error) = (&state.last_error, state.phase)
                && self.last_error.as_ref() != Some(err)
                && self.last_phase == ConnectionPhase::Error
            {
                // already toasted on phase change
            }
            self.last_error = state.last_error.clone();
        }
    }
}

impl eframe::App for UiApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_tray(ctx);

        if ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide_window(ctx);
        }

        let state = self.snapshot();
        self.maybe_toast(&state);

        // One-shot auto-connect from CLI address arg.
        if !self.auto_connect_done
            && let Some(addr) = self.auto_addr.clone()
        {
            self.auto_connect_done = true;
            self.session.send(Command::Connect(addr));
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Switch 2 Pro Controller");
            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Status:");
                let color = match state.phase {
                    ConnectionPhase::Active => Color32::LIGHT_GREEN,
                    ConnectionPhase::Error => Color32::LIGHT_RED,
                    ConnectionPhase::Scanning | ConnectionPhase::Connecting => Color32::YELLOW,
                    ConnectionPhase::Idle => MUTED,
                };
                let label = match state.phase {
                    ConnectionPhase::Scanning | ConnectionPhase::Connecting => {
                        let n = ((ctx.input(|i| i.time) * 2.0) as usize) % 4;
                        format!("{}{}", state.phase.label(), ".".repeat(n))
                    }
                    _ => state.phase.label().to_string(),
                };
                ui.colored_label(color, label);
            });
            if let Some(err) = &state.last_error {
                ui.colored_label(Color32::LIGHT_RED, err);
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("ViGEm:");
                match &state.vigem {
                    VigemStatus::Ready => {
                        ui.colored_label(Color32::LIGHT_GREEN, "Ready (virtual Xbox 360)");
                    }
                    VigemStatus::Unavailable { message } => {
                        ui.colored_label(Color32::YELLOW, "Unavailable");
                        if ui.link("Install ViGEmBus").clicked() {
                            let _ = open::that(VIGEM_URL);
                        }
                        ui.label(egui::RichText::new(message).small().color(MUTED));
                    }
                    VigemStatus::Unsupported => {
                        ui.colored_label(MUTED, "Unsupported on this OS");
                    }
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let can_rescan = state.phase != ConnectionPhase::Connecting
                    && state.phase != ConnectionPhase::Active;
                if ui
                    .add_enabled(can_rescan, egui::Button::new("Rescan"))
                    .clicked()
                {
                    self.session.send(Command::StartScan);
                }
                if ui
                    .add_enabled(
                        state.phase == ConnectionPhase::Active
                            || state.phase == ConnectionPhase::Connecting,
                        egui::Button::new("Disconnect"),
                    )
                    .clicked()
                {
                    self.session.send(Command::Disconnect);
                }
            });

            ui.add_space(6.0);
            ui.label("Devices:");
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .show(ui, |ui| {
                    if state.devices.is_empty() {
                        ui.colored_label(MUTED, "No Switch 2 Pro Controllers found yet.");
                    }
                    for d in &state.devices {
                        let active = state.selected_addr.as_deref() == Some(d.addr.as_str());
                        let _ = ui.selectable_label(active, format!("{}  {}", d.addr, d.name));
                    }
                });

            ui.add_space(10.0);
            ui.separator();
            ui.label("Live input");
            // Stick/counter only here; buttons have their own fixed row below.
            ui.monospace(format!(
                "#{:<3} L({:+.2},{:+.2}) R({:+.2},{:+.2})",
                state.live.counter,
                state.live.left.x,
                state.live.left.y,
                state.live.right.x,
                state.live.right.y,
            ));
            // Always reserve this row so stick widgets don't jump when buttons appear/disappear.
            let buttons = if state.live.buttons.is_empty() {
                "-".to_string()
            } else {
                format_buttons(state.live.buttons)
            };
            ui.label(format!("ProCon: {buttons}"));
            ui.label(format!("Xbox:  {}", format_xinput(&state.live)));
            ui.colored_label(
                MUTED,
                "Face buttons use position mapping (ProCon B -> Xbox A, A -> B).",
            );
            ui.horizontal(|ui| {
                draw_stick(ui, "Left", state.live.left);
                draw_stick(ui, "Right", state.live.right);
            });

            ui.add_space(8.0);
            ui.colored_label(MUTED, "Close window to hide to tray.");
        });

        // Keep UI responsive while scanning / connected.
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.session.send(Command::Quit);
        self.tray.take();
    }
}

fn main() -> eframe::Result<()> {
    let auto_addr = std::env::args().nth(1).map(|s| s.to_lowercase());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("switch2-procon")
            .with_inner_size([480.0, 560.0])
            .with_min_inner_size([400.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "switch2-procon",
        options,
        Box::new(move |cc| Ok(Box::new(UiApp::new(cc, auto_addr)))),
    )
}
