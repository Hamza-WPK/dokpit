use std::time::Instant;

use bollard::models::{ContainerInspectResponse, ContainerStatsResponse};
use bollard::query_parameters::{InspectContainerOptions, LogsOptions, StatsOptions};
use futures_util::StreamExt;
use relm4::RelmRemoveAllExt;
use relm4::adw::{self, prelude::*};
use relm4::prelude::*;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::context::{AppCtx, DockerEvent, Kind};
use crate::core;
use crate::ui::detail;
use crate::core::details::{self as model, LogBuffer, Net};
use crate::core::{format_bytes, format_rate};

const LOG_CAP: usize = 5000;
const NO_VALUE: &str = "—";

struct Stats {
    cpu: String,
    memory: String,
    rx: String,
    tx: String,
}

impl Default for Stats {
    fn default() -> Self {
        Self { cpu: NO_VALUE.into(), memory: NO_VALUE.into(), rx: NO_VALUE.into(), tx: NO_VALUE.into() }
    }
}

struct Task(JoinHandle<()>);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct Details {
    ctx: AppCtx,
    id: Option<String>,
    generation: u64,
    name: String,
    running: bool,
    logs: LogBuffer,
    query: String,
    paused: bool,
    log_task: Option<Task>,
    stats_task: Option<Task>,
    prev_net: Option<(Net, Instant)>,
    stats: Stats,
    info: gtk::Box,
    text: gtk::TextBuffer,
    text_view: gtk::TextView,
    end: gtk::TextMark,
}

#[derive(Debug)]
pub enum Msg {
    Show(Option<String>),
    Search(String),
    Pause(bool),
}

#[derive(Debug)]
pub enum Out {
    Close,
}

#[derive(Debug)]
pub enum Cmd {
    Event { id: String, action: String },
    Resync,
    Inspected(u64, Box<ContainerInspectResponse>),
    Logs(u64, String),
    Stats(u64, Box<ContainerStatsResponse>, Instant),
}

#[relm4::component(pub)]
impl Component for Details {
    type Init = AppCtx;
    type Input = Msg;
    type Output = Out;
    type CommandOutput = Cmd;

