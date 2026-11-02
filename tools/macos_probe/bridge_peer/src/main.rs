// A stand-in for the game plugin, for testing the macOS host without the game: see src/bin/drive.rs (drive the cabin)
// and src/bin/frame.rs (read a frame back). Build: cargo build --release (here).
// Opens WestPlay's bridge, prints what's in it, writes its own, holds it for a while.
use std::sync::atomic::Ordering;
fn main() {
    let mut args = std::env::args().skip(1);
    let value: u64 = args.next().unwrap().parse().unwrap();
    let hold: u64 = args.next().unwrap().parse().unwrap();
    let mapping = westplay_shared::BridgeMapping::open().unwrap();
    let bridge = mapping.bridge();
    let arch = if cfg!(target_arch = "x86_64") { "x86_64" } else { "arm64" };
    let truck = bridge.plugin.truck.read().unwrap();
    println!("{arch}: saw frame_seq {} truck {:?} matches {}", bridge.host.frame_seq.load(Ordering::Acquire), truck.id, mapping.matches());
    bridge.host.frame_seq.store(value, Ordering::Release);
    let mut t = truck; t.id = westplay_shared::FixedStr::new(&format!("written by {arch}"));
    bridge.plugin.truck.write(&t);
    std::thread::sleep(std::time::Duration::from_secs(hold));
}
