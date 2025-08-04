//! Single-threaded simulation world supporting multiple agents with message passing capabilities.
//! Provides a `LonePlanet` struct that manages agent execution, event scheduling, and local message
//! delivery in a deterministic single-threaded environment with configurable time bounds.
use bytemuck::{Pod, Zeroable};

use crate::{
    actors::{Actor, ConnectedActor, Context},
    env::Environment,
    objects::{Event, LocalScheduler, Msg, SchedulingTask, Transfer},
    AikaError,
};

pub(crate) struct TimeInfo {
    pub timestep: f64,
    pub terminal: f64,
}

/// A world that can contain multiple agents and run a simulation.
pub struct LonePlanet<
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub actors: Vec<Box<dyn Actor<MessageType>>>,
    pub connected_actors: Vec<Box<dyn ConnectedActor<MessageType>>>,
    pub env: Context<MessageType>,
    event_scheduler: LocalScheduler<CLOCK_SLOTS, CLOCK_HEIGHT, Event>,
    mail_scheduler: LocalScheduler<CLOCK_SLOTS, CLOCK_HEIGHT, Msg<MessageType>>,
    time_info: TimeInfo,
    connected: Vec<(bool, usize)>,
}

unsafe impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Pod + Zeroable + Clone>
    Send for LonePlanet<CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}
unsafe impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Pod + Zeroable + Clone>
    Sync for LonePlanet<CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
}

