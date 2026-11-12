use rk_mpp_rs::hal::vdpp2::Vdpp2Config;
use rk_mpp_rs::service::vdpp2_session::Vdpp2Processor;

#[test]
#[ignore = "requires RK3576 VDPP2 client 29 and /dev/dma_heap access"]
fn vdpp2_client_probe_reports_binding_commands() {
    let report = rk_mpp_rs::service::vdpp2_session::probe();
    assert!(report.present, "VDPP2 hardware bit 29 is absent");
    assert!(report.binds, "VDPP2 client 29 did not bind");
    assert!(report.reg_offset, "VDPP2 address offsets are unavailable");
    assert!(report.poll, "VDPP2 completion polling is unavailable");
}

#[test]
#[ignore = "requires RK3576 VDPP2 client 29 and /dev/dma_heap access"]
fn vdpp2_scales_nv12_to_nv12() {
    let config = Vdpp2Config::nv12_to_nv12(128, 128, 256, 256);
    let mut processor = Vdpp2Processor::open(config).unwrap();
    let src = processor.src_frame_mut();
    src.sync_write_begin().unwrap();
    for row in 0..128 {
        for column in 0..128 {
            src.as_mut_slice()[row * 128 + column] = ((row + column) & 0xff) as u8;
        }
    }
    for row in 0..64 {
        for column in 0..128 {
            src.as_mut_slice()[128 * 128 + row * 128 + column] = 128;
        }
    }
    processor.run().unwrap();
    assert!(processor
        .dst_frame()
        .iter()
        .take(256 * 256)
        .any(|sample| *sample != 0));
}
