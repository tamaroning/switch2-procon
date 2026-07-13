//! Switch 2 Pro Controller input report parsing.

use std::fmt;

/// Input characteristic (notify)
pub const INPUT_CHAR_UUID: &str = "7492866c-ec3e-4619-8258-32755ffcc0f9";

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct Buttons: u32 {
        const B      = 1 << 0;
        const A      = 1 << 1;
        const Y      = 1 << 2;
        const X      = 1 << 3;
        const R      = 1 << 4;
        const ZR     = 1 << 5;
        const PLUS   = 1 << 6;
        const RS     = 1 << 7;
        const DDOWN  = 1 << 8;
        const DRIGHT = 1 << 9;
        const DLEFT  = 1 << 10;
        const DUP    = 1 << 11;
        const L      = 1 << 12;
        const ZL     = 1 << 13;
        const MINUS  = 1 << 14;
        const LS     = 1 << 15;
        const HOME   = 1 << 16;
        const GR     = 1 << 17;
        const GL     = 1 << 18;
        const CAPT   = 1 << 19;
    }
}

/// Stick axis. Neutral is 0.0, roughly -1.0 .. 1.0.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stick {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ControllerState {
    pub buttons: Buttons,
    pub left: Stick,
    pub right: Stick,
    /// Report counter at byte 0 (for debugging)
    pub counter: u8,
}

impl ControllerState {
    /// Parse a raw report. Returns None if too short.
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 11 {
            return None;
        }

        let b2 = data[2];
        let b3 = data[3];
        let b4 = data[4];

        let mut buttons = Buttons::empty();
        if b2 & 0x01 != 0 {
            buttons |= Buttons::B;
        }
        if b2 & 0x02 != 0 {
            buttons |= Buttons::A;
        }
        if b2 & 0x04 != 0 {
            buttons |= Buttons::Y;
        }
        if b2 & 0x08 != 0 {
            buttons |= Buttons::X;
        }
        if b2 & 0x10 != 0 {
            buttons |= Buttons::R;
        }
        if b2 & 0x20 != 0 {
            buttons |= Buttons::ZR;
        }
        if b2 & 0x40 != 0 {
            buttons |= Buttons::PLUS;
        }
        if b2 & 0x80 != 0 {
            buttons |= Buttons::RS;
        }

        if b3 & 0x01 != 0 {
            buttons |= Buttons::DDOWN;
        }
        if b3 & 0x02 != 0 {
            buttons |= Buttons::DRIGHT;
        }
        if b3 & 0x04 != 0 {
            buttons |= Buttons::DLEFT;
        }
        if b3 & 0x08 != 0 {
            buttons |= Buttons::DUP;
        }
        if b3 & 0x10 != 0 {
            buttons |= Buttons::L;
        }
        if b3 & 0x20 != 0 {
            buttons |= Buttons::ZL;
        }
        if b3 & 0x40 != 0 {
            buttons |= Buttons::MINUS;
        }
        if b3 & 0x80 != 0 {
            buttons |= Buttons::LS;
        }

        if b4 & 0x01 != 0 {
            buttons |= Buttons::HOME;
        }
        if b4 & 0x04 != 0 {
            buttons |= Buttons::GR;
        }
        if b4 & 0x08 != 0 {
            buttons |= Buttons::GL;
        }
        if b4 & 0x10 != 0 {
            buttons |= Buttons::CAPT;
        }

        // 12-bit values packed in bytes 5-10
        let lx_raw = u16::from(data[5]) | (u16::from(data[6] & 0x0f) << 8);
        let ly_raw = (u16::from(data[6] & 0xf0) >> 4) | (u16::from(data[7]) << 4);
        let rx_raw = u16::from(data[8]) | (u16::from(data[9] & 0x0f) << 8);
        let ry_raw = (u16::from(data[9] & 0xf0) >> 4) | (u16::from(data[10]) << 4);

        Some(Self {
            buttons,
            left: Stick {
                x: stick_axis(lx_raw),
                y: stick_axis(ly_raw),
            },
            right: Stick {
                x: stick_axis(rx_raw),
                y: stick_axis(ry_raw),
            },
            counter: data[0],
        })
    }
}

