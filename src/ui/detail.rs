use relm4::RelmRemoveAllExt;
use relm4::adw::{self, prelude::*};
use relm4::gtk::{self, glib};

use crate::core::Section;

pub fn placeholder() -> adw::StatusPage {
    adw::StatusPage::builder()
        .icon_name("view-paged-symbolic")
        .title("No selection")
        .description("Select an item to see its details")
        .build()
}

pub fn fill(body: &gtk::Box, sections: &[Section]) {
    body.remove_all();
    for section in sections {
        let group = adw::PreferencesGroup::builder().title(section.title).build();
        if section.rows.is_empty() {
            group.set_description(Some("None"));
        }
        for (key, value) in &section.rows {
            group.add(
                &adw::ActionRow::builder()
                    .title(glib::markup_escape_text(key).as_str())
                    .subtitle(glib::markup_escape_text(value).as_str())
                    .build(),
            );
        }
        body.append(&group);
    }
}

pub struct Pane {
    stack: gtk::Stack,
    title: gtk::Label,
    body: gtk::Box,
}

impl Pane {
    /// `on_close` runs when the user clicks the close button.
    pub fn new(on_close: impl Fn() + 'static) -> Self {
        let title = gtk::Label::builder()
            .css_classes(["title-2"])
            .xalign(0.0)
            .hexpand(true)
            .selectable(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(18);
        content.set_margin_bottom(18);
        content.set_margin_start(18);
        content.set_margin_end(18);
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close details")
            .valign(gtk::Align::Start)
            .css_classes(["flat"])
            .build();
        close.connect_clicked(move |_| on_close());
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.append(&title);
        header.append(&close);
        content.append(&header);
        content.append(&body);
        let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&content).build();
        let stack = gtk::Stack::new();
        stack.add_named(&placeholder(), Some("empty"));
        stack.add_named(&scroll, Some("detail"));
        stack.set_visible(false);
        Self { stack, title, body }
    }

    pub fn widget(&self) -> &gtk::Stack {
        &self.stack
    }

    pub fn show(&self, title: &str, sections: &[Section]) {
        self.title.set_label(title);
        fill(&self.body, sections);
        self.stack.set_visible_child_name("detail");
        self.stack.set_visible(true);
    }

    pub fn clear(&self) {
        self.stack.set_visible(false);
    }
}