    view! {
        gtk::Stack {
            add_named[Some("empty")] = &detail::placeholder() {},
            add_named[Some("details")] = &gtk::Box {
                set_orientation: gtk::Orientation::Vertical,
                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_spacing: 6,
                    set_margin_all: 6,
                    gtk::Box {
                        gtk::Label {
                            set_hexpand: true,
                            set_ellipsize: gtk::pango::EllipsizeMode::Middle,
                            add_css_class: "dp-detail-name",
                            #[watch]
                            set_label: &model.name,
                        },
                        gtk::Button {
                            set_icon_name: "window-close-symbolic",
                            set_tooltip_text: Some("Close details"),
                            set_valign: gtk::Align::Center,
                            add_css_class: "flat",
                            connect_clicked[sender] => move |_| {
                                let _ = sender.output(Out::Close);
                            },
                        },
                    },
                    adw::ViewSwitcher {
                        set_halign: gtk::Align::Center,
                        set_policy: adw::ViewSwitcherPolicy::Wide,
                        set_stack: Some(&tabs),
                    },
                },
                #[name = "tabs"]
                adw::ViewStack {
                    set_vexpand: true,
                    add_titled[Some("info"), "Info"] = &gtk::ScrolledWindow {
                        set_hscrollbar_policy: gtk::PolicyType::Never,
                        #[local_ref]
                        info -> gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 18,
                            set_margin_all: 12,
                            add_css_class: "dp-info",
                        },
                    } -> {
                        set_icon_name: Some("dialog-information-symbolic"),
                    },
                    add_titled[Some("logs"), "Logs"] = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        gtk::Box {
                            set_spacing: 6,
                            set_margin_all: 6,
                            gtk::SearchEntry {
                                set_hexpand: true,
                                set_placeholder_text: Some("Search logs"),
                                connect_search_changed[sender] => move |e| sender.input(Msg::Search(e.text().into())),
                            },
                            gtk::ToggleButton {
                                set_icon_name: "media-playback-pause-symbolic",
                                set_tooltip_text: Some("Pause following"),
                                #[watch]
                                #[block_signal(pause_handler)]
                                set_active: model.paused,
                                connect_toggled[sender] => move |b| sender.input(Msg::Pause(b.is_active())) @pause_handler,
                            },
                        },
                        gtk::ScrolledWindow {
                            set_vexpand: true,
                            #[local_ref]
                            text_view -> gtk::TextView {
                                set_editable: false,
                                set_cursor_visible: false,
                                set_monospace: true,
                                set_wrap_mode: gtk::WrapMode::WordChar,
                                set_left_margin: 6,
                                set_right_margin: 6,
                            },
                        },
                    } -> {
                        set_icon_name: Some("utilities-terminal-symbolic"),
                    },
                    add_titled[Some("stats"), "Stats"] = &gtk::ScrolledWindow {
                        set_hscrollbar_policy: gtk::PolicyType::Never,
                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 12,
                            set_margin_all: 12,
                            gtk::Label {
                                set_label: "Container is not running",
                                add_css_class: "dim-label",
                                #[watch]
                                set_visible: !model.running,
                            },
                            adw::PreferencesGroup {
                                #[watch]
                                set_sensitive: model.running,
                                adw::ActionRow {
                                    set_title: "CPU",
                                    add_suffix = &gtk::Label {
                                        #[watch]
                                        set_label: &model.stats.cpu,
                                    },
                                },
                                adw::ActionRow {
                                    set_title: "Memory",
                                    add_suffix = &gtk::Label {
                                        #[watch]
                                        set_label: &model.stats.memory,
                                    },
                                },
                                adw::ActionRow {
                                    set_title: "Network RX",
                                    add_suffix = &gtk::Label {
                                        #[watch]
                                        set_label: &model.stats.rx,
                                    },
                                },
                                adw::ActionRow {
                                    set_title: "Network TX",
                                    add_suffix = &gtk::Label {
                                        #[watch]
                                        set_label: &model.stats.tx,
                                    },
                                },
                            },
                        },
                    } -> {
                        set_icon_name: Some("utilities-system-monitor-symbolic"),
                    },
                },
            },
            #[watch]
            set_visible_child_name: if model.id.is_some() { "details" } else { "empty" },
        }
    }

    fn init(ctx: AppCtx, root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let mut events = ctx.subscribe();
        sender.command(move |out, shutdown| {
            shutdown
                .register(async move {
                    loop {
                        let cmd = match events.recv().await {
                            Ok(DockerEvent::Event(ev)) if ev.typ == Some(Kind::CONTAINER) => {
                                let id = ev.actor.and_then(|a| a.id);
                                let (Some(id), Some(action)) = (id, ev.action) else { continue };
                                Cmd::Event { id, action }
                            }
                            Ok(DockerEvent::Resync) | Err(RecvError::Lagged(_)) => Cmd::Resync,
                            Ok(_) => continue,
                            Err(RecvError::Closed) => return,
                        };
                        if out.send(cmd).is_err() {
                            return;
                        }
                    }
                })
                .drop_on_shutdown()
        });
        let text = gtk::TextBuffer::new(None);
        let end = text.create_mark(None, &text.end_iter(), false);
        let model = Self {
            ctx,
            id: None,
            generation: 0,
            name: String::new(),
            running: false,
            logs: LogBuffer::new(LOG_CAP),
            query: String::new(),
            paused: false,
            log_task: None,
            stats_task: None,
            prev_net: None,
            stats: Stats::default(),
            info: gtk::Box::default(),
            text_view: gtk::TextView::with_buffer(&text),
            text,
            end,
        };
        let info = &model.info;
        let text_view = &model.text_view;
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Msg, sender: ComponentSender<Self>, _root: &Self::Root) {
        match msg {
            Msg::Show(id) if id == self.id => {}
            Msg::Show(id) => {
                self.id = id;
                self.load(&sender);
            }
            Msg::Search(query) => {
                self.query = query;
                self.render_logs(!self.paused);
            }
            Msg::Pause(paused) => {
                self.paused = paused;
                if !paused {
                    self.render_logs(true);
                }
            }
        }
    }

    fn update_cmd(&mut self, cmd: Cmd, sender: ComponentSender<Self>, _root: &Self::Root) {
        match cmd {
            Cmd::Event { id, action } if Some(&id) == self.id.as_ref() => match action.as_str() {
                "start" => self.load(&sender),
                "die" => {
                    self.stop_stats();
                    self.inspect(&sender);
                }
                "rename" => self.inspect(&sender),
                _ => {}
            },
            Cmd::Event { .. } => {}
            Cmd::Resync => self.load(&sender),
            Cmd::Inspected(generation, _) | Cmd::Logs(generation, _) | Cmd::Stats(generation, ..)
                if generation != self.generation => {}
            Cmd::Inspected(_, inspect) => self.show_info(&inspect, &sender),
            Cmd::Logs(_, chunk) => self.append_logs(&chunk),
            Cmd::Stats(_, stats, at) => self.show_stats(&stats, at),
        }
    }
}

