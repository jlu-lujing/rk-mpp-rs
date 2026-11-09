#ifndef RK_MPP_RS_H
#define RK_MPP_RS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define RK_MPP_API_VERSION_MAJOR 1u
#define RK_MPP_API_VERSION_MINOR 12u
#define RK_MPP_API_VERSION \
    ((RK_MPP_API_VERSION_MAJOR << 16) | RK_MPP_API_VERSION_MINOR)

enum rk_mpp_status {
    RK_MPP_OK = 0,
    RK_MPP_AGAIN = 1,
    RK_MPP_EOS = 2,
    RK_MPP_ERR_INVALID = -1,
    RK_MPP_ERR_UNSUPPORTED = -2,
    RK_MPP_ERR_PARSE = -3,
    RK_MPP_ERR_HARDWARE = -4,
    RK_MPP_ERR_RESOURCE = -5,
    RK_MPP_ERR_STATE = -6,
    RK_MPP_ERR_IO = -7,
    RK_MPP_ERR_TOO_LARGE = -8
};

enum rk_mpp_codec {
    RK_MPP_CODEC_H264 = 1,
    RK_MPP_CODEC_MJPEG = 2,
    RK_MPP_CODEC_H265 = 3,
    RK_MPP_CODEC_VP9 = 4,
    RK_MPP_CODEC_AV1 = 5,
    RK_MPP_CODEC_VP8 = 6,
    RK_MPP_CODEC_MPEG2 = 7,
    RK_MPP_CODEC_MPEG4 = 8,
    RK_MPP_CODEC_H263 = 9
};

enum rk_mpp_pixel_format {
    RK_MPP_PIXEL_FORMAT_NV12 = 1,
    RK_MPP_PIXEL_FORMAT_I420 = 2,
    /* Planar 4:2:0, little-endian 16-bit samples in all three planes. */
    RK_MPP_PIXEL_FORMAT_I420P10 = 3
};

enum rk_mpp_decoder_flags {
    RK_MPP_DECODER_FLAG_DMABUF_OUTPUT = 1u << 0
};

/*
 * H.264/MJPEG accept NV12 or I420. H.265 accepts NV12/I420 for 8-bit streams
 * and I420P10 for Main10. VP9/AV1 accept NV12/I420 for 8-bit and I420P10 for
 * 10-bit. VP8, MPEG-2, MPEG-4 and H.263 accept NV12/I420. Incompatible
 * format/stream combinations are errors.
 */

enum rk_mpp_packet_flags {
    RK_MPP_PACKET_FLAG_EOS = 1u << 0,
    RK_MPP_PACKET_FLAG_COMPLETE_ACCESS_UNIT = 1u << 1
};

typedef struct rk_mpp_decoder_config {
    uint32_t struct_size;
    uint32_t codec;
    uint32_t output_format;
    uint32_t flags;
    uint32_t max_reorder_frames;
    uint32_t reserved[3];
} rk_mpp_decoder_config_t;

/*
 * Plane layout:
 *   NV12:  plane 0 = Y, plane 1 = interleaved UV, plane 2 = unused.
 *   I420:  plane 0 = Y, plane 1 = U, plane 2 = V.
 *   I420P10: same as I420, but every sample is two bytes little-endian.
 *
 * Plane sizes and strides are bytes. Pointers remain valid until
 * rk_mpp_frame_release(), reset, or decoder close.
 */
typedef struct rk_mpp_frame {
    uint32_t struct_size;
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint32_t flags;
    uint32_t reserved0;
    int64_t pts;
    int64_t dts;
    int32_t poc;
    uint32_t err_info;
    const uint8_t *plane_data[3];
    uint64_t plane_sizes[3];
    uint32_t plane_strides[3];
} rk_mpp_frame_t;

typedef struct rk_mpp_decoder rk_mpp_decoder_t;

/*
 * ABI and lifetime rules:
 *   - Every create/destroy pair owns one independent decoder session.
 *   - A decoder handle is not internally synchronized. Use one handle per
 *     thread, or serialize calls externally.
 *   - Input packet bytes are copied before submit returns.
 *   - A returned frame remains decoder-owned until frame_release(), reset, or
 *     close. No other frame may be received before release.
 *   - EOS is sticky. flush() drains a tail, and submitting after EOS is an
 *     error. reset() starts a fresh session that may be reused.
 */
