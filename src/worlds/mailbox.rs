use super::Message;

/// A mailbox for agents to send and receive messages. WIP
pub struct Mailbox {
    messages: Vec<Vec<Message>>,
}

impl Mailbox {
    pub fn new(agent_count: usize) -> Self {
        Mailbox {
            messages: Vec::with_capacity(agent_count),
        }
    }

    pub fn send(&mut self, msg: Message) {
        self.messages[msg.to].push(msg);
    }

    pub fn receive(&mut self, id: usize) -> Vec<Message> {
        self.messages[id].drain(..).collect()
    }

    pub fn peek_messages(&self, id: usize) -> &[Message] {
        &self.messages[id]
    }
}
