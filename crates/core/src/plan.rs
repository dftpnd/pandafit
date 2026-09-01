use crate::media::MediaInfo;
use std::collections::BTreeMap;

pub const UDF_METADATA_BYTES: u64 = 8 * 1024 * 1024;
pub const SAFETY_MARGIN_DIVISOR: u64 = 50; // 2%
pub const SECTOR_BYTES: u64 = 2048;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeRange {
    pub start_s: f64,
    pub end_s: f64,
}

impl TimeRange {
    pub fn full(duration_s: f64) -> Self {
        Self { start_s: 0.0, end_s: duration_s }
    }
    pub fn duration_s(&self) -> f64 {
        (self.end_s - self.start_s).max(0.0)
    }
    pub fn is_full(&self, duration_s: f64) -> bool {
        self.start_s <= 0.0 && self.end_s >= duration_s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Opts {
    pub bitrate_bps: Option<u64>,
    pub channels: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackAction {
    Copy,
    Transcode { codec_id: String, opts: Opts },
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSource {
    /// Ёмкость прочитана из привода.
    Drive(String),
    Preset(String),
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub capacity_bytes: u64,
    pub source: TargetSource,
}

impl Target {
    pub fn preset(name: impl Into<String>, capacity_bytes: u64) -> Self {
        Self { capacity_bytes, source: TargetSource::Preset(name.into()) }
    }
    pub fn drive(node: impl Into<String>, capacity_bytes: u64) -> Self {
        Self { capacity_bytes, source: TargetSource::Drive(node.into()) }
    }

    /// Сколько байт реально можно отдать под файл.
    pub fn usable_bytes(&self) -> u64 {
        self.capacity_bytes
            .saturating_sub(UDF_METADATA_BYTES)
            .saturating_sub(self.capacity_bytes / SAFETY_MARGIN_DIVISOR)
    }
}

/// Номиналы для случая, когда привода нет под рукой.
pub const PRESETS: &[(&str, u64)] = &[
    ("BD-R XL 100 ГБ", 100_103_356_416),
    ("BD-R DL 50 ГБ", 50_050_629_632),
    ("BD-R 25 ГБ", 25_025_314_816),
    ("DVD+R DL 8.5 ГБ", 8_547_991_552),
    ("DVD 4.7 ГБ", 4_700_372_992),
];

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub actions: BTreeMap<usize, TrackAction>,
    pub range: TimeRange,
    pub target: Target,
}

impl Plan {
    /// Стартовое решение: ничего не трогаем, всё копируем целиком.
    pub fn from_media(media: &MediaInfo, target: Target) -> Self {
        let actions = media.tracks.iter().map(|t| (t.index, TrackAction::Copy)).collect();
        Self { actions, range: TimeRange::full(media.duration_s), target }
    }

    pub fn action(&self, index: usize) -> &TrackAction {
        self.actions.get(&index).unwrap_or(&TrackAction::Drop)
    }

    pub fn set_action(&mut self, index: usize, action: TrackAction) {
        self.actions.insert(index, action);
    }

    pub fn kept_indices(&self) -> Vec<usize> {
        self.actions
            .iter()
            .filter(|(_, a)| **a != TrackAction::Drop)
            .map(|(i, _)| *i)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::*;

    fn media() -> MediaInfo {
        MediaInfo {
            path: "/tmp/x.mkv".into(),
            duration_s: 100.0,
            tracks: vec![Track {
                index: 0,
                kind: TrackKind::Video,
                codec: "hevc".into(),
                language: None,
                title: None,
                channels: None,
                width: Some(3840),
                height: Some(2160),
                bps: crate::Estimated::exact(50_000_000),
            }],
            color: ColorInfo::Hdr10,
            file_bytes: 625_000_000,
            fps: 25.0,
        }
    }

    #[test]
    fn default_plan_copies_everything_full_length() {
        let p = Plan::from_media(&media(), Target::preset("BD-R DL", 50_050_629_632));
        assert_eq!(p.action(0), &TrackAction::Copy);
        assert_eq!(p.range.start_s, 0.0);
        assert_eq!(p.range.end_s, 100.0);
    }

    #[test]
    fn usable_capacity_subtracts_udf_and_safety_margin() {
        let t = Target::preset("BD-R DL", 50_050_629_632);
        // 8 МиБ метаданных UDF + 2% страховки
        let expected = 50_050_629_632 - 8 * 1024 * 1024 - 50_050_629_632 / 50;
        assert_eq!(t.usable_bytes(), expected);
    }

    #[test]
    fn time_range_reports_its_own_duration() {
        let r = TimeRange { start_s: 10.0, end_s: 70.0 };
        assert_eq!(r.duration_s(), 60.0);
    }
}
