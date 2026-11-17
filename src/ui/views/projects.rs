use std::path::PathBuf;
use std::process::Stdio;

use bollard::models::ContainerSummary;
use bollard::query_parameters::ListContainersOptions;
use relm4::adw::{self, prelude::*};
use relm4::gtk::glib;
use relm4::prelude::*;

use crate::context::{AppCtx, Kind};
use crate::core::{self, ComposeCmd, Project, ProjectRecord};
use crate::store;
use crate::ui::confirm::{Confirm, confirm};
use crate::ui::{self, Opener, detail};

pub struct ProjectsView {
    ctx: AppCtx,
    store_path: Option<PathBuf>,
    remembered: Vec<ProjectRecord>,
    containers: Vec<ContainerSummary>,
    projects: Vec<Project>,
    filter: String,
    selected: Option<String>,
    list: gtk::ListBox,
    on_select: glib::SignalHandlerId,
    pane: detail::Pane,
}

#[derive(Clone, Debug)]
pub enum Msg {
    Filter(String),
    Select(Option<String>),
    Compose(ComposeCmd, String),
    RunCompose(ComposeCmd, ProjectRecord),
    Forget(String),
    Open(Opener, PathBuf),
}

#[derive(Clone, Debug)]
pub enum Cmd {
    Refresh,
    Loaded(Vec<ContainerSummary>),
    Composed(Result<String, String>),
}

impl ProjectsView {
    fn save(&self) {
        if let Some(path) = &self.store_path
            && let Err(e) = store::save(path, &self.remembered)
        {
            self.ctx.error(format!("Save {}: {e}", path.display()));
        }
    }

    fn reload(&mut self) {
        self.projects = core::merge_projects(core::live_projects(&self.containers), &self.remembered);
        let records: Vec<ProjectRecord> = self.projects.iter().map(|p| p.record.clone()).collect();
        if records != self.remembered {
            self.remembered = records;
            self.save();
        }
        if !self.projects.iter().any(|p| Some(&p.record.name) == self.selected.as_ref()) {
            self.selected = None;
        }
    }

    fn render(&self, sender: &ComponentSender<Self>) {
        self.list.block_signal(&self.on_select);
        ui::clear(&self.list);
        for p in self.projects.iter().filter(|p| core::matches(&self.filter, &[&p.record.name])) {
            let row = row(p, core::up_count(&self.containers, &p.record.name), sender);
            self.list.append(&row);
            if self.selected.as_ref() == Some(&p.record.name) {
                self.list.select_row(Some(&row));
            }
        }
        self.list.unblock_signal(&self.on_select);
        self.show_detail();
    }

    fn show_detail(&self) {
        match self.projects.iter().find(|p| Some(&p.record.name) == self.selected.as_ref()) {
            Some(p) => self.pane.show(&p.record.name, &core::project_sections(p, &self.containers)),
            None => self.pane.clear(),
        }
    }
}

fn row(p: &Project, counts: Option<(usize, usize)>, sender: &ComponentSender<ProjectsView>) -> adw::ActionRow {
    let r = &p.record;
    let paths = if r.config_files.is_empty() {
        r.working_dir.display().to_string()
    } else {
        r.config_files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join("\n")
    };
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&r.name).as_str())
        .subtitle(glib::markup_escape_text(&paths).as_str())
        .subtitle_lines(0)
        .build();
    row.set_widget_name(&r.name);
    let style = core::state_class(counts);
    row.add_suffix(
        &gtk::Label::builder()
            .label(core::project_state(counts))
            .css_classes([style])
            .valign(gtk::Align::Center)
            .build(),
    );

    let has_files = !r.config_files.is_empty();
    let button = |icon: &str, tooltip: &str, sensitive: bool, msg: Msg| {
        let b = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .sensitive(sensitive)
            .build();
        let s = sender.clone();
        b.connect_clicked(move |_| s.input(msg.clone()));
        row.add_suffix(&b);
    };
    let name = r.name.clone();
    button("media-playback-start-symbolic", "docker compose up -d", has_files, Msg::Compose(ComposeCmd::Up, name.clone()));
    button("media-playback-stop-symbolic", "docker compose down", p.live, Msg::Compose(ComposeCmd::Down, name.clone()));
    button("folder-download-symbolic", "docker compose pull", has_files, Msg::Compose(ComposeCmd::Pull, name.clone()));
    for opener in [Opener::Files, Opener::Code] {
        let (s, d) = (sender.clone(), r.working_dir.clone());
        row.add_suffix(&opener.button(&r.working_dir, move |o| s.input(Msg::Open(o, d.clone()))));
    }
    button("edit-delete-symbolic", "Forget (remove from remembered projects)", !p.live, Msg::Forget(name));
    row
}

