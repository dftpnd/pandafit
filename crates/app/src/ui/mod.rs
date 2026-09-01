pub mod budget;
pub mod tracks;

use crate::session::{Phase, Session, SessionEvent};

const GIGABYTE_DISPLAY_THRESHOLD_BYTES: u64 = 100_000_000;

pub fn format_bytes(b: u64) -> String {
    match b {
        b if b >= GIGABYTE_DISPLAY_THRESHOLD_BYTES => format!("{:.2} ГБ", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} МБ", b as f64 / 1e6),
        b if b >= 1_000 => format!("{:.1} КБ", b as f64 / 1e3),
        b => format!("{b} Б"),
    }
}

pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

pub struct PandaFitApp {
    pub session: Session,
}

impl eframe::App for PandaFitApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match self.session.phase {
            Phase::Empty => self.show_empty(ui),
            Phase::Setup => self.show_setup(ui),
            _ => self.show_running(ui),
        }
    }
}

impl PandaFitApp {
    fn show_empty(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(120.0);
            ui.heading("PandaFit");
            ui.label("Перетащите видеофайл сюда или откройте его вручную");
            ui.add_space(12.0);
            if ui.button("Открыть файл…").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Видео", &["mkv", "mp4", "ts", "m2ts"])
                    .pick_file()
                {
                    self.open(&path);
                }
            }
        });
        let dropped: Vec<_> = ui.ctx().input(|i| i.raw.dropped_files.clone());
        if let Some(f) = dropped.into_iter().next() {
            let path = f.path().to_path_buf();
            self.open(&path);
        }
    }

    fn open(&mut self, path: &std::path::Path) {
        use pandafit_media::MediaProbe;
        match pandafit_media::FfprobeProbe::new().probe(path) {
            Ok(media) => self.session.apply(SessionEvent::FileOpened(media)),
            Err(e) => self.session.fail(e.to_string()),
        }
    }

    fn show_setup(&mut self, ui: &mut egui::Ui) {
        let mut events = Vec::new();
        egui::Panel::top("header").show(ui, |ui| {
            events.extend(budget::show_header(ui, &self.session));
        });
        egui::Panel::right("budget").min_size(280.0).show(ui, |ui| {
            budget::show(ui, &self.session);
        });
        egui::CentralPanel::default().show(ui, |ui| {
            events.extend(tracks::show(ui, &self.session));
        });
        for e in events {
            self.session.apply(e);
        }
    }

    fn show_running(&mut self, _ui: &mut egui::Ui) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_are_shown_in_decimal_gigabytes_like_disc_labels() {
        assert_eq!(format_bytes(50_050_629_632), "50.05 ГБ");
        assert_eq!(format_bytes(551_214_080), "0.55 ГБ");
        assert_eq!(format_bytes(1_000_000), "1.0 МБ");
        assert_eq!(format_bytes(512), "512 Б");
    }

    #[test]
    fn timecode_is_shown_as_hours_minutes_seconds() {
        assert_eq!(format_time(6890.176), "1:54:50");
        assert_eq!(format_time(59.0), "0:00:59");
    }
}
