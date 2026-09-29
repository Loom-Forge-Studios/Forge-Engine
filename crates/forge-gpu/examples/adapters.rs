//! Print every adapter the driver lists and what the pool decided for it.
//! `cargo run -p forge-gpu --example adapters [-- --software] [--multi]`

use forge_gpu::{AdapterPool, GpuMode, PoolOptions, SoftwarePolicy};

fn main() {
    let software = std::env::args().any(|a| a == "--software");
    let multi = std::env::args().any(|a| a == "--multi");
    let opts = PoolOptions {
        software: if software {
            SoftwarePolicy::Include
        } else {
            SoftwarePolicy::FallbackOnly
        },
        mode: if multi {
            GpuMode::Multi
        } else {
            GpuMode::Single
        },
        ..PoolOptions::default()
    };
    match AdapterPool::new(&opts) {
        Ok(pool) => {
            println!(
                "pool: {} device(s) (GPU mode {:?}), primary {}",
                pool.len(),
                pool.mode(),
                pool.primary().label()
            );
            if !pool.skipped_backends().is_empty() {
                println!(
                    "  not enumerated (Vulkan sufficed in GPU mode Single): {:?}",
                    pool.skipped_backends()
                );
            }
            for r in pool.report() {
                println!(
                    "  {:<60} {:?} {:?} -> {}",
                    r.facts.label(),
                    r.facts.backend,
                    r.facts.device_type,
                    r.verdict.reason()
                );
            }
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
