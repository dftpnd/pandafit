use crate::codec::{CodecProfile, EncodeCtx};
use crate::media::{Track, TrackKind};
use crate::note::{Note, NoteTarget};
use crate::plan::Opts;
use crate::Estimated;

/// Скорость кодирования относительно длительности материала, для прогноза времени.
const X265_SLOWDOWN_2160P: f64 = 2.6;

struct VideoCodec {
    id: &'static str,
    label: &'static str,
    ffmpeg_name: &'static str,
    preserves_dv: bool,
    /// Во сколько раз этот кодек эффективнее HEVC при том же качестве.
    efficiency: f64,
}

fn scale_filter(opts: &Opts, src: &Track) -> Option<String> {
    let target_h = opts.height?;
    if Some(target_h) == src.height {
        return None;
    }
    Some(format!("scale=-2:{}", target_h))
}

impl CodecProfile for VideoCodec {
    fn id(&self) -> &'static str {
        self.id
    }
    fn kind(&self) -> TrackKind {
        TrackKind::Video
    }
    fn label(&self) -> &'static str {
        self.label
    }
    fn preserves_dolby_vision(&self) -> bool {
        self.preserves_dv
    }

    fn default_opts(&self, src: &Track) -> Opts {
        // Половина исходного битрейта — безопасная стартовая точка для HEVC.
        let base = (src.bps.value as f64 / 2.0 * self.efficiency) as u64;
        Opts { bitrate_bps: Some(base), channels: None, height: src.height }
    }

    fn estimate_bps(&self, ctx: &EncodeCtx) -> Estimated<u64> {
        // VBV держит средний битрейт в пределах нескольких процентов от заданного.
        Estimated::sampled(ctx.opts.bitrate_bps.unwrap_or(ctx.src.bps.value))
    }

    fn args(&self, ctx: &EncodeCtx, out_idx: usize) -> Vec<String> {
        let bps = ctx.opts.bitrate_bps.unwrap_or(ctx.src.bps.value);
        let kbit = bps / 1000;
        let mut args = vec![
            format!("-c:v:{}", out_idx),
            self.ffmpeg_name.to_string(),
            format!("-b:v:{}", out_idx),
            format!("{}k", kbit),
            format!("-maxrate:v:{}", out_idx),
            format!("{}k", kbit * 12 / 10),
            format!("-bufsize:v:{}", out_idx),
            format!("{}k", kbit * 2),
            format!("-pix_fmt"),
            "yuv420p10le".to_string(),
        ];
        if let Some(vf) = scale_filter(ctx.opts, ctx.src) {
            args.push("-vf".to_string());
            args.push(vf);
        }
        args
    }

    fn notes(&self, ctx: &EncodeCtx) -> Vec<Note> {
        let mut notes = vec![Note::info(
            NoteTarget::Track(ctx.src.index),
            "Битрейт видео — сколько данных в секунду отводится картинке. \
             Вдвое меньше битрейт ≈ вдвое меньше файл, но на движении появляются квадраты.",
        )];
        if self.preserves_dv {
            let hours = ctx.duration_s * X265_SLOWDOWN_2160P / 3600.0;
            notes.push(Note::warning(
                NoteTarget::Track(ctx.src.index),
                format!(
                    "x265 сохранит Dolby Vision, но кодирование займёт примерно {:.1} ч \
                     вместо минут на видеокарте",
                    hours
                ),
            ));
        }
        if let Some(h) = ctx.opts.height {
            if Some(h) != ctx.src.height {
                notes.push(Note::warning(
                    NoteTarget::Track(ctx.src.index),
                    format!(
                        "Разрешение снизится с {}p до {}p — самый сильный рычаг по размеру, \
                         но на большом экране мелкие детали пропадут",
                        ctx.src.height.unwrap_or(0),
                        h
                    ),
                ));
            }
        }
        notes
    }
}

pub fn register(reg: &mut crate::codec::CodecRegistry) {
    reg.register(Box::new(VideoCodec {
        id: "hevc_nvenc",
        label: "HEVC на видеокарте (быстро)",
        ffmpeg_name: "hevc_nvenc",
        preserves_dv: false,
        efficiency: 1.0,
    }));
    reg.register(Box::new(VideoCodec {
        id: "hevc_x265_dv",
        label: "HEVC на процессоре с сохранением Dolby Vision (медленно)",
        ffmpeg_name: "libx265",
        preserves_dv: true,
        efficiency: 1.0,
    }));
    reg.register(Box::new(VideoCodec {
        id: "av1_nvenc",
        label: "AV1 на видеокарте (компактнее, читают не все плееры)",
        ffmpeg_name: "av1_nvenc",
        preserves_dv: false,
        efficiency: 0.7,
    }));
}

#[cfg(test)]
mod tests {
    use crate::codec::{CodecRegistry, EncodeCtx};
    use crate::media::{Track, TrackKind};
    use crate::plan::Opts;
    use crate::{Confidence, Estimated};

    fn uhd() -> Track {
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
        }
    }

    #[test]
    fn video_size_follows_requested_bitrate() {
        let reg = CodecRegistry::with_builtins();
        let src = uhd();
        let opts = Opts { bitrate_bps: Some(26_000_000), ..Default::default() };
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 100.0 };
        let bps = reg.get("hevc_nvenc").unwrap().estimate_bps(&ctx);
        assert_eq!(bps.value, 26_000_000);
        assert_eq!(bps.confidence, Confidence::Sampled);
    }

    #[test]
    fn nvenc_args_pin_bitrate_with_vbv_limiter() {
        let reg = CodecRegistry::with_builtins();
        let src = uhd();
        let opts = Opts { bitrate_bps: Some(26_000_000), height: Some(2160), ..Default::default() };
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 100.0 };
        let args = reg.get("hevc_nvenc").unwrap().args(&ctx, 0);
        assert!(args.windows(2).any(|w| w == ["-c:v:0", "hevc_nvenc"]));
        assert!(args.windows(2).any(|w| w == ["-b:v:0", "26000k"]));
        assert!(args.windows(2).any(|w| w == ["-maxrate:v:0", "31200k"]));
        assert!(args.windows(2).any(|w| w == ["-bufsize:v:0", "52000k"]));
    }

    #[test]
    fn downscale_adds_scale_filter() {
        let reg = CodecRegistry::with_builtins();
        let src = uhd();
        let opts = Opts { bitrate_bps: Some(12_000_000), height: Some(1080), ..Default::default() };
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 100.0 };
        let args = reg.get("hevc_nvenc").unwrap().args(&ctx, 0);
        assert!(args.windows(2).any(|w| w == ["-vf", "scale=-2:1080"]));
    }

    #[test]
    fn only_x265_profile_preserves_dolby_vision() {
        let reg = CodecRegistry::with_builtins();
        assert!(reg.get("hevc_x265_dv").unwrap().preserves_dolby_vision());
        assert!(!reg.get("hevc_nvenc").unwrap().preserves_dolby_vision());
        assert!(!reg.get("av1_nvenc").unwrap().preserves_dolby_vision());
    }

    #[test]
    fn x265_warns_about_encode_time() {
        let reg = CodecRegistry::with_builtins();
        let src = uhd();
        let opts = Opts { bitrate_bps: Some(26_000_000), ..Default::default() };
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 6890.0 };
        let notes = reg.get("hevc_x265_dv").unwrap().notes(&ctx);
        assert!(notes.iter().any(|n| n.text.contains("ч")));
    }
}
