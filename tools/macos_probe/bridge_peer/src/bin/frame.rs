// Reads WestPlay's frame block as the plugin will: the newest frame, its rate, and one saved as a PNG.
use westplay_shared::frames::{FrameReader, FramesMapping};
fn main() {
    let mapping = FramesMapping::open().unwrap();
    let mut reader = FrameReader::new(mapping.block());
    let start = std::time::Instant::now();
    let (mut taken, mut saved) = (0, false);
    while start.elapsed().as_secs() < 3 {
        if let Some(frame) = reader.take() {
            taken += 1;
            if !saved {
                saved = true;
                let mut rgba = frame.pixels.to_vec();
                for px in rgba.chunks_exact_mut(4) { px.swap(0, 2); }
                let file = std::fs::File::create(std::env::args().nth(1).unwrap()).unwrap();
                let mut enc = png::Encoder::new(std::io::BufWriter::new(file), frame.width, frame.height);
                enc.set_color(png::ColorType::Rgba);
                enc.write_header().unwrap().write_image_data(&rgba).unwrap();
                println!("frame {} {}x{}", frame.seq, frame.width, frame.height);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    println!("{} frames taken in 3 s", taken);
}
