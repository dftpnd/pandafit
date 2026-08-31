use crate::codec::{CodecProfile, EncodeCtx};
use crate::media::{Track, TrackKind};
use crate::note::{Note, NoteTarget};
use crate::plan::Opts;
use crate::Estimated;

/// Кодек с постоянным битрейтом: размер предсказуем точно.
struct CbrAudio {
    id: &'static str,
    label: &'static str,
    ffmpeg_name: &'static str,
    default_bps: u64,
    max_channels: u32,
}

impl CodecProfile for CbrAudio {
    fn id(&self) -> &'static str {
        self.id
    }
    fn kind(&self) -> TrackKind {
        TrackKind::Audio
    }
    fn label(&self) -> &'static str {
        self.label
    }

    fn default_opts(&self, src: &Track) -> Opts {
        Opts {
            bitrate_bps: Some(self.default_bps),
            channels: Some(src.channels.unwrap_or(2).min(self.max_channels)),
            height: None,
        }
    }

    fn estimate_bps(&self, ctx: &EncodeCtx) -> Estimated<u64> {
        Estimated::exact(ctx.opts.bitrate_bps.unwrap_or(self.default_bps))
    }

    fn args(&self, ctx: &EncodeCtx, out_idx: usize) -> Vec<String> {
        let bps = ctx.opts.bitrate_bps.unwrap_or(self.default_bps);
        let mut args = vec![
            format!("-c:a:{}", out_idx),
            self.ffmpeg_name.to_string(),
            format!("-b:a:{}", out_idx),
            format!("{}k", bps / 1000),
        ];
        let src_ch = ctx.src.channels.unwrap_or(2);
        if src_ch > self.max_channels {
            args.push(format!("-ac:a:{}", out_idx));
            args.push(self.max_channels.to_string());
        }
        args
    }

    fn notes(&self, ctx: &EncodeCtx) -> Vec<Note> {
        let mut notes = vec![Note::info(
            NoteTarget::Track(ctx.src.index),
            "Битрейт аудио — сколько данных в секунду отводится звуку. \
             640 кбит/с для 5.1 на слух неотличимы от исходника почти всегда.",
        )];
        let src_ch = ctx.src.channels.unwrap_or(2);
        if src_ch > self.max_channels {
            notes.push(Note::warning(
                NoteTarget::Track(ctx.src.index),
                format!(
                    "{} не умеет {}.1 — дорожка сложится в {}.1",
                    self.label,
                    src_ch - 1,
                    self.max_channels - 1
                ),
            ));
        }
        if matches!(ctx.src.codec.as_str(), "truehd" | "dts" | "flac" | "pcm_s24le") {
            notes.push(Note::warning(
                NoteTarget::Track(ctx.src.index),
                "Исходная дорожка без потерь — после перекодирования вернуть качество будет нельзя",
            ));
        }
        notes
    }
}

/// FLAC сжимает без потерь, но насколько — заранее неизвестно.
struct FlacAudio;

impl CodecProfile for FlacAudio {
    fn id(&self) -> &'static str {
        "flac"
    }
    fn kind(&self) -> TrackKind {
        TrackKind::Audio
    }
    fn label(&self) -> &'static str {
        "FLAC (без потерь)"
    }

    fn default_opts(&self, src: &Track) -> Opts {
        Opts { bitrate_bps: None, channels: src.channels, height: None }
    }

    fn estimate_bps(&self, ctx: &EncodeCtx) -> Estimated<u64> {
        // PCM 24 бита на 48 кГц, сжатие принимаем за 60%.
        let ch = ctx.src.channels.unwrap_or(2) as u64;
        let pcm = ch * 48_000 * 24;
        Estimated::guessed(pcm * 3 / 5)
    }

    fn args(&self, _ctx: &EncodeCtx, out_idx: usize) -> Vec<String> {
        vec![format!("-c:a:{}", out_idx), "flac".to_string()]
    }

    fn notes(&self, ctx: &EncodeCtx) -> Vec<Note> {
        vec![Note::warning(
            NoteTarget::Track(ctx.src.index),
            "FLAC не теряет качество, но его размер заранее неизвестен — \
             оценка показана диапазоном, а не точкой",
        )]
    }
}

