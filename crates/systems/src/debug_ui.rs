use common::Game;

pub fn compute_debug_ui(game: &mut Game) {
    let _span = tracy_client::span!("System: compute_debug_ui");

    let raw_input = egui::RawInput::default();
    let output = game.egui.ctx.run(raw_input, |ctx| {
        egui::Window::new("fantasy-rail").show(ctx, |ui| {
            ui.label(format!("Hello, world! {}", game.start_time.elapsed().as_secs_f32()));
        });
    });

    game.egui.output = output;
}
