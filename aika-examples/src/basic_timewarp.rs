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
    fn step(&mut self, time: &u64, state: &mut Lumi) -> Event {
        Event::new(*time, *time, self.id, Action::Timeout(1))
    }
    fn process_message(
        &mut self,
        msg: Message,
        time: u64,
        state: &mut Lumi,
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
    let terminal = 70000000;
    const lps: usize = 16;
    let mut gvt = GVT::<lps, 1, 128, 1>::start_engine(terminal);
    for i in 0..lps {
        let lp = Box::new(TestAgent::new(i));
        let idx = gvt.spawn_process::<u8>(lp, 1.0, 4096).unwrap();
        gvt.commit(idx, Object::Event(Event::new(0, 1, idx, Action::Timeout(1)))).unwrap();
        gvt.commit(idx, Object::Message(Message::new( 0 as *const u8, 0, 19, idx, (idx + 1) % 10))).unwrap();
    }
    gvt.init_comms().unwrap();
    let staticdown: &'static mut GVT<lps, 1, 128, 1> = Box::leak(gvt);
    let start = std::time::Instant::now();
    run(staticdown).unwrap();
    let elapsed = start.elapsed();
    println!("Benchmark Results:");
    println!("Total time: {:.2?}", elapsed);
    println!("Total events processed: {}", (terminal * lps));
    println!(
        "Events per second: {:.2}",
        (terminal * lps) as f64 / elapsed.as_secs_f64()
    );
    println!(
        "Average event processing time: {:.3?} per event",
        elapsed / (terminal * lps) as u32
    );
}