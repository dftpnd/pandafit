use super::{format_bytes, format_time};
use crate::session::{Session, SessionEvent};
use pandafit_core::note::Level;
use pandafit_core::plan::{Target, PRESETS};
use pandafit_core::Verdict;

pub fn show_header(ui: &mut egui::Ui, session: &Session) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let (Some(media), Some(plan)) = (&session.media, &session.plan) else {
        return events;
    };
    ui.horizontal(|ui| {
        ui.strong(media.path.file_name().unwrap_or_default().to_string_lossy());
        ui.label(format_time(media.duration_s));
        ui.label(format_bytes(media.file_bytes));
    });
    ui.horizontal(|ui| {
        ui.label("Цель:");
        egui::ComboBox::from_id_salt("target")
            .selected_text(format!(
                "{} — {}",
                match &plan.target.source {
                    pandafit_core::plan::TargetSource::Drive(d) => format!("диск в {d}"),
                    pandafit_core::plan::TargetSource::Preset(p) => p.clone(),
                    pandafit_core::plan::TargetSource::Manual => "вручную".into(),
                },
                format_bytes(plan.target.capacity_bytes)
            ))
            .show_ui(ui, |ui| {
                if ui.selectable_label(false, "Прочитать из привода /dev/sr0").clicked() {
                    use pandafit_media::DiscDevice;
                    if let Ok(s) = pandafit_media::SgDiscDevice.status("/dev/sr0") {
                        events.push(SessionEvent::TargetChosen(Target::drive(
                            &s.device,
                            s.capacity_bytes,
                        )));
                    }
                }
                for (name, cap) in PRESETS {
                    if ui.selectable_label(false, *name).clicked() {
                        events.push(SessionEvent::TargetChosen(Target::preset(*name, *cap)));
                    }
                }
            });
    });
    events
}

pub fn show(ui: &mut egui::Ui, session: &Session) {
    let Some(b) = &session.breakdown else { return };
    ui.heading("Бюджет");

    let (color, verdict_text) = match b.verdict {
        Verdict::Fits => (
            egui::Color32::from_rgb(60, 160, 70),
            format!(
                "влезает, запас {}",
                format_bytes(b.usable_bytes.saturating_sub(b.total.value))
            ),
        ),
        Verdict::Tight => (
            egui::Color32::from_rgb(200, 150, 40),
            "влезает впритык — лучше убрать ещё дорожку".to_string(),
        ),
        Verdict::Overflow { excess } => (
            egui::Color32::from_rgb(190, 60, 60),
            format!("не влезает, лишних {}", format_bytes(excess)),
        ),
    };
    ui.colored_label(
        color,
        format!("{} из {}", format_bytes(b.total.value), format_bytes(b.capacity_bytes)),
    );
    ui.colored_label(color, verdict_text);
    ui.separator();

    for t in &b.tracks {
        ui.horizontal(|ui| {
            ui.label(&t.label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mark = match t.bytes.confidence {
                    pandafit_core::Confidence::Exact => "•",
                    _ => "~",
                };
                ui.label(format!("{mark} {}", format_bytes(t.bytes.value)));
            });
        });
    }
    ui.horizontal(|ui| {
        ui.label("контейнер");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(format!("~ {}", format_bytes(b.container.value)));
        });
    });

    if !session.notes.is_empty() {
        ui.separator();
        for note in &session.notes {
            let (color, prefix) = match note.level {
                Level::Info => (egui::Color32::GRAY, "i"),
                Level::Warning => (egui::Color32::from_rgb(200, 150, 40), "!"),
                Level::Blocker => (egui::Color32::from_rgb(190, 60, 60), "стоп"),
            };
            ui.colored_label(color, format!("{prefix} {}", note.text));
        }
    }
}