pub fn register(reg: &mut crate::codec::CodecRegistry) {
    reg.register(Box::new(CbrAudio {
        id: "ac3",
        label: "AC3",
        ffmpeg_name: "ac3",
        default_bps: 640_000,
        max_channels: 6,
    }));
    reg.register(Box::new(CbrAudio {
        id: "eac3",
        label: "E-AC3",
        ffmpeg_name: "eac3",
        default_bps: 768_000,
        max_channels: 8,
    }));
    reg.register(Box::new(CbrAudio {
        id: "opus",
        label: "Opus",
        ffmpeg_name: "libopus",
        default_bps: 256_000,
        max_channels: 8,
    }));
    reg.register(Box::new(FlacAudio));
}

#[cfg(test)]
mod tests {
    use crate::codec::{CodecRegistry, EncodeCtx};
    use crate::media::{Track, TrackKind};
    use crate::note::Level;
    use crate::plan::Opts;
    use crate::{Confidence, Estimated};

    fn dts_71() -> Track {
        Track {
            index: 2,
            kind: TrackKind::Audio,
            codec: "dts".into(),
            language: Some("rus".into()),
            title: Some("А. Гаврилов".into()),
            channels: Some(8),
            width: None,
            height: None,
            bps: Estimated::exact(5_008_134),
        }
    }

    #[test]
    fn ac3_uses_requested_bitrate_exactly() {
        let reg = CodecRegistry::with_builtins();
        let src = dts_71();
        let opts = Opts { bitrate_bps: Some(640_000), ..Default::default() };
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 100.0 };
        let bps = reg.get("ac3").unwrap().estimate_bps(&ctx);
        assert_eq!(bps.value, 640_000);
        assert_eq!(bps.confidence, Confidence::Exact);
    }

    #[test]
    fn ac3_warns_about_downmix_from_seven_one() {
        let reg = CodecRegistry::with_builtins();
        let src = dts_71();
        let opts = Opts { bitrate_bps: Some(640_000), ..Default::default() };
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 100.0 };
        let notes = reg.get("ac3").unwrap().notes(&ctx);
        assert!(notes.iter().any(|n| n.level == Level::Warning && n.text.contains("5.1")));
    }

    #[test]
    fn ac3_args_force_six_channels_for_seven_one_source() {
        let reg = CodecRegistry::with_builtins();
        let src = dts_71();
        let opts = Opts { bitrate_bps: Some(640_000), ..Default::default() };
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 100.0 };
        let args = reg.get("ac3").unwrap().args(&ctx, 1);
        assert_eq!(
            args,
            vec!["-c:a:1", "ac3", "-b:a:1", "640k", "-ac:a:1", "6"]
        );
    }

    #[test]
    fn flac_estimate_is_a_guess_not_a_promise() {
        let reg = CodecRegistry::with_builtins();
        let src = dts_71();
        let opts = Opts::default();
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 100.0 };
        let bps = reg.get("flac").unwrap().estimate_bps(&ctx);
        assert_eq!(bps.confidence, Confidence::Guessed);
    }

    #[test]
    fn eac3_warns_about_downmix_from_ten_channels() {
        let reg = CodecRegistry::with_builtins();
        let src = Track {
            index: 3,
            kind: TrackKind::Audio,
            codec: "pcm_s24le".into(),
            language: Some("eng".into()),
            title: None,
            channels: Some(10),
            width: None,
            height: None,
            bps: Estimated::exact(10_000_000),
        };
        let opts = Opts { bitrate_bps: Some(768_000), ..Default::default() };
        let ctx = EncodeCtx { src: &src, opts: &opts, duration_s: 100.0 };
        let notes = reg.get("eac3").unwrap().notes(&ctx);
        assert!(notes.iter().any(|n| n.level == Level::Warning && n.text.contains("7.1")));
    }
}
