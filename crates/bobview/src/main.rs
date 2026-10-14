use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([480.0, 320.0]),
        ..Default::default()
    };
    eframe::run_ui_native("bobview", options, |ui, _frame| {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Hello from bobview");
        });
    })
}