uint32_t rk_mpp_api_version(void);
const char *rk_mpp_api_version_string(void);
const char *rk_mpp_strerror(int code);

int rk_mpp_decoder_config_init(rk_mpp_decoder_config_t *config,
                               uint32_t codec,
                               uint32_t output_format);
int rk_mpp_frame_init(rk_mpp_frame_t *frame);

int rk_mpp_decoder_create(const rk_mpp_decoder_config_t *config,
                          rk_mpp_decoder_t **out_decoder);

/*
 * The decoder copies packet bytes before returning. Packets may contain
 * arbitrary byte chunks for H.264/H.265 and the buffered legacy codecs.
 * VP9/VP8 require one raw coded packet
 * (VP9 may be a superframe) per submit; AV1 requires one temporal unit.
 * IVF/WebM demux belongs to the caller.
 * MPEG-2/MPEG-4/H.263 buffer chunks and emit decoded frames at EOS/flush.
 * EOS flushes a partial Annex-B tail; COMPLETE_ACCESS_UNIT is useful when a
 * caller already has one H.264/H.265 access unit per packet. For exact
 * per-picture timestamps, submit one video access unit or one JPEG per packet.
 * Multiple pictures in one packet inherit that packet's pts/dts.
 */
int rk_mpp_decoder_submit_packet(rk_mpp_decoder_t *decoder,
                                 const uint8_t *data,
                                 uint64_t size,
                                 int64_t pts,
                                 int64_t dts,
                                 uint32_t flags);

/*
 * Returns RK_MPP_OK with one frame, RK_MPP_AGAIN when more input is needed,
 * or RK_MPP_EOS after the flushed stream is fully drained.
 * The caller must release each returned frame before receiving another one.
 */
int rk_mpp_decoder_receive_frame(rk_mpp_decoder_t *decoder,
                                 rk_mpp_frame_t *frame);
/*
 * Duplicate the most recently received frame's dma-buf fd. The caller owns
 * *out_fd and must close it. offset/size are in bytes within that fd and
 * stride is bytes per row. CPU-backed frames return unsupported.
 */
int rk_mpp_decoder_export_frame_dmabuf(rk_mpp_decoder_t *decoder,
                                       uint32_t plane,
                                       int *out_fd,
                                       uint64_t *offset,
                                       uint64_t *size,
                                       uint32_t *stride);
void rk_mpp_frame_release(rk_mpp_decoder_t *decoder,
                          rk_mpp_frame_t *frame);

int rk_mpp_decoder_flush(rk_mpp_decoder_t *decoder);
/* reset() invalidates every previously returned frame and allows reuse. */
int rk_mpp_decoder_reset(rk_mpp_decoder_t *decoder);
int rk_mpp_decoder_recover(rk_mpp_decoder_t *decoder);
int rk_mpp_decoder_status(rk_mpp_decoder_t *decoder,
                          uint32_t *status_a,
                          uint32_t *status_b,
                          uint32_t *status_c);
/* Null-safe; invalidates every outstanding frame from this decoder. */
void rk_mpp_decoder_close(rk_mpp_decoder_t *decoder);

/*
 * The base encoder config selects fixed-QP H.264/H.265 or JPEG. Input frames
 * are tightly packed visible-size NV12 or I420; the SDK pads to coded geometry
 * and copies all input before submit returns. The advanced config may select
 * CBR/VBR/AVBR on VEPU580. RK3576 VEPU510 currently exposes only its
 * board-validated fixed-QP 320x240 QP26 GOP6 paths.
 */
typedef struct rk_mpp_encoder_config {
    uint32_t struct_size;
    uint32_t codec;
    uint32_t input_format;
    uint32_t width;
    uint32_t height;
    uint32_t profile; /* 0/66 baseline, 77 main, 100 high */
    uint32_t level;
    uint32_t gop;
    uint32_t qp;
    uint32_t fps_num;
    uint32_t fps_den;
    uint32_t jpeg_quality;      /* 0..=10 built-in JPEG quantisation tables */
    uint32_t jpeg_restart_mcus; /* 0 disables DRI, otherwise MCUs per restart */
    uint32_t flags;
    uint32_t reserved[3];
} rk_mpp_encoder_config_t;

