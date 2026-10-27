//! The key overview (`?`): every key on one dialog, in three groups
//! (ADR-0008). The status bar shows the ones that matter where the keyboard
//! is (ADR-0013); this shows them all.

use adw::prelude::*;

/// (group, [(keys, action)]). Keys: `a / b` are alternatives, `A+B` a combo.
pub const KEYS: &[(&str, &[(&str, &str)])] = &[
    (
        "READING",
        &[
            ("j / k", "line down / up"),
            ("↓ / ↑", "line down / up"),
            ("d / u", "half a page"),
            ("Space / Shift+Space", "a page"),
            ("g g / G", "top / end"),
            ("o", "outline: go to a heading"),
            ("Tab / Shift+Tab", "next / previous link"),
            ("Enter", "follow the link"),
            ("Alt+← / Alt+→", "back / forward"),
            ("Ctrl++ / Ctrl+− / Ctrl+0", "text size"),
        ],
    ),
    (
        "FILES",
        &[
            ("Ctrl+P", "open a file, newest first"),
            ("y", "copy the file's path"),
            ("] / [", "next / previous file"),
            ("J / K", "next / previous file"),
            ("Ctrl+B", "folder tree: focus, or hide"),
            ("j k h l", "tree: move, fold, unfold"),
            ("↓ ↑ ← →", "tree: move, fold, unfold"),
            ("Enter", "tree: open the file"),
            ("Esc", "tree: back to the page"),
            ("Ctrl+O", "open a folder"),
            ("Ctrl+Shift+O", "recent folders"),
        ],
    ),
    (
        "FIND AND MORE",
        &[
            ("Ctrl+F / /", "find in the page"),
            ("Enter / Shift+Enter", "next / previous match"),
            ("n / N", "next / previous, after Esc"),
            ("Esc", "close find, keep the place"),
            ("Ctrl+C", "copy the selection"),
            ("?", "this overview"),
            ("Ctrl+,", "settings"),
            ("F10", "menu"),
            ("Ctrl+W", "close the window"),
            ("Ctrl+Q", "quit"),
        ],
    ),
];

/// The parts of one key or combo: `Ctrl++` is Ctrl and plus.
fn combo_parts(alt: &str) -> Vec<&str> {
    if alt.len() > 1 && let Some(head) = alt.strip_suffix("++") {
        return vec![head, "+"];
    }
    if alt.len() > 1 && alt.contains('+') {
        return alt.split('+').collect();
    }
    vec![alt]
}

fn keys_cell(spec: &str) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let alternatives: Vec<&str> = if spec == "Ctrl+F / /" { vec!["Ctrl+F", "/"] } else { spec.split(" / ").collect() };
    for (i, alt) in alternatives.iter().enumerate() {
        if i > 0 {
            let slash = gtk::Label::new(Some("/"));
            slash.add_css_class("om-plus");
            b.append(&slash);
        }
        if *alt == "j k h l" || *alt == "↓ ↑ ← →" {
            for s in alt.split(' ') {
                b.append(&super::kbd(s));
            }
            continue;
        }
        for (j, part) in combo_parts(alt).into_iter().enumerate() {
            if j > 0 {
                let plus = gtk::Label::new(Some("+"));
                plus.add_css_class("om-plus");
                b.append(&plus);
            }
            b.append(&super::kbd(part));
        }
    }
    b
}

pub fn dialog() -> adw::Dialog {
    let cols = gtk::Box::new(gtk::Orientation::Horizontal, 36);
    cols.set_homogeneous(true);
    for (group, rows) in KEYS {
        let col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let title = gtk::Label::builder().label(*group).xalign(0.0).margin_top(8).margin_bottom(8).build();
        title.add_css_class("om-kg-title");
        col.append(&title);
        for (keys, action) in *rows {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("om-kr");
            let k = keys_cell(keys);
            k.set_valign(gtk::Align::Center);
            let a = gtk::Label::builder().label(*action).xalign(1.0).hexpand(true).wrap(true).build();
            a.add_css_class("om-ka");
            row.append(&k);
            row.append(&a);
            col.append(&row);
        }
        cols.append(&col);
    }
    let title = gtk::Label::builder().label("Keys").xalign(0.0).hexpand(true).build();
    title.add_css_class("om-dlg-title");
    let sub = gtk::Label::new(Some("Esc closes"));
    sub.add_css_class("om-dlg-sub");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.set_margin_bottom(8);
    head.append(&title);
    head.append(&sub);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body.set_margin_top(18);
    body.set_margin_bottom(22);
    body.set_margin_start(26);
    body.set_margin_end(26);
    body.append(&head);
    body.append(&cols);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&body)
        .build();
    let dialog = adw::Dialog::builder().title("Keys").content_width(1040).child(&scroller).build();
    dialog.add_css_class("om-dialog");
    dialog
}

#[cfg(test)]
mod tests {
    use super::combo_parts;

    #[test]
    fn plus_as_a_key() {
        assert_eq!(combo_parts("Ctrl++"), ["Ctrl", "+"]);
        assert_eq!(combo_parts("Ctrl+Shift+O"), ["Ctrl", "Shift", "O"]);
        assert_eq!(combo_parts("?"), ["?"]);
        assert_eq!(combo_parts("g g"), ["g g"]);
    }
}
