//! Virtual gamepad output.

use crate::input::{Buttons, ControllerState};
use tokio::sync::watch;

/// Shared interface for output backends.
pub trait GamepadOutput: Send {
    fn update(&mut self, state: &ControllerState) -> anyhow::Result<()>;
}

/// Map a stick axis into the XInput i16 range.
fn axis_to_i16(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Nintendo layout → Xbox layout (position-based).
/// Switch bottom=B / right=A maps to Xbox bottom=A / right=B.
pub fn to_xinput(state: &ControllerState) -> XInputState {
    let b = state.buttons;
    let mut buttons = 0u16;

    // face: position mapping
    if b.contains(Buttons::B) {
        buttons |= XBTN_A;
    }
    if b.contains(Buttons::A) {
        buttons |= XBTN_B;
    }
    if b.contains(Buttons::Y) {
        buttons |= XBTN_X;
    }
    if b.contains(Buttons::X) {
        buttons |= XBTN_Y;
    }

    if b.contains(Buttons::L) {
        buttons |= XBTN_LB;
    }
    if b.contains(Buttons::R) {
        buttons |= XBTN_RB;
    }
    if b.contains(Buttons::MINUS) {
        buttons |= XBTN_BACK;
    }
    if b.contains(Buttons::PLUS) {
        buttons |= XBTN_START;
    }
    if b.contains(Buttons::LS) {
        buttons |= XBTN_LTHUMB;
    }
    if b.contains(Buttons::RS) {
        buttons |= XBTN_RTHUMB;
    }
    if b.contains(Buttons::DUP) {
        buttons |= XBTN_UP;
    }
    if b.contains(Buttons::DDOWN) {
        buttons |= XBTN_DOWN;
    }
    if b.contains(Buttons::DLEFT) {
        buttons |= XBTN_LEFT;
    }
    if b.contains(Buttons::DRIGHT) {
        buttons |= XBTN_RIGHT;
    }
    if b.contains(Buttons::HOME) {
        buttons |= XBTN_GUIDE;
    }

    XInputState {
        buttons,
        left_trigger: if b.contains(Buttons::ZL) { 255 } else { 0 },
        right_trigger: if b.contains(Buttons::ZR) { 255 } else { 0 },
        thumb_lx: axis_to_i16(state.left.x),
        thumb_ly: axis_to_i16(state.left.y),
        thumb_rx: axis_to_i16(state.right.x),
        thumb_ry: axis_to_i16(state.right.y),
    }
}

/// Format the Xbox-side buttons/triggers the game will see after `to_xinput`.
pub fn format_xinput(state: &ControllerState) -> String {
    let x = to_xinput(state);
    let mut parts = Vec::new();
    const NAMES: &[(u16, &str)] = &[
        (XBTN_A, "A"),
        (XBTN_B, "B"),
        (XBTN_X, "X"),
        (XBTN_Y, "Y"),
        (XBTN_LB, "LB"),
        (XBTN_RB, "RB"),
        (XBTN_BACK, "Back"),
        (XBTN_START, "Start"),
        (XBTN_LTHUMB, "LS"),
        (XBTN_RTHUMB, "RS"),
        (XBTN_UP, "Up"),
        (XBTN_DOWN, "Down"),
        (XBTN_LEFT, "Left"),
        (XBTN_RIGHT, "Right"),
        (XBTN_GUIDE, "Guide"),
    ];
    for (flag, name) in NAMES {
        if x.buttons & flag != 0 {
            parts.push(*name);
        }
    }
    if x.left_trigger > 0 {
        parts.push("LT");
    }
    if x.right_trigger > 0 {
        parts.push("RT");
    }
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(" ")
    }
}

/// XInput-compatible intermediate state (before ViGEm).
#[derive(Debug, Clone, Copy, Default)]
pub struct XInputState {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
}

// XInput button flags (XUSB_GAMEPAD_*)
const XBTN_UP: u16 = 0x0001;
const XBTN_DOWN: u16 = 0x0002;
const XBTN_LEFT: u16 = 0x0004;
const XBTN_RIGHT: u16 = 0x0008;
const XBTN_START: u16 = 0x0010;
const XBTN_BACK: u16 = 0x0020;
const XBTN_LTHUMB: u16 = 0x0040;
const XBTN_RTHUMB: u16 = 0x0080;
const XBTN_LB: u16 = 0x0100;
const XBTN_RB: u16 = 0x0200;
const XBTN_GUIDE: u16 = 0x0400; // Xbox Guide button
const XBTN_A: u16 = 0x1000;
const XBTN_B: u16 = 0x2000;
const XBTN_X: u16 = 0x4000;
const XBTN_Y: u16 = 0x8000;

