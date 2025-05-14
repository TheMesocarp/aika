# aika
An attempt at a high-performance Discrete Event Simulator (DES), built in Rust!

> Work In Progress, Do Not Use Yet

## what for?

This is an agent-based discrete event simulator, where each simulation houses agents on a single-threaded `World`, which can all be managed and housed by a `Universe`. This can be used for anything from simple Monte Carlo simulations, to real-time sensor processing, and even IoT device coordination. See `/examples/` for more details. 

The library is very low overhead during runtime, so it particularly excels in resource constrained environments such as on Raspberry Pi's or RockPi's. It's still very early in development, but the goal will be to eventually shift to `no-std` where possible to better support for edge devices. 

## architecture

The core event processing architecture is centered around a [Hierarchical Timing Wheel](https://dl.acm.org/doi/pdf/10.1145/37499.37504) (found in the `clock.rs` script), to allow $O(1)$ event queuing and retrieval within the wheel horizon, then using a `BTreeSet<Reverse<Event>>` min-heap as an overflow buffer for far-future scheduling at $O(\log n)$. The clock is structured around the type `[[Vec<Event>; SLOTS]; HEIGHT]` in order to reduce heap allocations as much as possible (the inner `Vec` is so that we still have flexibility in terms of maximum number of concurrent events).

The clock is only rolled when a particular wheel has been emptied, only cascading from the first slot of the coarser wheel (or overflow buffer if the timing wheel has only one layer). Some simple index arithmetic during the event insertion and rotation is enough to make the clock fully zero copy as well. 

This clock provides synchronicity for agents within a particular `World` struct, meaning that an asynchronous runtime isn't actually necessary, at least for event processing. Currently, this one component is the primary source of the single-threaded performance the simulator is achieving.

Beyond this, there is also a messaging layer to allow agents to communicate between event processing. Lastly at the `World` level, there is a `Logger` that comes with each simulator, that allows the states of agents and the shared simulation state to be logged along with any events and messages ordered by their corresponding timestamp.

Each of these additional features is toggleable in the `Config`, in order to reduce runtime overhead as much as possible whenever certain features are not needed. They all are preliminary implementations of these features and are likely still due to change significantly, as no serious optimizations have been done to reduce dynamic allocation or copy overheads.
## usage

The API has similar ease of use to many other agent based simulators. Generate a configuration for the simulation, create a world with that config, spawn the agents in that world, and schedule an initial event before running. A practical example of this looks like this: 

```rust
let config = Config::new(1.0, Some(2000000.0), 100, 100, false, false, false);
let mut world = World::<256, 1>::create(config);
let agent_test = TestAgent::new(0, "Agent".to_string());
world.spawn(Box::new(agent_test));
world.schedule(0.0, 0).unwrap();
assert!(world.run().await.unwrap() == ());
```

## benchmark

Benchmark tests can be found in the `/benches/` directory, there is currently only one, for event scehduling throughput for a simple monte carlo simulator. 40 million monte carlo events are scheduled and processed during this test with no extraneous agent logic, with results as follows for the two systems tested on:


| CPU Model | RAM | Low | Average | High | Events/Sec (EPS) |
|-----------|-----|-----|---------|------|------------|
| i7-13700F | 32 GB | 1.0052 s | 1.0068 s | 1.0087 s | ~39.8e6 |
| Apple M2 | 8 GB  | 0.9455 s | 0.9474 s | 0.9498 s | ~42.2e6 |

## contributions

We are extremely open to community collaboration! I'll be putting issues up for features or optimizations that need work, so feel free to lay claim to one, and submit a pull request when ready!
