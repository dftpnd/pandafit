use pandafit_core::codec::CodecRegistry;
use pandafit_core::compile::{needs_dv_chain, BuildRequest, Step};
use pandafit_core::plan::{Opts, Plan, Target, TrackAction};
use pandafit_core::{
    estimate, explain, has_blockers, BitrateProfile, MediaInfo, Note, SizeBreakdown, Verdict,
};
use pandafit_media::ProgressEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Empty,
    Setup,
    Running,
    Done,
    Failed,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    FileOpened(MediaInfo),
    TargetChosen(Target),
    TrackActionChanged { index: usize, action: TrackAction },
    TrimChanged { start_s: f64, end_s: f64 },
    ProfileReady(BitrateProfile),
    Progress(ProgressEvent),
}

const LOG_CAPACITY: usize = 500;

pub struct Session {
    pub(crate) phase: Phase,
    pub(crate) media: Option<MediaInfo>,
    pub(crate) plan: Option<Plan>,
    pub(crate) profile: Option<BitrateProfile>,
    pub(crate) breakdown: Option<SizeBreakdown>,
    pub(crate) notes: Vec<Note>,
    pub(crate) steps: Vec<Step>,
    pub(crate) log: Vec<String>,
    pub(crate) current_step: Option<&'static str>,
    pub(crate) position_s: f64,
    pub(crate) bytes_written: u64,
    pub(crate) error: Option<String>,
    registry: CodecRegistry,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        Self {
            phase: Phase::Empty,
            media: None,
            plan: None,
            profile: None,
            breakdown: None,
            notes: Vec::new(),
            steps: Vec::new(),
            log: Vec::new(),
            current_step: None,
            position_s: 0.0,
            bytes_written: 0,
            error: None,
            registry: CodecRegistry::with_builtins(),
        }
    }

    pub fn registry(&self) -> &CodecRegistry {
        &self.registry
    }

    pub fn begin_running(&mut self) {
        self.phase = Phase::Running;
        self.error = None;
    }

    pub fn fail(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.phase = Phase::Failed;
    }

    pub fn apply(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::FileOpened(media) => {
                let target = self
                    .plan
                    .as_ref()
                    .map(|p| p.target.clone())
                    .unwrap_or_else(|| Target::preset("BD-R DL 50 ГБ", 50_050_629_632));
                self.plan = Some(Plan::from_media(&media, target));
                self.media = Some(media);
                self.profile = None;
                self.log.clear();
                self.steps.clear();
                self.current_step = None;
                self.position_s = 0.0;
                self.bytes_written = 0;
                self.error = None;
                self.phase = Phase::Setup;
            }
            SessionEvent::TargetChosen(target) => {
                if let Some(p) = &mut self.plan {
                    p.target = target;
                }
            }
            SessionEvent::TrackActionChanged { index, action } => {
                let action = self.fill_missing_transcode_defaults(index, action);
                if let Some(p) = &mut self.plan {
                    p.set_action(index, action);
                }
            }
            SessionEvent::TrimChanged { start_s, end_s } => {
                if let Some(p) = &mut self.plan {
                    p.range.start_s = start_s;
                    p.range.end_s = end_s;
                }
            }
            SessionEvent::ProfileReady(profile) => self.profile = Some(profile),
            SessionEvent::Progress(ev) => self.on_progress(ev),
        }
        self.recompute();
    }

    fn fill_missing_transcode_defaults(&self, index: usize, action: TrackAction) -> TrackAction {
        let (TrackAction::Transcode { codec_id, opts }, Some(track)) =
            (&action, self.media.as_ref().and_then(|m| m.track(index)))
        else {
            return action;
        };
        if opts.bitrate_bps.is_some() {
            return action;
        }
        let Some(profile) = self.registry.get(codec_id) else {
            return action;
        };
        let defaults = profile.default_opts(track);
        TrackAction::Transcode {
            codec_id: codec_id.clone(),
            opts: Opts {
                bitrate_bps: opts.bitrate_bps.or(defaults.bitrate_bps),
                channels: opts.channels.or(defaults.channels),
                height: opts.height.or(defaults.height),
            },
        }
    }

    fn on_progress(&mut self, ev: ProgressEvent) {
        match ev {
            ProgressEvent::Started { step_id, title } => {
                self.current_step = Some(step_id);
                self.push_log_line(format!("— {title}"));
                self.begin_running();
            }
            ProgressEvent::Tick { position_s, bytes_written, .. } => {
                self.position_s = position_s;
                if bytes_written > 0 {
                    self.bytes_written = bytes_written;
                }
            }
            ProgressEvent::Log { line, .. } => {
                self.push_log_line(line);
            }
            ProgressEvent::Finished { step_id } => {
                if self.steps.last().map(|s| s.id) == Some(step_id) {
                    self.phase = Phase::Done;
                }
            }
            ProgressEvent::Failed { message, tail, .. } => {
                self.fail(message);
                self.log.extend(tail);
            }
            ProgressEvent::Cancelled => {
                self.fail("отменено");
            }
        }
    }

    fn push_log_line(&mut self, line: String) {
        if self.log.len() == LOG_CAPACITY {
            self.log.remove(0);
        }
        self.log.push(line);
    }

    fn recompute(&mut self) {
        let (Some(media), Some(plan)) = (&self.media, &self.plan) else {
            return;
        };
        self.breakdown = Some(estimate(media, plan, &self.registry, self.profile.as_ref()));
        self.notes = explain(media, plan, &self.registry);
    }

    pub fn can_build(&self) -> bool {
        !has_blockers(&self.notes)
            && matches!(
                self.breakdown.as_ref().map(|b| b.verdict),
                Some(Verdict::Fits) | Some(Verdict::Tight)
            )
    }

    pub fn needs_dv_tools(&self) -> bool {
        match (&self.media, &self.plan) {
            (Some(m), Some(p)) => needs_dv_chain(m, p, &self.registry),
            _ => false,
        }
    }

    pub fn projected_total(&self) -> Option<u64> {
        let plan = self.plan.as_ref()?;
        let done = self.position_s - plan.range.start_s;
        if done <= 1.0 || self.bytes_written == 0 {
            return None;
        }
        let total_s = plan.range.duration_s();
        Some((self.bytes_written as f64 * total_s / done).round() as u64)
    }

    pub fn build_steps(&mut self, req: &BuildRequest) -> Vec<Step> {
        let (Some(media), Some(plan)) = (&self.media, &self.plan) else {
            return Vec::new();
        };
        self.steps = pandafit_core::compile(media, plan, &self.registry, req);
        self.steps.clone()
    }
}

