//! [`HandoverPicker`] — a port of `pi-intercom/ui/handover-picker.ts` (`v0.16.1`, 318 lines): the
//! overlay bare `/handover` (and `h` in the `/intercom` list) opens to choose where this session is
//! handed over to — a live peer on this machine, a new session in a project path, or a session on
//! another machine — plus an optional "next task" for the receiver.
//!
//! WIRING: `crate::extension`'s `open_handover_picker` hands a [`HandoverPickerHost`] to
//! `HostServices::open_overlay` (`v0.16.1 index.ts:3070-3073`) and reads the
//! [`HandoverPickerResult`] back from the shared cell once the overlay closes.
//!
//! The remote half is fetched ON DEMAND ("Fetch sessions from other machines"), never on open,
//! because it runs over SSH through Herdr (`RemoteSessionLister`'s own doc upstream). pi drives it
//! with promises that call `requestRender`; here the fetch runs on a spawned task that posts
//! [`RemoteEvent`]s to a channel, and the picker drains them before every key, render and tick —
//! the host ticks it every [`REFRESH_MS`] while it is open. Results that arrive after the picker
//! closed are dropped, as upstream's `closed` flag drops them (`:174-188`).
//!
//! **CYRUP-DELTA (product name):** an empty remote machine reads `"<label>: no cyrup sessions"`
//! where upstream says `no Pi sessions` (`:258`) — the same `Pi` → `cyrup` substitution
//! [`crate::cross_machine::DiscoveryError`] makes in its "Pi agent" sentences, because the agents
//! Herdr lists for cyrup are cyrup sessions.

use std::sync::{Arc, Mutex};

use cyrup_ext::{InteractiveOverlay, OverlayKey, OverlayLine, OverlayOptions, OverlayOutcome};

use crate::cross_machine::{
    CommandRunner, DISCOVERY_TIMEOUT, DiscoveryDeps, RemoteAgent, SavedMachine,
    list_machine_agents, list_saved_machines,
};
use crate::identity::short_session_id;
use crate::transport::protocol::SessionInfo;
use crate::ui::input::Input;
use crate::ui::overlay::{OverlayTheme, key_to_data, to_overlay_line};
use crate::ui::session_list::{box_row, herdr_location_text};
use crate::ui::{
    DefaultKeybindings, Keybindings, Theme, middle_truncate, visible_width, wrap_text,
};

/// `PICKER_WIDTH = 88` (`v0.16.1 handover-picker.ts:8`).
pub const PICKER_WIDTH: usize = 88;
/// `MAX_BODY_LINES = 24` (`:9`).
pub const MAX_BODY_LINES: usize = 24;
/// `CONTEXT_WARNING_PCT = 80` (`:10`).
pub const CONTEXT_WARNING_PCT: f64 = 80.0;
/// How often the host ticks the picker so a remote answer is painted without a keystroke.
pub const REFRESH_MS: u64 = 100;

/// `HandoverPickerTarget` (`:12-15`).
#[derive(Clone, Debug, PartialEq)]
pub enum HandoverPickerTarget {
    /// `{ kind: "local"; session }`. Boxed: a [`SessionInfo`] dwarfs the other variants.
    Local(Box<SessionInfo>),
    /// `{ kind: "project" }` — the caller asks for the path next.
    Project,
    /// `{ kind: "remote"; target }` — `"<sessionId ?? name>@<machine label>"`.
    Remote(String),
}

/// `HandoverPickerResult` (`:17-20`).
#[derive(Clone, Debug, PartialEq)]
pub struct HandoverPickerResult {
    /// Where to hand over to.
    pub target: HandoverPickerTarget,
    /// The trimmed next task, `None` when blank (`this.taskInput.getValue().trim() || undefined`).
    pub goal: Option<String>,
}

/// `RemoteSessionLister` (`:22-26`) — "Remote listing runs over SSH through Herdr, so it is
/// injected." Errors are the human sentence upstream's `errorMessage(error)` would show.
#[async_trait::async_trait]
pub trait RemoteSessionLister: Send + Sync {
    /// `listMachines()`.
    async fn list_machines(&self) -> Result<Vec<SavedMachine>, String>;
    /// `listAgents(machine)`.
    async fn list_agents(&self, machine: &SavedMachine) -> Result<Vec<RemoteAgent>, String>;
}

/// The production lister (`v0.16.1 index.ts:3065-3069`): `listSavedMachines` /
/// `listMachineAgents` over the session's process runner and `HERDR_BIN_PATH ?? "herdr"`.
pub struct HerdrRemoteLister {
    runner: Arc<dyn CommandRunner>,
    herdr_bin: String,
}

impl HerdrRemoteLister {
    /// A lister running `herdr_bin` through `runner`.
    #[must_use]
    pub fn new(runner: Arc<dyn CommandRunner>, herdr_bin: String) -> Self {
        Self { runner, herdr_bin }
    }

    fn deps(&self) -> DiscoveryDeps<'_> {
        DiscoveryDeps {
            run: self.runner.as_ref(),
            herdr_bin: &self.herdr_bin,
            discovery_timeout: DISCOVERY_TIMEOUT,
        }
    }
}