#[relm4::component(pub)]
impl Component for ProjectsView {
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
                gtk::SearchEntry {
                    set_placeholder_text: Some("Filter projects"),
                    set_margin_all: 6,
                    connect_search_changed[sender] => move |e| sender.input(Msg::Filter(e.text().into())),
                },
                gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,
                    #[local_ref]
                    list -> gtk::ListBox {
                        set_margin_all: 12,
                        set_valign: gtk::Align::Start,
                        add_css_class: "boxed-list",
                    },
                },
            },
            set_end_child: Some(model.pane.widget()),
        }
    }

    fn init(ctx: AppCtx, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        ctx.on_change(&sender, &[Kind::CONTAINER], Cmd::Refresh);
        let path = store::default_path();
        let (store_path, remembered) = match store::load(&path) {
            Ok(records) => (Some(path), records),
            Err(e) => {
                ctx.error(format!("Load {}: {e} (remembered projects will not be saved)", path.display()));
                (None, Vec::new())
            }
        };
        let list = gtk::ListBox::new();
        let pane = detail::Pane::new({
            let list = list.clone();
            move || list.unselect_all()
        });
        let on_select = list.connect_row_selected({
            let sender = sender.clone();
            move |_, row| sender.input(Msg::Select(row.map(|r| r.widget_name().into())))
        });
        let mut model = Self {
            ctx,
            store_path,
            remembered,
            containers: Vec::new(),
            projects: Vec::new(),
            filter: String::new(),
            selected: None,
            list,
            on_select,
            pane,
        };
        model.reload();
        model.render(&sender);
        let list = &model.list;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::Filter(f) => {
                self.filter = f;
                self.render(&sender);
            }
            Msg::Select(name) => {
                self.selected = name;
                self.show_detail();
            }
            Msg::Compose(cmd, name) => {
                let Some(record) = self.projects.iter().find(|p| p.record.name == name).map(|p| p.record.clone()) else {
                    return;
                };
                let mut items = vec![format!("Project: {}", record.name)];
                items.extend(record.config_files.iter().map(|f| format!("File: {}", f.display())));
                if cmd == ComposeCmd::Down {
                    items.extend(
                        core::project_members(&self.containers, &name)
                            .map(|c| format!("Container: {}", core::display_name(c))),
                    );
                }
                let req = Confirm {
                    heading: format!("Run docker compose {} on {}?", cmd.name(), record.name),
                    items,
                    action: format!("Compose {}", cmd.name()),
                    option: None,
                    destructive: cmd == ComposeCmd::Down,
                };
                confirm(req, move |_| sender.input(Msg::RunCompose(cmd, record)));
            }
            Msg::RunCompose(cmd, record) => {
                let args = core::compose_args(self.ctx.host(), &record, cmd);
                let what = format!("Compose {} {}", cmd.name(), record.name);
                self.ctx.toast(format!("{what}: running"));
                sender.oneshot_command(async move {
                    let out = tokio::process::Command::new("docker").args(&args).stdin(Stdio::null()).output().await;
                    Cmd::Composed(match out {
                        Ok(o) if o.status.success() => Ok(format!("{what}: done")),
                        Ok(o) => {
                            let stderr = String::from_utf8_lossy(&o.stderr);
                            let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or_default();
                            Err(format!("{what}: {} {last}", o.status))
                        }
                        Err(e) => Err(format!("{what}: {e}")),
                    })
                });
            }
            Msg::Open(opener, dir) => opener.open(&self.ctx, &dir),
            Msg::Forget(name) => {
                core::forget(&mut self.remembered, &name);
                self.save();
                self.reload();
                self.render(&sender);
            }
        }
    }

    fn update_cmd(&mut self, cmd: Cmd, sender: ComponentSender<Self>, _root: &Self::Root) {
        match cmd {
            Cmd::Refresh => self.ctx.run(
                &sender,
                "List containers",
                |d| async move { d.list_containers(Some(ListContainersOptions { all: true, ..Default::default() })).await },
                Cmd::Loaded,
            ),
            Cmd::Loaded(containers) if containers == self.containers => {}
            Cmd::Loaded(containers) => {
                self.containers = containers;
                self.reload();
                self.render(&sender);
            }
            Cmd::Composed(Ok(msg)) => self.ctx.toast(msg),
            Cmd::Composed(Err(msg)) => self.ctx.error(msg),
        }
    }
}
