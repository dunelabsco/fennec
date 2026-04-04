pub mod events;
pub use events::{InboundMessage, OutboundMessage};

use tokio::sync::mpsc;

#[derive(Clone)]
pub struct MessageBus {
    inbound_tx: mpsc::Sender<InboundMessage>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
}

pub struct BusReceiver {
    pub inbound_rx: mpsc::Receiver<InboundMessage>,
    pub outbound_rx: mpsc::Receiver<OutboundMessage>,
}

impl MessageBus {
    pub fn new(buffer: usize) -> (Self, BusReceiver) {
        let (inbound_tx, inbound_rx) = mpsc::channel(buffer);
        let (outbound_tx, outbound_rx) = mpsc::channel(buffer);
        (
            Self {
                inbound_tx,
                outbound_tx,
            },
            BusReceiver {
                inbound_rx,
                outbound_rx,
            },
        )
    }

    pub async fn publish_inbound(&self, msg: InboundMessage) -> anyhow::Result<()> {
        self.inbound_tx
            .send(msg)
            .await
            .map_err(|e| anyhow::anyhow!("Bus send error: {}", e))
    }

    pub async fn publish_outbound(&self, msg: OutboundMessage) -> anyhow::Result<()> {
        self.outbound_tx
            .send(msg)
            .await
            .map_err(|e| anyhow::anyhow!("Bus send error: {}", e))
    }

    pub fn inbound_sender(&self) -> mpsc::Sender<InboundMessage> {
        self.inbound_tx.clone()
    }

    pub fn outbound_sender(&self) -> mpsc::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }
}
