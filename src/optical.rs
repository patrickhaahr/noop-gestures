use serde::{Deserialize, Serialize};

use crate::frame::Frame;
use crate::timeline::Stamped;

const REALTIME_RAW_PACKET_TYPE: u8 = 43;
const OPTICAL_FRAME_BYTES: usize = 1932;
const WAVEFORM_OFFSET: usize = 42;
const STRIDE: usize = 4;
const SAMPLES: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpticalSample {
    pub t: u64,
    pub p: i32,
}

impl Stamped for OpticalSample {
    type Raw = i32;

    fn stamp(t: u64, raw: &i32) -> Self {
        Self { t, p: *raw }
    }

    fn t(&self) -> u64 {
        self.t
    }
}

#[must_use]
pub fn decode(frame: &Frame) -> Option<Vec<i32>> {
    let bytes = frame.bytes();
    if frame.packet_type() != REALTIME_RAW_PACKET_TYPE || bytes.len() != OPTICAL_FRAME_BYTES {
        return None;
    }
    (0..SAMPLES)
        .map(|i| {
            let at = WAVEFORM_OFFSET + STRIDE * i;
            let raw = bytes.get(at..at + 3)?;
            Some(i32::from_le_bytes([0, *raw.first()?, *raw.get(1)?, *raw.get(2)?]) >> 8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Reassembler;

    fn optical_frame(first: [u8; 3], last: [u8; 3]) -> Vec<u8> {
        let mut inner = vec![0u8; OPTICAL_FRAME_BYTES - 8];
        inner[0] = REALTIME_RAW_PACKET_TYPE;
        inner[38..41].copy_from_slice(&first);
        inner[38 + 4 * 99..41 + 4 * 99].copy_from_slice(&last);
        crate::frame::wrap(&inner)
    }

    #[test]
    fn decodes_signed_24_bit_samples() {
        let frames =
            Reassembler::default().feed(&optical_frame([0x81, 0xDD, 0x03], [0xFF, 0xFF, 0xFF]));
        let samples = frames.first().and_then(decode).unwrap_or_default();
        assert_eq!(
            (samples.len(), samples.first(), samples.last()),
            (SAMPLES, Some(&253_313), Some(&-1))
        );
    }
}