#[async_trait::async_trait]
impl RemoteSessionLister for HerdrRemoteLister {
    async fn list_machines(&self) -> Result<Vec<SavedMachine>, String> {
        list_saved_machines(&self.deps())
            .await
            .map_err(|e| e.to_string())
    }

    async fn list_agents(&self, machine: &SavedMachine) -> Result<Vec<RemoteAgent>, String> {
        list_machine_agents(machine, &self.deps())
            .await
            .map_err(|e| e.to_string())
    }
}

/// `MachineEntry` (`:35-38`), without the machine (kept alongside it).
#[derive(Clone, Debug)]
enum MachineState {
    Fetching,
    Done(Vec<RemoteAgent>),
    Error(String),
}

/// `RemoteState` (`:40-44`).
#[derive(Clone, Debug)]
enum RemoteState {
    Idle,
    Listing,
    Error(String),
    Machines(Vec<(SavedMachine, MachineState)>),
}

/// `Item` (`:46-50`). Remote agents are identified by position, which is stable: an entry moves
/// from fetching to done once and its agent list never changes afterwards — the property
/// upstream's `item.agent === selected.agent` object identity relies on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Local(usize),
    Project,
    Fetch,
    Remote { machine: usize, agent: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    List,
    Task,
}

/// What the spawned fetch posts back to the picker.
#[derive(Debug)]
enum RemoteEvent {
    /// `listMachines()` settled; `Ok` is already filtered to enabled machines.
    Machines(Result<Vec<SavedMachine>, String>),
    /// One machine's `listAgents()` settled.
    Agents {
        index: usize,
        result: Result<Vec<RemoteAgent>, String>,
    },
}

/// `isSubagentChild(session)` (`:52-55`, `29de444`): "pi-subagents registers children under a
/// "subagent-<agent>-<run>" session id; their name is the task text." cyrup's subagent children
/// register under the same prefix (`cyrup-ext-subagents/src/spawn/intercom_target.rs`).
#[must_use]
pub fn is_subagent_child(session: &SessionInfo) -> bool {
    session.id.starts_with("subagent-")
}

fn as_f64(n: &serde_json::Number) -> f64 {
    n.as_f64().unwrap_or(0.0)
}

/// pi `HandoverPicker` (`:61-318`).
pub struct HandoverPicker {
    lister: Arc<dyn RemoteSessionLister>,
    runtime: tokio::runtime::Handle,
    local_sessions: Vec<SessionInfo>,
    remote: RemoteState,
    closed: bool,
    selected_index: usize,
    focus: Focus,
    task_input: Input,
    inbox: Option<tokio::sync::mpsc::UnboundedReceiver<RemoteEvent>>,
    fetch: Option<tokio::task::JoinHandle<()>>,
    result: Option<HandoverPickerResult>,
}

impl Drop for HandoverPicker {
    fn drop(&mut self) {
        if let Some(fetch) = self.fetch.take() {
            fetch.abort();
        }
    }
}

