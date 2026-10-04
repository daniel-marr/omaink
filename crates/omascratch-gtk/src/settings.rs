//! Settings window (gear button) and the first-run notebooks-folder prompt.
//!
//! Settings live in `~/.config/omascratch/settings.toml` (per machine, never
//! in the synced notebooks folder). Changing the notebooks folder only
//! re-points the app: nothing is moved or deleted.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::{gio, glib, prelude::*};
use libadwaita as adw;
use libadwaita::prelude::*;

use omascratch_store::{self as store, PressureCurve, Settings, SideButton, Smoothing};

/// Show a message dialog over `parent`.
fn alert(parent: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let d = adw::AlertDialog::new(Some(heading), Some(body));
    d.add_response("ok", "OK");
    d.present(Some(parent));
}

/// Pick a folder with the portal-backed chooser.
async fn choose_folder(parent: Option<gtk::Window>, title: &str, initial: &Path) -> Option<PathBuf> {
    let dialog = gtk::FileDialog::builder().title(title).modal(true).build();
    dialog.set_initial_folder(Some(&gio::File::for_path(initial)));
    dialog.select_folder_future(parent.as_ref()).await.ok()?.path()
}

/// Persist a new notebooks folder (keeping the other settings).
fn save_root(root: &Path) -> Result<(), String> {
    let mut settings = Settings::load_or_default(&store::config_dir());
    settings.notebooks_root = root.to_path_buf();
    settings.save(&store::config_dir()).map_err(|e| e.to_string())
}

/// Load settings, apply `edit`, save, and hand the result to `on_changed`.
fn update(on_changed: &Rc<dyn Fn(&Settings)>, edit: impl FnOnce(&mut Settings)) {
    let mut settings = Settings::load_or_default(&store::config_dir());
    edit(&mut settings);
    if let Err(e) = settings.save(&store::config_dir()) {
        eprintln!("omascratch: could not save settings: {e}");
    }
    on_changed(&settings);
}

/// A dropdown row over `options` (label, value); `pick` reads the current
/// value, `set` stores a new one.
fn choice_row<T: Copy + PartialEq + 'static>(
    title: &str,
    subtitle: &str,
    options: &'static [(&'static str, T)],
    current: T,
    on_pick: impl Fn(T) + 'static,
) -> adw::ComboRow {
    let row = adw::ComboRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    let labels: Vec<&str> = options.iter().map(|(l, _)| *l).collect();
    row.set_model(Some(&gtk::StringList::new(&labels)));
    row.set_selected(options.iter().position(|(_, v)| *v == current).unwrap_or(0) as u32);
    row.connect_selected_notify(move |r| {
        if let Some((_, v)) = options.get(r.selected() as usize) {
            on_pick(*v);
        }
    });
    row
}

/// Open the Settings window. `on_root_changed` runs after the user has
/// confirmed and saved a new notebooks folder; `on_changed` after any other
/// setting is saved (apply it live).
pub fn present_settings(
    parent: &gtk::Widget,
    current_root: PathBuf,
    on_root_changed: Rc<dyn Fn(PathBuf)>,
    on_changed: Rc<dyn Fn(&Settings)>,
) {
    let current = Settings::load_or_default(&store::config_dir());
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    dialog.set_search_enabled(false);
    let page = adw::PreferencesPage::new();

    // -- Storage --
    let storage = adw::PreferencesGroup::new();
    storage.set_title("Storage");
    storage.set_description(Some(
        "Each notebook is saved by name as a folder in here. To sync, point Dropbox, Google Drive or Syncthing at this folder.",
    ));
    let row = adw::ActionRow::new();
    row.set_title("Notebooks folder");
    row.set_subtitle(&current_root.display().to_string());
    row.set_subtitle_selectable(true);

    let open_btn = gtk::Button::from_icon_name("folder-open-symbolic");
    open_btn.set_tooltip_text(Some("Open in file manager"));
    open_btn.add_css_class("flat");
    open_btn.set_valign(gtk::Align::Center);
    {
        let root = current_root.clone();
        open_btn.connect_clicked(move |b| {
            let window = b.root().and_downcast::<gtk::Window>();
            gtk::FileLauncher::new(Some(&gio::File::for_path(&root))).launch(
                window.as_ref(),
                gio::Cancellable::NONE,
                |_| {},
            );
        });
    }
    let change_btn = gtk::Button::with_label("Change…");
    change_btn.set_valign(gtk::Align::Center);
    {
        let dialog = dialog.clone();
        let current = current_root.clone();
        change_btn.connect_clicked(move |b| {
            let window = b.root().and_downcast::<gtk::Window>();
            let dialog = dialog.clone();
            let current = current.clone();
            let on_changed = on_root_changed.clone();
            glib::spawn_future_local(async move {
                let Some(new_root) = choose_folder(window, "Choose notebooks folder", &current).await else {
                    return;
                };
                if new_root == current {
                    return;
                }
                if let Err(msg) = store::validate_notebooks_root(&new_root, Some(&current)) {
                    alert(&dialog, "Can't use that folder", &msg);
                    return;
                }
                let confirm = adw::AlertDialog::new(
                    Some("Switch notebooks folder?"),
                    Some(&format!(
                        "OmaScratch will save and load notebooks in\n{}\n\nNotebooks in {} stay where they are — nothing is moved or deleted.",
                        new_root.display(),
                        current.display()
                    )),
                );
                confirm.add_response("cancel", "Cancel");
                confirm.add_response("switch", "Switch folder");
                confirm.set_response_appearance("switch", adw::ResponseAppearance::Suggested);
                confirm.set_default_response(Some("switch"));
                confirm.set_close_response("cancel");
                let dialog2 = dialog.clone();
                confirm.connect_response(None, move |_, resp| {
                    if resp != "switch" {
                        return;
                    }
                    match save_root(&new_root) {
                        Ok(()) => {
                            dialog2.close();
                            on_changed(new_root.clone());
                        }
                        Err(e) => alert(&dialog2, "Couldn't save settings", &e),
                    }
                });
                confirm.present(Some(&dialog));
            });
        });
    }
    row.add_suffix(&open_btn);
    row.add_suffix(&change_btn);
    storage.add(&row);
    page.add(&storage);

    // -- Pen & ink --
    let ink = adw::PreferencesGroup::new();
    ink.set_title("Pen &amp; ink");
    {
        let cb = on_changed.clone();
        ink.add(&choice_row(
            "Pressure response",
            "How hard you press for a thick line (new strokes)",
            &[("Soft", PressureCurve::Soft), ("Normal", PressureCurve::Normal), ("Firm", PressureCurve::Firm)],
            current.ink.pressure,
            move |v| update(&cb, |s| s.ink.pressure = v),
        ));
    }
    {
        let cb = on_changed.clone();
        ink.add(&choice_row(
            "Smoothing",
            "Evens out shaky lines (all strokes)",
            &[("Light", Smoothing::Light), ("Normal", Smoothing::Normal), ("Strong", Smoothing::Strong)],
            current.ink.smoothing,
            move |v| update(&cb, |s| s.ink.smoothing = v),
        ));
    }
    {
        let cb = on_changed.clone();
        ink.add(&choice_row(
            "Pen side button",
            "Eraser: click to switch to the eraser, hold while drawing to erase",
            &[("Eraser", SideButton::Eraser), ("Off", SideButton::Off)],
            current.ink.side_button,
            move |v| update(&cb, |s| s.ink.side_button = v),
        ));
    }
    {
        let row = adw::SwitchRow::new();
        row.set_title("Draw with mouse and touch");
        row.set_subtitle("When off, only the pen draws; mouse and finger drags pan the page");
        row.set_active(current.ink.mouse_draws);
        let cb = on_changed.clone();
        row.connect_active_notify(move |r| {
            let on = r.is_active();
            update(&cb, |s| s.ink.mouse_draws = on);
        });
        ink.add(&row);
    }
    page.add(&ink);

    dialog.add(&page);
    dialog.present(Some(parent));
}

