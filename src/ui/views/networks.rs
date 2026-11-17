mod create;

use bollard::models::{
    ContainerSummary, Network, NetworkConnectRequest, NetworkCreateRequest, NetworkDisconnectRequest,
};
use bollard::query_parameters::{ListContainersOptionsBuilder, ListNetworksOptions};
use futures_util::future::try_join;
use relm4::adw::{self, prelude::*};
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::context::{AppCtx, Kind};
use crate::core;
use crate::ui::confirm::{Confirm, confirm};
use crate::ui;
use crate::core::networks::{self as model, Member, NetRow};

pub struct NetworksView {
    ctx: AppCtx,
    nets: Vec<NetRow>,
    containers: Vec<ContainerSummary>,
    query: String,
    selected: Option<String>,
    attached: Vec<Member>,
    candidates: Vec<Member>,
    list: gtk::ListBox,
    on_select: glib::SignalHandlerId,
    members: gtk::ListBox,
    picker: gtk::DropDown,
}

#[derive(Debug)]
pub enum Msg {
    Filter(String),
    Select(Option<String>),
    Create,
    DoCreate(String, String),
    Remove,
    DoRemove(String),
    Connect,
    Disconnect(String),
}

#[derive(Clone, Debug)]
pub enum Cmd {
    Refresh,
    Loaded(Vec<Network>, Vec<ContainerSummary>),
    Done(String),
}

fn done<T>(msg: String) -> impl FnOnce(T) -> Cmd {
    move |_| Cmd::Done(msg)
}

impl NetworksView {
    fn current(&self) -> Option<&NetRow> {
        let name = self.selected.as_ref()?;
        self.nets.iter().find(|n| &n.name == name)
    }

    fn summary(&self) -> String {
        self.current()
            .map(|n| format!("{} · {} · {}", n.driver, n.scope, core::short_id(&n.id)))
            .unwrap_or_default()
    }

    fn render_list(&self) {
        self.list.block_signal(&self.on_select);
        ui::clear(&self.list);
        for net in self.nets.iter().filter(|n| n.matches(&self.query)) {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&net.name).as_str())
                .subtitle(glib::markup_escape_text(&format!("{} · {}", net.driver, net.scope)).as_str())
                .build();
            row.set_widget_name(&net.name);
            self.list.append(&row);
            if self.selected.as_ref() == Some(&net.name) {
                self.list.select_row(Some(&row));
            }
        }
        self.list.unblock_signal(&self.on_select);
    }

    fn render_detail(&mut self, sender: &ComponentSender<Self>, force: bool) {
        let net = self.current().map(|n| n.name.clone()).unwrap_or_default();
        let attached = model::attached(&self.containers, &net);
        if force || attached != self.attached {
            ui::clear(&self.members);
            for m in &attached {
                let row = adw::ActionRow::builder()
                    .title(glib::markup_escape_text(&m.name).as_str())
                    .subtitle(glib::markup_escape_text(&m.ip).as_str())
                    .build();
                let button = gtk::Button::builder().label("Disconnect").valign(gtk::Align::Center).build();
                let (sender, id) = (sender.clone(), m.id.clone());
                button.connect_clicked(move |_| sender.input(Msg::Disconnect(id.clone())));
                row.add_suffix(&button);
                self.members.append(&row);
            }
            self.attached = attached;
        }
        let candidates = model::connectable(&self.containers, &net);
        if force || candidates != self.candidates {
            let names: Vec<&str> = candidates.iter().map(|m| m.name.as_str()).collect();
            self.picker.set_model(Some(&gtk::StringList::new(&names)));
            self.candidates = candidates;
        }
    }
}

#[relm4::component(pub)]
impl Component for NetworksView {
    type Init = AppCtx;
    type Input = Msg;
    type Output = ();
    type CommandOutput = Cmd;