impl HandoverPicker {
    /// pi's constructor (`:76-94`): peers other than this session and its subagent children, most
    /// recently active first; a `preselect_session_id` that names one of them is highlighted and
    /// moves focus to the task field.
    #[must_use]
    pub fn new(
        current_session: &SessionInfo,
        sessions: Vec<SessionInfo>,
        lister: Arc<dyn RemoteSessionLister>,
        preselect_session_id: Option<&str>,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let mut local_sessions: Vec<SessionInfo> = sessions
            .into_iter()
            .filter(|s| s.id != current_session.id && !is_subagent_child(s))
            .collect();
        // `.sort((a, b) => b.lastActivity - a.lastActivity)` — a stable sort, as JS's is.
        local_sessions.sort_by(|a, b| {
            as_f64(&b.last_activity)
                .partial_cmp(&as_f64(&a.last_activity))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let preselected =
            preselect_session_id.and_then(|id| local_sessions.iter().position(|s| s.id == id));
        Self {
            lister,
            runtime,
            local_sessions,
            remote: RemoteState::Idle,
            closed: false,
            selected_index: preselected.unwrap_or(0),
            focus: if preselected.is_some() {
                Focus::Task
            } else {
                Focus::List
            },
            task_input: Input::new(),
            inbox: None,
            fetch: None,
            result: None,
        }
    }

    /// Whether the picker has closed (`done` was called).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The choice the picker closed with; `None` after Escape.
    #[must_use]
    pub fn result(&self) -> Option<&HandoverPickerResult> {
        self.result.as_ref()
    }

    /// `items()` (`:96-106`).
    fn items(&self) -> Vec<Item> {
        let mut items: Vec<Item> = (0..self.local_sessions.len()).map(Item::Local).collect();
        items.push(Item::Project);
        items.push(Item::Fetch);
        if let RemoteState::Machines(machines) = &self.remote {
            for (m, (_, state)) in machines.iter().enumerate() {
                if let MachineState::Done(agents) = state {
                    items.extend((0..agents.len()).map(|a| Item::Remote {
                        machine: m,
                        agent: a,
                    }));
                }
            }
        }
        items
    }

    fn close(&mut self, result: Option<HandoverPickerResult>) {
        self.closed = true;
        self.result = result;
        self.inbox = None;
        if let Some(fetch) = self.fetch.take() {
            fetch.abort();
        }
    }

    /// Apply every remote answer that has arrived. Returns whether anything changed.
    pub fn drain(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(event) = self.inbox.as_mut().and_then(|rx| rx.try_recv().ok()) else {
                return changed;
            };
            if self.closed {
                return changed;
            }
            changed = true;
            match event {
                RemoteEvent::Machines(Err(error)) => self.remote = RemoteState::Error(error),
                RemoteEvent::Machines(Ok(machines)) => {
                    self.remote = RemoteState::Machines(
                        machines
                            .into_iter()
                            .map(|m| (m, MachineState::Fetching))
                            .collect(),
                    );
                }
                RemoteEvent::Agents { index, result } => {
                    // "Earlier machines can finish later, so keep the highlighted remote session
                    // selected." (`:189-194`)
                    let selected = self.items().get(self.selected_index).copied();
                    if let RemoteState::Machines(machines) = &mut self.remote
                        && let Some((_, state)) = machines.get_mut(index)
                    {
                        *state = match result {
                            Ok(agents) => MachineState::Done(agents),
                            Err(error) => MachineState::Error(error),
                        };
                    }
                    if let Some(selected @ Item::Remote { .. }) = selected
                        && let Some(at) = self.items().iter().position(|item| *item == selected)
                    {
                        self.selected_index = at;
                    }
                }
            }
        }
    }

    /// pi `handleInput(data)` (`:110-139`). Returns whether the picker should repaint.
    pub fn handle_input(&mut self, keybindings: &dyn Keybindings, data: &str) -> bool {
        self.drain();
        if keybindings.matches(data, "tui.select.cancel") {
            self.close(None);
            return true;
        }
        if keybindings.matches(data, "tui.input.tab") {
            self.focus = match self.focus {
                Focus::List => Focus::Task,
                Focus::Task => Focus::List,
            };
            return true;
        }
        let items = self.items();
        let last = items.len().saturating_sub(1);
        if keybindings.matches(data, "tui.select.up") {
            self.selected_index = if self.selected_index == 0 {
                last
            } else {
                self.selected_index - 1
            };
            return true;
        }
        if keybindings.matches(data, "tui.select.down") {
            self.selected_index = if self.selected_index >= last {
                0
            } else {
                self.selected_index + 1
            };
            return true;
        }
        if keybindings.matches(data, "tui.select.confirm") {
            if let Some(item) = items.get(self.selected_index).copied() {
                self.activate(item);
            }
            return true;
        }
        if self.focus != Focus::Task {
            return false;
        }
        self.task_input.handle_input(keybindings, data);
        true
    }

    /// A bracketed paste the host decoded before routing — pi delivers it to `taskInput` through
    /// `handleInput`, so it lands only while the task field has focus.
    pub fn handle_paste(&mut self, text: &str) -> bool {
        if self.focus != Focus::Task {
            return false;
        }
        self.task_input.handle_paste(text);
        true
    }

    /// `activate(item)` (`:141-151`).
    fn activate(&mut self, item: Item) {
        let target = match item {
            Item::Fetch => {
                self.fetch_remote();
                return;
            }
            Item::Local(i) => match self.local_sessions.get(i) {
                Some(session) => HandoverPickerTarget::Local(Box::new(session.clone())),
                None => return,
            },
            Item::Project => HandoverPickerTarget::Project,
            Item::Remote { machine, agent } => {
                let RemoteState::Machines(machines) = &self.remote else {
                    return;
                };
                let Some((machine, MachineState::Done(agents))) = machines.get(machine) else {
                    return;
                };
                let Some(agent) = agents.get(agent) else {
                    return;
                };
                HandoverPickerTarget::Remote(format!(
                    "{}@{}",
                    agent.session_id.as_deref().unwrap_or(&agent.name),
                    machine.label
                ))
            }
        };
        let goal = Some(self.task_input.value().trim().to_string()).filter(|g| !g.is_empty());
        self.close(Some(HandoverPickerResult { target, goal }));
    }

    /// `fetchRemote()` (`:153-196`): the busy guard, the `listing` state, then the spawned listing.
    fn fetch_remote(&mut self) {
        let busy = match &self.remote {
            RemoteState::Listing => true,
            RemoteState::Machines(machines) => machines
                .iter()
                .any(|(_, state)| matches!(state, MachineState::Fetching)),
            _ => false,
        };
        if busy {
            return;
        }
        self.remote = RemoteState::Listing;
        // "A new fetch drops the previous remote rows."
        self.selected_index = self
            .selected_index
            .min(self.items().len().saturating_sub(1));
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.inbox = Some(rx);
        let lister = self.lister.clone();
        self.fetch = Some(self.runtime.spawn(async move {
            let machines: Vec<SavedMachine> = match lister.list_machines().await {
                Ok(machines) => machines.into_iter().filter(|m| m.enabled).collect(),
                Err(error) => {
                    let _ = tx.send(RemoteEvent::Machines(Err(error)));
                    return;
                }
            };
            let _ = tx.send(RemoteEvent::Machines(Ok(machines.clone())));
            // `Promise.all(machines.map(…))` — concurrent, each answer posted as it lands. The set
            // aborts every listing still running if this task is aborted (the picker closed).
            let mut set = tokio::task::JoinSet::new();
            for (index, machine) in machines.into_iter().enumerate() {
                let lister = lister.clone();
                let tx = tx.clone();
                set.spawn(async move {
                    let result = lister.list_agents(&machine).await;
                    let _ = tx.send(RemoteEvent::Agents { index, result });
                });
            }
            while set.join_next().await.is_some() {}
        }));
    }

    /// `remoteLabel(machine, agent)` (`:198-201`).
    fn remote_label(machine: &SavedMachine, agent: &RemoteAgent) -> String {
        let name = match &agent.session_id {
            Some(id) if agent.name == *id => short_session_id(id),
            _ => agent.name.clone(),
        };
        format!("{name}@{}", machine.label)
    }

    /// pi `render(width)` (`:203-317`). Every line is exactly `min(width, 88)` columns.
    #[must_use]
    pub fn render(
        &self,
        theme: &dyn Theme,
        keybindings: &dyn Keybindings,
        width: usize,
    ) -> Vec<String> {
        let inner_width = width.clamp(1, PICKER_WIDTH);
        if inner_width == 1 {
            return vec![theme.fg("accent", "│")];
        }
        let content_width = inner_width.saturating_sub(2);
        let border = |text: &str| theme.fg("accent", text);
        let separator = border(&format!("├{}┤", "─".repeat(content_width)));
        let row = |text: &str| box_row(theme, content_width, text);
        let dim = |text: &str| theme.fg("dim", text);
        // `wrapTextWithAnsi(text, Math.max(1, contentWidth - 2))` (`29de444`): a long machine error
        // wraps instead of being cut off.
        let error_rows = |text: &str| -> Vec<String> {
            wrap_text(text, content_width.saturating_sub(2).max(1))
                .into_iter()
                .map(|line| row(&theme.fg("error", &format!("  {line}"))))
                .collect()
        };
        let path_width = content_width.saturating_sub(4).max(8);

        let items = self.items();
        let mut body: Vec<String> = Vec::new();
        let mut selected_start = 0usize;
        let mut selected_end = 0usize;
        // "`suffix` follows the title and keeps its own colors; `details` are dim secondary lines."
        let mut push_item =
            |body: &mut Vec<String>, item: Item, title: &str, details: &[String], suffix: &str| {
                let selected = items.iter().position(|i| *i == item) == Some(self.selected_index);
                if selected {
                    selected_start = body.len();
                }
                let prefix = if selected {
                    theme.fg("accent", "→ ")
                } else {
                    "  ".to_string()
                };
                let title = if selected {
                    theme.fg("accent", title)
                } else {
                    title.to_string()
                };
                body.push(row(&format!("{prefix}{title}{suffix}")));
                for line in details {
                    body.push(row(&format!("  {}", dim(line))));
                }
                if selected {
                    selected_end = body.len();
                }
            };

        body.push(row(&theme.bold(" This machine")));
        for (i, session) in self.local_sessions.iter().enumerate() {
            let status = session.status.as_deref().map_or(String::new(), |status| {
                let first = status.split(" · ").next().unwrap_or_default();
                let styled = if first == "idle" {
                    dim(status)
                } else {
                    theme.fg("warning", status)
                };
                format!(" · {styled}")
            });
            let pct = session.context_pct.as_ref().map_or(String::new(), |pct| {
                let text = format!("{pct}% ctx");
                let styled = if as_f64(pct) >= CONTEXT_WARNING_PCT {
                    theme.fg("warning", &text)
                } else {
                    dim(&text)
                };
                format!(" · {styled}")
            });
            let mut details = vec![format!(
                "{} • {}",
                middle_truncate(&session.cwd, path_width),
                session.model
            )];
            details.extend(herdr_location_text(session));
            let name = session
                .name
                .as_deref()
                .filter(|n| !n.is_empty())
                .unwrap_or("Unnamed session");
            push_item(
                &mut body,
                Item::Local(i),
                &format!("{name} ({})", short_session_id(&session.id)),
                &details,
                &format!("{status}{pct}"),
            );
        }
        push_item(
            &mut body,
            Item::Project,
            "+ New session in a project path…",
            &[],
            "",
        );

        body.push(separator.clone());
        body.push(row(&theme.bold(" Other machines")));
        push_item(
            &mut body,
            Item::Fetch,
            if matches!(self.remote, RemoteState::Idle) {
                "Fetch sessions from other machines"
            } else {
                "Fetch again"
            },
            &[],
            "",
        );
        match &self.remote {
            RemoteState::Idle => {}
            RemoteState::Listing => body.push(row(&dim("  Listing saved Herdr machines…"))),
            RemoteState::Error(error) => body.extend(error_rows(error)),
            RemoteState::Machines(machines) => {
                if machines.is_empty() {
                    body.push(row(&dim("  No enabled saved Herdr machines.")));
                }
                for (m, (machine, state)) in machines.iter().enumerate() {
                    match state {
                        MachineState::Fetching => {
                            body.push(row(&dim(&format!("  {}: fetching…", machine.label))));
                        }
                        MachineState::Error(error) => body.extend(error_rows(error)),
                        MachineState::Done(agents) if agents.is_empty() => {
                            body.push(row(&dim(&format!(
                                "  {}: no cyrup sessions",
                                machine.label
                            ))));
                        }
                        MachineState::Done(agents) => {
                            for (a, agent) in agents.iter().enumerate() {
                                let details: Vec<String> = agent
                                    .cwd
                                    .as_deref()
                                    .map(|cwd| middle_truncate(cwd, path_width))
                                    .into_iter()
                                    .collect();
                                let suffix = agent.status.as_deref().map_or(String::new(), |s| {
                                    let styled = if s == "idle" {
                                        dim(s)
                                    } else {
                                        theme.fg("warning", s)
                                    };
                                    format!(" · {styled}")
                                });
                                push_item(
                                    &mut body,
                                    Item::Remote {
                                        machine: m,
                                        agent: a,
                                    },
                                    &Self::remote_label(machine, agent),
                                    &details,
                                    &suffix,
                                );
                            }
                        }
                    }
                }
            }
        }

        // The windowed body (`:265-273`), in signed space to keep upstream's `Math.floor`.
        let visible: Vec<String> = if body.len() > MAX_BODY_LINES {
            let window = MAX_BODY_LINES as i64 - 1;
            let selected_length = selected_end as i64 - selected_start as i64;
            let start = (selected_start as i64 - (window - selected_length).div_euclid(2))
                .min(body.len() as i64 - window)
                .max(0) as usize;
            let mut visible: Vec<String> = body
                .iter()
                .skip(start)
                .take(MAX_BODY_LINES - 1)
                .cloned()
                .collect();
            visible.push(row(&dim(&format!(
                " {}/{}",
                self.selected_index + 1,
                items.len()
            ))));
            visible
        } else {
            body
        };

        let task_label = " Next task (optional): ";
        let task_line = match self.focus {
            Focus::Task => format!(
                "{task_label}{}",
                self.task_input.render(
                    theme,
                    content_width
                        .saturating_sub(visible_width(task_label))
                        .max(1)
                )
            ),
            Focus::List => dim(&format!("{task_label}{}", self.task_input.value())),
        };
        let keys = |id: &str| keybindings.get_keys(id).join("/");
        let footer = format!(
            "{}: Hand over • {}: {} • {}: Close",
            keys("tui.select.confirm"),
            keys("tui.input.tab"),
            match self.focus {
                Focus::List => "Next task",
                Focus::Task => "List",
            },
            keys("tui.select.cancel"),
        );

        let mut lines = vec![
            border(&format!("╭{}╮", "─".repeat(content_width))),
            row(&theme.bold(" Hand over to")),
            separator.clone(),
        ];
        lines.extend(visible);
        lines.push(separator.clone());
        lines.push(row(&task_line));
        lines.push(separator);
        lines.push(row(&dim(&format!(" {footer}"))));
        lines.push(border(&format!("╰{}╯", "─".repeat(content_width))));
        lines
    }
}

/// The live picker: [`HandoverPicker`] behind the host's overlay seam — pi `ctx.ui.custom((tui,
/// theme, keybindings, done) => new HandoverPicker(…, done), { overlay: true, overlayOptions: {
/// width: 88 } })` (`v0.16.1 index.ts:3070-3073`). `done(result)` is the `result` cell, read by
/// the caller once `open_overlay` returns; `None` there is Escape.
pub struct HandoverPickerHost {
    picker: HandoverPicker,
    result: Arc<Mutex<Option<HandoverPickerResult>>>,
}

impl HandoverPickerHost {
    /// Wrap `picker`, publishing its choice into `result`.
    #[must_use]
    pub fn new(picker: HandoverPicker, result: Arc<Mutex<Option<HandoverPickerResult>>>) -> Self {
        Self { picker, result }
    }

