use crate::codec::{CodecRegistry, EncodeCtx};
use crate::media::{MediaInfo, TrackKind};
use crate::plan::{Plan, TrackAction};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressKind {
    /// `ffmpeg -progress pipe:1` — пары ключ=значение в stdout.
    FfmpegPipe,
    X265Stderr,
    DoviTool,
    Growisofs,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub id: &'static str,
    pub title: String,
    pub program: String,
    pub args: Vec<String>,
    pub progress: ProgressKind,
    /// Файл, который шаг создаёт. Если он уже есть, шаг можно пропустить.
    pub produces: Option<PathBuf>,
}

impl Step {
    /// Строка, которую можно скопировать в терминал как есть.
    pub fn command_line(&self) -> String {
        let quote = |s: &str| {
            if s.chars().all(|c| c.is_ascii_alphanumeric() || "-_=:.,+%".contains(c)) {
                s.to_string()
            } else {
                format!("'{}'", s.replace('\'', r"'\''"))
            }
        };
        let mut out = quote(&self.program);
        for a in &self.args {
            out.push(' ');
            out.push_str(&quote(a));
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuildRequest {
    pub output: PathBuf,
    /// Папка для промежуточных файлов, переживающих падение.
    pub workdir: PathBuf,
    pub burn_device: Option<String>,
}

fn fmt_seconds(s: f64) -> String {
    format!("{:.3}", s)
}

/// Индексы выходных потоков считаются отдельно внутри каждого типа:
/// ffmpeg адресует их как `-c:a:0`, `-c:a:1`, `-c:v:0`.
struct OutIdx {
    video: usize,
    audio: usize,
    subtitle: usize,
}

impl OutIdx {
    fn next(&mut self, kind: TrackKind) -> usize {
        let slot = match kind {
            TrackKind::Video => &mut self.video,
            TrackKind::Audio => &mut self.audio,
            _ => &mut self.subtitle,
        };
        let i = *slot;
        *slot += 1;
        i
    }
}

fn ffmpeg_build_step(
    media: &MediaInfo,
    plan: &Plan,
    registry: &CodecRegistry,
    req: &BuildRequest,
) -> Step {
    let duration = plan.range.duration_s();
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-y".into()];

    // -ss и -to до -i: ffmpeg режет по контейнеру, это и быстрее, и точнее по времени.
    if plan.range.start_s > 0.0 {
        args.push("-ss".into());
        args.push(fmt_seconds(plan.range.start_s));
    }
    if !plan.range.is_full(media.duration_s) {
        args.push("-to".into());
        args.push(fmt_seconds(plan.range.end_s));
    }
    args.push("-i".into());
    args.push(media.path.to_string_lossy().into_owned());

    let kept: Vec<_> = media
        .tracks
        .iter()
        .filter(|t| t.kind != TrackKind::Attachment)
        .filter(|t| *plan.action(t.index) != TrackAction::Drop)
        .collect();

    for t in &kept {
        args.push("-map".into());
        args.push(format!("0:{}", t.index));
    }

    // Копирование — умолчание; перекодируемые потоки перекрывают его точечно.
    args.push("-c".into());
    args.push("copy".into());

    let mut idx = OutIdx { video: 0, audio: 0, subtitle: 0 };
    for t in &kept {
        let out_idx = idx.next(t.kind);
        if let TrackAction::Transcode { codec_id, opts } = plan.action(t.index) {
            if let Some(profile) = registry.get(codec_id) {
                let ctx = EncodeCtx { src: t, opts, duration_s: duration };
                args.extend(profile.args(&ctx, out_idx));
            }
        }
    }

    // Метаданные HDR10 переносятся всегда: без них картинка выцветает.
    args.push("-map_metadata".into());
    args.push("0".into());
    args.push("-map_chapters".into());
    args.push("0".into());

    args.push("-progress".into());
    args.push("pipe:1".into());
    args.push("-nostats".into());
    args.push(req.output.to_string_lossy().into_owned());

    Step {
        id: "build",
        title: "Сборка файла".into(),
        program: "ffmpeg".into(),
        args,
        progress: ProgressKind::FfmpegPipe,
        produces: Some(req.output.clone()),
    }
}

fn burn_step(req: &BuildRequest, device: &str) -> Step {
    Step {
        id: "burn",
        title: format!("Запись на диск {device}"),
        program: "growisofs".into(),
        args: vec![
            "-Z".into(),
            device.to_string(),
            "-V".into(),
            "PANDAFIT".into(),
            "-udf".into(),
            "-allow-limited-size".into(),
            req.output.to_string_lossy().into_owned(),
        ],
        progress: ProgressKind::Growisofs,
        produces: None,
    }
}

pub fn compile(
    media: &MediaInfo,
    plan: &Plan,
    registry: &CodecRegistry,
    req: &BuildRequest,
) -> Vec<Step> {
    let mut steps = vec![ffmpeg_build_step(media, plan, registry, req)];
    if let Some(dev) = &req.burn_device {
        steps.push(burn_step(req, dev));
    }
    steps
}

#[cfg(test)]
#[path = "../tests/common/fixtures.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::*;
    use super::fixtures::thor;
    use crate::codec::CodecRegistry;
    use crate::plan::{Opts, Plan, Target, TrackAction};

    fn req() -> BuildRequest {
        BuildRequest {
            output: "/out/thor.bd50.mkv".into(),
            workdir: "/out/.pandafit".into(),
            burn_device: None,
        }
    }

    fn remux_plan() -> (crate::media::MediaInfo, Plan) {
        let m = thor();
        let mut p = Plan::from_media(&m, Target::preset("BD-R DL", 50_050_629_632));
        for i in [2usize, 3, 4, 5, 7] {
            p.set_action(i, TrackAction::Drop);
        }
        (m, p)
    }

    #[test]
    fn pure_remux_is_a_single_ffmpeg_step() {
        let (m, p) = remux_plan();
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].id, "build");
        assert_eq!(steps[0].program, "ffmpeg");
        assert_eq!(steps[0].progress, ProgressKind::FfmpegPipe);
    }

