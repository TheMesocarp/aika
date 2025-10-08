# Aika
[![License: LGPL v2.1](https://img.shields.io/badge/License-LGPL_v2.1-blue.svg)](https://www.gnu.org/licenses/lgpl-2.1)
![Tests](https://github.com/TheMesocarp/aika/workflows/Tests/badge.svg)

A Rust-native coordination layer for multi-agent systems, with support for single-threaded, multi-threaded, and distributed execution. Finnish for "Time". Built entirely from systems theory first developed in the mid '80s through early '90s.

> DO NOT USE `aika::mt` engines yet, still experimental!

## Roadmap

In its current state, the framework supports single-threaded and multi-threaded hybrid execution, with both point-to-point and broadcast messaging support. The aim is to continue expanding into conservative synchronization support as well. A long term list of goals can be seen below:

- [x] single-threaded world (found in `st::LonePlanet`) execution with messaging support via lock-free shared buffers. 
- [x] bench single-threaded `st::LonePlanet` on more complex and distant scheduling tasks.
- [x] multi-threaded support via hybrid synchronization via a modified [Clustered Time Warp](https://dl.acm.org/doi/abs/10.1145/214283.214317) architecture for multi-threaded execution (found in `mt::engines::hlocal`). (**In Progress**)
- [ ] scheduling overhead benchmark and *PHOLD* benchmark for `mt::engines::hlocal`.
- [ ] implement `mt::engines::hdist`, the variant of aika's hybrid model that uses decentralized GVT computation and message passing, instead of a central coordinator thread.
- [ ] conservative synchronization via a [Chandy-Misra-Bryant](https://dl.acm.org/doi/10.1145/130611.130613) (CMB) inspired architecture for multi-threaded execution (soon to be found in `mt::conservative`). 
- [ ] *PHOLD* benches for conservative multi-threaded execution scheme.

## Usage

Import into your Cargo.toml via `cargo add aika`, then `use crate aika::prelude::*` to import the necessary supports for your simulation.

The API has similar ease of use to many other multi-agent simulators like `SimPy`. Create a world with a particular configuration, spawn the agents in that world, initialize the support layers (whether we want messaging or not), and schedule an initial event before running. A practical example of this for an `st::World` looks like this: 

```rust
let mut world = world!(u8)(Stateless)?;
world.set_terminal_time(400000);
world.spawn_actor(YourActor);
world.schedule(1, 0)?; 
world.run()?;
```

The multi-threaded `hlocal` engine has a similar set up to this, however requires a bit more direct configuration, with respect to which planets own which agents and how the initial scheduling works. An easy example of what this looks like:

```rust
// Create stager
let mut stager = stager!(YourMsgType)?;

// Create configuration
let config = Config::new(
    clusters: 1,
    batch_size: 128,
    block_duration: 20,
    terminal: 2048,
    checkpoint_frequency: 10
)
stager.config(config)?;

// create execution contexts and spawn actors on it
stager.create_cluster(YourEnv)?;
stager.spawn_actor_on_cluster(0, YourActor)?;

// schedule all actors on that cluster
stager.schedule_cluster(0, 0)?;

// run the simulation
stager = stager.run(RunMode::Fast)?;
```

## Contributing

Contributors are welcome and greatly appreciated! Please feel free to submit a Pull Request or claim an issue youd like to work on. For major changes, please open an issue first to discuss what you would like to change. If you would like to work more closely with Mesocarp on other projects as well, please email me at `sushi@fibered.cat`, would love to chat!

## License

This project is licensed under the LGPL-2.1 copyleft license - see the [LICENSE](LICENSE) file for details.
