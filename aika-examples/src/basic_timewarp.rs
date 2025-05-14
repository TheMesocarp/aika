use aika::prelude::*;

pub struct TestAgent {
    pub id: usize,
}

impl TestAgent {
    pub fn new(id: usize) -> Self {
        TestAgent { id }
    }
}

impl Agent for TestAgent {
    fn step(&mut self, time: &u64, _supports: Supports) -> Event {
        Event::new(*time, *time, self.id, Action::Timeout(1))
    }
}

impl LogicalProcess for TestAgent {
    fn step(&mut self, time: &u64, _state: &mut Lumi) -> Event {
        Event::new(*time, *time + 1, self.id, Action::Timeout(1))
    }
    fn process_message(
        &mut self,
        msg: Message,
        time: u64,
        _state: &mut Lumi,
    ) -> HandlerOutput {
        HandlerOutput::Messages(Annihilator(
            Message {
                data: msg.data,
                sent: time,
                received: time + 19,
                from: msg.to,
                to: msg.from,
            },
            AntiMessage {
                sent: time,
                received: time + 19,
                from: msg.to,
                to: msg.from,
            },
        ))
    }
}

fn main() {
    let terminal = 1000000;
    const LPS: usize = 16;
    let mut gvt = GVT::<LPS, 10, 128, 1>::start_engine(terminal);
    for i in 0..LPS {
        let lp = Box::new(TestAgent::new(i));
        let idx = gvt.spawn_process::<u8>(lp, 1.0, 4096).unwrap();
        gvt.commit(idx, Object::Event(Event::new(0, 1, idx, Action::Timeout(1)))).unwrap();
    }
    gvt.init_comms().unwrap();
    gvt.commit(2, Object::Message(Message::new( 0 as *const u8, 0, 1, 1, 2))).unwrap();
    let staticdown: &'static mut GVT<LPS, 10, 128, 1> = Box::leak(gvt);
    let start = std::time::Instant::now();
    run(staticdown).unwrap();
    let elapsed = start.elapsed();
    println!("Benchmark Results:");
    println!("------------------");
    println!("Total agents: {LPS:?}");
    println!("Total time: {:.2?}", elapsed);
    println!("Total events processed: {:.3}m", (terminal * LPS) as f64 / 1000000.0 );
    println!(
        "Events per second: {:.3}m",
        (terminal * LPS) as f64 / (elapsed.as_secs_f64() * 1000000.0)
    );
    println!(
        "Average event processing time: {:.3?} per event",
        elapsed / (terminal * LPS) as u32
    );
}