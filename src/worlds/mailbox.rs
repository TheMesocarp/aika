use super::Message;
use tokio::sync::{
    mpsc::{channel, error, Receiver, Sender},
    watch,
};

/// A mailbox for agents to send and receive messages. WIP
pub struct Mailbox<'a> {
    tx: Sender<Message<'a>>,
    rx: Receiver<Message<'a>>,
    mailbox: Vec<Message<'a>>,
    pause_rx: watch::Receiver<bool>,
}

impl<'a> Mailbox<'a> {
    pub fn new(buffer_size: usize, pause_rx: watch::Receiver<bool>) -> Self {
        let (tx, rx) = channel(buffer_size);
        Mailbox {
            tx,
            rx,
            mailbox: Vec::new(),
            pause_rx,
        }
    }

    pub fn is_paused(&self) -> bool {
        *self.pause_rx.borrow()
    }

    pub async fn wait_for_resume(&mut self) {
        while self.is_paused() {
            self.pause_rx.changed().await.unwrap();
        }
    }

    pub async fn send(&self, msg: Message<'a>) -> Result<(), error::SendError<Message<'a>>> {
        self.tx.send(msg).await
    }

    pub async fn receive(&mut self) -> Option<Message<'a>> {
        self.rx.recv().await
    }

    pub fn peek_messages(&self) -> &[Message<'a>] {
        &self.mailbox
    }

    pub async fn collect_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.mailbox.push(msg);
        }
    }
}
