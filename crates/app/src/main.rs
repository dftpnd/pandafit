use pandafit::session::Session;
use pandafit::ui::PandaFitApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1180.0, 760.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PandaFit",
        options,
        Box::new(|_cc| Ok(Box::new(PandaFitApp { session: Session::new() }))),
    )
}
