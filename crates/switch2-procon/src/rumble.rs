//! Switch 2 Pro Controller HD rumble (LRA) packet encoding.

/// BLE write characteristic for Pro Controller 2 HD rumble output.
pub const RUMBLE_CHAR_UUID: &str = "cc483f51-9258-427d-a939-630c31f72b05";

const LF_FREQ: u16 = 0x0E1;
const HF_FREQ: u16 = 0x1E1;

/// Encode one 5-byte LRA operation (LF + HF frequency/amplitude).
fn encode_lra_op(lf_amp: u16, hf_amp: u16) -> [u8; 5] {
    let mut value: u64 = 0;
    value |= u64::from(LF_FREQ) & 0x1FF;
    value |= (u64::from(lf_amp) & 0x3FF) << 10;
    value |= (u64::from(HF_FREQ) & 0x1FF) << 20;
    value |= (u64::from(hf_amp) & 0x3FF) << 30;
    let bytes = value.to_le_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4]]
}

/// Build a 16-byte motor block: state + 3 identical LRA ops.
fn motor_block(seq: u8, amp: u8) -> [u8; 16] {
    let lf_amp = (u32::from(amp) * 0x3FF / 255) as u16;
    let op = encode_lra_op(lf_amp, 0);
    // 0x70 = enable + ops_cnt=3; low nibble is transaction id.
    let mut block = [0u8; 16];
    block[0] = 0x70 | (seq & 0x0F);
    block[1..6].copy_from_slice(&op);
    block[6..11].copy_from_slice(&op);
    block[11..16].copy_from_slice(&op);
    block
}

/// Build a 42-byte HD rumble packet for independent left/right motors.
///
/// `left` / `right` are XInput-style 0..=255 amplitudes
/// (large motor → left, small motor → right).
pub fn build_packet(seq: u8, left: u8, right: u8) -> [u8; 42] {
    let mut pkt = [0u8; 42];
    pkt[1..17].copy_from_slice(&motor_block(seq, left));
    pkt[17..33].copy_from_slice(&motor_block(seq, right));
    pkt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_layout_is_42_bytes_with_zero_prefix() {
        let pkt = build_packet(1, 128, 64);
        assert_eq!(pkt.len(), 42);
        assert_eq!(pkt[0], 0);
        assert_eq!(pkt[1], 0x71);
        assert_eq!(pkt[17], 0x71);
        // trailing pad
        assert!(pkt[33..].iter().all(|&b| b == 0));
    }

    #[test]
    fn zero_amp_still_sends_valid_state() {
        let pkt = build_packet(0, 0, 0);
        assert_eq!(pkt[1], 0x70);
        assert_eq!(pkt[2..7], [0xE1, 0x00, 0x10, 0x1E, 0x00]);
    }
}
