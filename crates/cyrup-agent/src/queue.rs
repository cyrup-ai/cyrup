//! Steering / follow-up queues + their drain modes (func-02 §9).

use crate::event::AgentMessage;
use std::collections::VecDeque;

/// Drain granularity for a queue (func-02 §9). Default `OneAtATime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum QueueMode {
    #[default]
    OneAtATime,
    All,
}

/// Global tool-execution preference (func-02 R-02-014). Default `Parallel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToolExecution {
    #[default]
    Parallel,
    Sequential,
}

/// A FIFO queue of pending steering/follow-up messages with a per-queue drain mode.
#[derive(Default)]
pub struct PendingQueue {
    items: VecDeque<AgentMessage>,
    mode: QueueMode,
}

impl PendingQueue {
    pub fn new(mode: QueueMode) -> Self {
        Self { items: VecDeque::new(), mode }
    }

    pub fn push(&mut self, m: AgentMessage) {
        self.items.push_back(m);
    }

    pub fn set_mode(&mut self, mode: QueueMode) {
        self.mode = mode;
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Drain per the current mode: the single oldest message, or all of them (func-02 R-02-034/035).
    pub fn drain(&mut self) -> Vec<AgentMessage> {
        match self.mode {
            QueueMode::OneAtATime => self.items.pop_front().into_iter().collect(),
            QueueMode::All => self.items.drain(..).collect(),
        }
    }

    /// Take everything (for abort restore, func-02 R-02-037), ignoring mode.
    pub fn take_all(&mut self) -> Vec<AgentMessage> {
        self.items.drain(..).collect()
    }
}
