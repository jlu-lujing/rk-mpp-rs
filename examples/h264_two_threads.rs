// Temporary: serial vs concurrent VEPU580 H.264 sessions, same process.
use rk_mpp_rs::codec::h264::encoder::EncoderConfig;
use rk_mpp_rs::codec::h264::split_annex_b;
use rk_mpp_rs::hal::h264e::{probe_nv12, Vepu580H264};

fn fnv(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn one(w: usize, h: usize) -> (usize, usize, u64) {
    let input = probe_nv12(w, h);
    let mut encoder = Vepu580H264::open(EncoderConfig::new(w, h)).unwrap();
    let stream = encoder.encode_idr(&input).unwrap().annex_b;
    (stream.len(), split_annex_b(&stream).len(), fnv(&stream))
}

fn main() {
    let threads: usize = std::env::var("THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    for (w, h) in [(64usize, 64usize), (320, 240), (640, 480)] {
        let handles: Vec<_> = (0..threads)
            .map(|_| std::thread::spawn(move || one(w, h)))
            .collect();
        let results: Vec<_> = handles.into_iter().map(|t| t.join().unwrap()).collect();
        let unique: std::collections::BTreeSet<_> = results.iter().copied().collect();
        println!(
            "{w}x{h} threads={threads} results={results:?} distinct={}",
            unique.len()
        );
    }
}
