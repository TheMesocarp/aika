# Aika
[![License: LGPL v2.1](https://img.shields.io/badge/License-LGPL_v2.1-blue.svg)](https://www.gnu.org/licenses/lgpl-2.1)

A Rust-native coordination layer for multi-agent systems, with support for single-threaded, multi-threaded, and distributed execution. Finnish for "Time". Built entirely from systems theory first developed in the mid '80s through early '90s.

> Work In Progress, changes are unstable

## Roadmap

In its current state, the framework supports single-threaded and multi-threaded hybrid execution, with both point-to-point and broadcast messaging support. The aim is to continue expanding into conservative synchronization support as well. A near term list of goals can be seen below:

- [x] single-threaded world (found in `st::World`) execution with messaging support via lock-free shared buffers. 
- [x] bench single-threaded `st::World` on more complex and distant scheduling tasks.
- [x] multi-threaded support via hybrid synchronization via a modified [Clustered Time Warp](https://dl.acm.org/doi/abs/10.1145/214283.214317) architecture for multi-threaded execution (found in `mt::hybrid`).
- [ ] scheduling overhead benchmark and *PHOLD* benchmark for `mt::hybrid::Engine` (next up).
- [ ] conservative synchronization via a [Chandy-Misra-Bryant](https://dl.acm.org/doi/10.1145/130611.130613) (CMB) inspired architecture for multi-threaded execution (soon to be found in `mt::conservative`). 
- [ ] *PHOLD* benches for both conservative and hybrid multi-threaded execution schemes.
- [ ] port core synchronization logic from each multi-threaded execution type to work over IPC and containerize the LP logic.
- [ ] (eventually) shift to MPI-like communication interface over a shared memory abstraction for real direct comms support.

## Usage

The API has similar ease of use to many other multi-agent simulators like `SimPy`. Create a world with a particular configuration, spawn the agents in that world, initialize the support layers (whether we want messaging or not), and schedule an initial event before running. A practical example of this for an `st::World` looks like this: 

```rust
let mut world = World::<8, 128, 1, u8>::init(40000000.0, 1.0)?;
let agent = TestAgent::new(0);
world.spawn_agent(Box::new(agent));
world.init_support_layers(None)?;
world.schedule(1, 0)?; 
world.run()?;
```

## Contributing

Contributors are welcome and greatly appreciated! Please feel free to submit a Pull Request or claim an issue youd like to work on. For major changes, please open an issue first to discuss what you would like to change. If you would like to work more closely with Mesocarp on other projects as well, please email me at `sushi@fibered.cat`, would love to chat!

## License

This project is licensed under the LGPL-2.1 copyleft license - see the [LICENSE](LICENSE) file for details.
