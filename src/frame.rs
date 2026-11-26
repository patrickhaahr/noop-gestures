const START_OF_FRAME: u8 = 0xAA;
const MIN_FRAME_BYTES: usize = 11;
const MAX_FRAME_BYTES: usize = 8192;
const COMMAND_PACKET_TYPE: u8 = 35;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    GetBatteryLevel,
    RealtimeRawStream { on: bool },
    ImuDataStream { on: bool },
}

impl Command {
    const fn number_and_payload(self) -> (u8, &'static [u8]) {
        match self {
            Self::GetBatteryLevel => (26, &[0]),
            Self::RealtimeRawStream { on: true } => (63, &[1]),
            Self::RealtimeRawStream { on: false } => (63, &[0]),
            Self::ImuDataStream { on: true } => (106, &[1, 1]),
            Self::ImuDataStream { on: false } => (106, &[1, 0]),
        }
    }

    #[must_use]
    pub fn encode(self, seq: u8) -> Vec<u8> {
        let (number, payload) = self.number_and_payload();
        let mut inner = vec![COMMAND_PACKET_TYPE, seq, number];
        inner.extend_from_slice(payload);
        let length = u16::try_from(inner.len() + 4)
            .unwrap_or(u16::MAX)
            .to_le_bytes();
        let mut frame = vec![START_OF_FRAME, length[0], length[1], crc8(&length)];
        frame.extend_from_slice(&inner);
        frame.extend_from_slice(&crc32fast::hash(&inner).to_le_bytes());
        frame
    }
}

pub fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |crc, &byte| {
        (0..8).fold(crc ^ byte, |c, _| {
            if c & 0x80 == 0 {
                c << 1
            } else {
                (c << 1) ^ 0x07
            }
        })
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame(Vec<u8>);

impl Frame {
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn packet_type(&self) -> u8 {
        self.0.get(4).copied().unwrap_or_default()
    }
}

#[derive(Default)]
pub struct Reassembler {
    buffer: Vec<u8>,
    pub rejected: u64,
}

enum Scan {
    NeedMore,
    Skip(usize),
    Reject,
    Complete(usize),
}

impl Reassembler {
    pub fn feed(&mut self, fragment: &[u8]) -> Vec<Frame> {
        self.buffer.extend_from_slice(fragment);
        let mut frames = Vec::new();
        loop {
            match self.scan() {
                Scan::NeedMore => break,
                Scan::Skip(n) => {
                    self.buffer.drain(..n);
                }
                Scan::Reject => {
                    self.rejected += 1;
                    self.buffer.drain(..1);
                }
                Scan::Complete(total) => {
                    frames.push(Frame(self.buffer.drain(..total).collect()));
                }
            }
        }
        frames
    }

    fn scan(&self) -> Scan {
        let Some(start) = self.buffer.iter().position(|&b| b == START_OF_FRAME) else {
            return if self.buffer.is_empty() {
                Scan::NeedMore
            } else {
                Scan::Skip(self.buffer.len())
            };
        };
        if start > 0 {
            return Scan::Skip(start);
        }
        let [_, lo, hi, header_crc, ..] = self.buffer[..] else {
            return Scan::NeedMore;
        };
        if crc8(&[lo, hi]) != header_crc {
            return Scan::Reject;
        }
        let length = usize::from(u16::from_le_bytes([lo, hi]));
        let total = length + 4;
        if !(MIN_FRAME_BYTES..=MAX_FRAME_BYTES).contains(&total) {
            return Scan::Reject;
        }
        let Some(frame) = self.buffer.get(..total) else {
            return Scan::NeedMore;
        };
        let (Some(inner), Some(trailer)) = (frame.get(4..length), frame.get(length..total)) else {
            return Scan::Reject;
        };
        if crc32fast::hash(inner).to_le_bytes()[..] == *trailer {
            Scan::Complete(total)
        } else {
            Scan::Reject
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_battery_request_like_noop() {
        assert_eq!(
            Command::GetBatteryLevel.encode(0),
            [
                0xAA, 0x08, 0x00, 0xA8, 0x23, 0x00, 0x1A, 0x00, 0x17, 0x25, 0xEE, 0x23
            ]
        );
    }

    #[test]
    fn reassembles_fragmented_frame_after_garbage() {
        let frame = Command::RealtimeRawStream { on: true }.encode(3);
        let mut stream = vec![0x01, 0xAA, 0x02];
        stream.extend_from_slice(&frame);
        let (a, b) = stream.split_at(6);
        let mut reassembler = Reassembler::default();
        assert_eq!(reassembler.feed(a), Vec::<Frame>::new());
        assert_eq!(reassembler.feed(b), vec![Frame(frame)]);
    }

    #[test]
    fn rejects_corrupt_trailer_and_recovers() {
        let mut bad = Command::GetBatteryLevel.encode(0);
        if let Some(last) = bad.last_mut() {
            *last ^= 0xFF;
        }
        let good = Command::GetBatteryLevel.encode(1);
        let mut reassembler = Reassembler::default();
        let frames = reassembler.feed(&[bad, good.clone()].concat());
        assert_eq!(frames, vec![Frame(good)]);
        assert!(reassembler.rejected >= 1);
    }
}