impl Details {
    fn load(&mut self, sender: &ComponentSender<Self>) {
        self.generation += 1;
        self.stop_stats();
        self.log_task = None;
        self.logs = LogBuffer::new(LOG_CAP);
        self.paused = false;
        self.text.set_text("");
        self.name.clear();
        self.info.remove_all();
        if self.id.is_some() {
            self.inspect(sender);
            self.start_logs(sender);
        }
    }

    fn inspect(&self, sender: &ComponentSender<Self>) {
        let (Some(id), generation) = (self.id.clone(), self.generation) else { return };
        self.ctx.run(
            sender,
            "Inspect container",
            |d| async move { d.inspect_container(&id, None::<InspectContainerOptions>).await },
            move |r| Cmd::Inspected(generation, Box::new(r)),
        );
    }

    fn show_info(&mut self, inspect: &ContainerInspectResponse, sender: &ComponentSender<Self>) {
        self.name = inspect.name.as_deref().unwrap_or_default().trim_start_matches('/').to_string();
        if !inspect.state.as_ref().and_then(|s| s.running).unwrap_or(false) {
            self.stop_stats();
        } else if self.stats_task.is_none() {
            self.running = true;
            self.start_stats(sender);
        }
        detail::fill(&self.info, &model::info_sections(inspect));
    }

    fn start_logs(&mut self, sender: &ComponentSender<Self>) {
        let (Some(docker), Some(id)) = (self.ctx.docker().cloned(), self.id.clone()) else { return };
        let (out, ctx, generation) = (sender.command_sender().clone(), self.ctx.clone(), self.generation);
        self.log_task = Some(Task(relm4::spawn(async move {
            let opts = LogsOptions { follow: true, stdout: true, stderr: true, tail: "1000".into(), ..Default::default() };
            let mut stream = docker.logs(&id, Some(opts)).ready_chunks(512);
            while let Some(batch) = stream.next().await {
                let mut chunk = String::new();
                for item in batch {
                    match item {
                        Ok(line) => chunk.push_str(&line.to_string()),
                        Err(e) => return ctx.error(format!("Container logs: {e}")),
                    }
                }
                out.emit(Cmd::Logs(generation, chunk));
            }
        })));
    }

    fn append_logs(&mut self, chunk: &str) {
        let delta = self.logs.push(chunk, &self.query);
        if self.paused {
            return;
        }
        if delta.removed > 0 {
            let mut start = self.text.start_iter();
            let mut cut = self.text.iter_at_line(delta.removed as i32).unwrap_or_else(|| self.text.end_iter());
            self.text.delete(&mut start, &mut cut);
        }
        self.text.insert(&mut self.text.end_iter(), &delta.added);
        self.text_view.scroll_to_mark(&self.end, 0.0, false, 0.0, 0.0);
    }

    fn render_logs(&self, follow: bool) {
        self.text.set_text(&self.logs.text(&self.query));
        if follow {
            self.text_view.scroll_to_mark(&self.end, 0.0, false, 0.0, 0.0);
        }
    }

    fn start_stats(&mut self, sender: &ComponentSender<Self>) {
        let (Some(docker), Some(id)) = (self.ctx.docker().cloned(), self.id.clone()) else { return };
        let (out, ctx, generation) = (sender.command_sender().clone(), self.ctx.clone(), self.generation);
        self.stats_task = Some(Task(relm4::spawn(async move {
            let mut stream = docker.stats(&id, Some(StatsOptions { stream: true, one_shot: false }));
            while let Some(item) = stream.next().await {
                match item {
                    Ok(stats) => out.emit(Cmd::Stats(generation, Box::new(stats), Instant::now())),
                    Err(e) => return ctx.error(format!("Container stats: {e}")),
                }
            }
        })));
    }

    fn stop_stats(&mut self) {
        self.stats_task = None;
        self.running = false;
        self.prev_net = None;
        self.stats = Stats::default();
    }

    fn show_stats(&mut self, stats: &ContainerStatsResponse, at: Instant) {
        let Some(memory) = core::memory(stats) else { return };
        self.stats.cpu = format!("{:.1} %", core::cpu_percent(stats));
        self.stats.memory = format!("{} / {}", format_bytes(memory.used), format_bytes(memory.limit));
        let net = model::net_totals(stats);
        let (rx_rate, tx_rate) = match self.prev_net {
            Some((prev, then)) => net.rate(prev, at.duration_since(then).as_secs_f64()),
            None => (0.0, 0.0),
        };
        self.prev_net = Some((net, at));
        self.stats.rx = format!("{}  ·  {}", format_rate(rx_rate), format_bytes(net.rx));
        self.stats.tx = format!("{}  ·  {}", format_rate(tx_rate), format_bytes(net.tx));
    }
}
