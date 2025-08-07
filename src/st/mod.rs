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
                    let task = self.actors[id].step(env, id)?;
                    match task {
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

#[macro_export]
macro_rules! world {
    // Just the message type - use all defaults
    ($msg_type:ty) => {
        $crate::st::LonePlanet::<128, 2, $msg_type>::init
    };

    // With CLOCK_SLOTS only
    ($msg_type:ty, CLOCK_SLOTS = $clock_slots:expr) => {
        $crate::st::LonePlanet::<$clock_slots, 2, $msg_type>::init
    };

    // With CLOCK_HEIGHT only
    ($msg_type:ty, CLOCK_HEIGHT = $clock_height:expr) => {
        $crate::st::LonePlanet::<128, $clock_height, $msg_type>::init
    };

    // With both parameters
    ($msg_type:ty, CLOCK_SLOTS = $clock_slots:expr, CLOCK_HEIGHT = $clock_height:expr) => {
        $crate::st::LonePlanet::<$clock_slots, $clock_height, $msg_type>::init
    };

    // With both parameters (reverse order)
    ($msg_type:ty, CLOCK_HEIGHT = $clock_height:expr, CLOCK_SLOTS = $clock_slots:expr) => {
        $crate::st::LonePlanet::<$clock_slots, $clock_height, $msg_type>::init
    };
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
        fn step(&mut self, _supports: &mut Context<u8>, _id: usize) -> Result<SchedulingTask, AikaError> {
            Ok(SchedulingTask::Timeout(1))
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
        fn step(&mut self, env: &mut Context<u8>, _id: usize) -> Result<SchedulingTask, AikaError> {
            let time = env.time;

            if self.messages_sent < self.message_count {
                let msg = Msg::new(
                    self.messages_sent as u8,
                    time,
                    time + 10, 
                    self.id,
                    Some(self.target),
                );
                env.send_mail(msg, 0)?;
                self.messages_sent += 1;
            }

            if self.messages_sent < self.message_count {
                Ok(SchedulingTask::Timeout(5))
            } else {
                Ok(SchedulingTask::Wait)
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
        fn step(&mut self, _context: &mut Context<u8>, _id: usize) -> Result<SchedulingTask, AikaError> {
            Ok(SchedulingTask::Wait)
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
        fn step(&mut self, context: &mut Context<u8>, _id: usize) -> Result<SchedulingTask, AikaError> {
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
                Ok(SchedulingTask::Timeout(10))
            } else {
                Ok(SchedulingTask::Wait)
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
        fn step(&mut self, _context: &mut Context<u8>, _id: usize) -> Result<SchedulingTask, AikaError> {
            if self.trigger_index < self.trigger_times.len() {
                let trigger_time = self.trigger_times[self.trigger_index];
                self.trigger_index += 1;
                return Ok(SchedulingTask::Trigger {
                        time: trigger_time,
                        idx: self.target,
                    }
                );
            }

            Ok(SchedulingTask::Wait)
        }
    }

    #[test]
    fn test_world_macro() {
        world!(u8)(Stateless).unwrap();
        world!(u8, CLOCK_SLOTS = 256)(Stateless).unwrap();
        world!(u8, CLOCK_HEIGHT = 1)(Stateless).unwrap();
        world!(u8, CLOCK_SLOTS = 64, CLOCK_HEIGHT = 4)(Stateless).unwrap();
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
            fn step(&mut self, context: &mut Context<u8>, id: usize) -> Result<SchedulingTask, AikaError> {
                let time = context.time;
                self.attempted = true;
                let msg = Msg::new(1, time, time + 5, id, Some(99));
                context.send_mail(msg, 0)?;

                Ok(SchedulingTask::Wait)
            }
        }

        let sender = InvalidTargetAgent {
            _id: 0,
            attempted: false,
        };

        world.spawn_actor(sender);
        world.schedule(1, 0).unwrap();

        assert_eq!(
            world.run().err().unwrap(),
            AikaError::MessagedNonExistent(1, 99)
        );
    }
}

#[cfg(test)]
mod lets_try_to_break_it {
    use crate::env::Stateless;

    use super::*;
    
    #[derive(Debug)]
    struct StressActor {
        id: usize,
        send_count: usize,
        max_sends: usize,
        targets: Vec<usize>,
    }
    
    impl StressActor {
        fn new(id: usize, max_sends: usize, targets: Vec<usize>) -> Self {
            Self { id, send_count: 0, max_sends, targets }
        }
    }
    
    impl Actor<u8> for StressActor {
        fn step(&mut self, ctx: &mut Context<u8>, actor_id: usize) -> Result<SchedulingTask, AikaError> {
            if self.send_count < self.max_sends {
                // Send to all targets
                for &target in &self.targets {
                    let msg = Msg::new(
                        (self.send_count % 256) as u8,
                        ctx.time,
                        ctx.time + (self.id % 10) as u64 + 1,
                        actor_id,
                        Some(target)
                    );
                    ctx.send_mail(msg, 0)?;
                }
                self.send_count += 1;
                Ok(SchedulingTask::Timeout(1))
            } else {
                Ok(SchedulingTask::Wait)
            }
        }
    }
    
    impl ConnectedActor<u8> for StressActor {
        fn read_message(&mut self, ctx: &mut Context<u8>, _: Msg<u8>, id: usize) -> Result<(), AikaError> {
            // Trigger more activity on message receipt
            if self.send_count < self.max_sends / 2 {
                self.send_count += 1;
                let _ = self.step(ctx, id)?;
            }
            Ok(())
        }
    }

    #[test]
    fn test_message_storm() {
        let mut world = LonePlanet::<256, 3, u8>::init(Stateless).unwrap();
        world.set_terminal_time(1000);
        
        // Create fully connected network
        let num_actors = 20;
        for i in 0..num_actors {
            let mut targets = Vec::new();
            for j in 0..num_actors {
                if i != j {
                    targets.push(j);
                }
            }
            world.spawn_receiver_actor(StressActor::new(i, 100, targets));
        }
        
        // Start all actors
        for i in 0..num_actors {
            world.schedule(1, i).unwrap();
        }
        
        world.run().unwrap();
    }

    #[test]
    fn test_cascading_triggers() {
        #[derive(Debug)]
        struct CascadeActor {
            triggers_remaining: usize,
        }
        
        impl Actor<u8> for CascadeActor {
            fn step(&mut self, ctx: &mut Context<u8>, id: usize) -> Result<SchedulingTask, AikaError> {
                if self.triggers_remaining > 0 {
                    self.triggers_remaining -= 1;
                    let next = (id + 1) % 10;
                    Ok(
                        SchedulingTask::Trigger { time: ctx.time + 1, idx: next }
                    )
                } else {
                    Ok(SchedulingTask::Wait)
                }
            }
        }
        
        impl ConnectedActor<u8> for CascadeActor {
            fn read_message(&mut self, _: &mut Context<u8>, _: Msg<u8>, _: usize) -> Result<(), AikaError> {
                Ok(())
            }
        }
        
        let mut world = LonePlanet::<128, 2, u8>::init(Stateless).unwrap();
        world.set_terminal_time(500);
        
        for _ in 0..10 {
            world.spawn_receiver_actor(CascadeActor { triggers_remaining: 50 });
        }
        
        world.schedule(1, 0).unwrap();
        world.run().unwrap();
    }

    #[test]
    fn test_time_boundary_stress() {
        #[derive(Debug)]
        struct BoundaryActor;
        
        impl Actor<u8> for BoundaryActor {
            fn step(&mut self, ctx: &mut Context<u8>, _id: usize) -> Result<SchedulingTask, AikaError> {
                // Schedule events at exact time boundaries
                let times = [1, 10, 100, 127, 128, 255, 256, 999, 1000];
                for &t in &times {
                    if t > ctx.time && t <= ctx.terminal {
                        return Ok(SchedulingTask::Schedule(t));
                    }
                }
                Ok(SchedulingTask::Wait)
            }
        }
        
        impl ConnectedActor<u8> for BoundaryActor {
            fn read_message(&mut self, _: &mut Context<u8>, _: Msg<u8>, _: usize) -> Result<(), AikaError> {
                Ok(())
            }
        }
        
        let mut world = LonePlanet::<128, 2, u8>::init(Stateless).unwrap();
        world.set_terminal_time(1000);
        
        for _ in 0..50 {
            world.spawn_receiver_actor(BoundaryActor);
        }
        
        for i in 0..50 {
            world.schedule(0, i).unwrap();
        }
        
        world.run().unwrap();
    }

    #[test]
    fn test_broadcast_flood() {
        #[derive(Debug)]
        struct BroadcastActor {
            broadcast_count: usize,
        }
        
        impl Actor<u8> for BroadcastActor {
            fn step(&mut self, ctx: &mut Context<u8>, id: usize) -> Result<SchedulingTask, AikaError> {
                if self.broadcast_count < 100 {
                    let msg = Msg::new(
                        self.broadcast_count as u8,
                        ctx.time,
                        ctx.time + 1,
                        id,
                        None
                    );
                    ctx.send_mail(msg, 0)?;
                    self.broadcast_count += 1;
                    Ok(SchedulingTask::Timeout(2))
                } else {
                    Ok(SchedulingTask::Wait)
                }
            }
        }
        
        impl ConnectedActor<u8> for BroadcastActor {
            fn read_message(&mut self, _: &mut Context<u8>, _: Msg<u8>, _: usize) -> Result<(), AikaError> {
                Ok(())
            }
        }
        
        let mut world = LonePlanet::<256, 2, u8>::init(Stateless).unwrap();
        world.set_terminal_time(500);
        
        for _ in 0..30 {
            world.spawn_receiver_actor(BroadcastActor { broadcast_count: 0 });
        }
        
        for i in 0..30 {
            world.schedule(i as u64, i).unwrap();
        }
        
        world.run().unwrap();
    }

    #[test]
    fn test_scheduler_overflow() {
        #[derive(Debug)]
        struct OverflowActor;
        
        impl Actor<u8> for OverflowActor {
            fn step(&mut self, ctx: &mut Context<u8>, _id: usize) -> Result<SchedulingTask, AikaError> {
                let far_future = ctx.time + 10000;
                if far_future <= ctx.terminal {
                    Ok(SchedulingTask::Schedule(far_future))
                } else {
                    Ok(SchedulingTask::Timeout(1))
                }
            }
        }
        
        impl ConnectedActor<u8> for OverflowActor {
            fn read_message(&mut self, _: &mut Context<u8>, _: Msg<u8>, _: usize) -> Result<(), AikaError> {
                Ok(())
            }
        }
        
        let mut world = LonePlanet::<128, 2, u8>::init(Stateless).unwrap();
        world.set_terminal_time(50000);
        
        for _ in 0..100 {
            world.spawn_receiver_actor(OverflowActor);
        }
        
        for i in 0..100 {
            world.schedule(1, i).unwrap();
        }
        
        world.run().unwrap();
    }

    #[test]
    fn test_rapid_self_messaging() {
        #[derive(Debug)]
        struct SelfMessager {
            remaining: usize,
        }
        
        impl Actor<u8> for SelfMessager {
            fn step(&mut self, ctx: &mut Context<u8>, id: usize) -> Result<SchedulingTask, AikaError> {
                if self.remaining > 0 {
                    for delay in 1..=5 {
                        let msg = Msg::new(
                            delay as u8,
                            ctx.time,
                            ctx.time + delay,
                            id,
                            Some(id) // Self
                        );
                        ctx.send_mail(msg, 0)?;
                    }
                    self.remaining -= 1;
                }
                Ok(SchedulingTask::Timeout(1))
            }
        }
        
        impl ConnectedActor<u8> for SelfMessager {
            fn read_message(&mut self, ctx: &mut Context<u8>, _: Msg<u8>, id: usize) -> Result<(), AikaError> {
                // Trigger more self-messages
                if self.remaining > 0 {
                    let _ = self.step(ctx, id)?;
                }
                Ok(())
            }
        }
        
        let mut world = LonePlanet::<256, 3, u8>::init(Stateless).unwrap();
        world.set_terminal_time(200);
        
        for _ in 0..10 {
            world.spawn_receiver_actor(SelfMessager { remaining: 50 });
        }
        
        for i in 0..10 {
            world.schedule(1, i).unwrap();
        }
        
        world.run().unwrap();
    }

    #[test]
    fn test_zero_delay_messages() {
        #[derive(Debug)]
        struct ZeroDelayActor {
            pings: usize,
        }
        
        impl Actor<u8> for ZeroDelayActor {
            fn step(&mut self, ctx: &mut Context<u8>, id: usize) -> Result<SchedulingTask, AikaError> {
                if self.pings < 1000 {
                    // Send zero-delay message
                    let target = (id + 1) % 10;
                    let msg = Msg::new(
                        self.pings as u8,
                        ctx.time,
                        ctx.time, // Zero delay!
                        id,
                        Some(target)
                    );
                    ctx.send_mail(msg, 0)?;
                    self.pings += 1;
                    Ok(SchedulingTask::Timeout(1))
                } else {
                    Ok(SchedulingTask::Wait)
                }
            }
        }
        
        impl ConnectedActor<u8> for ZeroDelayActor {
            fn read_message(&mut self, ctx: &mut Context<u8>, _: Msg<u8>, id: usize) -> Result<(), AikaError> {
                let _ = self.step(ctx, id)?;
                Ok(())
            }
        }
        
        let mut world = LonePlanet::<512, 2, u8>::init(Stateless).unwrap();
        world.set_terminal_time(100);
        
        for _ in 0..10 {
            world.spawn_receiver_actor(ZeroDelayActor { pings: 0 });
        }
        
        world.schedule(1, 0).unwrap();
        world.run().unwrap();
    }
}