//! Obsidian-style sidebar: an icon-free Folders ▸ Notes tree with indent
//! guides, collapsible folders, single-click open, double-click inline rename,
//! drag-and-drop reordering/nesting, and a bottom bar with a notebook switcher,
//! help and settings — rebuilt from `Library::rows()` after every change.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use gtk4::{gdk, glib, pango, prelude::*};
use libadwaita as adw;
use libadwaita::prelude::*;

use omaink_core::FolderId;

use crate::library::{Library, Row};

const INDENT: i32 = 14;

#[derive(Clone)]
enum RowRef {
    Folder(FolderId),
    Note { path: PathBuf },
}

#[derive(Clone)]
pub struct Sidebar {
    pub widget: gtk::Box,
    /// NOTES header; the app size-groups it with the draw toolbar so the
    /// search box starts level with the top of the page.
    pub header: gtk::Box,
    inner: Rc<Inner>,
}

struct Inner {
    library: Rc<Library>,
    list: gtk::ListBox,
    search: gtk::SearchEntry,
    /// Lower-cased title filter; empty = normal tree.
    query: RefCell<String>,
    notebook_label: gtk::Label,
    selected: RefCell<Option<PathBuf>>,
    /// Multi-selection (Ctrl/Shift+click); empty = just the open note.
    multi: RefCell<Vec<PathBuf>>,
    /// Shift+click range anchor (last plain or Ctrl click).
    anchor: RefCell<Option<PathBuf>>,
    row_refs: RefCell<Vec<RowRef>>,
    editing: RefCell<Option<usize>>,
    collapsed: RefCell<HashSet<FolderId>>,
    drag: RefCell<Option<RowRef>>,
    on_open_note: RefCell<Option<Box<dyn Fn(&Path)>>>,
    on_rename_note: RefCell<Option<Box<dyn Fn(&Path, &str)>>>,
    on_notebook_renamed: RefCell<Option<Box<dyn Fn(&Path, &Path)>>>,
    on_notebook_deleted: RefCell<Option<Box<dyn Fn(&Path)>>>,
    on_settings: RefCell<Option<Box<dyn Fn()>>>,
}

