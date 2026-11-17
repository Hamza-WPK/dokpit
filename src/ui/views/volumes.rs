use std::collections::HashMap;
use std::convert::Infallible;

use bollard::models::{ContainerSummary, Volume};
use bollard::query_parameters::{
    ListContainersOptions, ListVolumesOptions, ListVolumesOptionsBuilder, RemoveVolumeOptions,
};
use futures_util::future::try_join;
use relm4::adw::{self, prelude::*};
use relm4::gtk::glib::{self, markup_escape_text};
use relm4::prelude::*;

use crate::context::{AppCtx, Kind};
use crate::core;
use crate::ui::confirm::{Confirm, confirm};
use crate::ui::{self, detail};

pub struct VolumesView {
    ctx: AppCtx,
    volumes: Vec<Volume>,
    containers: Vec<ContainerSummary>,
    query: String,
    selected: Option<String>,
    list: gtk::ListBox,
    on_select: glib::SignalHandlerId,
    pane: detail::Pane,
}

#[derive(Debug)]
pub enum Msg {
    Filter(String),
    Select(Option<String>),
    Remove,
    DoRemove(String),
    Prune,
    DoPrune(Vec<String>),
}

#[derive(Clone, Debug)]
pub enum Cmd {
    Refresh,
    Loaded(Vec<Volume>, Vec<ContainerSummary>),
    PruneListed(Vec<String>),
    Removed,
    Pruned(usize, Vec<String>),
}

#[relm4::component(pub)]
impl Component for VolumesView {
    type Init = AppCtx;
    type Input = Msg;
    type Output = ();
    type CommandOutput = Cmd;