#[cfg(test)]
#[path = "../../core/tests/common/fixtures.rs"]
mod fixtures;

#[cfg(test)]
mod tests {
    use super::fixtures::thor;
    use super::*;
    use pandafit_core::plan::{Opts, Target, TrackAction};
    use pandafit_core::Verdict;

    fn opened() -> Session {
        let mut s = Session::new();
        s.apply(SessionEvent::FileOpened(thor()));
        s.apply(SessionEvent::TargetChosen(Target::preset("BD-R DL", 50_050_629_632)));
        s
    }

    #[test]
    fn opening_a_file_switches_to_setup_and_estimates_immediately() {
        let s = opened();
        assert_eq!(s.phase, Phase::Setup);
        let b = s.breakdown.as_ref().unwrap();
        assert!(matches!(b.verdict, Verdict::Overflow { .. }));
    }

    #[test]
    fn dropping_tracks_updates_the_estimate_and_flips_the_verdict() {
        let mut s = opened();
        for i in [2usize, 3, 4, 5, 7] {
            s.apply(SessionEvent::TrackActionChanged { index: i, action: TrackAction::Drop });
        }
        assert!(matches!(s.breakdown.as_ref().unwrap().verdict, Verdict::Fits));
    }

    #[test]
    fn a_blocker_disables_building() {
        let mut s = opened();
        for i in 1..=7 {
            s.apply(SessionEvent::TrackActionChanged { index: i, action: TrackAction::Drop });
        }
        assert!(!s.can_build());
    }

    #[test]
    fn overflow_also_disables_building() {
        let s = opened();
        assert!(!s.can_build(), "переполненный план не должен собираться");
    }

    #[test]
    fn transcoding_uses_codec_defaults_when_opts_are_empty() {
        let mut s = opened();
        s.apply(SessionEvent::TrackActionChanged {
            index: 5,
            action: TrackAction::Transcode { codec_id: "ac3".into(), opts: Opts::default() },
        });
        let t = s.breakdown.as_ref().unwrap().tracks.iter().find(|t| t.index == 5).unwrap();
        assert_eq!(t.bytes.value, 551_214_080);
    }

    #[test]
    fn transcoding_keeps_a_user_chosen_height_and_only_fills_the_missing_bitrate() {
        let mut s = opened();
        s.apply(SessionEvent::TrackActionChanged {
            index: 0,
            action: TrackAction::Transcode {
                codec_id: "hevc_nvenc".into(),
                opts: Opts { bitrate_bps: None, channels: None, height: Some(1080) },
            },
        });
        let action = s.plan.as_ref().unwrap().action(0).clone();
        let TrackAction::Transcode { opts, .. } = action else {
            panic!("ожидали Transcode");
        };
        assert_eq!(opts.height, Some(1080));
        assert!(opts.bitrate_bps.is_some());
    }

    #[test]
    fn trim_narrows_the_plan_range() {
        let mut s = opened();
        s.apply(SessionEvent::TrimChanged { start_s: 0.0, end_s: 6700.0 });
        assert_eq!(s.plan.as_ref().unwrap().range.end_s, 6700.0);
    }

    #[test]
    fn live_projection_extrapolates_from_written_bytes() {
        let mut s = opened();
        for i in [2usize, 3, 4, 5, 7] {
            s.apply(SessionEvent::TrackActionChanged { index: i, action: TrackAction::Drop });
        }
        s.begin_running();
        s.apply(SessionEvent::Progress(pandafit_media::ProgressEvent::Tick {
            step_id: "build",
            position_s: 3445.0,
            bytes_written: 26_000_000_000,
            speed: Some(1.0),
        }));
        let projected = s.projected_total().unwrap();
        assert!(projected > 50_000_000_000, "прогноз {projected}");
    }

    #[test]
    fn opening_a_new_file_clears_the_previous_build_traces() {
        let mut s = opened();
        s.fail("сборка не удалась");
        s.log.push("хвост лога прошлой сборки".into());
        s.steps.push(pandafit_core::compile::Step {
            id: "build",
            title: "старый шаг".into(),
            program: "ffmpeg".into(),
            args: Vec::new(),
            progress: pandafit_core::compile::ProgressKind::FfmpegPipe,
            produces: None,
            prepare: None,
        });
        s.current_step = Some("build");
        s.position_s = 1234.0;
        s.bytes_written = 999;

        s.apply(SessionEvent::FileOpened(thor()));

        assert_eq!(s.phase, Phase::Setup);
        assert!(s.log.is_empty());
        assert!(s.steps.is_empty());
        assert!(s.current_step.is_none());
        assert_eq!(s.position_s, 0.0);
        assert_eq!(s.bytes_written, 0);
        assert!(s.error.is_none());
    }
}
