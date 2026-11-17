use std::collections::HashMap;

use bollard::models::{ContainerSummary, ImagePruneResponse, ImageSummary};
use bollard::query_parameters::{
    CreateImageOptions, ListContainersOptions, ListImagesOptions, PruneImagesOptionsBuilder, RemoveImageOptions,
};
use futures_util::StreamExt;
use futures_util::future::try_join;
use relm4::gtk::{glib, prelude::*};
use relm4::prelude::*;

use crate::context::{AppCtx, Kind};
use crate::core::{self, images::{self as model, ImageRow, PullProgress}};
use crate::ui::confirm::{Confirm, confirm};
use crate::ui::{self, detail};

pub struct ImagesView {
    ctx: AppCtx,
    images: Vec<ImageSummary>,
    containers: Vec<ContainerSummary>,
    rows: Vec<ImageRow>,
    shown: Vec<ImageRow>,
    filter: String,
    selected: Option<String>,
    pulling: bool,
    progress: String,
    list: gtk::ListBox,
    on_select: glib::SignalHandlerId,
    pane: detail::Pane,
    pull_entry: gtk::Entry,
}

#[derive(Debug)]
pub enum Msg {
    Filter(String),
    Select(Option<usize>),
    Remove,
    DoRemove { target: String, force: bool },
    Prune,
    DoPrune,
    Pull,
}

#[derive(Clone, Debug)]
pub enum Cmd {
    Refresh,
    Loaded(Vec<ImageSummary>, Vec<ContainerSummary>),
    PruneList(Vec<String>),
    Pruned(ImagePruneResponse),
    Removed,
    Progress(String),
    PullDone(bool),
}

#[relm4::component(pub)]
impl Component for ImagesView {
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
                set_spacing: 6,
                set_margin_all: 6,

                gtk::Box {
                    set_spacing: 6,
                    gtk::SearchEntry {
                        set_hexpand: true,
                        set_placeholder_text: Some("Filter images"),
                        connect_search_changed[sender] => move |e| sender.input(Msg::Filter(e.text().into())),
                    },
                    gtk::Button {
                        set_label: "Remove",
                        #[watch]
                        set_sensitive: model.selected.is_some(),
                        connect_clicked => Msg::Remove,
                    },
                    gtk::Button {
                        set_label: "Prune",
                        set_tooltip_text: Some("Remove images not used by any container"),
                        connect_clicked => Msg::Prune,
                    },
                },
                gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,
                    #[local_ref]
                    list -> gtk::ListBox {},
                },
                gtk::Label {
                    set_xalign: 0.0,
                    set_ellipsize: gtk::pango::EllipsizeMode::End,
                    add_css_class: "dim-label",
                    #[watch]
                    set_visible: !model.progress.is_empty(),
                    #[watch]
                    set_label: &model.progress,
                },
                gtk::Box {
                    set_spacing: 6,
                    #[local_ref]
                    pull_entry -> gtk::Entry {
                        set_hexpand: true,
                        set_placeholder_text: Some("Image to pull, e.g. nginx:latest"),
                        #[watch]
                        set_sensitive: !model.pulling,
                        connect_activate => Msg::Pull,
                    },
                    gtk::Button {
                        set_label: "Pull",
                        #[watch]
                        set_sensitive: !model.pulling,
                        connect_clicked => Msg::Pull,
                    },
                },
            },
            set_end_child: Some(model.pane.widget()),
        }
    }

    fn init(ctx: AppCtx, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        ctx.on_change(&sender, &[Kind::IMAGE, Kind::CONTAINER], Cmd::Refresh);
        let list = gtk::ListBox::new();
        let pane = detail::Pane::new({
            let list = list.clone();
            move || list.unselect_all()
        });
        let on_select = list.connect_row_selected({
            let sender = sender.clone();
            move |_, row| sender.input(Msg::Select(row.map(|r| r.index() as usize)))
        });
        let model = Self {
            ctx,
            images: Vec::new(),
            containers: Vec::new(),
            rows: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            selected: None,
            pulling: false,
            progress: String::new(),
            list,
            on_select,
            pane,
            pull_entry: gtk::Entry::new(),
        };
        let list = &model.list;
        let pull_entry = &model.pull_entry;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::Filter(text) => {
                self.filter = text;
                self.render();
            }
            Msg::Select(i) => {
                self.selected = i.and_then(|i| self.shown.get(i)).map(|r| r.target.clone());
                self.show_detail();
            }
            Msg::Remove => {
                let Some(row) = self.rows.iter().find(|r| Some(&r.target) == self.selected.as_ref()) else {
                    return;
                };
                confirm(
                    Confirm {
                        heading: "Remove image?".into(),
                        items: vec![format!("{}:{} ({}, {})", row.repo, row.tag, row.id, row.size)],
                        action: "Remove".into(),
                        option: Some("Force".into()),
                        destructive: true,
                    },
                    {
                        let target = row.target.clone();
                        move |force| sender.input(Msg::DoRemove { target, force })
                    },
                );
            }
            Msg::DoRemove { target, force } => self.ctx.run(
                &sender,
                "Remove image",
                |d| async move {
                    let opts = RemoveImageOptions { force, ..Default::default() };
                    d.remove_image(&target, Some(opts), None).await
                },
                |_| Cmd::Removed,
            ),
            Msg::Prune => self.ctx.run(
                &sender,
                "Prune images",
                |d| async move {
                    let images = d.list_images(None::<ListImagesOptions>).await?;
                    let opts = ListContainersOptions { all: true, ..Default::default() };
                    let used: Vec<_> =
                        d.list_containers(Some(opts)).await?.into_iter().filter_map(|c| c.image_id).collect();
                    Ok::<_, bollard::errors::Error>(model::unused(&images, &used))
                },
                Cmd::PruneList,
            ),
            Msg::DoPrune => self.ctx.run(
                &sender,
                "Prune images",
                |d| async move {
                    let filters = HashMap::from([("dangling", vec!["false"])]);
                    d.prune_images(Some(PruneImagesOptionsBuilder::new().filters(&filters).build())).await
                },
                Cmd::Pruned,
            ),
            Msg::Pull => self.pull(&sender),
        }
    }

    fn update_cmd(&mut self, cmd: Cmd, sender: ComponentSender<Self>, _root: &Self::Root) {
        match cmd {
            Cmd::Refresh => self.ctx.run(
                &sender,
                "List images",
                |d| async move {
                    let containers = ListContainersOptions { all: true, ..Default::default() };
                    try_join(d.list_images(None::<ListImagesOptions>), d.list_containers(Some(containers))).await
                },
                |(images, containers)| Cmd::Loaded(images, containers),
            ),
            Cmd::Loaded(images, containers) if images == self.images && containers == self.containers => {}
            Cmd::Loaded(images, containers) => {
                self.containers = containers;
                if images != self.images {
                    self.rows = model::rows(&images);
                    self.images = images;
                    if !self.rows.iter().any(|r| Some(&r.target) == self.selected.as_ref()) {
                        self.selected = None;
                    }
                    self.render();
                }
                self.show_detail();
            }
            Cmd::PruneList(items) if items.is_empty() => self.ctx.toast("No unused images to prune"),
            Cmd::PruneList(items) => confirm(
                Confirm {
                    heading: format!("Prune {} unused images?", items.len()),
                    items,
                    action: "Prune".into(),
                    option: None,
                    destructive: true,
                },
                move |_| sender.input(Msg::DoPrune),
            ),
            Cmd::Pruned(r) => self.ctx.toast(format!(
                "Pruned {} images, reclaimed {}",
                r.images_deleted.map_or(0, |d| d.len()),
                core::format_bytes(u64::try_from(r.space_reclaimed.unwrap_or(0)).unwrap_or(0)),
            )),
            Cmd::Removed => self.ctx.toast("Image removed"),
            Cmd::Progress(line) => self.progress = line,
            Cmd::PullDone(ok) => {
                self.pulling = false;
                if ok {
                    self.pull_entry.set_text("");
                } else {
                    self.progress.clear();
                }
            }
        }
    }
}

