#ifndef RK_MPP_RS_H
#define RK_MPP_RS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define RK_MPP_API_VERSION_MAJOR 1u
#define RK_MPP_API_VERSION_MINOR 1u
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
    RK_MPP_CODEC_H265 = 3
};

enum rk_mpp_pixel_format {
    RK_MPP_PIXEL_FORMAT_NV12 = 1,
    RK_MPP_PIXEL_FORMAT_I420 = 2,
    /* Planar 4:2:0, little-endian 16-bit samples in all three planes. */
    RK_MPP_PIXEL_FORMAT_I420P10 = 3
};

/*
 * H.264/MJPEG accept NV12 or I420. H.265 accepts NV12/I420 for 8-bit streams
 * and I420P10 for Main10; incompatible format/stream combinations are errors.
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
 * arbitrary byte chunks. EOS flushes a partial tail; COMPLETE_ACCESS_UNIT is
 * useful when a caller already has one H.264/H.265 access unit per packet. For
 * exact per-picture timestamps, submit one video access unit or one JPEG per
 * packet. Multiple pictures in one packet inherit that packet's pts/dts.
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