    fn outcome(&mut self, changed: bool) -> OverlayOutcome {
        if self.picker.is_closed() {
            *self.result.lock().unwrap_or_else(|e| e.into_inner()) = self.picker.result().cloned();
            return OverlayOutcome::Close;
        }
        if changed {
            OverlayOutcome::Redraw
        } else {
            OverlayOutcome::Ignored
        }
    }
}

impl InteractiveOverlay for HandoverPickerHost {
    fn render(&mut self, width: usize, _height: usize) -> Vec<OverlayLine> {
        self.picker.drain();
        self.picker
            .render(&OverlayTheme, &DefaultKeybindings, width)
            .iter()
            .map(|line| to_overlay_line(line))
            .collect()
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        let Some(data) = key_to_data(key) else {
            return OverlayOutcome::Ignored;
        };
        let changed = self.picker.handle_input(&DefaultKeybindings, &data);
        self.outcome(changed)
    }

    fn handle_paste(&mut self, text: &str) -> OverlayOutcome {
        let changed = self.picker.handle_paste(text);
        self.outcome(changed)
    }

    fn refresh_ms(&self) -> u64 {
        REFRESH_MS
    }

    fn tick(&mut self) -> bool {
        self.picker.drain()
    }

    fn should_close(&self) -> bool {
        self.picker.is_closed()
    }