impl<const CLOCK_SLOTS: usize, const CLOCK_HEIGHT: usize, MessageType: Pod + Zeroable + Clone>
    LonePlanet<CLOCK_SLOTS, CLOCK_HEIGHT, MessageType>
{
    /// Initialize a new world with the provided time information and world state arena allocation size
    pub fn init(
        env: impl Environment + 'static,
        terminal: f64,
        timestep: f64,
    ) -> Result<Self, AikaError> {
        let mail_scheduler = LocalScheduler::new()?;
        let event_scheduler = LocalScheduler::new()?;
        let term = (terminal / timestep) as u64;
        Ok(Self {
            actors: Vec::new(),
            connected_actors: Vec::new(),
            env: Context::new(env, false, 0, term),
            mail_scheduler,
            event_scheduler,
            time_info: TimeInfo { timestep, terminal },
            connected: Vec::new(),
        })
    }

    pub fn spawn_connected_actor(&mut self, agent: Box<dyn ConnectedActor<MessageType>>) -> usize {
        self.connected_actors.push(agent);
        self.connected.push((true, self.connected_actors.len() - 1));
        self.actors.len() + self.connected_actors.len() - 1
    }

    /// Spawn a new `Agent` to the `LonePlanet`.
    pub fn spawn_actor(&mut self, agent: Box<dyn Actor<MessageType>>) -> usize {
        self.actors.push(agent);
        self.connected.push((false, self.actors.len() - 1));
        self.actors.len() + self.connected_actors.len() - 1
    }

    fn commit(&mut self, event: Event) {
        self.event_scheduler.insert(event)
    }

    fn commit_mail(&mut self, mail: Msg<MessageType>) {
        self.mail_scheduler.insert(mail);
    }

    /// Get the current time of the simulation.
    #[inline(always)]
    pub fn now(&self) -> u64 {
        self.event_scheduler.now()
    }

    /// Get the time information of the simulation.
    pub fn time_info(&self) -> (f64, f64) {
        (self.time_info.timestep, self.time_info.terminal)
    }

    /// Schedule an event for an agent at a given time.
    pub fn schedule(&mut self, time: u64, agent: usize) -> Result<(), AikaError> {
        if time < self.now() {
            return Err(AikaError::TimeTravel);
        } else if time as f64 * self.time_info.timestep > self.time_info.terminal {
            return Err(AikaError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, agent, SchedulingTask::Wait));
        Ok(())
    }

    /// Run the simulation.
    pub fn run(&mut self) -> Result<(), AikaError> {
        loop {
            if (self.now() + 1) as f64 * self.time_info.timestep > self.time_info.terminal {
                break;
            }
            if let Ok(msgs) = self.mail_scheduler.tick() {
                for msg in msgs {
                    let id = msg.to;
                    if id.is_none() {
                        let count = self.connected_actors.len();
                        for i in 0..count {
                            self.connected_actors[i].read_message(&mut self.env, msg, i);
                        }
                        continue;
                    }
                    let id = id.unwrap();
                    if self.connected.len() <= id {
                        return Err(AikaError::MessagedNonExistent(self.connected.len(), id));
                    }
                    let status = self.connected[id];
                    if status.0 {
                        self.connected_actors[status.1].read_message(&mut self.env, msg, id);
                    } else {
                        return Err(AikaError::MessagedAnUnreachableActor);
                    }
                }
            }
            if let Ok(events) = self.event_scheduler.tick() {
                for event in events {
                    if event.time as f64 * self.time_info.timestep > self.time_info.terminal {
                        break;
                    }

                    let env = &mut self.env;
                    let id = event.agent;
                    let status = self.connected[id];
                    let event = if status.0 {
                        self.connected_actors[status.1].step(env, id)?
                    } else {
                        self.actors[status.1].step(env, id)?
                    };
                    match event.yield_ {
                        SchedulingTask::Timeout(time) => {
                            if (self.now() + time) as f64 * self.time_info.timestep
                                > self.time_info.terminal
                            {
                                continue;
                            }

                            self.commit(Event::new(
                                self.now(),
                                self.now() + time,
                                event.agent,
                                SchedulingTask::Wait,
                            ));
                        }
                        SchedulingTask::Schedule(time) => {
                            self.commit(Event::new(
                                self.now(),
                                time,
                                event.agent,
                                SchedulingTask::Wait,
                            ));
                        }
                        SchedulingTask::Trigger { time, idx } => {
                            self.commit(Event::new(self.now(), time, idx, SchedulingTask::Wait));
                        }
                        SchedulingTask::Wait => {}
                        SchedulingTask::Break => {
                            break;
                        }
                    }
                }
            }

            let sends = std::mem::take(&mut self.env.outbox);
            for msg in sends {
                if msg.to_world.is_some() {
                    if Some(self.env.cluster_id) == msg.to_world {
                        match msg.open_letter() {
                            Transfer::Msg(msg) => self.commit_mail(msg),
                            Transfer::AntiMsg(_) => return Err(AikaError::ThreadPanic),
                        }
                        continue;
                    }
                } else {
                    match msg.open_letter() {
                        Transfer::Msg(msg) => self.commit_mail(msg),
                        Transfer::AntiMsg(_) => return Err(AikaError::ThreadPanic),
                    }
                }
            }

            self.event_scheduler.increment();
            self.mail_scheduler.increment();
            self.env.time += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::actors::ConnectedActor;
    use crate::env::Stateless;

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

    impl Actor<u8> for TestAgent {
        fn step(&mut self, supports: &mut Context<u8>, id: usize) -> Result<Event, AikaError> {
            let time = supports.time;
            Ok(Event::new(time, time, id, SchedulingTask::Timeout(1)))
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

    impl Actor<u8> for SendingAgent {
        fn step(&mut self, env: &mut Context<u8>, _id: usize) -> Result<Event, AikaError> {
            let time = env.time;

            // Send messages until we've sent the desired count
            if self.messages_sent < self.message_count {
                let msg = Msg::new(
                    self.messages_sent as u8,
                    time,
                    time + 10, // Deliver 10 time units later
                    self.id,
                    Some(self.target),
                );
                env.send_mail(msg, 0)?;
                self.messages_sent += 1;
            }

            // Continue sending every 5 time units
            if self.messages_sent < self.message_count {
                Ok(Event::new(time, time, self.id, SchedulingTask::Timeout(5)))
            } else {
                Ok(Event::new(time, time, self.id, SchedulingTask::Wait))
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

    impl Actor<u8> for ReceivingAgent {
        fn step(&mut self, context: &mut Context<u8>, id: usize) -> Result<Event, AikaError> {
            Ok(Event::new(
                context.time,
                context.time,
                id,
                SchedulingTask::Wait,
            ))
        }
    }

    impl ConnectedActor<u8> for ReceivingAgent {
        fn read_message(&mut self, _env: &mut Context<u8>, msg: Msg<u8>, _actor_id: usize) {
            self.messages_received.borrow_mut().push(msg);
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

    impl Actor<u8> for BroadcastingAgent {
        fn step(&mut self, context: &mut Context<u8>, id: usize) -> Result<Event, AikaError> {
            let time = context.time;

            if self.broadcasts_sent < self.broadcast_count {
                let msg = Msg::new(
                    (100 + self.broadcasts_sent) as u8,
                    time,
                    time + 5,
                    self.id,
                    None, // None means broadcast
                );
                context.send_mail(msg, 0)?;
                self.broadcasts_sent += 1;
            }

            if self.broadcasts_sent < self.broadcast_count {
                Ok(Event::new(time, time, id, SchedulingTask::Timeout(10)))
            } else {
                Ok(Event::new(time, time, id, SchedulingTask::Wait))
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

    impl Actor<u8> for TriggeringAgent {
        fn step(&mut self, context: &mut Context<u8>, id: usize) -> Result<Event, AikaError> {
            let time = context.time;

            // Check if we should trigger the target
            if self.trigger_index < self.trigger_times.len() {
                let trigger_time = self.trigger_times[self.trigger_index];
                self.trigger_index += 1;
                return Ok(Event::new(
                    time,
                    time,
                    id,
                    SchedulingTask::Trigger {
                        time: trigger_time,
                        idx: self.target,
                    },
                ));
            }

            Ok(Event::new(time, time, id, SchedulingTask::Wait))
        }
    }

    #[test]
    fn test_run() {
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless, 400000.0, 1.0).unwrap();
        let agent_test = TestAgent::new(0);
        world.spawn_actor(Box::new(agent_test));
        world.schedule(1, 0).unwrap();
        world.run().unwrap();
    }

    #[test]
    fn test_simple_message_passing() {
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless, 100.0, 1.0).unwrap();

        // Create sender and receiver
        let sender = SendingAgent::new(0, 1, 3);
        let receiver = ReceivingAgent::new(1);
        let received_messages = receiver.messages_received.clone();

        world.spawn_actor(Box::new(sender));
        world.spawn_connected_actor(Box::new(receiver));

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
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless, 100.0, 1.0).unwrap();

        // Create one broadcaster and two receivers
        let broadcaster = BroadcastingAgent::new(0, 2);
        let receiver1 = ReceivingAgent::new(1);
        let receiver2 = ReceivingAgent::new(2);

        let received1 = receiver1.messages_received.clone();
        let received2 = receiver2.messages_received.clone();

        world.spawn_actor(Box::new(broadcaster));
        world.spawn_connected_actor(Box::new(receiver1));
        world.spawn_connected_actor(Box::new(receiver2));

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
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless, 100.0, 1.0).unwrap();

        // Create a triggering agent that will trigger agent 1 at specific times
        let trigger_times = vec![10, 20, 30];
        let triggerer = TriggeringAgent::new(0, 1, trigger_times);

        // Create a simple agent that will be triggered
        let triggered = TestAgent::new(1);

        world.spawn_actor(Box::new(triggerer));
        world.spawn_actor(Box::new(triggered));

        // Only schedule the triggerer initially
        world.schedule(1, 0).unwrap();

        world.run().unwrap();

        // The triggered agent should have run at times 10, 20, and 30
        // We can verify this by checking the clock time advanced past 30
        assert!(world.now() >= 30);
    }

    #[test]
    fn test_multiple_simultaneous_messages() {
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless, 50.0, 1.0).unwrap();

        // Create multiple senders all targeting the same receiver
        let sender1 = SendingAgent::new(0, 3, 2);
        let sender2 = SendingAgent::new(1, 3, 2);
        let sender3 = SendingAgent::new(2, 3, 2);
        let receiver = ReceivingAgent::new(3);

        let received = receiver.messages_received.clone();

        world.spawn_actor(Box::new(sender1));
        world.spawn_actor(Box::new(sender2));
        world.spawn_actor(Box::new(sender3));
        world.spawn_connected_actor(Box::new(receiver));

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
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless, 50.0, 1.0).unwrap();

        // Agent that tries to send to non-existent agent
        pub struct InvalidTargetAgent {
            _id: usize,
            attempted: bool,
        }

        impl Actor<u8> for InvalidTargetAgent {
            fn step(&mut self, context: &mut Context<u8>, id: usize) -> Result<Event, AikaError> {
                let time = context.time;
                self.attempted = true;
                let msg = Msg::new(1, time, time + 5, id, Some(99));
                context.send_mail(msg, 0)?;

                Ok(Event::new(time, time, id, SchedulingTask::Wait))
            }
        }

        let sender = InvalidTargetAgent {
            _id: 0,
            attempted: false,
        };

        world.spawn_actor(Box::new(sender));
        world.schedule(1, 0).unwrap();

        // This should run without panicking
        assert_eq!(
            world.run().err().unwrap(),
            AikaError::MessagedNonExistent(1, 99)
        );
    }
}