enum rk_mpp_rate_control_mode {
    RK_MPP_RATE_CONTROL_FIXED_QP = 0,
    RK_MPP_RATE_CONTROL_CBR = 1,
    RK_MPP_RATE_CONTROL_VBR = 2,
    RK_MPP_RATE_CONTROL_AVBR = 3
};

/*
 * Rate control is kept outside the 68-byte V1 encoder config so binaries
 * compiled against the original layout remain safe. All bitrates are bits per
 * second. CBR/VBR/AVBR require min <= target <= max and min_qp <= initial_qp <=
 * max_qp. Fixed-QP ignores the bitrate fields.
 */
typedef struct rk_mpp_encoder_rate_control {
    uint32_t struct_size;
    uint32_t mode;
    int64_t bps_target;
    int64_t bps_min;
    int64_t bps_max;
    uint32_t initial_qp;
    uint32_t min_qp;
    uint32_t max_qp;
    uint32_t reserved[5];
} rk_mpp_encoder_rate_control_t;

/*
 * Separate advanced configuration passed to
 * rk_mpp_encoder_create_advanced(). V1.10 uses the separate V2 type below so
 * the existing initializer never writes beyond this 104-byte structure.
 */
typedef struct rk_mpp_encoder_advanced_config {
    uint32_t struct_size;
    uint32_t flags; /* must be zero in V1.9 */
    rk_mpp_encoder_rate_control_t rate_control;
    uint32_t reserved[8];
} rk_mpp_encoder_advanced_config_t;

#define RK_MPP_MAX_ROI_REGIONS 8u

enum rk_mpp_roi_flags {
    RK_MPP_ROI_FLAG_FORCE_INTRA = 1u << 0,
    RK_MPP_ROI_FLAG_ABSOLUTE_QP = 1u << 1
};

typedef struct rk_mpp_encoder_roi_region {
    uint32_t struct_size;
    uint32_t flags;
    int32_t x;
    int32_t y;
    int32_t width;
    int32_t height;
    int32_t qp; /* absolute QP, or signed relative adjustment */
    uint32_t reserved[3];
} rk_mpp_encoder_roi_region_t;

enum rk_mpp_gdr_mode {
    RK_MPP_GDR_DISABLED = 0,
    RK_MPP_GDR_ROW = 1,
    RK_MPP_GDR_COL = 2
};

typedef struct rk_mpp_encoder_gdr {
    uint32_t struct_size;
    uint32_t mode;
    /* H.264: macroblock rows/columns per refresh; H.265: CTU rows/columns. */
    uint32_t refresh_blocks;
    uint32_t reserved;
} rk_mpp_encoder_gdr_t;

enum rk_mpp_slice_split_mode {
    RK_MPP_SLICE_SPLIT_NONE = 0,
    RK_MPP_SLICE_SPLIT_BY_CTU = 1,
    RK_MPP_SLICE_SPLIT_BY_BYTE = 2
};

enum rk_mpp_slice_output_mode {
    RK_MPP_SLICE_OUTPUT_COMPLETE_ACCESS_UNIT = 0,
    RK_MPP_SLICE_OUTPUT_LOW_DELAY_PACKETS = 1
};

typedef struct rk_mpp_encoder_slice_split {
    uint32_t struct_size;
    uint32_t mode;
    /* H.264 macroblocks or H.265 CTUs for BY_CTU; target bytes for BY_BYTE. */
    uint32_t argument;
    uint32_t output_mode;
    uint32_t reserved[2];
} rk_mpp_encoder_slice_split_t;

/*
 * V1.10 advanced configuration. This separate full-size type avoids changing
 * the 104-byte V1.9 initializer. ROI regions are copied during create.
 */
typedef struct rk_mpp_encoder_advanced_config_v2 {
    uint32_t struct_size;
    uint32_t flags;
    rk_mpp_encoder_rate_control_t rate_control;
    rk_mpp_encoder_gdr_t gdr;
    rk_mpp_encoder_slice_split_t slice_split;
    uint32_t roi_count;
    uint32_t reserved0;
    rk_mpp_encoder_roi_region_t rois[RK_MPP_MAX_ROI_REGIONS];
    uint32_t reserved[8];
} rk_mpp_encoder_advanced_config_v2_t;

