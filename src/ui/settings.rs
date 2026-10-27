//! The settings dialog (ADR-0014): libadwaita's preferences dialog, as in
//! omarepos and omabus. A change applies at once; there is nothing to save.

use adw::prelude::*;

use crate::settings::{self, Settings, TEXT_SIZES, Width};

/// Change the settings with `f` and apply them everywhere.
fn change(f: impl FnOnce(&mut Settings)) {
    let mut s = settings::get();
    f(&mut s);
    if s != settings::get() {
        settings::set(s);
        super::apply_settings();
    }
}

fn combo(title: &str, subtitle: &str, items: &[&str], selected: u32) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(items))
        .selected(selected)
        .build();
    if !subtitle.is_empty() {
        row.set_subtitle(subtitle);
    }
    row
}

fn switch(title: &str, subtitle: &str, on: bool) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).active(on).build();
    if !subtitle.is_empty() {
        row.set_subtitle(subtitle);
    }
    row
}

pub fn dialog() -> adw::PreferencesDialog {
    let s = settings::get();

    let sizes: Vec<String> = TEXT_SIZES.iter().map(|n| format!("{n} %")).collect();
    let sizes: Vec<&str> = sizes.iter().map(String::as_str).collect();
    let size_at = TEXT_SIZES.iter().position(|n| *n == s.text_size).unwrap_or(2) as u32;
    let text_size = combo("Text size", "Ctrl+plus and Ctrl+minus change it for a while; Ctrl+0 comes back here", &sizes, size_at);
    text_size.connect_selected_notify(|row| {
        let n = TEXT_SIZES.get(row.selected() as usize).copied().unwrap_or(100);
        change(|s| s.text_size = n);
    });
    let width_at = Width::ALL.iter().position(|w| *w == s.width).unwrap_or(0) as u32;
    let width = combo("Page width", "Narrow keeps lines near 66 characters", &["Narrow", "Wide", "Full window"], width_at);
    width.connect_selected_notify(|row| {
        let w = Width::ALL.get(row.selected() as usize).copied().unwrap_or(Width::Narrow);
        change(|s| s.width = w);
    });
    let reading = adw::PreferencesGroup::builder().title("Reading").build();
    reading.add(&text_size);
    reading.add(&width);

    let side = combo("Folder tree", "", &["On the right", "On the left"], u32::from(s.tree_left));
    side.connect_selected_notify(|row| {
        let left = row.selected() == 1;
        change(|s| s.tree_left = left);
    });
    let hints = switch("Key hints in the status bar", "The keys that matter where the keyboard is; ? shows all of them", s.key_hints);
    hints.connect_active_notify(|row| {
        let on = row.is_active();
        change(|s| s.key_hints = on);
    });
    let window = adw::PreferencesGroup::builder().title("Window").build();
    window.add(&side);
    window.add(&hints);

    let hidden = switch("Show hidden files and folders", "Names starting with a dot", s.hidden);
    hidden.connect_active_notify(|row| {
        let on = row.is_active();
        change(|s| s.hidden = on);
    });
    let gitignore = switch("Leave out what .gitignore leaves out", "Build output, dependencies and the like", s.gitignore);
    gitignore.connect_active_notify(|row| {
        let on = row.is_active();
        change(|s| s.gitignore = on);
    });
    let folder = adw::PreferencesGroup::builder()
        .title("Folder")
        .description("omamv shows .md and .txt files")
        .build();
    folder.add(&hidden);
    folder.add(&gitignore);

    let web = switch(
        "Show pictures from the web",
        "Off: pictures a document links from the internet are not loaded, and no site learns it was read",
        s.web_pictures,
    );
    web.connect_active_notify(|row| {
        let on = row.is_active();
        change(|s| s.web_pictures = on);
    });
    let privacy = adw::PreferencesGroup::builder().title("Privacy").build();
    privacy.add(&web);

    let page = adw::PreferencesPage::new();
    page.add(&reading);
    page.add(&window);
    page.add(&folder);
    page.add(&privacy);
    let dialog = adw::PreferencesDialog::builder().title("Settings").build();
    dialog.add(&page);
    dialog
}
