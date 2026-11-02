// A stand-in for the game's plugin, driving WestPlay's screen through the bridge as the plugin does: the
// interaction key held, mouse counts, buttons and the wheel. Commands, in order:
//   click X Y | rclick X Y | move X Y | wheel N (notches, + away) | wait MS | shot FILE | type TEXT | release
use std::sync::atomic::Ordering;
use std::thread::sleep;
use std::time::Duration;
use westplay_shared::frames::{FrameReader, FramesMapping};
use westplay_shared::{BridgeMapping, PointerBlock};

// The host's pointer curve (crates/host/src/pointer.rs), sensitivity 1: page pixels for `c` counts in one batch
// after a pause (its interval then 50 ms).
fn travel(c: f32) -> f32 {
    let speed = (c.abs() / 50.0).min(4.0);
    c * 1.3 * (1.0 + 0.55 * speed)
}
fn counts_for(d: f32) -> i32 {
    let (mut lo, mut hi) = (0.0f32, 2000.0f32);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if travel(mid) < d.abs() { lo = mid } else { hi = mid }
    }
    (hi.round() as i32) * d.signum() as i32
}

fn main() {
    let bridge = BridgeMapping::open().unwrap();
    assert!(bridge.matches(), "bridge of another version");
    let b = bridge.bridge();
    let p = &b.plugin.pointer;
    // As the game's plugin in a running truck, electrics on (`GAME_PID`: a process standing in for the game, one
    // with a window on screen, or none): its telemetry and heartbeat, kept up while this runs.
    if let Ok(pid) = std::env::var("GAME_PID") {
        b.plugin.pid.store(pid.parse().unwrap(), Ordering::Release);
    }
    let telemetry = westplay_shared::Telemetry {
        electric_enabled: true,
        engine_enabled: true,
        dashboard_backlight: 1.0,
        game_time: 600,
        local_scale: 19.0,
        x: -31000.0,
        z: -5000.0,
        ..Default::default()
    };
    std::thread::spawn(move || {
        let bridge = BridgeMapping::open().unwrap();
        loop {
            bridge.bridge().plugin.telemetry.write(&telemetry);
            bridge.bridge().plugin.heartbeat.fetch_add(1, Ordering::Release);
            sleep(Duration::from_millis(16));
        }
    });
    sleep(Duration::from_millis(300));
    let frames = FramesMapping::open().unwrap();
    let mut reader = FrameReader::new(frames.block());
    p.active.store(1, Ordering::Release);
    sleep(Duration::from_millis(60));
    let moveto = |x: f32, y: f32| {
        // Into the corner (clamped there), then out by exact counts, each after a pause.
        p.dx.fetch_add(-6000, Ordering::AcqRel);
        p.dy.fetch_add(-6000, Ordering::AcqRel);
        sleep(Duration::from_millis(70));
        p.dx.fetch_add(counts_for(x), Ordering::AcqRel);
        sleep(Duration::from_millis(70));
        p.dy.fetch_add(counts_for(y), Ordering::AcqRel);
        sleep(Duration::from_millis(70));
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut at = 0;
    while at < args.len() {
        let arg = |i: usize| args[at + i].clone();
        match args[at].as_str() {
            "move" => { moveto(arg(1).parse().unwrap(), arg(2).parse().unwrap()); at += 3; }
            "click" | "rclick" => {
                moveto(arg(1).parse().unwrap(), arg(2).parse().unwrap());
                let button = if args[at] == "click" { PointerBlock::LEFT } else { PointerBlock::RIGHT };
                p.buttons.store(button, Ordering::Release);
                sleep(Duration::from_millis(80));
                p.buttons.store(0, Ordering::Release);
                sleep(Duration::from_millis(80));
                at += 3;
            }
            "wheel" => { p.wheel.fetch_add(arg(1).parse::<i32>().unwrap() * 120, Ordering::AcqRel); sleep(Duration::from_millis(80)); at += 2; }
            "wait" => { sleep(Duration::from_millis(arg(1).parse().unwrap())); at += 2; }
            "release" => { p.active.store(0, Ordering::Release); sleep(Duration::from_millis(60)); at += 1; }
            "shot" => {
                // The newest frame: let the host send it (it resends its last at least every 500 ms).
                let mut taken = None;
                let until = std::time::Instant::now() + Duration::from_millis(1200);
                while std::time::Instant::now() < until {
                    if let Some(f) = reader.take() { taken = Some((f.width, f.height, f.pixels.to_vec())); }
                    sleep(Duration::from_millis(5));
                }
                let (w, h, mut px) = taken.expect("no frame");
                for c in px.chunks_exact_mut(4) { c.swap(0, 2); }
                let file = std::fs::File::create(arg(1)).unwrap();
                let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
                enc.set_color(png::ColorType::Rgba);
                enc.write_header().unwrap().write_image_data(&px).unwrap();
                println!("shot {}", arg(1));
                at += 2;
            }
            other => panic!("unknown command {other}"),
        }
    }
    p.active.store(0, Ordering::Release);
}
