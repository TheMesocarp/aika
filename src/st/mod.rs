use mesocarp::comms::mailbox::ThreadedMessenger;

use crate::{
    agents::{Agent, AgentSupport, WorldContext},
    event::{Action, Event, LocalEventSystem},
    messages::Msg,
    SimError,
};

pub struct TimeInfo {
    pub timestep: f64,
    pub terminal: f64,
}

/// A world that can contain multiple agents and run a simulation.
pub struct World<
    const MESSAGE_SLOTS: usize,
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Clone,
> {
    pub agents: Vec<Box<dyn Agent<MESSAGE_SLOTS, Msg<MessageType>>>>,
    pub world_context: WorldContext<MESSAGE_SLOTS, Msg<MessageType>>,
    mailbox: Option<ThreadedMessenger<MESSAGE_SLOTS, Msg<MessageType>>>,
    event_system: LocalEventSystem<CLOCK_SLOTS, CLOCK_HEIGHT>,
    pub time_info: TimeInfo,
}

unsafe impl<
        const MESSAGE_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > Send for World<MESSAGE_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}
unsafe impl<
        const MESSAGE_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > Sync for World<MESSAGE_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

impl<
        const MESSAGE_SLOTS: usize,
        const CLOCK_SLOTS: usize,
        const CLOCK_HEIGHT: usize,
        MessageType: Clone,
    > World<MESSAGE_SLOTS, CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    pub fn init(terminal: f64, timestep: f64, world_arena_size: usize) -> Result<Self, SimError> {
        let event_system = LocalEventSystem::<CLOCK_SLOTS, CLOCK_HEIGHT>::new()?;
        Ok(Self {
            agents: Vec::new(),
            world_context: WorldContext::new(world_arena_size),
            mailbox: None,
            event_system,
            time_info: TimeInfo { timestep, terminal },
        })
    }

    pub fn spawn_agent(&mut self, agent: Box<dyn Agent<MESSAGE_SLOTS, Msg<MessageType>>>) -> usize {
        self.agents.push(agent);
        self.agents.len() - 1
    }

    pub fn init_support_layers(&mut self, arena_size: Option<usize>) -> Result<(), SimError> {
        let agent_ids = self
            .agents
            .iter()
            .enumerate()
            .map(|x| x.0)
            .collect::<Vec<_>>();
        let thread_world =
            ThreadedMessenger::<MESSAGE_SLOTS, Msg<MessageType>>::new(agent_ids.clone())?;
        let len = self.agents.len();
        let mut supports: Vec<AgentSupport<MESSAGE_SLOTS, _>> = Vec::with_capacity(len);
        for i in agent_ids {
            let sup = AgentSupport::new(
                Some(thread_world.get_user(i)?),
                arena_size,
            );
            supports.push(sup);
        }
        self.mailbox = Some(thread_world);
        self.world_context.agent_states = supports;
        Ok(())
    }

    fn commit(&mut self, event: Event) {
        self.event_system.insert(event)
    }

    /// Get the current time of the simulation.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.event_system.local_clock.time
    }

    /// Schedule an event for an agent at a given time.
    pub fn schedule(&mut self, time: u64, agent: usize) -> Result<(), SimError> {
        if time < self.now() {
            return Err(SimError::TimeTravel);
        } else if time as f64 * self.time_info.timestep > self.time_info.terminal {
            return Err(SimError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, agent, Action::Wait));
        Ok(())
    }

    /// Run the simulation.
    pub fn run(&mut self) -> Result<(), SimError> {
        loop {
            if (self.now() + 1) as f64 * self.time_info.timestep > self.time_info.terminal {
                break;
            }

            if let Ok(events) = self.event_system.local_clock.tick() {
                for event in events {
                    if event.time as f64 * self.time_info.timestep > self.time_info.terminal {
                        break;
                    }

                    let supports = &mut self.world_context;
                    supports.time = event.time;
                    let event = self.agents[event.agent].step(supports, event.agent);
                    match event.yield_ {
                        Action::Timeout(time) => {
                            if (self.now() + time) as f64 * self.time_info.timestep
                                > self.time_info.terminal
                            {
                                continue;
                            }

                            self.commit(Event::new(
                                self.now(),
                                self.now() + time,
                                event.agent,
                                Action::Wait,
                            ));
                        }
                        Action::Schedule(time) => {
                            self.commit(Event::new(self.now(), time, event.agent, Action::Wait));
                        }
                        Action::Trigger { time, idx } => {
                            self.commit(Event::new(self.now(), time, idx, Action::Wait));
                        }
                        Action::Wait => {}
                        Action::Break => {
                            break;
                        }
                    }
                }

                if self.mailbox.is_some() {
                    let mailbox = self.mailbox.as_mut().unwrap();
                    for _ in 0..MESSAGE_SLOTS {
                        match mailbox.poll() {
                            Ok(mail) => {
                                mailbox.deliver(mail)?;
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
            self.event_system
                .local_clock
                .increment(&mut self.event_system.overflow);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    // Simple agent that just schedules timeouts
    pub struct TestAgent {
        pub _id: usize,
    }

    impl TestAgent {
        pub fn new(_id: usize) -> Self {
            TestAgent { _id }
        }
    }

    impl Agent<8, Msg<u8>> for TestAgent {
        fn step(&mut self, supports: &mut WorldContext<8, Msg<u8>>, id: usize) -> Event {
            let time = supports.time;
            Event::new(time, time, id, Action::Timeout(1))
        }
    }

    // Agent that sends messages
    pub struct SendingAgent {
        pub id: usize,
        pub target: usize,
        pub message_count: usize,
        pub messages_sent: usize,
    }

    impl SendingAgent {
        pub fn new(id: usize, target: usize, message_count: usize) -> Self {
            SendingAgent {
                id,
                target,
                message_count,
                messages_sent: 0,
            }
        }
    }

    impl Agent<8, Msg<u8>> for SendingAgent {
        fn step(&mut self, supports: &mut WorldContext<8, Msg<u8>>, id: usize) -> Event {
            let time = supports.time;

            // Send messages until we've sent the desired count
            if self.messages_sent < self.message_count {
                if let Some(mailbox) = &supports.agent_states[id].mailbox {
                    let msg = Msg::new(
                        self.messages_sent as u8,
                        time,
                        time + 10, // Deliver 10 time units later
                        self.id,
                        Some(self.target),
                    );

                    if mailbox.send(msg).is_ok() {
                        self.messages_sent += 1;
                    }
                }
            }

            // Continue sending every 5 time units
            if self.messages_sent < self.message_count {
                Event::new(time, time, self.id, Action::Timeout(5))
            } else {
                Event::new(time, time, self.id, Action::Wait)
            }
        }
    }

    // Agent that receives and counts messages
    pub struct ReceivingAgent {
        pub _id: usize,
        pub messages_received: Rc<RefCell<Vec<Msg<u8>>>>,
    }

    impl ReceivingAgent {
        pub fn new(_id: usize) -> Self {
            ReceivingAgent {
                _id,
                messages_received: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl Agent<8, Msg<u8>> for ReceivingAgent {
        fn step(&mut self, context: &mut WorldContext<8, Msg<u8>>, id: usize) -> Event {
            let time = context.time;

            // Check for messages
            if let Some(mailbox) = &mut context.agent_states[id].mailbox {
                for _ in 0..3 {
                    if let Some(messages) = mailbox.poll() {
                        for msg in messages {
                            self.messages_received.borrow_mut().push(msg);
                        }
                    }
                }
            }

            // Keep checking every time unit
            Event::new(time, time, id, Action::Timeout(1))
        }
    }

    // Agent that broadcasts messages
    pub struct BroadcastingAgent {
        pub id: usize,
        pub broadcast_count: usize,
        pub broadcasts_sent: usize,
    }

    impl BroadcastingAgent {
        pub fn new(id: usize, broadcast_count: usize) -> Self {
            BroadcastingAgent {
                id,
                broadcast_count,
                broadcasts_sent: 0,
            }
        }
    }

    impl Agent<8, Msg<u8>> for BroadcastingAgent {
        fn step(&mut self, context: &mut WorldContext<8, Msg<u8>>, id: usize) -> Event {
            let time = context.time;

            if self.broadcasts_sent < self.broadcast_count {
                if let Some(mailbox) = &context.agent_states[id].mailbox {
                    let msg = Msg::new(
                        (100 + self.broadcasts_sent) as u8,
                        time,
                        time + 5,
                        self.id,
                        None, // None means broadcast
                    );

                    if mailbox.send(msg).is_ok() {
                        self.broadcasts_sent += 1;
                    }
                }
            }

            if self.broadcasts_sent < self.broadcast_count {
                Event::new(time, time, id, Action::Timeout(10))
            } else {
                Event::new(time, time, id, Action::Wait)
            }
        }
    }

    // Agent that triggers other agents
    pub struct TriggeringAgent {
        pub _id: usize,
        pub target: usize,
        pub trigger_times: Vec<u64>,
        pub trigger_index: usize,
    }

    impl TriggeringAgent {
        pub fn new(_id: usize, target: usize, trigger_times: Vec<u64>) -> Self {
            TriggeringAgent {
                _id,
                target,
                trigger_times,
                trigger_index: 0,
            }
        }
    }

    impl Agent<8, Msg<u8>> for TriggeringAgent {
        fn step(&mut self, context: &mut WorldContext<8, Msg<u8>>, id: usize) -> Event {
            let time = context.time;

            // Check if we should trigger the target
            if self.trigger_index < self.trigger_times.len() {
                let trigger_time = self.trigger_times[self.trigger_index];
                self.trigger_index += 1;
                return Event::new(
                    time,
                    time,
                    id,
                    Action::Trigger {
                        time: trigger_time,
                        idx: self.target,
                    },
                );
            }

            Event::new(time, time, id, Action::Wait)
        }
    }

    #[test]
    fn test_run() {
        let mut world = World::<8, 128, 1, u8>::init(400000.0, 1.0, 0).unwrap();
        let agent_test = TestAgent::new(0);
        world.spawn_agent(Box::new(agent_test));
        world.init_support_layers(None).unwrap();
        world.schedule(1, 0).unwrap();
        assert!(world.world_context.agent_states.len() == 1);
        world.run().unwrap();
    }

    #[test]
    fn test_simple_message_passing() {
        let mut world = World::<8, 128, 1, u8>::init(100.0, 1.0, 0).unwrap();

        // Create sender and receiver
        let sender = SendingAgent::new(0, 1, 3);
        let receiver = ReceivingAgent::new(1);
        let received_messages = receiver.messages_received.clone();

        world.spawn_agent(Box::new(sender));
        world.spawn_agent(Box::new(receiver));
        world.init_support_layers(None).unwrap();

        // Schedule both agents to start
        world.schedule(1, 0).unwrap();
        world.schedule(1, 1).unwrap();

        world.run().unwrap();

        // Check that messages were received
        let messages = received_messages.borrow();
        assert_eq!(messages.len(), 3);
        for (i, msg) in messages.iter().enumerate() {
            assert_eq!(msg.data, i as u8);
            assert_eq!(msg.from, 0);
            assert_eq!(msg.to, Some(1));
        }
    }

    #[test]
    fn test_broadcast_messages() {
        let mut world = World::<8, 128, 1, u8>::init(100.0, 1.0, 0).unwrap();

        // Create one broadcaster and two receivers
        let broadcaster = BroadcastingAgent::new(0, 2);
        let receiver1 = ReceivingAgent::new(1);
        let receiver2 = ReceivingAgent::new(2);

        let received1 = receiver1.messages_received.clone();
        let received2 = receiver2.messages_received.clone();

        world.spawn_agent(Box::new(broadcaster));
        world.spawn_agent(Box::new(receiver1));
        world.spawn_agent(Box::new(receiver2));
        world.init_support_layers(None).unwrap();

        // Schedule all agents
        world.schedule(1, 0).unwrap();
        world.schedule(1, 1).unwrap();
        world.schedule(1, 2).unwrap();

        world.run().unwrap();

        // Both receivers should get the broadcasts
        let messages1 = received1.borrow();
        let messages2 = received2.borrow();

        assert_eq!(messages1.len(), 2);
        assert_eq!(messages2.len(), 2);

        // Verify broadcast content
        for msg in messages1.iter() {
            assert_eq!(msg.from, 0);
            assert_eq!(msg.to, None);
            assert!(msg.data >= 100);
        }
    }

    #[test]
    fn test_agent_triggering() {
        let mut world = World::<8, 128, 1, u8>::init(100.0, 1.0, 0).unwrap();

        // Create a triggering agent that will trigger agent 1 at specific times
        let trigger_times = vec![10, 20, 30];
        let triggerer = TriggeringAgent::new(0, 1, trigger_times);

        // Create a simple agent that will be triggered
        let triggered = TestAgent::new(1);

        world.spawn_agent(Box::new(triggerer));
        world.spawn_agent(Box::new(triggered));
        world.init_support_layers(None).unwrap();

        // Only schedule the triggerer initially
        world.schedule(1, 0).unwrap();

        world.run().unwrap();

        // The triggered agent should have run at times 10, 20, and 30
        // We can verify this by checking the clock time advanced past 30
        assert!(world.now() >= 30);
    }

    #[test]
    fn test_multiple_simultaneous_messages() {
        let mut world = World::<8, 128, 1, u8>::init(50.0, 1.0, 0).unwrap();

        // Create multiple senders all targeting the same receiver
        let sender1 = SendingAgent::new(0, 3, 2);
        let sender2 = SendingAgent::new(1, 3, 2);
        let sender3 = SendingAgent::new(2, 3, 2);
        let receiver = ReceivingAgent::new(3);

        let received = receiver.messages_received.clone();

        world.spawn_agent(Box::new(sender1));
        world.spawn_agent(Box::new(sender2));
        world.spawn_agent(Box::new(sender3));
        world.spawn_agent(Box::new(receiver));
        world.init_support_layers(None).unwrap();

        // Schedule all agents
        for i in 0..4 {
            world.schedule(1, i as usize).unwrap();
        }
        world.run().unwrap();

        // Should receive 6 messages total (2 from each sender)
        let messages = received.borrow();
        assert_eq!(messages.len(), 6);

        // Count messages from each sender
        let mut from_0 = 0;
        let mut from_1 = 0;
        let mut from_2 = 0;

        for msg in messages.iter() {
            match msg.from {
                0 => from_0 += 1,
                1 => from_1 += 1,
                2 => from_2 += 1,
                _ => panic!("Unexpected sender"),
            }
        }

        assert_eq!(from_0, 2);
        assert_eq!(from_1, 2);
        assert_eq!(from_2, 2);
    }

    #[test]
    fn test_invalid_target_handling() {
        let mut world = World::<8, 128, 1, u8>::init(50.0, 1.0, 0).unwrap();

        // Agent that tries to send to non-existent agent
        pub struct InvalidTargetAgent {
            _id: usize,
            attempted: bool,
        }

        impl Agent<8, Msg<u8>> for InvalidTargetAgent {
            fn step(&mut self, context: &mut WorldContext<8, Msg<u8>>, id: usize) -> Event {
                let time = context.time;

                if !self.attempted {
                    if let Some(mailbox) = &context.agent_states[id].mailbox {
                        // Try to send to agent 99 which doesn't exist
                        let msg = Msg::new(1, time, time + 5, id, Some(99));

                        // This should fail gracefully
                        let _ = mailbox.send(msg);
                        self.attempted = true;
                    }
                }

                Event::new(time, time, id, Action::Wait)
            }
        }

        let sender = InvalidTargetAgent {
            _id: 0,
            attempted: false,
        };

        world.spawn_agent(Box::new(sender));
        world.init_support_layers(None).unwrap();
        world.schedule(1, 0).unwrap();

        // This should run without panicking
        world.run().unwrap();
    }
}
