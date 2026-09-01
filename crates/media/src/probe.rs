use pandafit_core::media::{ColorInfo, MediaInfo, Track, TrackKind};
use pandafit_core::Estimated;
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("не удалось запустить ffprobe: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("ffprobe завершился с ошибкой: {0}")]
    Failed(String),
    #[error("не удалось разобрать ответ ffprobe: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("в файле нет ни одного потока")]
    Empty,
}

pub trait MediaProbe {
    fn probe(&self, path: &Path) -> Result<MediaInfo, ProbeError>;
}

#[derive(Deserialize)]
struct Raw {
    streams: Vec<RawStream>,
    #[serde(default)]
    format: RawFormat,
}

#[derive(Deserialize, Default)]
struct RawFormat {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct RawStream {
    index: usize,
    codec_name: Option<String>,
    codec_type: String,
    width: Option<u32>,
    height: Option<u32>,
    channels: Option<u32>,
    bit_rate: Option<String>,
    r_frame_rate: Option<String>,
    color_transfer: Option<String>,
    #[serde(default)]
    tags: RawTags,
    #[serde(default)]
    side_data_list: Vec<RawSideData>,
}

#[derive(Deserialize, Default)]
struct RawTags {
    language: Option<String>,
    title: Option<String>,
    #[serde(rename = "BPS")]
    bps: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawSideData {
    dv_profile: Option<u8>,
    dv_bl_signal_compatibility_id: Option<u8>,
}

fn parse_rational(s: &str) -> Option<f64> {
    let (a, b) = s.split_once('/')?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    if b == 0.0 {
        None
    } else {
        Some(a / b)
    }
}

fn guess_bps(codec: &str, channels: Option<u32>, height: Option<u32>) -> u64 {
    match codec {
        "truehd" | "dts" => 4_500_000,
        "flac" | "pcm_s24le" => channels.unwrap_or(2) as u64 * 700_000,
        "ac3" | "eac3" | "aac" | "opus" => channels.unwrap_or(2) as u64 * 110_000,
        "subrip" | "ass" | "srt" => 100,
        "hdmv_pgs_subtitle" => 30_000,
        _ => match height {
            Some(h) if h >= 2160 => 50_000_000,
            Some(h) if h >= 1080 => 12_000_000,
            _ => 3_000_000,
        },
    }
}

fn kind_of(codec_type: &str) -> TrackKind {
    match codec_type {
        "video" => TrackKind::Video,
        "audio" => TrackKind::Audio,
        "subtitle" => TrackKind::Subtitle,
        _ => TrackKind::Attachment,
    }
}

fn is_cover_attachment(kind: TrackKind, codec: &str) -> bool {
    kind == TrackKind::Video && matches!(codec, "mjpeg" | "png")
}

fn dolby_vision_compatibility_implies_hdr10_base(id: Option<u8>) -> bool {
    matches!(id, Some(1) | Some(2))
}

fn color_of(stream: &RawStream) -> Option<ColorInfo> {
    let hdr10 = stream.color_transfer.as_deref() == Some("smpte2084");
    if let Some(side_data) = stream.side_data_list.iter().find(|d| d.dv_profile.is_some()) {
        return Some(ColorInfo::DolbyVision {
            profile: side_data.dv_profile.unwrap_or(8),
            has_hdr10_base: dolby_vision_compatibility_implies_hdr10_base(
                side_data.dv_bl_signal_compatibility_id,
            ) || hdr10,
        });
    }
    if hdr10 {
        return Some(ColorInfo::Hdr10);
    }
    None
}

fn bps_of(stream: &RawStream, codec: &str) -> Estimated<u64> {
    stream
        .tags
        .bps
        .as_deref()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Estimated::exact)
        .or_else(|| stream.bit_rate.as_deref().and_then(|v| v.parse::<u64>().ok()).map(Estimated::exact))
        .unwrap_or_else(|| Estimated::guessed(guess_bps(codec, stream.channels, stream.height)))
}

pub fn parse_ffprobe(json: &str, path: &Path, file_bytes: u64) -> Result<MediaInfo, ProbeError> {
    let raw: Raw = serde_json::from_str(json)?;
    if raw.streams.is_empty() {
        return Err(ProbeError::Empty);
    }
    let duration_s = raw.format.duration.as_deref().and_then(|d| d.parse().ok()).unwrap_or(0.0);

    let mut fps = 0.0;
    let mut color = ColorInfo::Sdr;
    let mut tracks = Vec::new();

    for s in &raw.streams {
        let kind = kind_of(&s.codec_type);
        let codec = s.codec_name.clone().unwrap_or_default();

        if kind == TrackKind::Attachment || is_cover_attachment(kind, &codec) {
            continue;
        }

        if kind == TrackKind::Video {
            if let Some(r) = s.r_frame_rate.as_deref().and_then(parse_rational) {
                fps = r;
            }
            if let Some(detected) = color_of(s) {
                color = detected;
            }
        }

        tracks.push(Track {
            index: s.index,
            kind,
            codec: codec.clone(),
            language: s.tags.language.clone(),
            title: s.tags.title.clone(),
            channels: s.channels,
            width: s.width,
            height: s.height,
            bps: bps_of(s, &codec),
        });
    }

    Ok(MediaInfo { path: path.to_path_buf(), duration_s, fps, tracks, color, file_bytes })
}

pub struct FfprobeProbe;

impl FfprobeProbe {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FfprobeProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl MediaProbe for FfprobeProbe {
    fn probe(&self, path: &Path) -> Result<MediaInfo, ProbeError> {
        let out = Command::new("ffprobe")
            .args([
                "-v", "error", "-of", "json",
                "-show_entries", "format=duration,bit_rate,size",
                "-show_entries",
                "stream=index,codec_name,codec_type,width,height,channels,bit_rate,r_frame_rate,color_transfer,color_primaries",
                "-show_entries", "stream_tags=language,title,BPS",
                "-show_entries",
                "stream_side_data=side_data_type,dv_profile,dv_bl_signal_compatibility_id",
            ])
            .arg(path)
            .output()?;
        if !out.status.success() {
            return Err(ProbeError::Failed(String::from_utf8_lossy(&out.stderr).into_owned()));
        }
        let file_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        parse_ffprobe(&String::from_utf8_lossy(&out.stdout), path, file_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandafit_core::media::{ColorInfo, TrackKind};
    use pandafit_core::Confidence;

    const FIXTURE: &str = include_str!("../tests/fixtures/uhd_remux.handwritten.json");

    fn parsed() -> pandafit_core::MediaInfo {
        parse_ffprobe(FIXTURE, std::path::Path::new("/tmp/thor.mkv"), 61_909_045_268).unwrap()
    }

    #[test]
    fn reads_duration_and_frame_rate() {
        let m = parsed();
        assert!((m.duration_s - 6890.176).abs() < 0.01);
        assert!((m.fps - 23.976).abs() < 0.01);
    }

    #[test]
    fn bps_tag_wins_over_declared_stream_bitrate() {
        let m = parsed();
        let video = m.video().unwrap();
        assert_eq!(video.bps.value, 54_857_256);
        assert_eq!(video.bps.confidence, Confidence::Exact);
    }

    #[test]
    fn falls_back_to_a_guess_when_no_bitrate_is_known() {
        let json = r#"{"streams":[{"index":0,"codec_name":"ac3","codec_type":"audio","channels":6}],
                       "format":{"duration":"100.0"}}"#;
        let m = parse_ffprobe(json, std::path::Path::new("/x.mkv"), 1000).unwrap();
        assert_eq!(m.tracks[0].bps.confidence, Confidence::Guessed);
        assert!(m.tracks[0].bps.value > 0);
    }

    #[test]
    fn detects_dolby_vision_profile_and_hdr10_base() {
        let m = parsed();
        match m.color {
            ColorInfo::DolbyVision { profile, has_hdr10_base } => {
                assert_eq!(profile, 8);
                assert!(has_hdr10_base);
            }
            other => panic!("ожидали Dolby Vision, получили {other:?}"),
        }
    }

    #[test]
    fn attachments_are_kept_out_of_the_track_list() {
        let m = parsed();
        assert!(m.tracks.iter().all(|t| t.kind != TrackKind::Attachment));
    }

    #[test]
    fn counts_seven_audio_tracks_and_seven_subtitles() {
        let m = parsed();
        assert_eq!(m.tracks.iter().filter(|t| t.kind == TrackKind::Audio).count(), 7);
        assert_eq!(m.tracks.iter().filter(|t| t.kind == TrackKind::Subtitle).count(), 7);
    }
}

#[cfg(test)]
mod synthetic_fixture_tests {
    use super::*;
    use pandafit_core::media::{ColorInfo, TrackKind};

