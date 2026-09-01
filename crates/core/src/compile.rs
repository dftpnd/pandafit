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

/// Файл, который исполнитель шагов обязан записать на диск перед запуском команды.
/// Нужен шагам, настраиваемым конфигом, — сам домен файлов не пишет.
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedFile {
    pub path: PathBuf,
    pub contents: String,
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
    /// Файл конфигурации, который нужно записать на диск перед запуском команды.
    pub prepare: Option<PreparedFile>,
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
        prepare: None,
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
        prepare: None,
    }
}

/// Нужна ли отдельная цепочка ради сохранения Dolby Vision.
pub fn needs_dv_chain(media: &MediaInfo, plan: &Plan, registry: &CodecRegistry) -> bool {
    if !media.color.is_dolby_vision() {
        return false;
    }
    let Some(video) = media.video() else { return false };
    match plan.action(video.index) {
        TrackAction::Transcode { codec_id, .. } => {
            registry.get(codec_id).is_some_and(|p| p.preserves_dolby_vision())
        }
        _ => false,
    }
}

fn dv_chain(
    media: &MediaInfo,
    plan: &Plan,
    registry: &CodecRegistry,
    req: &BuildRequest,
) -> Vec<Step> {
    let video = media.video().expect("цепочка DV вызвана без видеодорожки");
    let duration = plan.range.duration_s();
    let trimmed = !plan.range.is_full(media.duration_s);

    let rpu_raw = req.workdir.join("rpu.bin");
    let rpu_used = if trimmed { req.workdir.join("rpu.trimmed.bin") } else { rpu_raw.clone() };
    let hevc = req.workdir.join("video.hevc");
    let src = media.path.to_string_lossy().into_owned();

    let mut steps = Vec::new();

    // 1. Достаём слой динамических метаданных из исходного потока.
    steps.push(Step {
        id: "dv_extract",
        title: "Извлечение метаданных Dolby Vision".into(),
        program: "dovi_tool".into(),
        args: vec![
            "extract-rpu".into(),
            src.clone(),
            "-o".into(),
            rpu_raw.to_string_lossy().into_owned(),
        ],
        progress: ProgressKind::DoviTool,
        produces: Some(rpu_raw.clone()),
        prepare: None,
    });

    // 2. RPU обязан совпасть с видео покадрово — при обрезке режем и его.
    // dovi_tool editor настраивается JSON-конфигом (-j), а не флагом области кадра:
    // список "remove" вычёркивает диапазоны кадров вне выбранного отрезка.
    if trimmed {
        let fps = media.fps;
        let first = (plan.range.start_s * fps).round() as i64;
        let last = (plan.range.end_s * fps).round() as i64;
        let mut remove = Vec::new();
        if first > 0 {
            remove.push(format!("0-{}", first - 1));
        }
        remove.push(format!("{}-", last + 1));
        let config_path = req.workdir.join("rpu-edit.json");
        let contents = format!(
            "{{\n  \"remove\": [{}]\n}}\n",
            remove.iter().map(|r| format!("\"{r}\"")).collect::<Vec<_>>().join(", ")
        );
        steps.push(Step {
            id: "dv_trim",
            title: "Обрезка метаданных Dolby Vision под новый диапазон".into(),
            program: "dovi_tool".into(),
            args: vec![
                "editor".into(),
                "-i".into(),
                rpu_raw.to_string_lossy().into_owned(),
                "-j".into(),
                config_path.to_string_lossy().into_owned(),
                "-o".into(),
                rpu_used.to_string_lossy().into_owned(),
            ],
            progress: ProgressKind::DoviTool,
            produces: Some(rpu_used.clone()),
            prepare: Some(PreparedFile { path: config_path, contents }),
        });
    }

    // 3. Кодирование с передачей RPU внутрь x265.
    let bitrate_kbit = match plan.action(video.index) {
        TrackAction::Transcode { opts, .. } => {
            opts.bitrate_bps.unwrap_or(video.bps.value) / 1000
        }
        _ => video.bps.value / 1000,
    };
    let mut enc_args: Vec<String> = vec!["-hide_banner".into(), "-y".into()];
    if plan.range.start_s > 0.0 {
        enc_args.push("-ss".into());
        enc_args.push(fmt_seconds(plan.range.start_s));
    }
    if trimmed {
        enc_args.push("-to".into());
        enc_args.push(fmt_seconds(plan.range.end_s));
    }
    enc_args.extend([
        "-i".into(),
        src.clone(),
        "-map".into(),
        format!("0:{}", video.index),
        "-an".into(),
        "-sn".into(),
        "-c:v".into(),
        "libx265".into(),
        "-pix_fmt".into(),
        "yuv420p10le".into(),
        "-b:v".into(),
        format!("{bitrate_kbit}k"),
        "-x265-params".into(),
        format!(
            "dolby-vision-rpu={}:dolby-vision-profile=8.1:vbv-maxrate={}:vbv-bufsize={}:hrd=1",
            rpu_used.to_string_lossy(),
            bitrate_kbit * 12 / 10,
            bitrate_kbit * 2
        ),
        "-f".into(),
        "hevc".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        hevc.to_string_lossy().into_owned(),
    ]);
    let _ = (registry, duration);
    steps.push(Step {
        id: "dv_encode",
        title: "Кодирование видео с сохранением Dolby Vision".into(),
        program: "ffmpeg".into(),
        args: enc_args,
        progress: ProgressKind::FfmpegPipe,
        produces: Some(hevc.clone()),
        prepare: None,
    });

    // 4. Сборка контейнера: mkvmerge надёжнее ffmpeg проставляет флаги DV.
    // Звук и субтитры — разные аргументы mkvmerge, смешивать их в один список нельзя.
    let mut mux: Vec<String> = vec![
        "-o".into(),
        req.output.to_string_lossy().into_owned(),
        hevc.to_string_lossy().into_owned(),
    ];
    let kept_audio: Vec<String> = media
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .filter(|t| *plan.action(t.index) != TrackAction::Drop)
        .map(|t| t.index.to_string())
        .collect();
    let kept_subs: Vec<String> = media
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Subtitle)
        .filter(|t| *plan.action(t.index) != TrackAction::Drop)
        .map(|t| t.index.to_string())
        .collect();
    if kept_audio.is_empty() {
        mux.push("--no-audio".into());
    } else {
        mux.push("--audio-tracks".into());
        mux.push(kept_audio.join(","));
    }
    if kept_subs.is_empty() {
        mux.push("--no-subtitles".into());
    } else {
        mux.push("--subtitle-tracks".into());
        mux.push(kept_subs.join(","));
    }
    mux.push("--no-video".into());
    if trimmed {
        mux.push("--split".into());
        mux.push(format!("parts:{}s-{}s", plan.range.start_s, plan.range.end_s));
    }
    mux.push(src);
    steps.push(Step {
        id: "dv_mux",
        title: "Сборка итогового MKV".into(),
        program: "mkvmerge".into(),
        args: mux,
        progress: ProgressKind::None,
        produces: Some(req.output.clone()),
        prepare: None,
    });

    steps
}

