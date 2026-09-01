use pandafit_core::codec::CodecRegistry;
use pandafit_core::compile::{needs_dv_chain, BuildRequest, Step};
use pandafit_core::plan::{Plan, Target, TrackAction};
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

pub struct Session {
    pub phase: Phase,
    pub media: Option<MediaInfo>,
    pub plan: Option<Plan>,
    pub profile: Option<BitrateProfile>,
    pub breakdown: Option<SizeBreakdown>,
    pub notes: Vec<Note>,
    pub steps: Vec<Step>,
    pub log: Vec<String>,
    pub current_step: Option<&'static str>,
    pub position_s: f64,
    pub bytes_written: u64,
    pub error: Option<String>,
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
                self.phase = Phase::Setup;
            }
            SessionEvent::TargetChosen(target) => {
                if let Some(p) = &mut self.plan {
                    p.target = target;
                }
            }
            SessionEvent::TrackActionChanged { index, action } => {
                let action = match (&action, self.media.as_ref().and_then(|m| m.track(index))) {
                    (TrackAction::Transcode { codec_id, opts }, Some(track))
                        if opts.bitrate_bps.is_none() =>
                    {
                        match self.registry.get(codec_id) {
                            Some(p) => TrackAction::Transcode {
                                codec_id: codec_id.clone(),
                                opts: p.default_opts(track),
                            },
                            None => action,
                        }
                    }
                    _ => action,
                };
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

    fn on_progress(&mut self, ev: ProgressEvent) {
        match ev {
            ProgressEvent::Started { step_id, title } => {
                self.current_step = Some(step_id);
                self.log.push(format!("— {title}"));
                self.phase = Phase::Running;
            }
            ProgressEvent::Tick { position_s, bytes_written, .. } => {
                self.position_s = position_s;
                if bytes_written > 0 {
                    self.bytes_written = bytes_written;
                }
            }
            ProgressEvent::Log { line, .. } => {
                if self.log.len() == 500 {
                    self.log.remove(0);
                }
                self.log.push(line);
            }
            ProgressEvent::Finished { step_id } => {
                if self.steps.last().map(|s| s.id) == Some(step_id) {
                    self.phase = Phase::Done;
                }
            }
            ProgressEvent::Failed { message, tail, .. } => {
                self.error = Some(message);
                self.log.extend(tail);
                self.phase = Phase::Failed;
            }
            ProgressEvent::Cancelled => {
                self.error = Some("отменено".into());
                self.phase = Phase::Failed;
            }
        }
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
        s.phase = Phase::Running;
        s.apply(SessionEvent::Progress(pandafit_media::ProgressEvent::Tick {
            step_id: "build",
            position_s: 3445.0,
            bytes_written: 26_000_000_000,
            speed: Some(1.0),
        }));
        let projected = s.projected_total().unwrap();
        assert!(projected > 50_000_000_000, "прогноз {projected}");
    }
}
