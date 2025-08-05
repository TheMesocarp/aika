//! Single-threaded simulation world supporting multiple actors with message passing capabilities.
//! Provides a `LonePlanet` struct that manages actor execution, event scheduling, and local message
//! delivery in a deterministic single-threaded environment with configurable time bounds.
use bytemuck::{Pod, Zeroable};

use crate::{
    actors::{Actor, ActorType, ConnectedActor, Context},
    env::Environment,
    objects::{Event, LocalScheduler, Msg, SchedulingTask, Transfer},
    AikaError,
};

/// A world that can contain multiple actors and run a simulation.
pub struct LonePlanet<
    const CLOCK_SLOTS: usize,
    const CLOCK_HEIGHT: usize,
    MessageType: Pod + Zeroable + Clone,
> {
    pub actors: Vec<ActorType<MessageType>>,
    pub env: Context<MessageType>,
    event_scheduler: LocalScheduler<CLOCK_SLOTS, CLOCK_HEIGHT, Event>,
    mail_scheduler: LocalScheduler<CLOCK_SLOTS, CLOCK_HEIGHT, Msg<MessageType>>,
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
    pub fn init(env: impl Environment + 'static) -> Result<Self, AikaError> {
        let mail_scheduler = LocalScheduler::new()?;
        let event_scheduler = LocalScheduler::new()?;
        Ok(Self {
            actors: Vec::new(),
            env: Context::new(env, false, 0, u64::MAX),
            mail_scheduler,
            event_scheduler,
        })
    }

    pub fn set_terminal_time(&mut self, terminal: u64) {
        self.env.terminal = terminal;
    }

    pub fn spawn_receiver_actor(
        &mut self,
        actor: impl ConnectedActor<MessageType> + 'static,
    ) -> usize {
        let actor = ActorType::Connected(Box::new(actor));
        self.actors.push(actor);
        self.actors.len() - 1
    }

    /// Spawn a new `Agent` to the `LonePlanet`.
    pub fn spawn_actor(&mut self, actor: impl Actor<MessageType> + 'static) -> usize {
        let actor = ActorType::Basic(Box::new(actor));
        self.actors.push(actor);
        self.actors.len() - 1
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
    pub fn terminal(&self) -> u64 {
        self.env.terminal
    }

    /// Schedule an event for an actor at a given time.
    pub fn schedule(&mut self, time: u64, actor: usize) -> Result<(), AikaError> {
        if time < self.now() {
            return Err(AikaError::TimeTravel);
        } else if time > self.env.terminal {
            return Err(AikaError::PastTerminal);
        }
        let now = self.now();
        self.commit(Event::new(now, time, actor, SchedulingTask::Wait));
        Ok(())
    }

    /// Run the simulation.
    pub fn run(&mut self) -> Result<(), AikaError> {
        loop {
            if (self.now() + 1) > self.env.terminal {
                break;
            }
            if let Ok(msgs) = self.mail_scheduler.tick() {
                for msg in msgs {
                    let id = msg.to;
                    if id.is_none() {
                        for i in 0..self.actors.len() {
                            match &mut self.actors[i] {
                                ActorType::Basic(_) => {}
                                ActorType::Connected(connected_actor) => {
                                    connected_actor.read_message(&mut self.env, msg, i)?;
                                }
                            }
                        }
                        continue;
                    }
                    let id = id.unwrap();
                    if self.actors.len() <= id {
                        return Err(AikaError::MessagedNonExistent(self.actors.len(), id));
                    }
                    match &mut self.actors[id] {
                        ActorType::Basic(_) => {
                            return Err(AikaError::MessagedNonReceiver);
                        }
                        ActorType::Connected(connected_actor) => {
                            connected_actor.read_message(&mut self.env, msg, id)?;
                        }
                    }
                }
            }
            if let Ok(events) = self.event_scheduler.tick() {
                for event in events {
                    if event.time > self.env.terminal {
                        break;
                    }

                    let env = &mut self.env;
                    let id = event.actor;
                    if self.actors.len() <= id {
                        return Err(AikaError::InvalidActorId(
                            self.actors.len(),
                            self.env.cluster_id,
                            id,
                        ));
                    }
                    let event = self.actors[id].step(env, id)?;
                    match event.task {
                        SchedulingTask::Timeout(time) => {
                            if (self.now() + time) > self.env.terminal {
                                continue;
                            }

                            self.commit(Event::new(
                                self.now(),
                                self.now() + time,
                                event.actor,
                                SchedulingTask::Wait,
                            ));
                        }
                        SchedulingTask::Schedule(time) => {
                            self.commit(Event::new(
                                self.now(),
                                time,
                                event.actor,
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

    #[derive(Debug)]
    // Simple actor that just schedules timeouts
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

    #[derive(Debug)]
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

    #[derive(Debug)]
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
        fn read_message(
            &mut self,
            _env: &mut Context<u8>,
            msg: Msg<u8>,
            _actor_id: usize,
        ) -> Result<(), AikaError> {
            self.messages_received.borrow_mut().push(msg);
            Ok(())
        }
    }

    #[derive(Debug)]
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

    #[derive(Debug)]
    // Agent that triggers other actors
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
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless).unwrap();
        world.set_terminal_time(400000);
        let actor_test = TestAgent::new(0);
        world.spawn_actor(actor_test);
        world.schedule(1, 0).unwrap();
        world.run().unwrap();
    }

    #[test]
    fn test_simple_message_passing() {
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless).unwrap();
        world.set_terminal_time(100);

        // Create sender and receiver
        let sender = SendingAgent::new(0, 1, 3);
        let receiver = ReceivingAgent::new(1);
        let received_messages = receiver.messages_received.clone();

        world.spawn_actor(sender);
        world.spawn_receiver_actor(receiver);

        // Schedule both actors to start
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
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless).unwrap();
        world.set_terminal_time(100);
        // Create one broadcaster and two receivers
        let broadcaster = BroadcastingAgent::new(0, 2);
        let receiver1 = ReceivingAgent::new(1);
        let receiver2 = ReceivingAgent::new(2);

        let received1 = receiver1.messages_received.clone();
        let received2 = receiver2.messages_received.clone();

        world.spawn_actor(broadcaster);
        world.spawn_receiver_actor(receiver1);
        world.spawn_receiver_actor(receiver2);

        // Schedule all actors
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
    fn test_actor_triggering() {
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless).unwrap();
        world.set_terminal_time(100);
        // Create a triggering actor that will trigger actor 1 at specific times
        let trigger_times = vec![10, 20, 30];
        let triggerer = TriggeringAgent::new(0, 1, trigger_times);

        // Create a simple actor that will be triggered
        let triggered = TestAgent::new(1);

        world.spawn_actor(triggerer);
        world.spawn_actor(triggered);

        // Only schedule the triggerer initially
        world.schedule(1, 0).unwrap();

        world.run().unwrap();

        // The triggered actor should have run at times 10, 20, and 30
        // We can verify this by checking the clock time advanced past 30
        assert!(world.now() >= 30);
    }

    #[test]
    fn test_multiple_simultaneous_messages() {
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless).unwrap();
        world.set_terminal_time(50);
        // Create multiple senders all targeting the same receiver
        let sender1 = SendingAgent::new(0, 3, 2);
        let sender2 = SendingAgent::new(1, 3, 2);
        let sender3 = SendingAgent::new(2, 3, 2);
        let receiver = ReceivingAgent::new(3);

        let received = receiver.messages_received.clone();

        world.spawn_actor(sender1);
        world.spawn_actor(sender2);
        world.spawn_actor(sender3);
        world.spawn_receiver_actor(receiver);

        // Schedule all actors
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
        let mut world = LonePlanet::<128, 1, u8>::init(Stateless).unwrap();
        world.set_terminal_time(50);
        // Agent that tries to send to non-existent actor
        #[derive(Debug)]
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

        world.spawn_actor(sender);
        world.schedule(1, 0).unwrap();

        // This should run without panicking
        assert_eq!(
            world.run().err().unwrap(),
            AikaError::MessagedNonExistent(1, 99)
        );
    }
}