pub fn compile(
    media: &MediaInfo,
    plan: &Plan,
    registry: &CodecRegistry,
    req: &BuildRequest,
) -> Vec<Step> {
    let mut steps = if needs_dv_chain(media, plan, registry) {
        dv_chain(media, plan, registry, req)
    } else {
        vec![ffmpeg_build_step(media, plan, registry, req)]
    };
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

    fn dv_plan(trim: bool) -> (crate::media::MediaInfo, Plan) {
        let m = thor();
        let mut p = Plan::from_media(&m, Target::preset("BD-R 25", 25_025_314_816));
        for i in [2usize, 3, 4, 5, 7] {
            p.set_action(i, TrackAction::Drop);
        }
        p.set_action(
            0,
            TrackAction::Transcode {
                codec_id: "hevc_x265_dv".into(),
                opts: Opts { bitrate_bps: Some(23_000_000), height: Some(2160), ..Default::default() },
            },
        );
        if trim {
            p.range.end_s = 6700.0;
        }
        (m, p)
    }

    #[test]
    fn dv_chain_without_trim_has_three_steps() {
        let (m, p) = dv_plan(false);
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let ids: Vec<_> = steps.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["dv_extract", "dv_encode", "dv_mux"]);
    }

    #[test]
    fn trimming_inserts_an_rpu_editor_step() {
        let (m, p) = dv_plan(true);
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let ids: Vec<_> = steps.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec!["dv_extract", "dv_trim", "dv_encode", "dv_mux"]);
    }

    #[test]
    fn encode_step_passes_the_rpu_to_x265() {
        let (m, p) = dv_plan(false);
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let enc = steps.iter().find(|s| s.id == "dv_encode").unwrap();
        let joined = enc.args.join(" ");
        assert!(joined.contains("dolby-vision-rpu=/out/.pandafit/rpu.bin"));
        assert!(joined.contains("dolby-vision-profile=8.1"));
        assert!(joined.contains("vbv-maxrate"));
    }

    #[test]
    fn trimmed_chain_feeds_the_edited_rpu_to_the_encoder() {
        let (m, p) = dv_plan(true);
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let enc = steps.iter().find(|s| s.id == "dv_encode").unwrap();
        assert!(enc.args.join(" ").contains("rpu.trimmed.bin"));
    }

    #[test]
    fn every_dv_step_declares_what_it_produces() {
        let (m, p) = dv_plan(true);
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        for s in steps.iter().filter(|s| s.id.starts_with("dv_")) {
            assert!(s.produces.is_some(), "шаг {} не объявил результат", s.id);
        }
    }

    #[test]
    fn nvenc_transcode_stays_on_the_simple_path() {
        let (m, mut p) = dv_plan(false);
        p.set_action(
            0,
            TrackAction::Transcode {
                codec_id: "hevc_nvenc".into(),
                opts: Opts { bitrate_bps: Some(23_000_000), ..Default::default() },
            },
        );
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].id, "build");
    }

    /// dovi_tool editor настраивается JSON-конфигом (`-j`), а не флагом обрезки.
    /// Конфиг обязан посчитать границы диапазона из настоящей частоты кадров, а не
    /// из зашитого числа — здесь фикстура даёт 23.976, а не круглые 24.
    #[test]
    fn rpu_trim_range_is_computed_from_real_frame_rate() {
        let (m, p) = dv_plan(true);
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let trim = steps.iter().find(|s| s.id == "dv_trim").unwrap();
        let prepared = trim.prepare.as_ref().expect("dv_trim обязан готовить конфиг");
        assert_eq!(prepared.path, PathBuf::from("/out/.pandafit/rpu-edit.json"));
        // start_s == 0, значит первый оставляемый кадр — нулевой, головного диапазона нет.
        // Последний оставляемый кадр считается из настоящей частоты кадров фикстуры (23.976),
        // а не из зашитой двадцатичетвёрки — на 24 fps число вышло бы другим.
        let last = (6700.0f64 * 23.976).round() as u64;
        let expected_tail = format!("{}-", last + 1);
        assert!(
            prepared.contents.contains(&expected_tail),
            "конфиг {:?} не содержит хвостовой диапазон {expected_tail}",
            prepared.contents
        );
        // Флага обрезки области кадра тут быть не должно — это не про то.
        assert!(!trim.args.iter().any(|a| a == "--active-area-offsets"));
        assert!(trim.args.contains(&"-j".to_string()));
    }

    #[test]
    fn full_range_trim_has_no_leading_removed_span() {
        let (m, mut p) = dv_plan(true);
        p.range.start_s = 0.0;
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let trim = steps.iter().find(|s| s.id == "dv_trim").unwrap();
        let prepared = trim.prepare.as_ref().unwrap();
        // Первый оставляемый кадр — нулевой, значит головного диапазона в списке быть не должно.
        // Ищем именно элемент-диапазон "0-...", а не случайное совпадение внутри хвостового числа.
        assert!(
            !prepared.contents.contains("\"0-"),
            "лишний головной диапазон: {}",
            prepared.contents
        );
    }

    #[test]
    fn trim_with_a_nonzero_start_removes_a_leading_span_too() {
        let (m, mut p) = dv_plan(true);
        p.range.start_s = 120.0;
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let trim = steps.iter().find(|s| s.id == "dv_trim").unwrap();
        let prepared = trim.prepare.as_ref().unwrap();
        let first = (120.0f64 * 23.976).round() as u64;
        let expected_head = format!("0-{}", first - 1);
        assert!(
            prepared.contents.contains(&expected_head),
            "конфиг {:?} не содержит головной диапазон {expected_head}",
            prepared.contents
        );
    }

    #[test]
    fn dv_mux_splits_kept_tracks_into_audio_and_subtitle_lists() {
        let (m, p) = dv_plan(false);
        let steps = compile(&m, &p, &CodecRegistry::with_builtins(), &req());
        let mux = steps.iter().find(|s| s.id == "dv_mux").unwrap();
        let a_pos = mux.args.iter().position(|x| x == "--audio-tracks").expect("нет --audio-tracks");
        let audio_list = &mux.args[a_pos + 1];
        let s_pos = mux.args.iter().position(|x| x == "--subtitle-tracks").expect("нет --subtitle-tracks");
        let sub_list = &mux.args[s_pos + 1];
        // В фикстуре thor() с dv_plan(false) остаются аудио 1 и 6, субтитр 8.
        assert_eq!(audio_list, "1,6");
        assert_eq!(sub_list, "8");
        assert!(!audio_list.split(',').any(|i| i == "8"), "субтитр попал в список звука");
        assert!(mux.args.iter().any(|x| x == "--no-video"));
    }
}
