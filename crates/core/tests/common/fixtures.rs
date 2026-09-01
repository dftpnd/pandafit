use pandafit_core::media::*;
use pandafit_core::Estimated;

pub fn thor() -> MediaInfo {
    let audio = |index, codec: &str, lang: &str, title: &str, ch, bps| Track {
        index,
        kind: TrackKind::Audio,
        codec: codec.into(),
        language: Some(lang.into()),
        title: Some(title.into()),
        channels: Some(ch),
        width: None,
        height: None,
        bps: Estimated::exact(bps),
    };
    MediaInfo {
        path: "/home/mgu/Downloads/thor.mkv".into(),
        duration_s: 6890.176,
        color: ColorInfo::DolbyVision { profile: 8, has_hdr10_base: true },
        file_bytes: 61_909_045_268,
        tracks: vec![
            Track {
                index: 0,
                kind: TrackKind::Video,
                codec: "hevc".into(),
                language: None,
                title: None,
                channels: None,
                width: Some(3840),
                height: Some(2160),
                bps: Estimated::exact(54_857_256),
            },
            audio(1, "ac3", "rus", "Dub, Blu-Ray", 6, 640_000),
            audio(2, "dts", "rus", "А. Гаврилов", 8, 5_008_134),
            audio(3, "dts", "rus", "Ю. Сербин", 8, 5_064_566),
            audio(4, "ac3", "ukr", "Dub, R5", 6, 448_000),
            audio(5, "truehd", "eng", "Atmos", 8, 4_937_856),
            audio(6, "ac3", "eng", "Surround", 6, 640_000),
            audio(7, "ac3", "eng", "Commentary", 2, 224_000),
            Track {
                index: 8,
                kind: TrackKind::Subtitle,
                codec: "subrip".into(),
                language: Some("rus".into()),
                title: Some("Blu-Ray".into()),
                channels: None,
                width: None,
                height: None,
                bps: Estimated::exact(63),
            },
        ],
    }
}