    fn options(&self) -> OverlayOptions {
        OverlayOptions {
            width: Some(PICKER_WIDTH as u16),
            ..OverlayOptions::default()
        }
    }
}

/// Ports of `test/handover-picker.test.ts` (`v0.16.1`), plus the overlay adapter.
#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use super::*;
    use crate::ui::PlainTheme;
    use std::time::Duration;

    /// The upstream test's keybindings: one key per id, `getKeys` answers the id's last segment.
    struct TestKeys;
    fn key(id: &str) -> &'static str {
        match id {
            "tui.select.up" => "\x1b[A",
            "tui.select.down" => "\x1b[B",
            "tui.select.confirm" => "\r",
            "tui.select.cancel" => "\x1b",
            "tui.input.tab" => "\t",
            "tui.editor.deleteCharBackward" => "\x7f",
            _ => "",
        }
    }
    impl Keybindings for TestKeys {
        fn matches(&self, data: &str, action: &str) -> bool {
            !key(action).is_empty() && key(action) == data
        }
        fn get_keys(&self, action: &str) -> Vec<String> {
            vec![action.rsplit('.').next().unwrap_or_default().to_string()]
        }
    }

    fn session(id: &str, name: &str, last_activity: u64) -> SessionInfo {
        SessionInfo {
            endpoint_epoch: None,
            id: id.to_string(),
            name: Some(name.to_string()),
            runtime_fallback_alias: None,
            cwd: format!("/work/{name}"),
            model: "model".to_string(),
            pid: 1u32.into(),
            started_at: 0u64.into(),
            last_activity: last_activity.into(),
            status: None,
            peer_uid: None,
            trusted_local: None,
            context_pct: None,
            context_tokens: None,
            context_window: None,
            tmux_pane: None,
            herdr_pane_id: None,
            herdr_location: None,
            extra: Default::default(),
        }
    }

    fn me() -> SessionInfo {
        session("self-0000", "planner", 0)
    }

    fn roster() -> Vec<SessionInfo> {
        let mut unnamed = session("unnamed-00", "subagent-chat-unnamed", 1);
        unnamed.runtime_fallback_alias = Some(true);
        vec![
            me(),
            session(
                "subagent-worker-run-1",
                "worker: You are the writer for lane x",
                0,
            ),
            unnamed,
            session("adapter-00", "adapter", 2),
        ]
    }

    struct NoRemote;
    #[async_trait::async_trait]
    impl RemoteSessionLister for NoRemote {
        async fn list_machines(&self) -> Result<Vec<SavedMachine>, String> {
            panic!("remote listing must not run on open")
        }
        async fn list_agents(&self, _: &SavedMachine) -> Result<Vec<RemoteAgent>, String> {
            Ok(Vec::new())
        }
    }

    fn open(lister: Arc<dyn RemoteSessionLister>, preselect: Option<&str>) -> HandoverPicker {
        HandoverPicker::new(
            &me(),
            roster(),
            lister,
            preselect,
            tokio::runtime::Handle::current(),
        )
    }

    fn press(picker: &mut HandoverPicker, keys: &[&str]) {
        for k in keys {
            let data = if key(k).is_empty() { *k } else { key(k) };
            picker.handle_input(&TestKeys, data);
        }
    }

    fn text(picker: &HandoverPicker) -> String {
        picker.render(&PlainTheme, &TestKeys, 88).join("\n")
    }

    /// Let the spawned listing run until `done` holds, draining as the host's tick would.
    async fn settle(picker: &mut HandoverPicker, done: impl Fn(&HandoverPicker) -> bool) {
        for _ in 0..400 {
            picker.drain();
            if done(picker) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the remote listing never settled:\n{}", text(picker));
    }

    fn machine(label: &str, enabled: bool) -> SavedMachine {
        SavedMachine {
            label: label.to_string(),
            target: format!("10.0.0.{}", label.len()),
            enabled,
        }
    }

    fn agent(name: &str) -> RemoteAgent {
        RemoteAgent {
            name: name.to_string(),
            session_id: None,
            cwd: None,
            status: None,
        }
    }

    #[tokio::test]
    async fn handover_picker_lists_local_peers_without_self_or_subagent_children() {
        let mut picker = open(Arc::new(NoRemote), None);
        let shown = text(&picker);
        assert!(!shown.contains("planner"), "{shown}");
        assert!(!shown.contains("You are the writer"), "{shown}");
        let adapter = shown.find("adapter (adapter-").expect("adapter listed");
        let unnamed = shown.find("subagent-chat-unnamed").expect("unnamed listed");
        assert!(adapter < unnamed, "most recently active first:\n{shown}");
        press(&mut picker, &["tui.select.confirm"]);
        assert_eq!(
            picker.result().map(|r| r.target.clone()),
            Some(HandoverPickerTarget::Local(Box::new(roster()[3].clone())))
        );
    }

    #[tokio::test]
    async fn handover_picker_returns_the_next_task_typed_in_the_task_field() {
        let mut picker = open(Arc::new(NoRemote), Some("unnamed-00"));
        press(&mut picker, &["port", " the fix", "tui.select.confirm"]);
        assert_eq!(
            picker.result().cloned(),
            Some(HandoverPickerResult {
                target: HandoverPickerTarget::Local(Box::new(roster()[2].clone())),
                goal: Some("port the fix".to_string()),
            })
        );
    }

    #[tokio::test]
    async fn handover_picker_accepts_pasted_and_kitty_encoded_next_task_text() {
        let mut picker = open(Arc::new(NoRemote), None);
        press(
            &mut picker,
            &[
                "tui.input.tab",
                "\x1b[200~port the fix\x1b[201~",
                "\x1b[32u",
                "\x1b[97u",
                "tui.select.confirm",
            ],
        );
        assert_eq!(
            picker.result().and_then(|r| r.goal.clone()).as_deref(),
            Some("port the fix a")
        );
    }

    #[tokio::test]
    async fn a_host_decoded_paste_lands_only_in_a_focused_task_field() {
        let mut picker = open(Arc::new(NoRemote), None);
        assert!(!picker.handle_paste("ignored"));
        press(&mut picker, &["tui.input.tab"]);
        assert!(picker.handle_paste("port\nthe fix"));
        press(&mut picker, &["tui.select.confirm"]);
        assert_eq!(
            picker.result().and_then(|r| r.goal.clone()).as_deref(),
            Some("portthe fix")
        );
    }

    /// Two machines answer; a third is disabled and never listed.
    struct TwoMachines {
        session_id: String,
    }
    #[async_trait::async_trait]
    impl RemoteSessionLister for TwoMachines {
        async fn list_machines(&self) -> Result<Vec<SavedMachine>, String> {
            Ok(vec![
                machine("workmac", true),
                machine("macmini", true),
                machine("off", false),
            ])
        }
        async fn list_agents(&self, machine: &SavedMachine) -> Result<Vec<RemoteAgent>, String> {
            assert_ne!(machine.label, "off", "a disabled machine is never asked");
            if machine.label == "macmini" {
                return Err(format!(
                    "Saved Herdr machine \"macmini\" is unreachable: {}end-of-error.",
                    "slow ".repeat(20)
                ));
            }
            Ok(vec![RemoteAgent {
                name: self.session_id.clone(),
                session_id: Some(self.session_id.clone()),
                cwd: Some("/work/remote".to_string()),
                status: Some("idle".to_string()),
            }])
        }
    }

    #[tokio::test]
    async fn handover_picker_fetches_other_machines_on_demand_and_targets_a_remote_session_by_id() {
        let session_id = "00000000-0000-4000-8000-000000000001".to_string();
        let mut picker = open(
            Arc::new(TwoMachines {
                session_id: session_id.clone(),
            }),
            None,
        );
        press(
            &mut picker,
            &[
                "tui.select.down",
                "tui.select.down",
                "tui.select.down",
                "tui.select.confirm",
            ],
        );
        settle(&mut picker, |p| {
            !text(p).contains("fetching…") && !text(p).contains("Listing")
        })
        .await;
        let shown = text(&picker);
        assert!(shown.contains("00000000@workmac · idle"), "{shown}");
        assert!(
            shown.contains("Saved Herdr machine \"macmini\" is unreachable"),
            "{shown}"
        );
        assert!(
            shown.contains("end-of-error."),
            "long machine errors wrap instead of being cut off:\n{shown}"
        );
        assert!(!shown.contains("off"), "{shown}");
        press(&mut picker, &["tui.select.down", "tui.select.confirm"]);
        assert_eq!(
            picker.result().cloned(),
            Some(HandoverPickerResult {
                target: HandoverPickerTarget::Remote(format!("{session_id}@workmac")),
                goal: None,
            })
        );
    }

    /// `slow` answers only when the test says so.
    struct SlowThenFast {
        slow: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<Vec<RemoteAgent>>>>,
    }
    #[async_trait::async_trait]
    impl RemoteSessionLister for SlowThenFast {
        async fn list_machines(&self) -> Result<Vec<SavedMachine>, String> {
            Ok(vec![machine("slow", true), machine("fast", true)])
        }
        async fn list_agents(&self, machine: &SavedMachine) -> Result<Vec<RemoteAgent>, String> {
            if machine.label == "fast" {
                return Ok(vec![agent("fast-agent")]);
            }
            let rx = self.slow.lock().await.take().expect("asked once");
            Ok(rx.await.unwrap_or_default())
        }
    }

    #[tokio::test]
    async fn handover_picker_keeps_the_highlighted_remote_session_when_an_earlier_machine_answers_later()
     {
        let (answer_slow, slow) = tokio::sync::oneshot::channel();
        let mut picker = open(
            Arc::new(SlowThenFast {
                slow: tokio::sync::Mutex::new(Some(slow)),
            }),
            None,
        );
        press(
            &mut picker,
            &[
                "tui.select.down",
                "tui.select.down",
                "tui.select.down",
                "tui.select.confirm",
            ],
        );
        settle(&mut picker, |p| text(p).contains("fast-agent@fast")).await;
        press(&mut picker, &["tui.select.down"]);
        answer_slow.send(vec![agent("slow-agent")]).unwrap();
        settle(&mut picker, |p| text(p).contains("slow-agent@slow")).await;
        press(&mut picker, &["tui.select.confirm"]);
        assert_eq!(
            picker.result().cloned(),
            Some(HandoverPickerResult {
                target: HandoverPickerTarget::Remote("fast-agent@fast".to_string()),
                goal: None,
            })
        );
    }

    #[tokio::test]
    async fn handover_picker_renders_lines_at_the_declared_overlay_width() {
        let mut picker = open(Arc::new(NoRemote), None);
        for focus in ["list", "task"] {
            for width in [1usize, 2, 20, 50, 88, 120] {
                for line in picker.render(&PlainTheme, &TestKeys, width) {
                    assert_eq!(
                        visible_width(&line),
                        width.min(88),
                        "{focus} {width}: {line:?}"
                    );
                }
                // The live theme's markers are zero-width too.
                for line in picker.render(&OverlayTheme, &TestKeys, width) {
                    assert_eq!(
                        to_overlay_line(&line).plain_text().chars().count(),
                        visible_width(&line)
                    );
                }
            }
            press(
                &mut picker,
                &[
                    "tui.input.tab",
                    "a long next task that does not fit in a narrow overlay",
                ],
            );
        }
    }

    #[tokio::test]
    async fn a_busy_context_and_status_are_warnings_and_idle_is_dim() {
        use cyrup_ext::{OverlayColor, ThemeRole};
        let mut hot = session("hot-0000", "hot", 5);
        hot.context_pct = Some(85u32.into());
        hot.status = Some("thinking · read".to_string());
        let mut cool = session("cool-0000", "cool", 4);
        cool.context_pct = Some(12u32.into());
        cool.status = Some("idle".to_string());
        let picker = HandoverPicker::new(
            &me(),
            vec![me(), hot, cool],
            Arc::new(NoRemote),
            None,
            tokio::runtime::Handle::current(),
        );
        let spans: Vec<(String, Option<OverlayColor>)> = picker
            .render(&OverlayTheme, &DefaultKeybindings, 88)
            .iter()
            .flat_map(|line| to_overlay_line(line).spans)
            .map(|span| (span.text, span.fg))
            .collect();
        let role_of = |text: &str| {
            spans
                .iter()
                .find(|(t, _)| t == text)
                .map(|(_, fg)| *fg)
                .unwrap_or_else(|| panic!("{text:?} not painted: {spans:#?}"))
        };
        let theme = |role| Some(OverlayColor::Theme(role));
        assert_eq!(role_of("85% ctx"), theme(ThemeRole::Warning));
        assert_eq!(role_of("thinking · read"), theme(ThemeRole::Warning));
        assert_eq!(role_of("12% ctx"), theme(ThemeRole::Dim));
        assert_eq!(role_of("idle"), theme(ThemeRole::Dim));
        assert_eq!(
            role_of(" enter: Hand over • tab: Next task • escape/ctrl+c: Close"),
            theme(ThemeRole::Dim)
        );
    }

    #[tokio::test]
    async fn a_listing_failure_is_shown_and_fetch_again_retries() {
        struct Failing(std::sync::atomic::AtomicUsize);
        #[async_trait::async_trait]
        impl RemoteSessionLister for Failing {
            async fn list_machines(&self) -> Result<Vec<SavedMachine>, String> {
                if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    return Err("Could not list Herdr saved machines: exit 1".to_string());
                }
                Ok(Vec::new())
            }
            async fn list_agents(&self, _: &SavedMachine) -> Result<Vec<RemoteAgent>, String> {
                Ok(Vec::new())
            }
        }
        let mut picker = open(Arc::new(Failing(0.into())), None);
        press(
            &mut picker,
            &["tui.select.down", "tui.select.down", "tui.select.down"],
        );
        assert!(text(&picker).contains("Fetch sessions from other machines"));
        press(&mut picker, &["tui.select.confirm"]);
        assert!(text(&picker).contains("Listing saved Herdr machines…"));
        settle(&mut picker, |p| text(p).contains("exit 1")).await;
        assert!(text(&picker).contains("Fetch again"));
        press(&mut picker, &["tui.select.confirm"]);
        settle(&mut picker, |p| {
            text(p).contains("No enabled saved Herdr machines.")
        })
        .await;
    }

    #[tokio::test]
    async fn the_overlay_adapter_publishes_the_choice_and_escape_publishes_nothing() {
        use cyrup_ext::OverlayKeyCode;
        let result = Arc::new(Mutex::new(None));
        let mut host = HandoverPickerHost::new(open(Arc::new(NoRemote), None), result.clone());
        assert_eq!(host.options().width, Some(88));
        assert_eq!(
            host.handle_key(OverlayKey::plain(OverlayKeyCode::Tab)),
            OverlayOutcome::Redraw
        );
        for c in "next".chars() {
            host.handle_key(OverlayKey::plain(OverlayKeyCode::Char(c)));
        }
        let frame: Vec<String> = host.render(88, 40).iter().map(|l| l.plain_text()).collect();
        assert!(
            frame
                .iter()
                .any(|l| l.contains("Next task (optional): > next")),
            "{frame:#?}"
        );
        assert!(
            frame
                .iter()
                .any(|l| l.contains("enter: Hand over • tab: List • escape/ctrl+c: Close")),
            "{frame:#?}"
        );
        assert_eq!(
            host.handle_key(OverlayKey::plain(OverlayKeyCode::Enter)),
            OverlayOutcome::Close
        );
        assert_eq!(
            result.lock().unwrap().clone().map(|r| r.goal),
            Some(Some("next".to_string()))
        );

        let result = Arc::new(Mutex::new(None));
        let mut host = HandoverPickerHost::new(open(Arc::new(NoRemote), None), result.clone());
        assert_eq!(
            host.handle_key(OverlayKey::ctrl(OverlayKeyCode::Char('c'))),
            OverlayOutcome::Close
        );
        assert!(host.should_close());
        assert_eq!(result.lock().unwrap().clone(), None);
    }
}
