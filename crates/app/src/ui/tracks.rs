use super::format_bytes;
use crate::session::{Session, SessionEvent};
use pandafit_core::media::TrackKind;
use pandafit_core::plan::{Opts, TrackAction};

pub fn show(ui: &mut egui::Ui, session: &Session) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let (Some(media), Some(plan), Some(breakdown)) =
        (&session.media, &session.plan, &session.breakdown)
    else {
        return events;
    };

    egui::ScrollArea::vertical().show(ui, |ui| {
        for track in &media.tracks {
            let action = plan.action(track.index).clone();
            let size = breakdown
                .tracks
                .iter()
                .find(|t| t.index == track.index)
                .map(|t| format_bytes(t.bytes.value))
                .unwrap_or_else(|| "—".into());

            ui.horizontal(|ui| {
                let mut kept = action != TrackAction::Drop;
                if ui.checkbox(&mut kept, "").changed() {
                    events.push(SessionEvent::TrackActionChanged {
                        index: track.index,
                        action: if kept { TrackAction::Copy } else { TrackAction::Drop },
                    });
                }
                ui.label(track.label());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(size);
                });
            });

            if action != TrackAction::Drop {
                ui.horizontal(|ui| {
                    ui.add_space(24.0);
                    let current = match &action {
                        TrackAction::Copy => "Копировать".to_string(),
                        TrackAction::Transcode { codec_id, .. } => session
                            .registry()
                            .get(codec_id)
                            .map(|p| p.label().to_string())
                            .unwrap_or_else(|| codec_id.clone()),
                        TrackAction::Drop => "Удалить".to_string(),
                    };
                    egui::ComboBox::from_id_salt(("action", track.index))
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(false, "Копировать").clicked() {
                                events.push(SessionEvent::TrackActionChanged {
                                    index: track.index,
                                    action: TrackAction::Copy,
                                });
                            }
                            for profile in session.registry().for_kind(track.kind) {
                                if ui.selectable_label(false, profile.label()).clicked() {
                                    events.push(SessionEvent::TrackActionChanged {
                                        index: track.index,
                                        action: TrackAction::Transcode {
                                            codec_id: profile.id().into(),
                                            opts: Opts::default(),
                                        },
                                    });
                                }
                            }
                        });

                    if let TrackAction::Transcode { codec_id, opts } = &action {
                        if let Some(bps) = opts.bitrate_bps {
                            let mut mbit = bps as f64 / 1e6;
                            let range = if track.kind == TrackKind::Video {
                                2.0..=80.0
                            } else {
                                0.064..=1.5
                            };
                            if ui
                                .add(egui::Slider::new(&mut mbit, range).text("Мбит/с"))
                                .changed()
                            {
                                events.push(SessionEvent::TrackActionChanged {
                                    index: track.index,
                                    action: TrackAction::Transcode {
                                        codec_id: codec_id.clone(),
                                        opts: Opts {
                                            bitrate_bps: Some((mbit * 1e6) as u64),
                                            ..*opts
                                        },
                                    },
                                });
                            }
                        }
                    }
                });
            }
            ui.separator();
        }
    });

    events
}
