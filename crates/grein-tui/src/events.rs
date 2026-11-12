use crossterm::event::KeyEvent;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::{interval_at, Instant};

/// Events produced by external agent streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    Output { agent_id: String, content: String },
    StatusUpdate { status: String },
}

/// Multiplexed application events handled by the TUI event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Tick,
    Key(KeyEvent),
    Agent(AgentEvent),
}

/// Asynchronous event multiplexer handling ticks, user input, and agent streams.
#[derive(Debug)]
pub struct EventHandler {
    receiver: UnboundedReceiver<Event>,
    sender: UnboundedSender<Event>,
    tick_task: JoinHandle<()>,
}

impl EventHandler {
    /// Creates a new EventHandler with the given periodic tick duration.
    pub fn new(tick_rate: Duration) -> Self {
        let (sender, receiver) = unbounded_channel();
        let tick_sender = sender.clone();

        let tick_task = tokio::spawn(async move {
            let mut interval = interval_at(Instant::now() + tick_rate, tick_rate);
            loop {
                interval.tick().await;
                if tick_sender.send(Event::Tick).is_err() {
                    break;
                }
            }
        });

        Self {
            receiver,
            sender,
            tick_task,
        }
    }

    /// Returns a cloned sender to produce events into the multiplexer.
    pub fn sender(&self) -> UnboundedSender<Event> {
        self.sender.clone()
    }

    /// Receives the next multiplexed event from the channel.
    pub async fn next(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }
}

impl Drop for EventHandler {
    fn drop(&mut self) {
        self.tick_task.abort();
    }
}
