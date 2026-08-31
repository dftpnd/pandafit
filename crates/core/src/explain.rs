use crate::codec::{CodecRegistry, EncodeCtx};
use crate::media::{MediaInfo, TrackKind};
use crate::note::{Level, Note, NoteTarget};
use crate::plan::{Plan, TrackAction};

pub fn has_blockers(notes: &[Note]) -> bool {
    notes.iter().any(|n| n.level == Level::Blocker)
}

pub fn explain(media: &MediaInfo, plan: &Plan, registry: &CodecRegistry) -> Vec<Note> {
    let mut notes = Vec::new();
    let duration = plan.range.duration_s();

    // Заметки самих кодеков — источник правды о последствиях конкретного выбора.
    for track in &media.tracks {
        if let TrackAction::Transcode { codec_id, opts } = plan.action(track.index) {
            if let Some(profile) = registry.get(codec_id) {
                let ctx = EncodeCtx { src: track, opts, duration_s: duration };
                notes.extend(profile.notes(&ctx));
            }
        }
    }

    // Dolby Vision при перекодировании видео.
    if let Some(video) = media.video() {
        if let TrackAction::Transcode { codec_id, .. } = plan.action(video.index) {
            let preserves = registry.get(codec_id).is_some_and(|p| p.preserves_dolby_vision());
            if media.color.is_dolby_vision() && !preserves {
                if media.color.survives_reencode_without_rpu() {
                    notes.push(Note::warning(
                        NoteTarget::Track(video.index),
                        "Dolby Vision будет потерян, останется HDR10. \
                         На телевизоре без поддержки DV разницы не будет, \
                         на телевизоре с DV тёмные сцены станут чуть менее детальными.",
                    ));
                } else {
                    notes.push(Note::blocker(
                        NoteTarget::Track(video.index),
                        "Dolby Vision Profile 5 нельзя перекодировать без сохранения RPU: \
                         без него цвет развалится в зелёно-фиолетовую кашу. \
                         Выберите профиль с сохранением DV или копируйте видео как есть.",
                    ));
                }
            }
            if !media.color.is_dolby_vision() && media.color == crate::media::ColorInfo::Hdr10 {
                notes.push(Note::info(
                    NoteTarget::Track(video.index),
                    "Метаданные HDR10 будут перенесены в результат — \
                     без них картинка на HDR-телевизоре стала бы блёклой.",
                ));
            }
        }
    }

    // Хотя бы одна звуковая дорожка должна остаться.
    let has_audio = media
        .tracks
        .iter()
        .any(|t| t.kind == TrackKind::Audio && *plan.action(t.index) != TrackAction::Drop);
    if !has_audio {
        notes.push(Note::blocker(
            NoteTarget::Whole,
            "Не осталось ни одной звуковой дорожки — фильм будет немым",
        ));
    }

    // Видео обязано остаться.
    let has_video = media
        .tracks
        .iter()
        .any(|t| t.kind == TrackKind::Video && *plan.action(t.index) != TrackAction::Drop);
    if !has_video {
        notes.push(Note::blocker(NoteTarget::Whole, "Не осталось видеодорожки"));
    }

    // Обрезка.
    if !plan.range.is_full(media.duration_s) {
        if let Some(video) = media.video() {
            if *plan.action(video.index) == TrackAction::Copy {
                notes.push(Note::warning(
                    NoteTarget::Trim,
                    "При копировании видео рез возможен только по ключевым кадрам — \
                     фактическая точка сдвинется на несколько секунд, она будет показана рядом",
                ));
            }
        }
        notes.push(Note::info(
            NoteTarget::Trim,
            "Титры весят меньше среднего кадра, поэтому экономия от обрезки \
             обычно в разы ниже, чем доля отрезанного времени",
        ));
        if plan.range.duration_s() <= 0.0 {
            notes.push(Note::blocker(NoteTarget::Trim, "Пустой диапазон обрезки"));
        }
    }

    notes
}

#[cfg(test)]
#[path = "../tests/common/fixtures.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::fixtures::thor;
    use super::{explain, has_blockers};
    use crate::codec::CodecRegistry;
    use crate::media::ColorInfo;
    use crate::note::Level;
    use crate::plan::{Opts, Plan, Target, TrackAction};

    fn plan_with_video_transcode(codec_id: &str) -> (crate::media::MediaInfo, Plan) {
        let m = thor();
        let mut p = Plan::from_media(&m, Target::preset("BD-R DL", 50_050_629_632));
        p.set_action(
            0,
            TrackAction::Transcode {
                codec_id: codec_id.into(),
                opts: Opts { bitrate_bps: Some(26_000_000), height: Some(2160), ..Default::default() },
            },
        );
        (m, p)
    }

    #[test]
    fn transcoding_profile_eight_warns_but_allows() {
        let (m, p) = plan_with_video_transcode("hevc_nvenc");
        let notes = explain(&m, &p, &CodecRegistry::with_builtins());
        assert!(notes.iter().any(|n| n.level == Level::Warning && n.text.contains("Dolby Vision")));
        assert!(!has_blockers(&notes));
    }

    #[test]
    fn transcoding_profile_five_is_blocked() {
        let (mut m, p) = plan_with_video_transcode("hevc_nvenc");
        m.color = ColorInfo::DolbyVision { profile: 5, has_hdr10_base: false };
        let notes = explain(&m, &p, &CodecRegistry::with_builtins());
        assert!(has_blockers(&notes));
        assert!(notes.iter().any(|n| n.level == Level::Blocker && n.text.contains("Profile 5")));
    }

    #[test]
    fn x265_profile_removes_the_dolby_vision_warning() {
        let (m, p) = plan_with_video_transcode("hevc_x265_dv");
        let notes = explain(&m, &p, &CodecRegistry::with_builtins());
        assert!(!notes
            .iter()
            .any(|n| n.level == Level::Warning && n.text.contains("Dolby Vision будет потерян")));
    }

    #[test]
    fn dropping_every_audio_track_is_blocked() {
        let m = thor();
        let mut p = Plan::from_media(&m, Target::preset("BD-R DL", 50_050_629_632));
        for i in 1..=7 {
            p.set_action(i, TrackAction::Drop);
        }
        let notes = explain(&m, &p, &CodecRegistry::with_builtins());
        assert!(notes.iter().any(|n| n.level == Level::Blocker && n.text.contains("звук")));
    }

    #[test]
    fn trimming_a_copied_stream_warns_about_keyframes() {
        let m = thor();
        let mut p = Plan::from_media(&m, Target::preset("BD-R DL", 50_050_629_632));
        p.range.end_s = 6700.0;
        let notes = explain(&m, &p, &CodecRegistry::with_builtins());
        assert!(notes.iter().any(|n| n.text.contains("ключев")));
    }
}
