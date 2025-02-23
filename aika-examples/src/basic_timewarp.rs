use aika::{prelude::*, TestAgent};

fn main() {
    let terminal = 20000000;
    let mut gvt = GVT::<10, 8, 128, 1>::start_engine(terminal);
    for i in 0..10 {
        let lp = Box::new(TestAgent::new(i));
        let idx = gvt.spawn_process::<u8>(lp, 1.0, 4096).unwrap();
        gvt.commit(idx, Object::Event(Event::new(0, 1, idx, Action::Timeout(1)))).unwrap(); //Object::Message(Message::new( 0 as *const u8, 0, 2, idx, (idx + 1) % 10))
    }
    gvt.init_comms().unwrap();
    let staticdown: &'static mut GVT<10, 8, 128, 1> = Box::leak(gvt);
    let start = std::time::Instant::now();
    run(staticdown).unwrap();
    let elapsed = start.elapsed();
    println!("Benchmark Results:");
    println!("Total time: {:.2?}", elapsed);
    println!("Total events processed: {}", (terminal * 10));
    println!(
        "Events per second: {:.2}",
        (terminal * 10) as f64 / elapsed.as_secs_f64()
    );
    println!(
        "Average event processing time: {:.3?} per event",
        elapsed / (terminal * 10) as u32
    );
}