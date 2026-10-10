use std::io::Read;
use std::time::Instant;

use sha2::{Digest, Sha256};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).expect("binary path required");
    let started = Instant::now();
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    let digest = format!("{:x}", hash.finalize());
    println!(
        "{{\"bytes\":{bytes},\"elapsed_ns\":{},\"sha256\":\"{digest}\"}}",
        started.elapsed().as_nanos()
    );
    Ok(())
}
