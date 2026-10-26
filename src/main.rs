//! The `gpustat` console script.

fn main() {
    // Rust's runtime starts with SIGPIPE ignored, so a closed pipe would make
    // every write fail instead of ending the process. Restore the default so
    // `gpustat --json | head` exits cleanly.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    std::process::exit(gpustat_rs::cli::run());
}
