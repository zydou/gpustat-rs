//! `gpustat` — one line per NVIDIA GPU: temperature, utilization, memory and
//! the processes using it.
//!
//! The binary is the product. This library target exists so the code and its
//! tests live outside `main.rs`; `cli::run` reads the host process' `argv`,
//! writes to stdout and returns an exit code, so it is not a library API and is
//! not stable.

pub mod cli;

mod format;
mod model;
mod nvml;
mod palette;
mod process;
mod util;
mod watch;
