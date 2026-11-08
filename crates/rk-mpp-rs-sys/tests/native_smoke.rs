use core::ffi::CStr;
use core::mem::size_of;
use rk_mpp_rs_sys::*;

#[test]
fn version_and_initializers_match_the_native_library() {
    unsafe {
        assert_eq!(rk_mpp_api_version(), RK_MPP_API_VERSION);
        let version = CStr::from_ptr(rk_mpp_api_version_string())
            .to_str()
            .unwrap();
        assert_eq!(version, "1.10");
        assert!(!rk_mpp_strerror(RK_MPP_OK).is_null());

        let mut decoder = rk_mpp_decoder_config_t::default();
        assert_eq!(
            rk_mpp_decoder_config_init(&mut decoder, RK_MPP_CODEC_H264, RK_MPP_PIXEL_FORMAT_NV12,),
            RK_MPP_OK
        );
        assert_eq!(
            decoder.struct_size as usize,
            size_of::<rk_mpp_decoder_config_t>()
        );

        let mut encoder = rk_mpp_encoder_config_t::default();
        assert_eq!(
            rk_mpp_encoder_config_init(
                &mut encoder,
                RK_MPP_CODEC_H264,
                320,
                240,
                RK_MPP_PIXEL_FORMAT_NV12,
            ),
            RK_MPP_OK
        );
        assert_eq!(
            encoder.struct_size as usize,
            size_of::<rk_mpp_encoder_config_t>()
        );

        let mut rate = rk_mpp_encoder_rate_control_t::default();
        assert_eq!(
            rk_mpp_encoder_rate_control_init(&mut rate, RK_MPP_RATE_CONTROL_CBR),
            RK_MPP_OK
        );
        assert_eq!(rate.mode, RK_MPP_RATE_CONTROL_CBR);

        let mut advanced = rk_mpp_encoder_advanced_config_t::default();
        assert_eq!(
            rk_mpp_encoder_advanced_config_init(&mut advanced, RK_MPP_RATE_CONTROL_VBR),
            RK_MPP_OK
        );
        assert_eq!(advanced.rate_control.mode, RK_MPP_RATE_CONTROL_VBR);

        let mut advanced_v2 = rk_mpp_encoder_advanced_config_v2_t::default();
        assert_eq!(
            rk_mpp_encoder_advanced_config_v2_init(&mut advanced_v2, RK_MPP_RATE_CONTROL_FIXED_QP,),
            RK_MPP_OK
        );
        assert_eq!(
            advanced_v2.struct_size as usize,
            size_of::<rk_mpp_encoder_advanced_config_v2_t>()
        );

        let mut roi = rk_mpp_encoder_roi_region_t::default();
        assert_eq!(rk_mpp_encoder_roi_region_init(&mut roi), RK_MPP_OK);
        assert_eq!(
            roi.struct_size as usize,
            size_of::<rk_mpp_encoder_roi_region_t>()
        );

        let mut gdr = rk_mpp_encoder_gdr_t::default();
        assert_eq!(
            rk_mpp_encoder_gdr_init(&mut gdr, RK_MPP_GDR_ROW, 2),
            RK_MPP_OK
        );
        let mut split = rk_mpp_encoder_slice_split_t::default();
        assert_eq!(
            rk_mpp_encoder_slice_split_init(
                &mut split,
                RK_MPP_SLICE_SPLIT_BY_CTU,
                96,
                RK_MPP_SLICE_OUTPUT_LOW_DELAY_PACKETS,
            ),
            RK_MPP_OK
        );

        let mut packet = rk_mpp_encoded_packet_t::default();
        assert_eq!(rk_mpp_encoded_packet_init(&mut packet), RK_MPP_OK);
        assert_eq!(
            packet.struct_size as usize,
            size_of::<rk_mpp_encoded_packet_t>()
        );
    }
}

#[test]
fn every_public_symbol_is_linkable() {
    let symbols: [usize; 34] = [
        rk_mpp_api_version as usize,
        rk_mpp_api_version_string as usize,
        rk_mpp_strerror as usize,
        rk_mpp_decoder_config_init as usize,
        rk_mpp_frame_init as usize,
        rk_mpp_decoder_create as usize,
        rk_mpp_decoder_submit_packet as usize,
        rk_mpp_decoder_receive_frame as usize,
        rk_mpp_frame_release as usize,
        rk_mpp_decoder_flush as usize,
        rk_mpp_decoder_reset as usize,
        rk_mpp_decoder_recover as usize,
        rk_mpp_decoder_status as usize,
        rk_mpp_decoder_close as usize,
        rk_mpp_encoder_config_init as usize,
        rk_mpp_encoder_rate_control_init as usize,
        rk_mpp_encoder_advanced_config_init as usize,
        rk_mpp_encoder_roi_region_init as usize,
        rk_mpp_encoder_gdr_init as usize,
        rk_mpp_encoder_slice_split_init as usize,
        rk_mpp_encoder_advanced_config_v2_init as usize,
        rk_mpp_encoded_packet_init as usize,
        rk_mpp_encoder_create as usize,
        rk_mpp_encoder_create_advanced as usize,
        rk_mpp_encoder_create_advanced_v2 as usize,
        rk_mpp_encoder_submit_frame as usize,
        rk_mpp_encoder_receive_packet as usize,
        rk_mpp_encoded_packet_release as usize,
        rk_mpp_encoder_flush as usize,
        rk_mpp_encoder_reset as usize,
        rk_mpp_encoder_close as usize,
        rk_jpeg_open as usize,
        rk_jpeg_frame as usize,
        rk_jpeg_close as usize,
    ];
    assert!(symbols.into_iter().all(|address| address != 0));
}
