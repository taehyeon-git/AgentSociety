use super::*;
use std::time::{Duration, Instant};

fn frame(app: &mut Dashboard, context: &egui::Context, events: Vec<egui::Event>) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(1320.0, 950.0),
            )),
            events,
            ..Default::default()
        },
        |context| app.render(context),
    );
}
fn click(app: &mut Dashboard, context: &egui::Context, name: &str) {
    frame(app, context, vec![]);
    frame(app, context, vec![]);
    let controls = app.picker.as_ref().map_or(&app.controls, |p| &p.controls);
    let position = controls
        .get(name)
        .unwrap_or_else(|| panic!("missing control {name}"))
        .center();
    frame(
        app,
        context,
        vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
    );
    frame(
        app,
        context,
        vec![egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    );
}
fn filename(app: &mut Dashboard, context: &egui::Context, name: &str) {
    click(app, context, "Filename");
    let modifiers = egui::Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    frame(
        app,
        context,
        vec![
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: true,
                repeat: false,
                modifiers,
            },
            egui::Event::Key {
                key: egui::Key::A,
                physical_key: Some(egui::Key::A),
                pressed: false,
                repeat: false,
                modifiers,
            },
            egui::Event::Text(name.into()),
        ],
    );
}
fn until(app: &Dashboard, condition: impl Fn(&View) -> bool) -> std::sync::Arc<View> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let view = app.worker.view();
        if condition(&view) {
            return view;
        }
        assert!(Instant::now() < deadline, "{}", view.status);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn real_egui_pointer_input_drives_restart_save_load_and_failed_load_preserves_run() {
    let directory = tempfile::tempdir().unwrap();
    let context = egui::Context::default();
    context.set_style(egui::Style {
        animation_time: 0.0,
        ..Default::default()
    });
    let mut app = Dashboard::new(
        SimulationConfig {
            agents: 8,
            ..Default::default()
        },
        false,
        None,
    )
    .unwrap();
    app.file_directory = directory.path().to_path_buf();
    click(&mut app, &context, "Step");
    until(&app, |v| v.snapshot.world.tick == 1);
    click(&mut app, &context, "Run");
    until(&app, |v| v.running && v.snapshot.world.tick >= 3);
    click(&mut app, &context, "Pause");
    until(&app, |v| !v.running);
    click(&mut app, &context, "Behavior settings");
    for name in [
        "Price negotiation",
        "Joint production",
        "Organizations and contracts",
    ] {
        click(&mut app, &context, name);
    }
    click(&mut app, &context, "Restart");
    let restarted = until(&app, |v| v.generation == 1);
    assert!(
        restarted.snapshot.config.organizations.enabled
            && restarted.snapshot.config.interactions.negotiation
            && restarted.snapshot.config.interactions.cooperation
    );
    assert_eq!(restarted.snapshot.world.tick, 0);
    click(&mut app, &context, "Step");
    let before = until(&app, |v| v.snapshot.world.tick == 1);
    click(&mut app, &context, "Save checkpoint...");
    filename(&mut app, &context, "ui.checkpoint.json");
    click(&mut app, &context, "Save");
    until(&app, |v| v.status.starts_with("Saved tick"));
    let saved = agentsociety_storage::read_checkpoint(&directory.path().join("ui.checkpoint.json"))
        .unwrap();
    assert_eq!(saved.simulation, before.snapshot);
    click(&mut app, &context, "Step");
    until(&app, |v| v.snapshot.world.tick == 2);
    click(&mut app, &context, "Load run...");
    click(&mut app, &context, "ui.checkpoint.json");
    click(&mut app, &context, "Open");
    let loaded = until(&app, |v| v.generation == 2);
    assert_eq!(loaded.snapshot, saved.simulation);
    std::fs::write(directory.path().join("broken.json"), b"{broken").unwrap();
    click(&mut app, &context, "Load run...");
    click(&mut app, &context, "broken.json");
    click(&mut app, &context, "Open");
    let failed = until(&app, |v| v.status.starts_with("Load failed:"));
    assert_eq!(failed.snapshot, saved.simulation);
    assert_eq!(failed.generation, 2);
    assert!(!failed.running);
}

#[test]
fn file_picker_navigation_replace_confirmation_and_future_branch_are_wired() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("saved")).unwrap();
    let context = egui::Context::default();
    let mut app = Dashboard::new(
        SimulationConfig {
            agents: 4,
            ..Default::default()
        },
        false,
        None,
    )
    .unwrap();
    app.file_directory = directory.path().to_path_buf();
    click(&mut app, &context, "Save checkpoint...");
    click(&mut app, &context, "[Folder] saved");
    filename(&mut app, &context, "test.json");
    click(&mut app, &context, "Save");
    until(&app, |v| v.status.starts_with("Saved tick"));
    let path = directory.path().join("saved/test.json");
    let bytes = std::fs::read(&path).unwrap();
    click(&mut app, &context, "Step");
    until(&app, |v| v.snapshot.world.tick == 1);
    click(&mut app, &context, "Save checkpoint...");
    filename(&mut app, &context, "test.json");
    click(&mut app, &context, "Save");
    click(&mut app, &context, "Back");
    click(&mut app, &context, "Cancel");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    click(&mut app, &context, "Save checkpoint...");
    filename(&mut app, &context, "test.json");
    click(&mut app, &context, "Save");
    click(&mut app, &context, "Replace");
    until(&app, |v| v.status.starts_with("Saved tick 1"));
    assert_eq!(
        agentsociety_storage::read_run(&path)
            .unwrap()
            .simulation
            .world
            .tick,
        1
    );
    let future = ScenarioConfig {
        name: "gui_branch".into(),
        events: vec![ScheduledIntervention {
            tick: 2,
            label: "Fiscal change".into(),
            intervention: Intervention::Fiscal {
                policy: FiscalPolicy {
                    cash_levy_bps: 100,
                    period: 1,
                    ..Default::default()
                },
            },
        }],
    };
    std::fs::write(
        directory.path().join("branch.json"),
        serde_json::to_vec(&future).unwrap(),
    )
    .unwrap();
    let before = app.worker.view().snapshot.clone();
    click(&mut app, &context, "Branch scenario...");
    click(&mut app, &context, "Up");
    click(&mut app, &context, "branch.json");
    click(&mut app, &context, "Open");
    let branched = until(&app, |v| v.generation == 1);
    assert_eq!(branched.snapshot.world, before.world);
    assert_eq!(branched.snapshot.config.scenario.name, "gui_branch");
    click(&mut app, &context, "Step");
    let after = until(&app, |v| v.snapshot.world.tick == 2);
    assert!(after.snapshot.stats.treasury_money > 0);
    assert_eq!(
        agentsociety_storage::read_run(&path).unwrap().simulation,
        before
    );
}
