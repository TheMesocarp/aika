use super::Message;
use tokio::sync::mpsc::{channel, error, Receiver, Sender};

/// A mailbox for agents to send and receive messages. WIP
pub struct Mailbox<'a> {
    tx: Sender<Message<'a>>,
    rx: Receiver<Message<'a>>,
    mailbox: Vec<Message<'a>>,
}

impl<'a> Mailbox<'a> {
    pub fn new(buffer_size: usize) -> Self {
        let (tx, rx) = channel(buffer_size);
        Mailbox {
            tx,
            rx,
            mailbox: Vec::new(),
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