/// First launch (no settings file yet): ask where notebooks should live,
/// then call `on_done` with the saved folder.
pub fn present_welcome(app: &adw::Application, on_done: Rc<dyn Fn(PathBuf)>) {
    // Keep the app alive while only this dialog is on screen.
    let hold = app.hold();
    let chosen = Rc::new(std::cell::RefCell::new(store::default_notebooks_root()));

    let dialog = adw::AlertDialog::new(
        Some("Welcome to OmaScratch"),
        Some("Where should your notebooks be saved? Each notebook becomes a folder in here. You can change this later in Settings."),
    );
    let path_label = gtk::Label::new(Some(&chosen.borrow().display().to_string()));
    path_label.set_wrap(true);
    path_label.set_selectable(true);
    path_label.add_css_class("monospace");
    let choose = gtk::Button::with_label("Choose folder…");
    choose.set_halign(gtk::Align::Center);
    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    extra.append(&path_label);
    extra.append(&choose);
    dialog.set_extra_child(Some(&extra));
    {
        let chosen = chosen.clone();
        let label = path_label.clone();
        choose.connect_clicked(move |b| {
            let window = b.root().and_downcast::<gtk::Window>();
            let chosen = chosen.clone();
            let label = label.clone();
            glib::spawn_future_local(async move {
                let start = chosen.borrow().parent().map(Path::to_path_buf).unwrap_or_else(|| chosen.borrow().clone());
                if let Some(p) = choose_folder(window, "Choose notebooks folder", &start).await {
                    label.set_text(&p.display().to_string());
                    *chosen.borrow_mut() = p;
                }
            });
        });
    }
    dialog.add_response("use", "Use this folder");
    dialog.set_response_appearance("use", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("use"));
    dialog.set_close_response("use");
    let hold = std::cell::RefCell::new(Some(hold));
    let app = app.clone();
    dialog.connect_response(None, move |_, _| {
        let root = chosen.borrow().clone();
        if let Err(msg) = store::validate_notebooks_root(&root, None) {
            // Explain, then ask again until a usable folder is picked.
            let again = adw::AlertDialog::new(Some("Can't use that folder"), Some(&msg));
            again.add_response("ok", "Choose again");
            let app = app.clone();
            let on_done = on_done.clone();
            let keep = hold.borrow_mut().take();
            again.connect_response(None, move |_, _| {
                // `keep` (the app hold) is owned here until the new prompt is up.
                let _ = &keep;
                present_welcome(&app, on_done.clone());
            });
            again.present(None::<&gtk::Widget>);
            return;
        }
        if let Err(e) = save_root(&root) {
            eprintln!("omascratch: could not save settings: {e}");
        }
        on_done(root);
        hold.borrow_mut().take();
    });
    dialog.present(None::<&gtk::Widget>);
}