    const SYNTHETIC_FIXTURE: &str = include_str!("../tests/fixtures/synthetic.ffprobe.json");

    fn parsed_synthetic() -> pandafit_core::MediaInfo {
        parse_ffprobe(SYNTHETIC_FIXTURE, std::path::Path::new("/tmp/synthetic.mkv"), 420_894).unwrap()
    }

    #[test]
    fn parses_a_real_ffprobe_output_without_error() {
        parsed_synthetic();
    }

    #[test]
    fn finds_one_video_track_and_two_audio_tracks() {
        let m = parsed_synthetic();
        assert_eq!(m.tracks.iter().filter(|t| t.kind == TrackKind::Video).count(), 1);
        assert_eq!(m.tracks.iter().filter(|t| t.kind == TrackKind::Audio).count(), 2);
    }

    #[test]
    fn reads_language_and_title_of_the_first_audio_track() {
        let m = parsed_synthetic();
        let dub = m.tracks.iter().find(|t| t.kind == TrackKind::Audio).unwrap();
        assert_eq!(dub.language.as_deref(), Some("rus"));
        assert_eq!(dub.title.as_deref(), Some("Дубляж"));
    }

    #[test]
    fn reads_duration_and_frame_rate_of_the_synthetic_clip() {
        let m = parsed_synthetic();
        assert!((m.duration_s - 5.0).abs() < 0.01);
        assert!((m.fps - 24.0).abs() < 0.01);
    }

    #[test]
    fn color_defaults_to_sdr_without_dolby_vision_side_data() {
        let m = parsed_synthetic();
        assert_eq!(m.color, ColorInfo::Sdr);
    }
}