/// Hardware center (~2048). Physical throw is much smaller than ±2048, so
/// dividing by 2048 leaves sticks around ±0.5–0.6 at full tilt — enough to
/// walk/dash, but many games need near-full magnitude for dash-jump momentum.
const STICK_CENTER: f32 = 2048.0;
const STICK_EXTENT: f32 = 1200.0;
const STICK_DEADZONE_RAW: f32 = 180.0;

fn stick_axis(raw: u16) -> f32 {
    let delta = raw as f32 - STICK_CENTER;
    if delta.abs() <= STICK_DEADZONE_RAW {
        return 0.0;
    }
    let sign = delta.signum();
    let mag = (delta.abs() - STICK_DEADZONE_RAW) / (STICK_EXTENT - STICK_DEADZONE_RAW);
    (sign * mag).clamp(-1.0, 1.0)
}

impl fmt::Display for ControllerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "#{:<3} L({:+.2},{:+.2}) R({:+.2},{:+.2})",
            self.counter, self.left.x, self.left.y, self.right.x, self.right.y
        )?;
        if !self.buttons.is_empty() {
            write!(f, " [{}]", format_buttons(self.buttons))?;
        }
        Ok(())
    }
}

pub fn format_buttons(b: Buttons) -> String {
    const NAMES: &[(Buttons, &str)] = &[
        (Buttons::A, "A"),
        (Buttons::B, "B"),
        (Buttons::X, "X"),
        (Buttons::Y, "Y"),
        (Buttons::L, "L"),
        (Buttons::R, "R"),
        (Buttons::ZL, "ZL"),
        (Buttons::ZR, "ZR"),
        (Buttons::MINUS, "-"),
        (Buttons::PLUS, "+"),
        (Buttons::LS, "LS"),
        (Buttons::RS, "RS"),
        (Buttons::HOME, "HOME"),
        (Buttons::CAPT, "CAPT"),
        (Buttons::GL, "GL"),
        (Buttons::GR, "GR"),
        (Buttons::DUP, "Up"),
        (Buttons::DDOWN, "Down"),
        (Buttons::DLEFT, "Left"),
        (Buttons::DRIGHT, "Right"),
    ];
    NAMES
        .iter()
        .filter(|(flag, _)| b.contains(*flag))
        .map(|(_, name)| *name)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Leading bytes of a real idle report (no buttons, sticks near neutral)
    const IDLE: &[u8] = &[
        0x06, 0x1c, 0x00, 0x00, 0x00, 0xf7, 0x37, 0x8a, 0x77, 0x08, 0x85, 0x30,
    ];

    #[test]
    fn parse_idle_no_buttons() {
        let s = ControllerState::parse(IDLE).unwrap();
        assert!(s.buttons.is_empty());
        assert_eq!(s.left.x, 0.0);
        assert_eq!(s.left.y, 0.0);
        assert_eq!(s.right.x, 0.0);
        assert_eq!(s.right.y, 0.0);
    }

    #[test]
    fn full_tilt_reaches_near_one() {
        // ~center+1200 on X (packed 12-bit in bytes 5-6 low nibble)
        let mut data = IDLE.to_vec();
        let raw: u16 = 2048 + 1200;
        data[5] = (raw & 0xff) as u8;
        data[6] = (data[6] & 0xf0) | ((raw >> 8) as u8 & 0x0f);
        let s = ControllerState::parse(&data).unwrap();
        assert!((s.left.x - 1.0).abs() < 0.02, "x={}", s.left.x);
    }

    #[test]
    fn parse_face_buttons() {
        let mut data = IDLE.to_vec();
        data[2] = 0x02 | 0x08; // A + X
        let s = ControllerState::parse(&data).unwrap();
        assert!(s.buttons.contains(Buttons::A));
        assert!(s.buttons.contains(Buttons::X));
        assert!(!s.buttons.contains(Buttons::B));
    }

    #[test]
    fn reject_short() {
        assert!(ControllerState::parse(&[0; 10]).is_none());
    }
}