impl ImagesView {
    fn render(&mut self) {
        self.shown = self.rows.iter().filter(|r| r.matches(&self.filter)).cloned().collect();
        self.list.block_signal(&self.on_select);
        ui::clear(&self.list);
        for row in &self.shown {
            self.list.append(&row_widget(row));
        }
        if let Some(i) = self.shown.iter().position(|r| Some(&r.target) == self.selected.as_ref()) {
            self.list.select_row(self.list.row_at_index(i as i32).as_ref());
        }
        self.list.unblock_signal(&self.on_select);
    }

    fn show_detail(&self) {
        let image = self.selected.as_ref().and_then(|t| self.images.iter().find(|i| &i.id == t || i.repo_tags.contains(t)));
        match (image, &self.selected) {
            (Some(image), Some(target)) => {
                let title = if target.starts_with("sha256:") { core::short_id(target) } else { target.clone() };
                self.pane.show(&title, &model::image_sections(image, &self.containers));
            }
            _ => self.pane.clear(),
        }
    }

    fn pull(&mut self, sender: &ComponentSender<Self>) {
        let Some((name, tag)) = model::pull_args(&self.pull_entry.text()) else { return };
        let Some(docker) = self.ctx.docker().cloned() else {
            self.ctx.error("Pull image: Docker is not connected");
            return;
        };
        let sep = if tag.starts_with("sha256:") { '@' } else { ':' };
        let reference = format!("{name}{sep}{tag}");
        let mut progress = PullProgress::new(reference.clone());
        self.pulling = true;
        self.progress = progress.line();
        let ctx = self.ctx.clone();
        sender.command(move |out, shutdown| {
            shutdown
                .register(async move {
                    let opts = CreateImageOptions { from_image: Some(name), tag: Some(tag), ..Default::default() };
                    let mut stream = docker.create_image(Some(opts), None, None);
                    let mut last = progress.line();
                    let result = loop {
                        let update = match stream.next().await {
                            None => break Ok(()),
                            Some(Err(e)) => Err(e.to_string()),
                            Some(Ok(info)) => progress.update(&info),
                        };
                        if let Err(e) = update {
                            break Err(e);
                        }
                        let line = progress.line();
                        if line != last {
                            out.emit(Cmd::Progress(line.clone()));
                            last = line;
                        }
                    };
                    if let Err(e) = &result {
                        ctx.error(format!("Pull {reference}: {e}"));
                    }
                    out.emit(Cmd::PullDone(result.is_ok()));
                })
                .drop_on_shutdown()
        });
    }
}

fn row_widget(row: &ImageRow) -> gtk::Box {
    let title = gtk::Label::builder()
        .label(format!("{}:{}", row.repo, row.tag))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .build();
    let subtitle = gtk::Label::builder()
        .label(format!("{} · {}", row.id, row.created))
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).hexpand(true).build();
    text.append(&title);
    text.append(&subtitle);
    let size = gtk::Label::builder().label(&row.size).xalign(1.0).width_chars(10).build();
    let b = gtk::Box::builder().spacing(12).margin_top(6).margin_bottom(6).margin_start(12).margin_end(12).build();
    b.append(&text);
    b.append(&size);
    b
}