impl Sidebar {
    pub fn new(library: Rc<Library>) -> Sidebar {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("sidebar-pane");
        widget.set_size_request(200, -1);

        // Header: notebook actions.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.add_css_class("sidebar-header");
        header.set_valign(gtk::Align::Fill);
        header.set_margin_start(14);
        header.set_margin_end(10);
        let title = gtk::Label::new(Some("NOTES"));
        title.add_css_class("heading");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        let new_note_btn = icon_button("document-new-symbolic", "New note");
        let new_folder_btn = icon_button("folder-new-symbolic", "New folder");
        new_note_btn.add_css_class("header-icon");
        new_folder_btn.add_css_class("header-icon");
        header.append(&title);
        header.append(&new_note_btn);
        header.append(&new_folder_btn);
        widget.append(&header);

        // Title search, where the tree starts.
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search notes"));
        search.add_css_class("sidebar-search");
        search.set_margin_start(10);
        search.set_margin_end(10);
        search.set_margin_bottom(4);
        widget.append(&search);

        // Tree.
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.add_css_class("navigation-sidebar");
        list.add_css_class("notes-tree");
        list.set_vexpand(true);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        widget.append(&scroller);

        // Bottom bar: notebook switcher | help | settings (Obsidian-style).
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let nb_bar = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        nb_bar.add_css_class("notebook-switcher");
        nb_bar.set_margin_top(3);
        nb_bar.set_margin_bottom(4);
        nb_bar.set_margin_start(6);
        nb_bar.set_margin_end(6);

        let notebook_label = gtk::Label::new(Some(&library.current_notebook_name()));
        notebook_label.set_ellipsize(pango::EllipsizeMode::End);
        notebook_label.set_xalign(0.0);
        let switcher = gtk::MenuButton::new();
        switcher.add_css_class("flat");
        switcher.set_hexpand(true);
        switcher.set_tooltip_text(Some("Switch or create notebook"));
        let sw_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        sw_box.append(&gtk::Image::from_icon_name("pan-up-symbolic"));
        sw_box.append(&notebook_label);
        notebook_label.set_hexpand(true);
        switcher.set_child(Some(&sw_box));

        let help_btn = glyph_button(help_glyph(), "Keyboard shortcuts");
        let gear_btn = glyph_button(gear_glyph(), "Settings");
        nb_bar.append(&switcher);
        nb_bar.append(&help_btn);
        nb_bar.append(&gear_btn);
        widget.append(&nb_bar);

        // Restore collapsed folders from persisted view state.
        let collapsed: HashSet<FolderId> = omaink_store::ViewState::load(&omaink_store::state_dir())
            .collapsed_folders
            .into_iter()
            .map(FolderId)
            .collect();

        let inner = Rc::new(Inner {
            library: library.clone(),
            list: list.clone(),
            search: search.clone(),
            query: RefCell::new(String::new()),
            notebook_label,
            selected: RefCell::new(None),
            multi: RefCell::new(Vec::new()),
            anchor: RefCell::new(None),
            row_refs: RefCell::new(Vec::new()),
            editing: RefCell::new(None),
            collapsed: RefCell::new(collapsed),
            drag: RefCell::new(None),
            on_open_note: RefCell::new(None),
            on_rename_note: RefCell::new(None),
            on_notebook_renamed: RefCell::new(None),
            on_notebook_deleted: RefCell::new(None),
            on_settings: RefCell::new(None),
        });
        let sidebar = Sidebar { widget, header: header.clone(), inner: inner.clone() };

        // Single click / Enter: open a note, or toggle a folder's collapse.
        {
            let sb = sidebar.clone();
            list.connect_row_activated(move |_, row| {
                let idx = row.index();
                if idx < 0 {
                    return;
                }
                let r = sb.inner.row_refs.borrow().get(idx as usize).cloned();
                match r {
                    Some(RowRef::Note { path }) => {
                        let had_multi = !sb.inner.multi.borrow().is_empty();
                        sb.inner.multi.borrow_mut().clear();
                        *sb.inner.anchor.borrow_mut() = Some(path.clone());
                        sb.open_path(&path);
                        if had_multi {
                            sb.refresh_idle();
                        }
                    }
                    Some(RowRef::Folder(id)) => {
                        {
                            let mut c = sb.inner.collapsed.borrow_mut();
                            if !c.remove(&id) {
                                c.insert(id);
                            }
                        }
                        sb.save_view_state();
                        sb.refresh_idle();
                    }
                    None => {}
                }
            });
        }

        // Delete: the multi-selection; Esc: clear it.
        {
            let keys = gtk::EventControllerKey::new();
            let sb = sidebar.clone();
            keys.connect_key_pressed(move |_, key, _, _| {
                if sb.inner.multi.borrow().is_empty() {
                    return glib::Propagation::Proceed;
                }
                match key {
                    gdk::Key::Delete | gdk::Key::KP_Delete => {
                        sb.confirm_delete_multi();
                        glib::Propagation::Stop
                    }
                    gdk::Key::Escape => {
                        sb.inner.multi.borrow_mut().clear();
                        sb.refresh_idle();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
            list.add_controller(keys);
        }

        let sb = sidebar.clone();
        new_note_btn.connect_clicked(move |_| {
            if let Some((_, path)) = sb.inner.library.new_note(None) {
                sb.set_selected(&path);
                sb.refresh_idle();
                sb.open_path(&path);
            }
        });
        let sb = sidebar.clone();
        new_folder_btn.connect_clicked(move |_| {
            sb.inner.library.new_folder(None);
            sb.refresh_idle();
        });

        // Drop onto empty space below rows → move to top level, at the end.
        {
            let sb = sidebar.clone();
            let drop = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
            drop.connect_drop(move |_, _, _, _| {
                let Some(src) = sb.inner.drag.borrow_mut().take() else { return false };
                if let RowRef::Note { path } = src {
                    sb.inner.library.drop_note_into_folder(&path, None);
                }
                sb.refresh_idle();
                true
            });
            list.add_controller(drop);
        }

        sidebar.build_switcher_popover(&switcher);
        sidebar.build_help_popover(&help_btn);
        {
            let sb = sidebar.clone();
            gear_btn.connect_clicked(move |_| {
                if let Some(f) = sb.inner.on_settings.borrow().as_ref() {
                    f();
                }
            });
        }
        {
            let sb = sidebar.clone();
            search.connect_search_changed(move |e| {
                *sb.inner.query.borrow_mut() = e.text().trim().to_lowercase();
                sb.refresh();
            });
            // Esc clears the search and returns to the full tree.
            let sb = sidebar.clone();
            search.connect_stop_search(move |e| {
                e.set_text("");
                *sb.inner.query.borrow_mut() = String::new();
                sb.refresh();
            });
        }
        sidebar.refresh();
        sidebar
    }

    /// The gear button: the app opens its Settings window.
    pub fn set_on_settings(&self, f: impl Fn() + 'static) {
        *self.inner.on_settings.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_on_open_note(&self, f: impl Fn(&Path) + 'static) {
        *self.inner.on_open_note.borrow_mut() = Some(Box::new(f));
    }

    /// Route note renames through the app so the currently open note's title is
    /// updated via storage (not written straight to disk, which autosave would
    /// clobber) and the on-canvas title stays in sync.
    pub fn set_on_rename_note(&self, f: impl Fn(&Path, &str) + 'static) {
        *self.inner.on_rename_note.borrow_mut() = Some(Box::new(f));
    }

    /// Fired after a notebook rename with (old_dir, new_dir), so the app can
    /// relocate the open note's save path.
    pub fn set_on_notebook_renamed(&self, f: impl Fn(&Path, &Path) + 'static) {
        *self.inner.on_notebook_renamed.borrow_mut() = Some(Box::new(f));
    }

    /// Fired after a notebook is trashed with its old dir.
    pub fn set_on_notebook_deleted(&self, f: impl Fn(&Path) + 'static) {
        *self.inner.on_notebook_deleted.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_selected(&self, path: &Path) {
        *self.inner.selected.borrow_mut() = Some(path.to_path_buf());
    }

    fn open_path(&self, path: &Path) {
        *self.inner.selected.borrow_mut() = Some(path.to_path_buf());
        if let Some(cb) = self.inner.on_open_note.borrow().as_ref() {
            cb(path);
        }
    }

    /// Persist collapsed-folder state to the XDG state dir.
    fn save_view_state(&self) {
        let vs = omaink_store::ViewState {
            collapsed_folders: self.inner.collapsed.borrow().iter().map(|f| f.0).collect(),
        };
        if let Err(e) = vs.save(&omaink_store::state_dir()) {
            tracing::warn!("could not save view state: {e}");
        }
    }

    /// Rebuild on the next idle tick. Use this when the trigger is a signal of
    /// a row widget that `refresh` would destroy (e.g. the rename entry's own
    /// activate/focus-leave), to avoid freeing a widget mid-emission.
    /// Ctrl+F: focus the title search.
    pub fn focus_search(&self) {
        self.inner.search.grab_focus();
    }

    /// Rows to show for a title search: matching notes plus their ancestor
    /// folders, with every folder expanded.
    fn search_rows(&self, query: &str) -> Vec<Row> {
        let mut out = Vec::new();
        // Folders seen above the current row, by depth; emitted on demand.
        let mut ancestors: Vec<(Row, bool)> = Vec::new();
        for row in self.inner.library.rows() {
            match &row {
                Row::Folder { depth, .. } => {
                    ancestors.truncate(*depth as usize);
                    ancestors.push((row.clone(), false));
                }
                Row::Note { title, depth, .. } => {
                    ancestors.truncate(*depth as usize);
                    if title.to_lowercase().contains(query) {
                        for (folder, shown) in ancestors.iter_mut() {
                            if !*shown {
                                out.push(folder.clone());
                                *shown = true;
                            }
                        }
                        out.push(row.clone());
                    }
                }
            }
        }
        out
    }

    fn refresh_idle(&self) {
        let sb = self.clone();
        glib::idle_add_local_once(move || sb.refresh());
    }

    /// Rebuild the list, honoring collapsed folders.
    pub fn refresh(&self) {
        let list = &self.inner.list;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        let selected = self.inner.selected.borrow().clone();
        let editing = *self.inner.editing.borrow();
        let collapsed = self.inner.collapsed.borrow().clone();

        let mut refs = Vec::new();
        let mut hide_below: Option<u32> = None;
        let mut visible_idx = 0usize;

        self.inner.multi.borrow_mut().retain(|p| p.exists());

        let query = self.inner.query.borrow().clone();
        let searching = !query.is_empty();
        let rows = if searching { self.search_rows(&query) } else { self.inner.library.rows() };
        if searching && rows.is_empty() {
            let empty = gtk::Label::new(Some("No matching notes"));
            empty.add_css_class("dim-label");
            empty.set_xalign(0.0);
            empty.set_margin_start(8);
            empty.set_margin_top(6);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&empty));
            row.set_activatable(false);
            row.set_selectable(false);
            list.append(&row);
        }

        for row in rows {
            let depth = match &row {
                Row::Folder { depth, .. } => *depth,
                Row::Note { depth, .. } => *depth,
            };
            // Skip anything beneath a collapsed folder.
            if let Some(hd) = hide_below {
                if depth > hd {
                    continue;
                }
                hide_below = None;
            }

            let is_collapsed_folder =
                !searching && matches!(&row, Row::Folder { id, .. } if collapsed.contains(id));
            if is_collapsed_folder {
                hide_below = Some(depth);
            }

            let is_selected = matches!(&row, Row::Note { path, .. } if selected.as_deref() == Some(path.as_path()));
            let (list_row, rref) =
                self.build_row(&row, visible_idx, editing == Some(visible_idx), is_collapsed_folder);
            list.append(&list_row);
            // With a multi-selection, highlighting comes only from it.
            if is_selected && self.inner.multi.borrow().is_empty() {
                list.select_row(Some(&list_row));
            }
            refs.push(rref);
            visible_idx += 1;
        }
        *self.inner.row_refs.borrow_mut() = refs;
    }

    fn build_row(
        &self,
        row: &Row,
        _idx: usize,
        editing: bool,
        collapsed: bool,
    ) -> (gtk::ListBoxRow, RowRef) {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        hbox.set_hexpand(true);
        hbox.set_margin_end(4);
        hbox.set_margin_start(2);

        let (rref, is_folder) = match row {
            Row::Folder { id, .. } => (RowRef::Folder(*id), true),
            Row::Note { path, .. } => (RowRef::Note { path: path.clone() }, false),
        };
        let (title_text, depth, conflict) = match row {
            Row::Folder { title, depth, .. } => (title.clone(), *depth, false),
            Row::Note { title, depth, conflict, .. } => (title.clone(), *depth, *conflict),
        };

        // Indent guides: one per ancestor level. Each is an INDENT-wide cell
        // with a 1px line centered in it, so the line sits directly under the
        // parent folder's chevron (also INDENT wide and centered).
        for _ in 0..depth {
            // Fixed-width cell with a 1px line placed at its horizontal center
            // via a left margin (no hexpand, which would make the cell greedy).
            let cell = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            cell.set_size_request(INDENT, -1);
            cell.set_hexpand(false);
            let line = gtk::Box::new(gtk::Orientation::Vertical, 0);
            line.set_size_request(1, -1);
            line.set_vexpand(true);
            line.set_margin_start(INDENT / 2);
            line.add_css_class("indent-line");
            cell.append(&line);
            hbox.append(&cell);
        }

        // Folder chevron (collapsible); notes get an aligning spacer of the
        // same width so note titles line up with folder titles.
        if is_folder {
            let chevron = gtk::Image::from_icon_name(if collapsed {
                "pan-end-symbolic"
            } else {
                "pan-down-symbolic"
            });
            chevron.add_css_class("folder-chevron");
            chevron.set_pixel_size(16);
            chevron.set_size_request(INDENT, -1);
            hbox.append(&chevron);
        } else {
            let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
            spacer.set_size_request(INDENT, -1);
            hbox.append(&spacer);
        }

        if editing {
            let entry = gtk::Entry::new();
            entry.set_text(&title_text);
            entry.set_hexpand(true);
            entry.add_css_class("rename-entry");
            hbox.append(&entry);

            let sb = self.clone();
            let rref2 = rref.clone();
            // Guard against the activate + focus-leave double fire.
            let commit = move |text: &str| {
                if sb.inner.editing.borrow().is_none() {
                    return;
                }
                *sb.inner.editing.borrow_mut() = None;
                let name = text.trim();
                if !name.is_empty() {
                    match &rref2 {
                        RowRef::Folder(id) => sb.inner.library.rename_folder(*id, name),
                        RowRef::Note { path } => {
                            let routed = sb
                                .inner
                                .on_rename_note
                                .borrow()
                                .as_ref()
                                .map(|cb| cb(path, name))
                                .is_some();
                            if !routed {
                                sb.inner.library.rename_note_on_disk(path, name);
                            }
                        }
                    }
                }
                sb.refresh_idle();
            };
            let c = commit.clone();
            entry.connect_activate(move |e| c(&e.text()));
            let c = commit.clone();
            let focus = gtk::EventControllerFocus::new();
            focus.connect_leave(move |ctrl| {
                if let Some(e) = ctrl.widget().and_downcast::<gtk::Entry>() {
                    c(&e.text());
                }
            });
            entry.add_controller(focus);
            let sb2 = self.clone();
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    *sb2.inner.editing.borrow_mut() = None;
                    sb2.refresh_idle();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            entry.add_controller(keys);
            entry.grab_focus();
        } else {
            let label = gtk::Label::new(Some(&title_text));
            label.set_xalign(0.0);
            label.set_hexpand(true);
            label.set_ellipsize(pango::EllipsizeMode::End);
            label.add_css_class("row-title");
            if is_folder {
                label.add_css_class("folder-title");
            }
            if conflict {
                label.set_tooltip_text(Some("Sync conflict copy"));
            }
            hbox.append(&label);
            hbox.append(&self.row_menu(&rref, &title_text));
        }

        let list_row = gtk::ListBoxRow::new();
        list_row.set_child(Some(&hbox));
        list_row.set_activatable(true);
        if let RowRef::Note { path } = &rref {
            if self.inner.multi.borrow().contains(path) {
                list_row.add_css_class("multi-selected");
            }
            // Ctrl+click toggles a note in the selection, Shift+click selects
            // a range; neither opens the note. Capture phase so the ListBox
            // doesn't activate the row first.
            let sb = self.clone();
            let path = path.clone();
            let click = gtk::GestureClick::new();
            click.set_button(gdk::BUTTON_PRIMARY);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed(move |g, n, _, _| {
                if n != 1 {
                    return;
                }
                let m = g.current_event_state();
                let ctrl = m.contains(gdk::ModifierType::CONTROL_MASK);
                let shift = m.contains(gdk::ModifierType::SHIFT_MASK);
                if !(ctrl || shift) {
                    return;
                }
                g.set_state(gtk::EventSequenceState::Claimed);
                if shift {
                    sb.select_range_to(&path, ctrl);
                } else {
                    sb.toggle_in_selection(&path);
                }
                sb.refresh_idle();
            });
            list_row.add_controller(click);
        }

        {
            let sb = self.clone();
            let rref2 = rref.clone();
            let title2 = title_text.clone();
            let dbl = gtk::GestureClick::new();
            dbl.set_button(gdk::BUTTON_PRIMARY);
            // Capture phase: see the double press before the ListBox consumes it.
            dbl.set_propagation_phase(gtk::PropagationPhase::Capture);
            dbl.connect_pressed(move |_, n, _, _| {
                if n == 2 {
                    match &rref2 {
                        RowRef::Folder(id) => sb.prompt_rename_folder(*id, &title2),
                        RowRef::Note { path } => sb.prompt_rename_note(path, &title2),
                    }
                }
            });
            list_row.add_controller(dbl);
        }

        self.attach_dnd(&list_row, &rref);
        (list_row, rref)
    }

    fn attach_dnd(&self, list_row: &gtk::ListBoxRow, rref: &RowRef) {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::MOVE);
        let sb = self.clone();
        let r = rref.clone();
        source.connect_prepare(move |_, _, _| {
            *sb.inner.drag.borrow_mut() = Some(r.clone());
            Some(gdk::ContentProvider::for_value(&"omaink-row".to_value()))
        });
        list_row.add_controller(source);

        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let sb = self.clone();
        let r = rref.clone();
        let row_weak = list_row.downgrade();
        target.connect_drop(move |_, _, _x, y| {
            let Some(src) = sb.inner.drag.borrow_mut().take() else { return false };
            let height = row_weak.upgrade().map(|w| w.height()).unwrap_or(26).max(1);
            let frac = y / height as f64;
            // Dragging a note that's part of the multi-selection moves them all.
            let group: Vec<PathBuf> = match &src {
                RowRef::Note { path } if sb.inner.multi.borrow().contains(path) => sb.inner.multi.borrow().clone(),
                RowRef::Note { path } => vec![path.clone()],
                RowRef::Folder(_) => Vec::new(),
            };
            match (&src, &r) {
                (RowRef::Note { .. }, RowRef::Note { path: tpath }) => {
                    if group.contains(tpath) {
                        return false;
                    }
                    let mut after = tpath.clone();
                    for (i, p) in group.iter().enumerate() {
                        if i == 0 {
                            sb.inner.library.drop_note_near_note(p, tpath, frac >= 0.5);
                        } else {
                            sb.inner.library.drop_note_near_note(p, &after, true);
                        }
                        after = p.clone();
                    }
                }
                (RowRef::Note { .. }, RowRef::Folder(fid)) => {
                    for p in &group {
                        sb.inner.library.drop_note_into_folder(p, Some(*fid));
                    }
                }
                // Sub-folders are disabled: dropping a folder on a folder
                // only reorders it (no nesting).
                (RowRef::Folder(sid), RowRef::Folder(tid)) => {
                    sb.inner.library.drop_folder_near_folder(*sid, *tid, frac >= 0.5);
                }
                (RowRef::Folder(_), RowRef::Note { .. }) => return false,
            }
            sb.refresh_idle();
            true
        });
        list_row.add_controller(target);
    }

    fn row_menu(&self, rref: &RowRef, title: &str) -> gtk::MenuButton {
        let title = title.to_string();
        let btn = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Actions")
            .build();
        btn.add_css_class("flat");
        btn.add_css_class("row-menu");
        let popover = gtk::Popover::new();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_top(4);
        vbox.set_margin_bottom(4);
        vbox.set_margin_start(4);
        vbox.set_margin_end(4);

        match rref {
            RowRef::Folder(id) => {
                let id = *id;
                let new_note = flat_button("New note here");
                let rename = flat_button("Rename");
                let del = flat_button("Delete folder");
                del.add_css_class("destructive-action");
                vbox.append(&new_note);
                vbox.append(&rename);
                vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                vbox.append(&del);

                let sb = self.clone();
                let pop = popover.clone();
                new_note.connect_clicked(move |_| {
                    if let Some((_, path)) = sb.inner.library.new_note(Some(id)) {
                        sb.set_selected(&path);
                        sb.refresh_idle();
                        sb.open_path(&path);
                    }
                    pop.popdown();
                });
                let sb = self.clone();
                let pop = popover.clone();
                let t = title.clone();
                rename.connect_clicked(move |_| {
                    pop.popdown();
                    sb.prompt_rename_folder(id, &t);
                });
                let sb = self.clone();
                let pop = popover.clone();
                let t = title.clone();
                del.connect_clicked(move |_| {
                    pop.popdown();
                    sb.confirm_delete_folder(id, &t);
                });
            }
            RowRef::Note { path } => {
                let path = path.clone();
                let multi_n = {
                    let m = self.inner.multi.borrow();
                    if m.len() > 1 && m.contains(&path) { m.len() } else { 0 }
                };
                let rename = flat_button("Rename");
                let del = flat_button(&if multi_n > 0 { format!("Delete {multi_n} notes") } else { "Delete note".into() });
                del.add_css_class("destructive-action");
                vbox.append(&rename);
                vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                vbox.append(&del);

                let sb = self.clone();
                let pop = popover.clone();
                let p2 = path.clone();
                let t = title.clone();
                rename.connect_clicked(move |_| {
                    pop.popdown();
                    sb.prompt_rename_note(&p2, &t);
                });
                let sb = self.clone();
                let pop = popover.clone();
                let t = title.clone();
                del.connect_clicked(move |_| {
                    pop.popdown();
                    if multi_n > 0 {
                        sb.confirm_delete_multi();
                    } else {
                        sb.confirm_delete_note(&path, &t);
                    }
                });
            }
        }
        popover.set_child(Some(&vbox));
        btn.set_popover(Some(&popover));
        btn
    }

    fn build_switcher_popover(&self, button: &gtk::MenuButton) {
        let sb = self.clone();
        button.set_create_popup_func(move |button| {
            let popover = gtk::Popover::new();
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
            vbox.set_margin_top(6);
            vbox.set_margin_bottom(6);
            vbox.set_margin_start(6);
            vbox.set_margin_end(6);
            for name in sb.inner.library.notebook_names() {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
                let name_btn = flat_button(&name);
                name_btn.set_hexpand(true);
                let sb2 = sb.clone();
                let pop = popover.clone();
                let n = name.clone();
                name_btn.connect_clicked(move |_| {
                    sb2.inner.library.select_notebook(&n);
                    sb2.inner.notebook_label.set_text(&n);
                    *sb2.inner.selected.borrow_mut() = None;
                    sb2.refresh_idle();
                    pop.popdown();
                });
                row.append(&name_btn);

                // Same ⋯ pattern as note/folder rows, acting on this notebook.
                let kebab = gtk::MenuButton::builder()
                    .icon_name("view-more-symbolic")
                    .tooltip_text("Notebook actions")
                    .build();
                kebab.add_css_class("flat");
                let kpop = gtk::Popover::new();
                let kv = gtk::Box::new(gtk::Orientation::Vertical, 2);
                kv.set_margin_top(4);
                kv.set_margin_bottom(4);
                kv.set_margin_start(4);
                kv.set_margin_end(4);
                let ren = flat_button("Rename…");
                let del = flat_button("Delete…");
                del.add_css_class("destructive-action");
                kv.append(&ren);
                kv.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                kv.append(&del);
                kpop.set_child(Some(&kv));
                kebab.set_popover(Some(&kpop));

                // Acting on a notebook selects it first, then runs the flow.
                let select_then = {
                    let sb = sb.clone();
                    let n = name.clone();
                    move || {
                        sb.inner.library.select_notebook(&n);
                        sb.inner.notebook_label.set_text(&n);
                        *sb.inner.selected.borrow_mut() = None;
                        sb.refresh_idle();
                    }
                };
                let sb2 = sb.clone();
                let pop2 = popover.clone();
                let kp = kpop.clone();
                let st = select_then.clone();
                ren.connect_clicked(move |_| {
                    kp.popdown();
                    pop2.popdown();
                    st();
                    sb2.prompt_rename_notebook();
                });
                let sb2 = sb.clone();
                let pop2 = popover.clone();
                let kp = kpop.clone();
                del.connect_clicked(move |_| {
                    kp.popdown();
                    pop2.popdown();
                    select_then();
                    sb2.confirm_delete_notebook();
                });
                row.append(&kebab);
                vbox.append(&row);
            }
            vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            let new_btn = flat_button("New notebook…");
            let sb2 = sb.clone();
            let pop = popover.clone();
            new_btn.connect_clicked(move |_| {
                pop.popdown();
                sb2.prompt_new_notebook();
            });
            vbox.append(&new_btn);
            popover.set_child(Some(&vbox));
            button.set_popover(Some(&popover));
        });
    }

    fn build_help_popover(&self, button: &gtk::Button) {
        let popover = gtk::Popover::new();
        popover.set_parent(button);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
        vbox.set_margin_top(10);
        vbox.set_margin_bottom(10);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        let heading = gtk::Label::new(Some("Keyboard shortcuts"));
        heading.add_css_class("heading");
        heading.set_xalign(0.0);
        vbox.append(&heading);
        for (k, v) in [
            ("Ctrl+Z / Ctrl+Shift+Z", "Undo / Redo"),
            ("F9", "Toggle sidebar"),
            ("F11", "Fullscreen canvas"),
            ("Ctrl+F", "Search note titles (Esc clears)"),
            ("Ctrl+= / Ctrl+−", "Zoom in / out"),
            ("Ctrl+0", "Reset view (100%, top-left)"),
            ("Home", "Go to the top-left of the page"),
            ("Scroll / middle-drag", "Pan"),
            ("Ctrl+Scroll", "Zoom"),
            ("Text tool: click the page", "New text box (click a box to edit)"),
            ("Click T again", "Text settings: style, size, color, lists"),
            ("Double-click a text box", "Edit it (in Select)"),
            ("Ctrl+B / I / U", "Bold / italic / underline"),
            ("Ctrl+Shift+H", "Highlight"),
            ("Ctrl+1", "Checkbox: add / tick / untick"),
            ("Esc", "Finish editing"),
            ("Ctrl+C / X / V", "Copy / cut / paste (images too)"),
            ("Delete", "Delete selection"),
            ("Pointer: drag empty canvas", "Select everything in the box"),
            ("Shift+click / Shift+drag", "Add to / remove from selection"),
            ("Drag a corner handle", "Resize selection"),
            ("Shift while drawing a shape", "Square / circle / 45°"),
            ("Toolbar image button", "Insert images from files"),
            ("Right-click / hold an image", "Pin to background / delete"),
            ("Pen button", "Toggle eraser"),
        ] {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            let key = gtk::Label::new(Some(k));
            key.set_xalign(0.0);
            key.add_css_class("dim-label");
            key.set_width_chars(22);
            let desc = gtk::Label::new(Some(v));
            desc.set_xalign(0.0);
            line.append(&key);
            line.append(&desc);
            vbox.append(&line);
        }
        popover.set_child(Some(&vbox));
        button.connect_clicked(move |_| popover.popup());
    }

    fn prompt_new_notebook(&self) {
        // A dialog, not a popover: popovers opened while another popover is
        // closing get dismissed by the grab teardown.
        let dialog = adw::AlertDialog::new(Some("New notebook"), None);
        let entry = gtk::Entry::builder().placeholder_text("Notebook name").build();
        entry.set_activates_default(true);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("create", "Create");
        dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("create"));
        dialog.set_close_response("cancel");
        let sb = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "create" {
                return;
            }
            let name = entry.text().trim().to_string();
            if !name.is_empty() {
                sb.inner.library.create_notebook(&name);
                sb.inner.notebook_label.set_text(&name);
                *sb.inner.selected.borrow_mut() = None;
                sb.refresh_idle();
            }
        });
        let win = self.widget.root().and_downcast::<gtk::Window>();
        dialog.present(win.as_ref());
    }
}

