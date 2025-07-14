# Aika
[![License: LGPL v2.1](https://img.shields.io/badge/License-LGPL_v2.1-blue.svg)](https://www.gnu.org/licenses/lgpl-2.1)
![Tests](https://github.com/TheMesocarp/aika/workflows/Tests/badge.svg)

A Rust-native coordination layer for multi-agent systems, with support for single-threaded, multi-threaded, and distributed execution. Finnish for "Time". Built entirely from systems theory first developed in the mid '80s through early '90s.

> Do Not Use `mt::hybrid` Yet! Changes Unstable

## Roadmap

In its current state, the framework supports single-threaded and multi-threaded hybrid execution, with both point-to-point and broadcast messaging support. The aim is to continue expanding into conservative synchronization support as well. A near term list of goals can be seen below:

- [x] single-threaded world (found in `st::World`) execution with messaging support via lock-free shared buffers. 
- [x] bench single-threaded `st::World` on more complex and distant scheduling tasks.
- [x] multi-threaded support via hybrid synchronization via a modified [Clustered Time Warp](https://dl.acm.org/doi/abs/10.1145/214283.214317) architecture for multi-threaded execution (found in `mt::hybrid`).
- [ ] scheduling overhead benchmark and *PHOLD* benchmark for `mt::hybrid::HybridEngine` (**in progress**).
- [ ] add direct support for multi-socket systems for the `HybridEngine` and `Journal`.
- [ ] conservative synchronization via a [Chandy-Misra-Bryant](https://dl.acm.org/doi/10.1145/130611.130613) (CMB) inspired architecture for multi-threaded execution (soon to be found in `mt::conservative`). 
- [ ] *PHOLD* benches for conservative multi-threaded execution scheme.
- [ ] (eventually) shift to MPI-like communication interface over a shared memory abstraction for real direct comms support.

## Usage

Import into your Cargo.toml via `cargo add aika`, then `use crate aika::prelude::*` to import the necessary supports for your simulation.

The API has similar ease of use to many other multi-agent simulators like `SimPy`. Create a world with a particular configuration, spawn the agents in that world, initialize the support layers (whether we want messaging or not), and schedule an initial event before running. A practical example of this for an `st::World` looks like this: 

```rust
let mut world = World::<8, 128, 1, u8>::init(40000000.0, 1.0)?;
let agent = TestAgent::new();
world.spawn_agent(Box::new(agent));
world.init_support_layers(None)?;
world.schedule(1, 0)?; 
world.run()?;
```

The multi-threaded hybrid engine has a similar set up to this, however requires a bit more direct configuration, with respect to arena allocation sizes and which planets own which agents. An easy example of what this looks like:

```rust
// Create configuration
let config = HybridConfig::new(NUM_PLANETS, 512) // NUM_PLANETS number of threaded worlds, 512 is the size of the world state arena allocation 
    .with_time_bounds(TERMINAL_TIME, 1.0) // TERMINAL_TIME for the sim to end, and 1.0 units of time per step
    .with_optimistic_sync(100, 200) // 100 step throttling window, 200 step checkpoints
    .with_uniform_worlds(1024, NUM_AGENTS, 256); // NUM_AGENTS agents per planet

// create a HybridEngine
let mut engine =
    HybridEngine::<128, 128, 2, MessageType>::create(config).unwrap();

// spawn your agents
for planet in 0..NUM_PLANETS {
    for agent in 0..NUM_AGENTS {
        engine.spawn_agent(planet, Box::new(TestAgent::new())).unwrap();
    }
}

// do something to schedule your agents initial steps
for planet in 0..NUM_PLANETS {
    for agent in 0..NUM_AGENTS {
        engine.schedule(planet, agent, 1).unwrap();
    }
}

// Run simulation
let result = engine.run();
```

## Contributing

Contributors are welcome and greatly appreciated! Please feel free to submit a Pull Request or claim an issue youd like to work on. For major changes, please open an issue first to discuss what you would like to change. If you would like to work more closely with Mesocarp on other projects as well, please email me at `sushi@fibered.cat`, would love to chat!

## License

This project is licensed under the LGPL-2.1 copyleft license - see the [LICENSE](LICENSE) file for details.
