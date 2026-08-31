use crate::codec::{CodecRegistry, EncodeCtx};
use crate::media::{MediaInfo, TrackKind};
use crate::plan::{Plan, TrackAction, SECTOR_BYTES};
use crate::profile::BitrateProfile;
use crate::{Confidence, Estimated};

/// Накладные расходы контейнера Matroska: заголовки блоков, cues, seekhead.
const CONTAINER_RATIO_PERMILLE: u64 = 5;
const CONTAINER_FIXED_BYTES: u64 = 2 * 1000 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct TrackSize {
    pub index: usize,
    pub label: String,
    pub bytes: Estimated<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Верхняя граница интервала ниже полезной ёмкости.
    Fits,
    /// Точка влезает, верхняя граница — нет.
    Tight,
    Overflow { excess: u64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SizeBreakdown {
    pub tracks: Vec<TrackSize>,
    pub container: Estimated<u64>,
    pub total: Estimated<u64>,
    /// Сумма покомпонентных верхних границ. Точные слагаемые не раздуваются
    /// из-за того, что оценочным является одно из соседних.
    pub total_upper: u64,
    pub capacity_bytes: u64,
    pub usable_bytes: u64,
    pub verdict: Verdict,
}

fn round_up_to_sector(bytes: u64) -> u64 {
    bytes.div_ceil(SECTOR_BYTES) * SECTOR_BYTES
}

pub fn estimate(
    media: &MediaInfo,
    plan: &Plan,
    registry: &CodecRegistry,
    profile: Option<&BitrateProfile>,
) -> SizeBreakdown {
    let duration = plan.range.duration_s();
    let full_length = plan.range.is_full(media.duration_s);
    let mut tracks = Vec::new();

    for track in &media.tracks {
        if track.kind == TrackKind::Attachment {
            continue;
        }
        let bytes = match plan.action(track.index) {
            TrackAction::Drop => continue,
            TrackAction::Copy => {
                // Для видео с известной кривой считаем интегралом, иначе линейно.
                match profile {
                    Some(p) if p.track_index == track.index && !p.is_empty() && !full_length => {
                        Estimated::sampled(p.bytes_between(plan.range.start_s, plan.range.end_s))
                    }
                    _ => {
                        let mut e = track.bytes_for(duration);
                        if !full_length && e.confidence == Confidence::Exact {
                            // Без кривой экономия от обрезки — только догадка.
                            e.confidence = Confidence::Guessed;
                        }
                        e
                    }
                }
            }
            TrackAction::Transcode { codec_id, opts } => {
                let Some(profile_codec) = registry.get(codec_id) else {
                    continue;
                };
                let ctx = EncodeCtx { src: track, opts, duration_s: duration };
                let bps = profile_codec.estimate_bps(&ctx);
                Estimated {
                    value: (bps.value as f64 * duration / 8.0).round() as u64,
                    confidence: bps.confidence,
                }
            }
        };
        tracks.push(TrackSize { index: track.index, label: track.label(), bytes });
    }

    let payload = Estimated::sum(tracks.iter().map(|t| t.bytes));
    let container = Estimated::sampled(
        payload.value * CONTAINER_RATIO_PERMILLE / 1000 + CONTAINER_FIXED_BYTES,
    );
    let total = Estimated {
        value: round_up_to_sector(payload.value + container.value),
        confidence: payload.confidence.max(container.confidence),
    };

    // Вычисляем total_upper как сумму покомпонентных верхних границ
    let total_upper_sum = tracks.iter().map(|t| t.bytes.upper()).sum::<u64>() + container.upper();
    let total_upper = round_up_to_sector(total_upper_sum);

    let usable = plan.target.usable_bytes();
    let verdict = if total_upper <= usable {
        Verdict::Fits
    } else if total.value <= usable {
        Verdict::Tight
    } else {
        Verdict::Overflow { excess: total.value - usable }
    };

    SizeBreakdown {
        tracks,
        container,
        total,
        total_upper,
        capacity_bytes: plan.target.capacity_bytes,
        usable_bytes: usable,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::thor;
    use crate::codec::CodecRegistry;
    use crate::plan::{Opts, Plan, Target, TrackAction};

    fn bd50() -> Target {
        Target::preset("BD-R DL", 50_050_629_632)
    }

    /// Основной сценарий: выбрасываем HD-дорожки, видео копируем.
    fn keep_two_ac3(plan: &mut Plan) {
        for i in [2usize, 3, 4, 5, 7] {
            plan.set_action(i, TrackAction::Drop);
        }
    }

    #[test]
    fn dropping_hd_audio_makes_the_remux_fit() {
        let m = thor();
        let reg = CodecRegistry::with_builtins();
        let mut plan = Plan::from_media(&m, bd50());
        keep_two_ac3(&mut plan);

        let b = estimate(&m, &plan, &reg, None);
        // видео 47.25 ГБ + два AC3 по 0.55 ГБ + субтитры + контейнер
        assert!(b.total.value > 48_000_000_000, "получилось {}", b.total.value);
        assert!(b.total.value < 49_500_000_000, "получилось {}", b.total.value);
        assert!(matches!(b.verdict, Verdict::Fits), "вердикт {:?}", b.verdict);
    }

    #[test]
    fn untouched_file_overflows_and_reports_the_excess() {
        let m = thor();
        let reg = CodecRegistry::with_builtins();
        let plan = Plan::from_media(&m, bd50());
        let b = estimate(&m, &plan, &reg, None);
        match b.verdict {
            Verdict::Overflow { excess } => assert!(excess > 10_000_000_000),
            other => panic!("ожидали переполнение, получили {other:?}"),
        }
    }

    #[test]
    fn dropped_tracks_do_not_appear_in_breakdown() {
        let m = thor();
        let reg = CodecRegistry::with_builtins();
        let mut plan = Plan::from_media(&m, bd50());
        keep_two_ac3(&mut plan);
        let b = estimate(&m, &plan, &reg, None);
        assert!(b.tracks.iter().all(|t| t.index != 5));
    }

    #[test]
    fn transcoding_dts_to_ac3_uses_the_new_bitrate() {
        let m = thor();
        let reg = CodecRegistry::with_builtins();
        let mut plan = Plan::from_media(&m, bd50());
        plan.set_action(
            2,
            TrackAction::Transcode {
                codec_id: "ac3".into(),
                opts: Opts { bitrate_bps: Some(640_000), ..Default::default() },
            },
        );
        let b = estimate(&m, &plan, &reg, None);
        let t = b.tracks.iter().find(|t| t.index == 2).unwrap();
        assert_eq!(t.bytes.value, 551_214_080);
    }

    #[test]
    fn tight_result_is_flagged_when_only_the_upper_bound_overflows() {
        let m = thor();
        let reg = CodecRegistry::with_builtins();
        let mut plan = Plan::from_media(&m, bd50());
        keep_two_ac3(&mut plan);
        // Перекодирование видео делает его оценку Sampled — окно между точкой
        // и верхней границей становится широким, и тест перестаёт быть хрупким.
        plan.set_action(
            0,
            TrackAction::Transcode {
                codec_id: "hevc_nvenc".into(),
                opts: Opts { bitrate_bps: Some(40_000_000), ..Default::default() },
            },
        );

        let b0 = estimate(&m, &plan, &reg, None);
        assert!(b0.total_upper > b0.total.value, "окно достоверности схлопнулось");

        // Целимся так, чтобы полезная ёмкость легла ровно между точкой и границей.
        let wanted_usable = (b0.total.value + b0.total_upper) / 2;
        let capacity = (wanted_usable + crate::plan::UDF_METADATA_BYTES) * 50 / 49;
        plan.target = Target::preset("узкая", capacity);

        let b = estimate(&m, &plan, &reg, None);
        assert!(b.total.value <= b.usable_bytes);
        assert!(b.total_upper > b.usable_bytes);
        assert!(matches!(b.verdict, Verdict::Tight), "вердикт {:?}", b.verdict);
    }

    #[test]
    fn control_numbers_match_expectations() {
        let m = thor();
        let reg = CodecRegistry::with_builtins();
        let mut plan = Plan::from_media(&m, bd50());
        keep_two_ac3(&mut plan);

        let b = estimate(&m, &plan, &reg, None);

        // Контрольные числа из письма
        assert_eq!(b.tracks.iter().map(|t| t.bytes.value).sum::<u64>(), 48_349_501_010, "payload должно быть 48 349 501 010");
        assert_eq!(b.container.value, 243_747_505, "контейнер должен быть 243 747 505");
        assert_eq!(b.total.value, 48_593_250_304, "total.value должно быть 48 593 250 304");
        assert_eq!(b.total_upper, 48_605_435_904, "total_upper должно быть 48 605 435 904");
        assert_eq!(b.usable_bytes, 49_041_228_432, "usable должно быть 49 041 228 432");
    }
}