impl Sidebar {
    /// Shared name dialog used by folder/note rename.
    fn name_dialog(&self, heading: &str, initial: &str, verb: &str, on_commit: impl Fn(&str) + 'static) {
        let dialog = adw::AlertDialog::new(Some(heading), None);
        let entry = gtk::Entry::new();
        entry.set_text(initial);
        entry.set_activates_default(true);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("ok", verb);
        dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("ok"));
        dialog.set_close_response("cancel");
        let e = entry.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp == "ok" {
                let name = e.text().trim().to_string();
                if !name.is_empty() {
                    on_commit(&name);
                }
            }
        });
        let win = self.widget.root().and_downcast::<gtk::Window>();
        dialog.present(win.as_ref());
        entry.grab_focus();
    }

    fn prompt_rename_folder(&self, id: FolderId, current: &str) {
        let sb = self.clone();
        self.name_dialog("Rename folder", current, "Rename", move |name| {
            sb.inner.library.rename_folder(id, name);
            sb.refresh_idle();
        });
    }

    fn prompt_rename_note(&self, path: &Path, current: &str) {
        let sb = self.clone();
        let path = path.to_path_buf();
        self.name_dialog("Rename note", current, "Rename", move |name| {
            let routed = sb
                .inner
                .on_rename_note
                .borrow()
                .as_ref()
                .map(|cb| cb(&path, name))
                .is_some();
            if !routed {
                sb.inner.library.rename_note_on_disk(&path, name);
            }
            sb.refresh_idle();
        });
    }

    /// Visible note rows, top to bottom.
    fn visible_notes(&self) -> Vec<PathBuf> {
        self.inner
            .row_refs
            .borrow()
            .iter()
            .filter_map(|r| match r {
                RowRef::Note { path } => Some(path.clone()),
                RowRef::Folder(_) => None,
            })
            .collect()
    }

    /// Ctrl+click: add/remove one note. Starting a selection includes the
    /// open note, like a file manager.
    fn toggle_in_selection(&self, path: &Path) {
        let mut multi = self.inner.multi.borrow_mut();
        if multi.is_empty() {
            if let Some(open) = self.inner.selected.borrow().clone() {
                if open != path {
                    multi.push(open);
                }
            }
        }
        if let Some(i) = multi.iter().position(|p| p == path) {
            multi.remove(i);
        } else {
            multi.push(path.to_path_buf());
        }
        drop(multi);
        *self.inner.anchor.borrow_mut() = Some(path.to_path_buf());
        self.sort_multi();
    }

    /// Shift+click: everything between the anchor (or open note) and
    /// `path`; with Ctrl too, add the range to the existing selection.
    fn select_range_to(&self, path: &Path, extend: bool) {
        let notes = self.visible_notes();
        let anchor = self.inner.anchor.borrow().clone().or_else(|| self.inner.selected.borrow().clone());
        let (Some(a), Some(b)) = (
            anchor.and_then(|a| notes.iter().position(|p| *p == a)),
            notes.iter().position(|p| p == path),
        ) else {
            return self.toggle_in_selection(path);
        };
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let mut multi = self.inner.multi.borrow_mut();
        if !extend {
            multi.clear();
        }
        for p in &notes[lo..=hi] {
            if !multi.contains(p) {
                multi.push(p.clone());
            }
        }
        drop(multi);
        self.sort_multi();
    }

    /// Keep the selection in on-screen order (moves keep their order).
    fn sort_multi(&self) {
        let notes = self.visible_notes();
        self.inner
            .multi
            .borrow_mut()
            .sort_by_key(|p| notes.iter().position(|q| q == p).unwrap_or(usize::MAX));
    }

    fn confirm_delete_multi(&self) {
        let paths = self.inner.multi.borrow().clone();
        if paths.is_empty() {
            return;
        }
        let n = paths.len();
        let dialog = adw::AlertDialog::new(
            Some(&if n == 1 { "Delete 1 note?".to_string() } else { format!("Delete {n} notes?") }),
            Some("They move to the notebook's trash."),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", &if n == 1 { "Delete note".to_string() } else { format!("Delete {n} notes") });
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        let sb = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "delete" {
                return;
            }
            for path in &paths {
                if let Some(id) = note_id_from_path(path) {
                    sb.inner.library.delete_note(id);
                }
                if sb.inner.selected.borrow().as_deref() == Some(path.as_path()) {
                    *sb.inner.selected.borrow_mut() = None;
                }
            }
            sb.inner.multi.borrow_mut().clear();
            sb.refresh_idle();
        });
        let win = self.widget.root().and_downcast::<gtk::Window>();
        dialog.present(win.as_ref());
    }

    fn confirm_delete_note(&self, path: &Path, title: &str) {
        let dialog = adw::AlertDialog::new(
            Some(&format!("Delete “{title}”?")),
            Some("The note moves to the notebook's trash."),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete note");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        let sb = self.clone();
        let path = path.to_path_buf();
        dialog.connect_response(None, move |_, resp| {
            if resp != "delete" {
                return;
            }
            if let Some(id) = note_id_from_path(&path) {
                sb.inner.library.delete_note(id);
                if sb.inner.selected.borrow().as_deref() == Some(path.as_path()) {
                    *sb.inner.selected.borrow_mut() = None;
                }
                sb.refresh_idle();
            }
        });
        let win = self.widget.root().and_downcast::<gtk::Window>();
        dialog.present(win.as_ref());
    }

    fn confirm_delete_folder(&self, id: FolderId, title: &str) {
        let dialog = adw::AlertDialog::new(
            Some(&format!("Delete “{title}”?")),
            Some("The folder moves to the trash; its notes move up one level."),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete folder");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        let sb = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "delete" {
                return;
            }
            sb.inner.library.delete_folder(id);
            sb.refresh_idle();
        });
        let win = self.widget.root().and_downcast::<gtk::Window>();
        dialog.present(win.as_ref());
    }

    fn prompt_rename_notebook(&self) {
        let current = self.inner.library.current_notebook_name();
        let dialog = adw::AlertDialog::new(Some("Rename notebook"), None);
        let entry = gtk::Entry::new();
        entry.set_text(&current);
        entry.set_activates_default(true);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("rename", "Rename");
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");
        let sb = self.clone();
        let entry2 = entry.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "rename" {
                return;
            }
            let name = entry2.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            if let Some((old_dir, new_dir)) = sb.inner.library.rename_current_notebook(&name) {
                sb.inner.notebook_label.set_text(&name);
                // Selected note path moved with the directory.
                let moved = sb.inner.selected.borrow().as_ref().and_then(|p| {
                    p.strip_prefix(&old_dir).ok().map(|rest| new_dir.join(rest))
                });
                if let Some(m) = moved {
                    *sb.inner.selected.borrow_mut() = Some(m);
                }
                sb.refresh_idle();
                if let Some(cb) = sb.inner.on_notebook_renamed.borrow().as_ref() {
                    cb(&old_dir, &new_dir);
                }
            }
        });
        let win = self.widget.root().and_downcast::<gtk::Window>();
        dialog.present(win.as_ref());
        entry.grab_focus();
    }

    fn confirm_delete_notebook(&self) {
        let name = self.inner.library.current_notebook_name();
        let dialog = adw::AlertDialog::new(
            Some(&format!("Delete “{name}”?")),
            Some("The notebook and all its notes move to the trash folder inside your notebooks directory."),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete notebook");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        let sb = self.clone();
        dialog.connect_response(None, move |_, resp| {
            if resp != "delete" {
                return;
            }
            if let Some(old_dir) = sb.inner.library.delete_current_notebook() {
                *sb.inner.selected.borrow_mut() = None;
                sb.inner.notebook_label.set_text(&sb.inner.library.current_notebook_name());
                sb.refresh();
                if let Some(cb) = sb.inner.on_notebook_deleted.borrow().as_ref() {
                    cb(&old_dir);
                }
            }
        });
        let win = self.widget.root().and_downcast::<gtk::Window>();
        dialog.present(win.as_ref());
    }
}

