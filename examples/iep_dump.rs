use rk_mpp_rs::hal::iep::{
    self, DeinterlaceJob, Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode, Iep2YuvSwap,
};

fn main() {
    let job = DeinterlaceJob {
        width: 720,
        height: 480,
        hor_stride: 720,
        ver_stride: 480,
        src_fmt: Iep2Fmt::Yuv420,
        src_swap: Iep2YuvSwap::SpUv,
        dst_fmt: Iep2Fmt::Yuv420,
        dst_swap: Iep2YuvSwap::SpUv,
        dil_mode: Iep2DilMode::I5O2,
        out_mode: Iep2OutMode::Line,
        field_order: Iep2FieldOrder::Tff,
        src_fds: [10, 11, 12],
        dst_fds: [13, 14],
        mv_fd: 15,
        md_fd: 16,
        md_lambda: 4,
        pd_mode: 0,
        _marker: std::marker::PhantomData,
    };
    let p = iep::build_deinterlace(&job);
    for row in p.chunks(8) {
        println!(
            "{}",
            row.iter()
                .map(|w| format!("{w:08x}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}