    #[test]
    fn remux_maps_only_kept_tracks_and_copies_them() {
        let (m, p) = remux_plan();
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let a = &steps[0].args;
        assert!(a.windows(2).any(|w| w == ["-map", "0:0"]));
        assert!(a.windows(2).any(|w| w == ["-map", "0:1"]));
        assert!(a.windows(2).any(|w| w == ["-map", "0:6"]));
        assert!(a.windows(2).any(|w| w == ["-map", "0:8"]));
        assert!(!a.windows(2).any(|w| w == ["-map", "0:5"]));
        assert!(a.windows(2).any(|w| w == ["-c", "copy"]));
    }

    #[test]
    fn trim_adds_seek_and_end_before_input() {
        let (m, mut p) = remux_plan();
        p.range.start_s = 0.0;
        p.range.end_s = 6700.0;
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let a = &steps[0].args;
        let to_pos = a.iter().position(|x| x == "-to").expect("нет -to");
        let i_pos = a.iter().position(|x| x == "-i").expect("нет -i");
        assert!(to_pos < i_pos, "-to должен стоять до -i для точного реза по контейнеру");
        assert_eq!(a[to_pos + 1], "6700.000");
    }

    #[test]
    fn audio_transcode_emits_per_output_stream_args() {
        let (m, mut p) = remux_plan();
        p.set_action(2, TrackAction::Copy);
        p.set_action(
            2,
            TrackAction::Transcode {
                codec_id: "ac3".into(),
                opts: Opts { bitrate_bps: Some(640_000), ..Default::default() },
            },
        );
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let a = &steps[0].args;
        // Дорожка 2 — вторая по счёту среди выходных аудио (после дорожки 1).
        assert!(a.windows(2).any(|w| w == ["-c:a:1", "ac3"]));
        assert!(a.windows(2).any(|w| w == ["-b:a:1", "640k"]));
    }

    #[test]
    fn burn_request_appends_a_growisofs_step() {
        let (m, p) = remux_plan();
        let mut r = req();
        r.burn_device = Some("/dev/sr0".into());
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &r);
        let last = steps.last().unwrap();
        assert_eq!(last.id, "burn");
        assert_eq!(last.program, "growisofs");
        assert!(last.args.windows(2).any(|w| w == ["-Z", "/dev/sr0"]));
        assert!(last.args.iter().any(|x| x == "-udf"));
        assert!(last.args.iter().any(|x| x == "-allow-limited-size"));
    }

    #[test]
    fn command_line_is_copyable_into_a_terminal() {
        let (m, p) = remux_plan();
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let line = steps[0].command_line();
        assert!(line.starts_with("ffmpeg "));
        assert!(line.contains("'/out/thor.bd50.mkv'"));
    }
}