fn note_id_from_path(path: &Path) -> Option<omaink_core::NoteId> {
    let stem = path.file_stem()?.to_str()?;
    stem.parse::<uuid::Uuid>().ok().map(omaink_core::NoteId)
}

fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tip));
    b
}

fn glyph_button(glyph: gtk::DrawingArea, tip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&glyph));
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tip));
    b
}

/// 18px outline icon on Lucide's 24-unit grid, in the widget's text color
/// (so it follows the theme like a symbolic icon).
fn outline_glyph(draw: impl Fn(&gtk::cairo::Context) + 'static) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(18);
    area.set_content_height(18);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |a, cr, w, h| {
        let c = a.color();
        cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64);
        let k = (w.min(h) as f64) / 24.0;
        cr.scale(k, k);
        cr.set_line_width(2.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        draw(cr);
    });
    area
}

/// Settings: a simplified gear — a ring with eight short teeth.
fn gear_glyph() -> gtk::DrawingArea {
    outline_glyph(|cr| {
        use std::f64::consts::TAU;
        cr.arc(12.0, 12.0, 6.5, 0.0, TAU);
        let _ = cr.stroke();
        cr.set_line_width(3.2);
        cr.set_line_cap(gtk::cairo::LineCap::Butt);
        for i in 0..8 {
            let a = TAU * i as f64 / 8.0;
            cr.move_to(12.0 + 7.0 * a.cos(), 12.0 + 7.0 * a.sin());
            cr.line_to(12.0 + 10.0 * a.cos(), 12.0 + 10.0 * a.sin());
        }
        let _ = cr.stroke();
    })
}

/// Help: a circle with a small question mark that clears the outline
/// (Lucide `circle-help` proportions, mark scaled down).
fn help_glyph() -> gtk::DrawingArea {
    outline_glyph(|cr| {
        use std::f64::consts::TAU;
        cr.arc(12.0, 12.0, 10.0, 0.0, TAU);
        let _ = cr.stroke();
        let _ = cr.save();
        cr.translate(12.0, 12.0);
        cr.scale(0.8, 0.8);
        cr.translate(-12.0, -12.0);
        // Hook: arc over the top, then down into the stem.
        cr.arc(12.0, 9.5, 2.9, 1.1 * std::f64::consts::PI, 2.25 * std::f64::consts::PI);
        cr.curve_to(14.0, 11.6, 12.0, 12.0, 12.0, 14.0);
        let _ = cr.restore();
        let _ = cr.stroke();
        // Dot, in the same scaled frame as the hook.
        cr.arc(12.0, 16.4, 1.15, 0.0, TAU);
        let _ = cr.fill();
    })
}

fn flat_button(label: &str) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    b.add_css_class("flat");
    b.set_halign(gtk::Align::Fill);
    if let Some(child) = b.child().and_downcast::<gtk::Label>() {
        child.set_xalign(0.0);
    }
    b
}