    view! {
        #[root]
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
                        set_placeholder_text: Some("Filter networks"),
                        connect_search_changed[sender] => move |e| sender.input(Msg::Filter(e.text().to_string())),
                    },
                    gtk::Button {
                        set_icon_name: "list-add-symbolic",
                        set_tooltip_text: Some("Create network"),
                        connect_clicked => Msg::Create,
                    },
                },
                gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,
                    #[local_ref]
                    list -> gtk::ListBox {
                        add_css_class: "navigation-sidebar",
                    },
                },
            },
            #[wrap(Some)]
            set_end_child = &gtk::Stack {
                add_named: (&detail_page, Some("detail")),
                #[watch]
                set_visible: model.current().is_some(),
            },
        },

        #[name = "detail_page"]
        gtk::ScrolledWindow {
            set_hscrollbar_policy: gtk::PolicyType::Never,
            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                set_spacing: 12,
                set_margin_all: 18,
                gtk::Box {
                    set_spacing: 6,
                    gtk::Label {
                        add_css_class: "title-2",
                        set_hexpand: true,
                        set_xalign: 0.0,
                        set_selectable: true,
                        #[watch]
                        set_label: model.selected.as_deref().unwrap_or_default(),
                    },
                    gtk::Button {
                        set_icon_name: "window-close-symbolic",
                        set_tooltip_text: Some("Close details"),
                        set_valign: gtk::Align::Start,
                        add_css_class: "flat",
                        connect_clicked[list] => move |_| list.unselect_all(),
                    },
                },
                gtk::Label {
                    add_css_class: "dim-label",
                    set_xalign: 0.0,
                    set_selectable: true,
                    #[watch]
                    set_label: &model.summary(),
                },
                gtk::Label {
                    add_css_class: "heading",
                    set_xalign: 0.0,
                    set_label: "Attached containers",
                },
                #[local_ref]
                members -> gtk::ListBox {
                    add_css_class: "boxed-list",
                    set_selection_mode: gtk::SelectionMode::None,
                    set_placeholder: Some(&gtk::Label::builder().label("No containers").margin_top(12).margin_bottom(12).build()),
                },
                gtk::Box {
                    set_spacing: 6,
                    #[local_ref]
                    picker -> gtk::DropDown {
                        set_hexpand: true,
                    },
                    gtk::Button {
                        set_label: "Connect",
                        connect_clicked => Msg::Connect,
                    },
                },
                gtk::Button {
                    set_label: "Remove network",
                    set_halign: gtk::Align::Start,
                    add_css_class: "destructive-action",
                    connect_clicked => Msg::Remove,
                },
            },
        },
    }

    fn init(ctx: AppCtx, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        ctx.on_change(&sender, &[Kind::NETWORK, Kind::CONTAINER], Cmd::Refresh);
        let list = gtk::ListBox::new();
        let on_select = list.connect_row_selected({
            let sender = sender.clone();
            move |_, row| sender.input(Msg::Select(row.map(|r| r.widget_name().to_string())))
        });
        let model = Self {
            ctx,
            nets: Vec::new(),
            containers: Vec::new(),
            query: String::new(),
            selected: None,
            attached: Vec::new(),
            candidates: Vec::new(),
            list,
            on_select,
            members: gtk::ListBox::new(),
            picker: gtk::DropDown::from_strings(&[]),
        };
        let list = &model.list;
        let members = &model.members;
        let picker = &model.picker;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::Filter(query) => {
                self.query = query;
                self.render_list();
            }
            Msg::Select(name) => {
                self.selected = name;
                self.render_detail(&sender, true);
            }
            Msg::Create => create::ask(move |name, driver| sender.input(Msg::DoCreate(name, driver))),
            Msg::DoCreate(name, driver) => {
                let req = NetworkCreateRequest { name: name.clone(), driver: Some(driver), ..Default::default() };
                self.ctx.run(
                    &sender,
                    "Create network",
                    |d| async move { d.create_network(req).await },
                    done(format!("Network {name} created")),
                );
            }
            Msg::Remove => {
                let Some(net) = self.current().map(|n| n.name.clone()) else { return };
                let mut items = vec![format!("Network: {net}")];
                items.extend(model::attached(&self.containers, &net).into_iter().map(|m| format!("Attached: {}", m.name)));
                confirm(
                    Confirm {
                        heading: "Remove network?".into(),
                        items,
                        action: "Remove".into(),
                        option: None,
                        destructive: true,
                    },
                    move |_| sender.input(Msg::DoRemove(net)),
                );
            }
            Msg::DoRemove(net) => {
                let toast = format!("Network {net} removed");
                self.ctx.run(&sender, "Remove network", |d| async move { d.remove_network(&net).await }, done(toast));
            }
            Msg::Connect => {
                let (Some(net), Some(m)) =
                    (self.selected.clone(), self.candidates.get(self.picker.selected() as usize).cloned())
                else {
                    return;
                };
                let toast = format!("{} connected to {net}", m.name);
                let req = NetworkConnectRequest { container: m.id, endpoint_config: None };
                self.ctx.run(
                    &sender,
                    "Connect container",
                    |d| async move { d.connect_network(&net, req).await },
                    done(toast),
                );
            }
            Msg::Disconnect(id) => {
                let Some(net) = self.selected.clone() else { return };
                let toast = format!("Container disconnected from {net}");
                let req = NetworkDisconnectRequest { container: id, force: Some(false) };
                self.ctx.run(
                    &sender,
                    "Disconnect container",
                    |d| async move { d.disconnect_network(&net, req).await },
                    done(toast),
                );
            }
        }
    }

    fn update_cmd(&mut self, cmd: Cmd, sender: ComponentSender<Self>, _root: &Self::Root) {
        match cmd {
            Cmd::Refresh => self.ctx.run(
                &sender,
                "List networks",
                |d| async move {
                    try_join(
                        d.list_networks(None::<ListNetworksOptions>),
                        d.list_containers(Some(ListContainersOptionsBuilder::new().all(true).build())),
                    )
                    .await
                },
                |(nets, containers)| Cmd::Loaded(nets, containers),
            ),
            Cmd::Loaded(nets, containers) => {
                let rows = model::rows(nets);
                self.containers = containers;
                if rows != self.nets {
                    self.nets = rows;
                    if self.current().is_none() {
                        self.selected = None;
                    }
                    self.render_list();
                }
                self.render_detail(&sender, false);
            }
            Cmd::Done(msg) => {
                self.ctx.toast(msg);
                sender.oneshot_command(async { Cmd::Refresh });
            }
        }
    }
}
