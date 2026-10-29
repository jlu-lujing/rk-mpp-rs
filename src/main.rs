//! Probe: does the pure-Rust MPP client reach the JPEG decoder?

mod dma_heap;
mod mpp;

use mpp::*;

fn main() {
    println!("rk-mpp-rs probe");

    match MppService::open(VPU_CLIENT_JPEG_DEC) {
        Ok(svc) => {
            println!("  session: client_type={}", svc.client_type());
            match svc.probe_hw_support() {
                Ok(mask) => {
                    println!("  hw support mask: 0x{mask:08x}");
                    println!(
                        "  jpeg dec present: {}",
                        (mask >> VPU_CLIENT_JPEG_DEC) & 1 == 1
                    );
                    println!(
                        "  rkvdec present:  {}",
                        (mask >> VPU_CLIENT_RKVDEC) & 1 == 1
                    );
                }
                Err(e) => println!("  probe_hw_support failed: {e}"),
            }
            match svc.hw_id() {
                Ok(id) => println!("  hw id: {id}"),
                Err(e) => println!("  hw_id failed: {e}"),
            }
        }
        Err(e) => println!("  open {MPP_SERVICE_PATH} failed: {e}"),
    }

    for heap in ["system", "system-uncached", "cma"] {
        match dma_heap::DmaBuf::alloc(heap, 1 << 20, false) {
            Ok(buf) => println!(
                "  dma-heap {heap}: {} bytes, fd={}",
                buf.len(),
                buf.fd()
            ),
            Err(e) => println!("  dma-heap {heap}: {e}"),
        }
    }
}
