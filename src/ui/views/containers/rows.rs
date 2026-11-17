use bollard::models::{ContainerSummary, ContainerSummaryStateEnum as State};
use relm4::adw::{self, prelude::*};
use relm4::{ComponentSender, gtk, gtk::glib};

use super::{ContainersView, Msg, Op};
use crate::core::{self, Group};
use crate::ui::Opener;

type Sender = ComponentSender<ContainersView>;

fn dot(running: bool) -> gtk::Box {
    let dot = gtk::Box::builder().valign(gtk::Align::Center).halign(gtk::Align::Center).css_classes(["dp-dot"]).build();
    if running {
        dot.add_css_class("running");
    }
    dot
}

pub fn group_header(group: &Group, open: bool, collapsible: bool, zebra: bool, sender: &Sender) -> gtk::Box {
    let (up, total) = (group.containers.iter().filter(|c| core::is_up(c)).count(), group.containers.len());
    let label = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    label.append(&gtk::Image::from_icon_name(if open { "pan-down-symbolic" } else { "pan-end-symbolic" }));
    label.append(&dot(up > 0));
    label.append(&gtk::Label::builder().label(group.project.as_deref().unwrap_or("Standalone")).css_classes(["heading"]).build());
    label.append(&gtk::Label::builder().label(format!("{up}/{total}")).css_classes(["dp-pill", core::state_class(Some((up, total)))]).build());
    let toggle = gtk::Button::builder().child(&label).css_classes(["flat"]).sensitive(collapsible).hexpand(true).build();
    label.set_halign(gtk::Align::Start);
    let (project, s) = (group.project.clone(), sender.clone());
    toggle.connect_clicked(move |_| s.input(Msg::Toggle(project.clone())));

    let actions = group_actions(group, sender);
    actions.set_halign(gtk::Align::End);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.add_css_class("dp-group");
    if zebra {
        header.add_css_class("dp-zebra");
    }
    header.append(&toggle);
    header.append(&actions);
    header
}

pub fn group_actions(group: &Group, sender: &Sender) -> gtk::Box {
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let Some(project) = &group.project else { return actions };
    if let Some(dir) = core::group_working_dir(group) {
        for opener in [Opener::Files, Opener::Code] {
            let (s, d) = (sender.clone(), dir.clone());
            actions.append(&opener.button(&dir, move |o| s.input(Msg::Open(o, d.clone()))));
        }
    }
    for (op, icon, tooltip) in [
        (Op::Start, "media-playback-start-symbolic", "Start project containers"),
        (Op::Stop, "media-playback-stop-symbolic", "Stop project containers"),
        (Op::Restart, "view-refresh-symbolic", "Restart project containers"),
        (Op::Remove, "user-trash-symbolic", "Remove project containers"),
    ] {
        let b = gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).css_classes(["flat", "dp-iconbtn"]).build();
        if matches!(op, Op::Remove) {
            b.add_css_class("dp-danger");
        }
        let (s, project) = (sender.clone(), project.clone());
        b.connect_clicked(move |_| s.input(Msg::Group(op, project.clone())));
        actions.append(&b);
    }
    actions
}

pub fn group_list(group: &Group, selected: Option<&str>, sender: &Sender) -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["boxed-list", "dp-list"]).build();
    let ids: Vec<String> = group.containers.iter().map(|c| c.id.clone().unwrap_or_default()).collect();
    for (c, id) in group.containers.iter().zip(&ids) {
        let row = container_row(c, id, sender);
        list.append(&row);
        if selected == Some(id.as_str()) {
            list.select_row(Some(&row));
        }
    }
    let s = sender.clone();
    list.connect_row_selected(move |list, row| {
        let Some(row) = row else { return };
        let mut sibling = list.parent().and_then(|p| p.first_child());
        while let Some(w) = sibling {
            if let Some(other) = w.downcast_ref::<gtk::ListBox>()
                && other != list
            {
                other.unselect_all();
            }
            sibling = w.next_sibling();
        }
        s.input(Msg::Select(ids[row.index() as usize].clone()));
    });
    list
}

fn container_row(c: &ContainerSummary, id: &str, sender: &Sender) -> adw::ActionRow {
    let image = c.image.as_deref().unwrap_or_default();
    let ports = core::format_ports(c.ports.as_deref().unwrap_or_default());
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&core::display_name(c)).as_str())
        .subtitle(glib::markup_escape_text(image).as_str())
        .title_lines(1)
        .subtitle_lines(1)
        .build();
    row.add_prefix(&dot(c.state == Some(State::RUNNING)));
    for port in ports.split(", ").filter(|p| !p.is_empty()) {
        row.add_suffix(&gtk::Label::builder().label(port).css_classes(["dp-chip"]).valign(gtk::Align::Center).build());
    }

    let style = match c.state {
        Some(State::RUNNING) => "success",
        Some(State::PAUSED | State::RESTARTING) => "warning",
        _ => "dim-label",
    };
    row.add_suffix(
        &gtk::Label::builder()
            .label(c.status.as_deref().unwrap_or_default())
            .css_classes([style])
            .valign(gtk::Align::Center)
            .build(),
    );

    let button = |icon: &str, tooltip: &str, msg: fn(String) -> Msg| {
        let b = gtk::Button::builder().icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).css_classes(["flat", "dp-iconbtn"]).build();
        let (s, id) = (sender.clone(), id.to_string());
        b.connect_clicked(move |_| s.input(msg(id.clone())));
        row.add_suffix(&b);
        b
    };
    if core::is_up(c) {
        button("media-playback-stop-symbolic", "Stop", Msg::Stop);
    } else {
        button("media-playback-start-symbolic", "Start", Msg::Start);
    }
    button("view-refresh-symbolic", "Restart", Msg::Restart);
    button("utilities-terminal-symbolic", "Shell", Msg::Shell).set_sensitive(c.state == Some(State::RUNNING));
    button("user-trash-symbolic", "Remove", Msg::Remove).add_css_class("dp-danger");
    row
}