    view! {
        gtk::Paned {
            set_position: 560,
            set_shrink_start_child: false,
            set_shrink_end_child: false,
            #[wrap(Some)]
            set_start_child = &gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                gtk::Box {
                    set_spacing: 6,
                    set_margin_all: 6,
                    gtk::SearchEntry {
                        set_hexpand: true,
                        set_placeholder_text: Some("Filter volumes"),
                        connect_search_changed[sender] => move |e| sender.input(Msg::Filter(e.text().into())),
                    },
                    gtk::Button {
                        set_icon_name: "user-trash-symbolic",
                        set_tooltip_text: Some("Remove volume"),
                        #[watch]
                        set_sensitive: model.selected.is_some(),
                        connect_clicked => Msg::Remove,
                    },
                    gtk::Button {
                        set_label: "Prune",
                        set_tooltip_text: Some("Remove unused anonymous volumes"),
                        connect_clicked => Msg::Prune,
                    },
                },
                gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,
                    #[local_ref]
                    list -> gtk::ListBox {},
                },
            },
            set_end_child: Some(model.pane.widget()),
        }
    }

    fn init(ctx: AppCtx, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        ctx.on_change(&sender, &[Kind::VOLUME, Kind::CONTAINER], Cmd::Refresh);
        let list = gtk::ListBox::new();
        let pane = detail::Pane::new({
            let list = list.clone();
            move || list.unselect_all()
        });
        let on_select = list.connect_row_selected({
            let sender = sender.clone();
            move |_, row| sender.input(Msg::Select(row.map(|r| r.widget_name().into())))
        });
        let model = Self {
            ctx,
            volumes: Vec::new(),
            containers: Vec::new(),
            query: String::new(),
            selected: None,
            list,
            on_select,
            pane,
        };
        let list = &model.list;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::Filter(query) => {
                self.query = query;
                self.render();
            }
            Msg::Select(name) => {
                self.selected = name;
                self.show_detail();
            }
            Msg::Remove => {
                let Some(name) = self.selected.clone() else { return };
                let req = Confirm {
                    heading: "Remove volume?".into(),
                    items: vec![name.clone()],
                    action: "Remove".into(),
                    option: None,
                    destructive: true,
                };
                confirm(req, move |_| sender.input(Msg::DoRemove(name)));
            }
            Msg::DoRemove(name) => self.ctx.run(
                &sender,
                "Remove volume",
                |d| async move { d.remove_volume(&name, None::<RemoveVolumeOptions>).await },
                |()| Cmd::Removed,
            ),
            Msg::Prune => self.ctx.run(
                &sender,
                "List unused volumes",
                |d| async move {
                    let filters = HashMap::from([("dangling", vec!["true"])]);
                    d.list_volumes(Some(ListVolumesOptionsBuilder::new().filters(&filters).build())).await
                },
                |r| Cmd::PruneListed(core::prune_candidates(r.volumes.unwrap_or_default())),
            ),
            Msg::DoPrune(names) => self.ctx.run(
                &sender,
                "Prune volumes",
                |d| async move {
                    let mut removed = 0;
                    let mut failed = Vec::new();
                    for name in &names {
                        match d.remove_volume(name, None::<RemoveVolumeOptions>).await {
                            Ok(()) => removed += 1,
                            Err(e) => failed.push(format!("{name}: {e}")),
                        }
                    }
                    Ok::<_, Infallible>((removed, failed))
                },
                |(removed, failed)| Cmd::Pruned(removed, failed),
            ),
        }
    }

    fn update_cmd(&mut self, cmd: Cmd, sender: ComponentSender<Self>, _root: &Self::Root) {
        match cmd {
            Cmd::Refresh => self.ctx.run(
                &sender,
                "List volumes",
                |d| async move {
                    let containers = ListContainersOptions { all: true, ..Default::default() };
                    try_join(d.list_volumes(None::<ListVolumesOptions>), d.list_containers(Some(containers))).await
                },
                |(r, containers)| {
                    let mut volumes = r.volumes.unwrap_or_default();
                    volumes.sort_by(|a, b| a.name.cmp(&b.name));
                    Cmd::Loaded(volumes, containers)
                },
            ),
            Cmd::Loaded(volumes, containers) if volumes == self.volumes && containers == self.containers => {}
            Cmd::Loaded(volumes, containers) => {
                self.containers = containers;
                if volumes != self.volumes {
                    self.volumes = volumes;
                    if !self.volumes.iter().any(|v| Some(&v.name) == self.selected.as_ref()) {
                        self.selected = None;
                    }
                    self.render();
                }
                self.show_detail();
            }
            Cmd::PruneListed(names) if names.is_empty() => self.ctx.toast("No unused volumes"),
            Cmd::PruneListed(names) => {
                let req = Confirm {
                    heading: format!("Remove {} unused volumes?", names.len()),
                    items: names.clone(),
                    action: "Prune".into(),
                    option: None,
                    destructive: true,
                };
                confirm(req, move |_| sender.input(Msg::DoPrune(names)));
            }
            Cmd::Removed => self.ctx.toast("Volume removed"),
            Cmd::Pruned(removed, failed) => {
                self.ctx.toast(format!("Removed {removed} volumes"));
                if !failed.is_empty() {
                    self.ctx.error(format!("Prune volumes: {}", failed.join("; ")));
                }
            }
        }
    }
}

impl VolumesView {
    fn render(&self) {
        self.list.block_signal(&self.on_select);
        ui::clear(&self.list);
        for v in self.volumes.iter().filter(|v| core::volume_matches(v, &self.query)) {
            let row = adw::ActionRow::builder()
                .title(markup_escape_text(&v.name).as_str())
                .subtitle(markup_escape_text(&format!("{} · {}", v.driver, v.mountpoint)).as_str())
                .build();
            row.set_widget_name(&v.name);
            self.list.append(&row);
            if self.selected.as_ref() == Some(&v.name) {
                self.list.select_row(Some(&row));
            }
        }
        self.list.unblock_signal(&self.on_select);
    }

    fn show_detail(&self) {
        match self.selected.as_ref().and_then(|name| self.volumes.iter().find(|v| &v.name == name)) {
            Some(v) => self.pane.show(&v.name, &core::volume_sections(v, &self.containers)),
            None => self.pane.clear(),
        }
    }
}
