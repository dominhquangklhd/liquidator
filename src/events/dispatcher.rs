use tokio::sync::mpsc;
use crate::events::event::Event;

pub struct Dispatcher {
    sender: mpsc::Sender<Event>,
}

impl Dispatcher {
    pub fn new(sender: mpsc::Sender<Event>) -> Self {
        Self { sender }
    }

    pub async fn publish(&self, event: Event) {
        if let Err(e) = self.sender.send(event).await {
            tracing::error!("Failed to send event: {:?}", e);
        }
    }
}
