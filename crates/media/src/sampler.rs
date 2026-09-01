use crate::probe::ProbeError;
use pandafit_core::{BitrateProfile, MediaInfo, Sample};
use serde::Deserialize;
use std::process::Command;

pub trait BitrateProfiler {
    fn profile(&self, media: &MediaInfo, track_index: usize)
        -> Result<BitrateProfile, ProbeError>;
}

#[derive(Deserialize)]
struct Packets {
    packets: Vec<Packet>,
}

#[derive(Deserialize)]
struct Packet {
    size: String,
}

pub fn parse_packet_window(json: &str, window_s: f64) -> Option<u64> {
    let p: Packets = serde_json::from_str(json).ok()?;
    if p.packets.is_empty() || window_s <= 0.0 {
        return None;
    }
    let bytes: u64 = p.packets.iter().filter_map(|x| x.size.parse::<u64>().ok()).sum();
    Some((bytes as f64 * 8.0 / window_s).round() as u64)
}

pub struct FfprobeSampler {
    pub window_s: f64,
    pub step_s: f64,
}

impl Default for FfprobeSampler {
    fn default() -> Self {
        Self { window_s: 2.0, step_s: 60.0 }
    }
}

impl FfprobeSampler {
    pub fn sample_times(&self, duration_s: f64) -> Vec<f64> {
        let mut t = 0.0;
        let mut out = Vec::new();
        while t + self.window_s < duration_s {
            out.push(t);
            t += self.step_s;
        }
        out
    }
}

impl BitrateProfiler for FfprobeSampler {
    fn profile(
        &self,
        media: &MediaInfo,
        track_index: usize,
    ) -> Result<BitrateProfile, ProbeError> {
        let mut samples = Vec::new();
        for t in self.sample_times(media.duration_s) {
            let out = Command::new("ffprobe")
                .args(["-v", "error", "-of", "json"])
                .args(["-select_streams", &track_index.to_string()])
                .args(["-show_entries", "packet=size"])
                .args(["-read_intervals", &format!("{t}%+{}", self.window_s)])
                .arg(&media.path)
                .output()?;
            if !out.status.success() {
                continue;
            }
            if let Some(bps) =
                parse_packet_window(&String::from_utf8_lossy(&out.stdout), self.window_s)
            {
                samples.push(Sample { t_s: t, bps });
            }
        }
        Ok(BitrateProfile::from_samples(track_index, samples))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_of_packets_becomes_bits_per_second() {
        let json = r#"{"packets":[{"size":"1000000"},{"size":"1000000"},{"size":"1000000"}]}"#;
        assert_eq!(parse_packet_window(json, 2.0), Some(12_000_000));
    }

    #[test]
    fn empty_window_yields_nothing() {
        assert_eq!(parse_packet_window(r#"{"packets":[]}"#, 2.0), None);
    }

    #[test]
    fn real_fixture_window_yields_a_plausible_bitrate() {
        let json = include_str!("../tests/fixtures/packets.window.json");
        let bps = parse_packet_window(json, 2.0).unwrap();
        assert!(bps > 100_000 && bps < 2_000_000, "получили {bps}");
    }

    #[test]
    fn sample_times_cover_the_whole_film_at_the_configured_step() {
        let s = FfprobeSampler::default();
        let times = s.sample_times(6890.176);
        assert_eq!(times[0], 0.0);
        assert!(times.len() >= 114 && times.len() <= 116, "точек {}", times.len());
        assert!(*times.last().unwrap() < 6890.176);
    }
}