/// XInput motor speeds from the virtual pad (large = left/LF, small = right/HF).
pub type RumbleMotors = (u8, u8);

/// Output handle: virtual pad updates + rumble feedback channel.
pub struct OutputBundle {
    pub gamepad: Box<dyn GamepadOutput>,
    pub rumble_rx: watch::Receiver<RumbleMotors>,
}

/// No-op output (for parse verification).
pub struct NullOutput;

impl GamepadOutput for NullOutput {
    fn update(&mut self, _state: &ControllerState) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
mod vigem_out {
    use super::*;
    use std::thread::JoinHandle;
    use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

    pub struct VigemOutput {
        target: Xbox360Wired<Client>,
        _rumble_thread: Option<JoinHandle<()>>,
    }

    impl VigemOutput {
        pub fn new(rumble_tx: watch::Sender<RumbleMotors>) -> anyhow::Result<Self> {
            let client = Client::connect().map_err(|e| {
                anyhow::anyhow!(
                    "Cannot connect to ViGEmBus: {e}\n\
                    Install the driver from https://github.com/nefarius/ViGEmBus/releases and retry"
                )
            })?;
            let mut target = Xbox360Wired::new(client, TargetId::XBOX360_WIRED);
            target
                .plugin()
                .map_err(|e| anyhow::anyhow!("Virtual pad plugin failed: {e}"))?;
            target
                .wait_ready()
                .map_err(|e| anyhow::anyhow!("Virtual pad wait_ready failed: {e}"))?;

            let notification = target
                .request_notification()
                .map_err(|e| anyhow::anyhow!("ViGEm rumble notification failed: {e}"))?;
            let rumble_thread = notification.spawn_thread(move |_, data| {
                let _ = rumble_tx.send((data.large_motor, data.small_motor));
            });

            Ok(Self {
                target,
                _rumble_thread: Some(rumble_thread),
            })
        }
    }

    impl GamepadOutput for VigemOutput {
        fn update(&mut self, state: &ControllerState) -> anyhow::Result<()> {
            let x = to_xinput(state);
            let gamepad = XGamepad {
                buttons: XButtons(x.buttons),
                left_trigger: x.left_trigger,
                right_trigger: x.right_trigger,
                thumb_lx: x.thumb_lx,
                thumb_ly: x.thumb_ly,
                thumb_rx: x.thumb_rx,
                thumb_ry: x.thumb_ry,
            };
            self.target
                .update(&gamepad)
                .map_err(|e| anyhow::anyhow!("ViGEm update failed: {e}"))?;
            Ok(())
        }
    }
}

#[cfg(windows)]
pub use vigem_out::VigemOutput;

/// ViGEm / virtual pad availability for UI display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VigemStatus {
    Ready,
    Unavailable { message: String },
    Unsupported,
}

impl VigemStatus {
    pub const RELEASES_URL: &'static str = "https://github.com/nefarius/ViGEmBus/releases";
}

/// Create the best available output. Windows uses ViGEm; otherwise / on failure uses Null.
pub fn create_output() -> (OutputBundle, VigemStatus) {
    let (rumble_tx, rumble_rx) = watch::channel((0u8, 0u8));

    #[cfg(windows)]
    {
        match VigemOutput::new(rumble_tx) {
            Ok(out) => (
                OutputBundle {
                    gamepad: Box::new(out),
                    rumble_rx,
                },
                VigemStatus::Ready,
            ),
            Err(e) => (
                OutputBundle {
                    gamepad: Box::new(NullOutput),
                    rumble_rx,
                },
                VigemStatus::Unavailable {
                    message: e.to_string(),
                },
            ),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = rumble_tx;
        (
            OutputBundle {
                gamepad: Box::new(NullOutput),
                rumble_rx,
            },
            VigemStatus::Unsupported,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::ControllerState;

    #[test]
    fn nintendo_b_maps_to_xbox_a() {
        let mut s = ControllerState::default();
        s.buttons = Buttons::B;
        let x = to_xinput(&s);
        assert_eq!(x.buttons & XBTN_A, XBTN_A);
        assert_eq!(x.buttons & XBTN_B, 0);
    }

    #[test]
    fn home_maps_to_guide() {
        let mut s = ControllerState::default();
        s.buttons = Buttons::HOME;
        let x = to_xinput(&s);
        assert_eq!(x.buttons & XBTN_GUIDE, XBTN_GUIDE);
    }
}