enum rk_mpp_encoded_packet_flags {
    RK_MPP_ENCODED_PACKET_FLAG_KEYFRAME = 1u << 0
};

typedef struct rk_mpp_encoded_packet {
    uint32_t struct_size;
    uint32_t flags;
    uint32_t reserved0;
    uint32_t reserved1;
    int64_t pts;
    int64_t dts;
    int32_t frame_num;
    int32_t poc;
    const uint8_t *data;
    uint64_t size;
    uint64_t reserved[2];
} rk_mpp_encoded_packet_t;

typedef struct rk_mpp_encoder rk_mpp_encoder_t;

int rk_mpp_encoder_config_init(rk_mpp_encoder_config_t *config,
                               uint32_t codec,
                               uint32_t width,
                               uint32_t height,
                               uint32_t input_format);
int rk_mpp_encoder_rate_control_init(
    rk_mpp_encoder_rate_control_t *rate_control,
    uint32_t mode);
int rk_mpp_encoder_advanced_config_init(
    rk_mpp_encoder_advanced_config_t *config,
    uint32_t rate_control_mode);
int rk_mpp_encoder_roi_region_init(rk_mpp_encoder_roi_region_t *region);
int rk_mpp_encoder_gdr_init(rk_mpp_encoder_gdr_t *gdr,
                            uint32_t mode,
                            uint32_t refresh_blocks);
int rk_mpp_encoder_slice_split_init(rk_mpp_encoder_slice_split_t *split,
                                    uint32_t mode,
                                    uint32_t argument,
                                    uint32_t output_mode);
int rk_mpp_encoder_advanced_config_v2_init(
    rk_mpp_encoder_advanced_config_v2_t *config,
    uint32_t rate_control_mode);
int rk_mpp_encoded_packet_init(rk_mpp_encoded_packet_t *packet);
int rk_mpp_encoder_create(const rk_mpp_encoder_config_t *config,
                          rk_mpp_encoder_t **out_encoder);
int rk_mpp_encoder_create_advanced(
    const rk_mpp_encoder_config_t *config,
    const rk_mpp_encoder_advanced_config_t *advanced_config,
    rk_mpp_encoder_t **out_encoder);
int rk_mpp_encoder_create_advanced_v2(
    const rk_mpp_encoder_config_t *config,
    const rk_mpp_encoder_advanced_config_v2_t *advanced_config,
    rk_mpp_encoder_t **out_encoder);
int rk_mpp_encoder_submit_frame(rk_mpp_encoder_t *encoder,
                                const uint8_t *data,
                                uint64_t size,
                                int64_t pts,
                                int64_t dts);
int rk_mpp_encoder_submit_frame_dmabuf(rk_mpp_encoder_t *encoder,
                                       int fd,
                                       uint64_t size,
                                       uint32_t hor_stride,
                                       uint32_t ver_stride,
                                       int64_t pts,
                                       int64_t dts);
/*
 * Returns OK with one packet, AGAIN when no packet is currently ready, or EOS
 * after flush and all queued packets have drained. Release each packet before
 * receiving again.
 */
int rk_mpp_encoder_receive_packet(rk_mpp_encoder_t *encoder,
                                  rk_mpp_encoded_packet_t *packet);
void rk_mpp_encoded_packet_release(rk_mpp_encoder_t *encoder,
                                   rk_mpp_encoded_packet_t *packet);
int rk_mpp_encoder_flush(rk_mpp_encoder_t *encoder);
int rk_mpp_encoder_reset(rk_mpp_encoder_t *encoder);
void rk_mpp_encoder_close(rk_mpp_encoder_t *encoder);

/*
 * Legacy single-JPEG inference helper kept for rknn-fastinfer compatibility.
 * New callers should use the rk_mpp_decoder_* API.
 */
typedef struct RkDec RkDec;
RkDec *rk_jpeg_open(const char *path,
                    uint32_t dst_w,
                    uint32_t dst_h,
                    uint32_t *out_w,
                    uint32_t *out_h);
int rk_jpeg_frame(RkDec *handle,
                  uint8_t *dst,
                  int32_t *pad_left,
                  int32_t *pad_top,
                  float *scale);
void rk_jpeg_close(RkDec *handle);

#ifdef __cplusplus
}
#endif

#endif
