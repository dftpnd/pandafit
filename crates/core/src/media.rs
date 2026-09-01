use crate::Estimated;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
    Attachment,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    /// Индекс потока в исходном файле, как его видит ffmpeg (`-map 0:<index>`).
    pub index: usize,
    pub kind: TrackKind,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    pub channels: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bps: Estimated<u64>,
}

impl Track {
    pub fn bytes_for(&self, seconds: f64) -> Estimated<u64> {
        let bytes = (self.bps.value as f64 * seconds / 8.0).round() as u64;
        Estimated { value: bytes, confidence: self.bps.confidence }
    }

    /// Человекочитаемая подпись для таблицы и разбивки бюджета.
    pub fn label(&self) -> String {
        let mut s = self.codec.to_uppercase();
        if let Some(ch) = self.channels {
            s.push_str(&format!(" {}ch", ch));
        }
        if let Some(h) = self.height {
            s.push_str(&format!(" {}p", h));
        }
        if let Some(l) = &self.language {
            s.push_str(&format!(" {}", l));
        }
        if let Some(t) = &self.title {
            s.push_str(&format!(" «{}»", t));
        }
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorInfo {
    Sdr,
    Hdr10,
    DolbyVision { profile: u8, has_hdr10_base: bool },
}

impl ColorInfo {
    /// Переживёт ли картинка перекодирование, если RPU потеряется.
    pub fn survives_reencode_without_rpu(self) -> bool {
        match self {
            ColorInfo::Sdr | ColorInfo::Hdr10 => true,
            ColorInfo::DolbyVision { has_hdr10_base, .. } => has_hdr10_base,
        }
    }

    pub fn is_dolby_vision(self) -> bool {
        matches!(self, ColorInfo::DolbyVision { .. })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub duration_s: f64,
    pub tracks: Vec<Track>,
    pub color: ColorInfo,
    /// Фактический размер исходного файла на диске.
    pub file_bytes: u64,
    /// Частота кадров видео. Нужна, чтобы покадрово синхронизировать RPU Dolby Vision с обрезкой.
    pub fps: f64,
}

impl MediaInfo {
    pub fn track(&self, index: usize) -> Option<&Track> {
        self.tracks.iter().find(|t| t.index == index)
    }

    pub fn video(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == TrackKind::Video)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Confidence;

    fn ac3_track() -> Track {
        Track {
            index: 1,
            kind: TrackKind::Audio,
            codec: "ac3".into(),
            language: Some("rus".into()),
            title: Some("Dub, Blu-Ray".into()),
            channels: Some(6),
            width: None,
            height: None,
            bps: crate::Estimated::exact(640_000),
        }
    }

    #[test]
    fn bytes_for_converts_bits_per_second_to_bytes() {
        // 640 кбит/с за 6890.176 с
        let b = ac3_track().bytes_for(6890.176);
        assert_eq!(b.value, 551_214_080);
        assert_eq!(b.confidence, Confidence::Exact);
    }

    #[test]
    fn dolby_vision_profile_five_has_no_hdr10_base() {
        let c = ColorInfo::DolbyVision { profile: 5, has_hdr10_base: false };
        assert!(!c.survives_reencode_without_rpu());
        let c = ColorInfo::DolbyVision { profile: 8, has_hdr10_base: true };
        assert!(c.survives_reencode_without_rpu());
    }
}
