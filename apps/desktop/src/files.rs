//! Small native egui file picker: directory navigation and explicit replacement, no extra runtime.
use eframe::egui;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    Save,
    Load,
    Scenario,
    Branch,
}
impl Purpose {
    fn title(self) -> &'static str {
        match self {
            Self::Save => "Save checkpoint",
            Self::Load => "Load saved run",
            Self::Scenario => "Choose restart scenario",
            Self::Branch => "Choose future scenario",
        }
    }
    fn accepts(self, path: &Path) -> bool {
        path.extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| match self {
                Self::Load => ["json", "jsonl", "ndjson", "sqlite", "sqlite3", "db"]
                    .iter()
                    .any(|e| ext.eq_ignore_ascii_case(e)),
                _ => ext.eq_ignore_ascii_case("json"),
            })
    }
}
pub(crate) struct Selection {
    pub purpose: Purpose,
    pub path: PathBuf,
    pub overwrite: bool,
}
struct Entry {
    path: PathBuf,
    directory: bool,
}
pub(crate) struct FilePicker {
    pub open: bool,
    purpose: Purpose,
    directory: PathBuf,
    address: String,
    filename: String,
    entries: Vec<Entry>,
    error: Option<String>,
    replacing: Option<PathBuf>,
    #[cfg(test)]
    pub(crate) controls: std::collections::BTreeMap<String, egui::Rect>,
}
impl FilePicker {
    pub fn new(purpose: Purpose, directory: PathBuf, filename: String) -> Self {
        let mut picker = Self {
            open: true,
            purpose,
            address: directory.display().to_string(),
            directory,
            filename,
            entries: Vec::new(),
            error: None,
            replacing: None,
            #[cfg(test)]
            controls: Default::default(),
        };
        picker.refresh();
        picker
    }
    fn refresh(&mut self) {
        let result = (|| -> std::io::Result<Vec<Entry>> {
            let mut entries = Vec::new();
            for entry in std::fs::read_dir(&self.directory)? {
                let entry = entry?;
                let directory = entry.file_type()?.is_dir() || entry.path().is_dir();
                if directory || self.purpose.accepts(&entry.path()) {
                    entries.push(Entry {
                        path: entry.path(),
                        directory,
                    });
                }
                if entries.len() > 2000 {
                    return Err(std::io::Error::other(
                        "Directory has too many matching entries; enter a more specific path.",
                    ));
                }
            }
            entries.sort_by_key(|e| {
                (
                    !e.directory,
                    e.path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_lowercase()),
                )
            });
            Ok(entries)
        })();
        match result {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => {
                self.entries.clear();
                self.error = Some(error.to_string());
            }
        }
    }
    fn navigate(&mut self, path: PathBuf) {
        if !path.is_dir() {
            self.error = Some("Choose an existing directory.".into());
            return;
        }
        self.directory = path;
        self.address = self.directory.display().to_string();
        self.replacing = None;
        self.refresh();
    }
    fn selected_path(&self) -> anyhow::Result<PathBuf> {
        anyhow::ensure!(!self.filename.trim().is_empty(), "Enter a file name.");
        let mut path = self.directory.join(self.filename.trim());
        if self.purpose == Purpose::Save && path.extension().is_none() {
            path.set_extension("json");
        }
        anyhow::ensure!(!path.is_dir(), "Choose a file, not a directory.");
        if self.purpose == Purpose::Save {
            anyhow::ensure!(
                path.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("json")),
                "Save checkpoints with a .json extension."
            );
            anyhow::ensure!(
                path.parent().is_some_and(Path::is_dir),
                "Destination directory does not exist."
            );
        } else {
            anyhow::ensure!(path.is_file(), "The selected file does not exist.");
        }
        Ok(path)
    }
    fn choose(&mut self) -> Option<Selection> {
        match self.selected_path() {
            Ok(path) if self.purpose == Purpose::Save && path.exists() => {
                self.replacing = Some(path);
                None
            }
            Ok(path) => {
                self.open = false;
                Some(Selection {
                    purpose: self.purpose,
                    path,
                    overwrite: false,
                })
            }
            Err(error) => {
                self.error = Some(error.to_string());
                None
            }
        }
    }
    fn button(&mut self, ui: &mut egui::Ui, label: &str) -> bool {
        let response = ui.button(label);
        #[cfg(test)]
        self.controls.insert(label.into(), response.rect);
        response.clicked()
    }
    pub fn show(&mut self, context: &egui::Context) -> Option<Selection> {
        let mut selected = None;
        let modal = egui::Modal::new(egui::Id::new("file_picker")).show(context, |ui| {
            ui.set_width(610.0);
            ui.heading(self.purpose.title());
            if let Some(path) = self.replacing.clone() {
                ui.label("Replace the existing checkpoint?");
                ui.label(path.display().to_string());
                ui.horizontal(|ui| {
                    if self.button(ui, "Replace") { self.open = false; selected = Some(Selection { purpose: self.purpose, path, overwrite: true }); }
                    if self.button(ui, "Back") { self.replacing = None; }
                });
                return;
            }
            ui.horizontal(|ui| {
                if self.button(ui, "Up") && let Some(parent) = self.directory.parent().map(Path::to_path_buf) { self.navigate(parent); }
                let response = ui.add(egui::TextEdit::singleline(&mut self.address).desired_width(450.0));
                if self.button(ui, "Go") || (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) { self.navigate(PathBuf::from(&self.address)); }
            });
            let mut directory = None;
            egui::ScrollArea::vertical().id_salt("file_entries").max_height(280.0).show(ui, |ui| {
                for entry in &self.entries {
                    let name = entry.path.file_name().unwrap_or_default().to_string_lossy();
                    let label = if entry.directory { format!("[Folder] {name}") } else { name.to_string() };
                    let response = ui.selectable_label(!entry.directory && self.filename == name, &label);
                    #[cfg(test)] self.controls.insert(label, response.rect);
                    if response.clicked() {
                        if entry.directory { directory = Some(entry.path.clone()); } else { self.filename = name.into_owned(); }
                    }
                }
            });
            if let Some(path) = directory { self.navigate(path); }
            ui.horizontal(|ui| {
                ui.label("File");
                let response = ui.add(egui::TextEdit::singleline(&mut self.filename).desired_width(490.0));
                #[cfg(test)] self.controls.insert("Filename".into(), response.rect);
                let _ = response;
            });
            if self.purpose == Purpose::Load { ui.small("Checkpoint/state JSON, SQLite database, or completed JSONL journal. Loading pauses the run."); }
            if self.purpose == Purpose::Branch { ui.small("All interventions must start after the current tick. Save to a new file to retain the branch."); }
            if let Some(error) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
            ui.horizontal(|ui| {
                if self.button(ui, if self.purpose == Purpose::Save { "Save" } else { "Open" }) { selected = self.choose(); }
                if self.button(ui, "Cancel") { self.open = false; }
            });
        });
        if modal.should_close() {
            self.open = false;
        }
        selected
    }
}
