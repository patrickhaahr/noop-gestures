use crate::frame::Frame;

const REALTIME_RAW_PACKET_TYPE: u8 = 43;
const IMU_FRAME_BYTES: usize = 1928;
const SAMPLES: usize = 100;
const ACCEL_OFFSETS: [usize; 3] = [89, 289, 489];
const GYRO_OFFSETS: [usize; 3] = [692, 892, 1092];
const ACCEL_COUNTS_PER_G: f32 = 4096.0;
const GYRO_DPS_PER_COUNT: f32 = 2000.0 / 32768.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub accel_g: [f32; 3],
    pub gyro_dps: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ImuBatch {
    pub strap_unix_s: u32,
    pub samples: Vec<Sample>,
}

impl ImuBatch {
    #[must_use]
    pub fn decode(frame: &Frame) -> Option<Self> {
        let bytes = frame.bytes();
        if frame.packet_type() != REALTIME_RAW_PACKET_TYPE || bytes.len() != IMU_FRAME_BYTES {
            return None;
        }
        let strap_unix_s = u32::from_le_bytes(bytes.get(15..19)?.try_into().ok()?);
        let axis = |base: usize, i: usize| -> Option<f32> {
            let at = base + 2 * i;
            let raw = bytes.get(at..at + 2)?;
            Some(f32::from(i16::from_le_bytes([*raw.first()?, *raw.get(1)?])))
        };
        let samples = (0..SAMPLES)
            .map(|i| {
                let a = ACCEL_OFFSETS.map(|o| axis(o, i));
                let g = GYRO_OFFSETS.map(|o| axis(o, i));
                Some(Sample {
                    accel_g: [a[0]?, a[1]?, a[2]?].map(|v| v / ACCEL_COUNTS_PER_G),
                    gyro_dps: [g[0]?, g[1]?, g[2]?].map(|v| v * GYRO_DPS_PER_COUNT),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            strap_unix_s,
            samples,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Reassembler;

    fn imu_frame(ax0: i16, gz99: i16) -> Vec<u8> {
        let mut inner = vec![0u8; IMU_FRAME_BYTES - 8];
        inner[0] = REALTIME_RAW_PACKET_TYPE;
        inner[11..15].copy_from_slice(&1_700_000_000u32.to_le_bytes());
        inner[85..87].copy_from_slice(&ax0.to_le_bytes());
        inner[1088 + 198..1088 + 200].copy_from_slice(&gz99.to_le_bytes());
        crate::frame::wrap(&inner)
    }

    #[test]
    fn decodes_first_accel_and_last_gyro_sample() {
        let frames = Reassembler::default().feed(&imu_frame(4096, -16384));
        let batch = frames.first().and_then(ImuBatch::decode);
        let batch = batch.unwrap_or_else(|| ImuBatch {
            strap_unix_s: 0,
            samples: vec![],
        });
        assert_eq!(batch.strap_unix_s, 1_700_000_000);
        assert_eq!(batch.samples.len(), SAMPLES);
        assert_eq!(batch.samples.first().map(|s| s.accel_g[0]), Some(1.0));
        assert_eq!(batch.samples.last().map(|s| s.gyro_dps[2]), Some(-1000.0));
    }
}
