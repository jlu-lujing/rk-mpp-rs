#define _GNU_SOURCE

#include <rk_mpi.h>

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static uint16_t le16(const uint8_t *p) {
    return (uint16_t)p[0] | (uint16_t)p[1] << 8;
}

static uint32_t le32(const uint8_t *p) {
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 |
           (uint32_t)p[3] << 24;
}

static uint64_t le64(const uint8_t *p) {
    return (uint64_t)le32(p) | (uint64_t)le32(p + 4) << 32;
}

static uint32_t align_up(uint32_t value, uint32_t alignment) {
    return (value + alignment - 1) & ~(alignment - 1);
}

static size_t output_layout(int coding, uint32_t width, uint32_t height,
                            uint32_t *hor_stride, uint32_t *ver_stride) {
    *ver_stride = align_up(height, 16);
    if (coding == MPP_VIDEO_CodingVP9) {
        *hor_stride = ((width + 255) & ~255u) | 256u;
        return (size_t)*hor_stride * *ver_stride * 9 / 5;
    }
    *hor_stride = align_up(width, 16);
    return (size_t)*hor_stride * *ver_stride * 3 / 2;
}

static int drain_frames(MppApi *mpi, MppCtx ctx, FILE *output,
                        MppFrame expected_frame, MppBuffer expected_buffer,
                        uint32_t expected_hor_stride,
                        size_t expected_buffer_size, int *returned_any) {
    int frames = 0;

    for (;;) {
        MppFrame frame = NULL;
        MPP_RET ret = mpi->decode_get_frame(ctx, &frame);
        if (ret != MPP_OK) {
            fprintf(stderr, "decode_get_frame failed: %d\n", ret);
            return -1;
        }
        if (!frame)
            break;
        if (mpp_frame_get_eos(frame)) {
            mpp_frame_deinit(&frame);
            break;
        }

        if (returned_any)
            *returned_any = 1;
        if (expected_frame && frames == 0 && frame != expected_frame) {
            fprintf(stderr, "advanced output frame identity changed: %p != %p\n",
                    frame, expected_frame);
            mpp_frame_deinit(&frame);
            return -1;
        }
        if (expected_buffer && frames == 0 &&
            mpp_frame_get_buffer(frame) != expected_buffer) {
            fprintf(stderr, "advanced output buffer identity changed\n");
            mpp_frame_deinit(&frame);
            return -1;
        }
        if (expected_frame && frames == 0 &&
            (mpp_frame_get_hor_stride(frame) != expected_hor_stride ||
             mpp_frame_get_buf_size(frame) < expected_buffer_size)) {
            fprintf(stderr,
                    "advanced output fell back from injected DMA-BUF: "
                    "stride %u/%u buffer %zu/%zu\n",
                    mpp_frame_get_hor_stride(frame), expected_hor_stride,
                    mpp_frame_get_buf_size(frame), expected_buffer_size);
            mpp_frame_deinit(&frame);
            return -1;
        }

        MppBuffer buffer = mpp_frame_get_buffer(frame);
        uint8_t *base = mpp_buffer_get_ptr(buffer);
        uint32_t width = mpp_frame_get_width(frame);
        uint32_t height = mpp_frame_get_height(frame);
        uint32_t hor_stride = mpp_frame_get_hor_stride(frame);
        uint32_t ver_stride = mpp_frame_get_ver_stride(frame);
        if (!base || !width || !height || hor_stride < width ||
            mpp_frame_get_fmt(frame) != MPP_FMT_YUV420SP) {
            fprintf(stderr, "invalid frame: %ux%u stride %ux%u fmt %u\n",
                    width, height, hor_stride, ver_stride,
                    mpp_frame_get_fmt(frame));
            mpp_frame_deinit(&frame);
            return -1;
        }

        for (uint32_t row = 0; row < height; row++) {
            if (fwrite(base + row * hor_stride, 1, width, output) != width) {
                mpp_frame_deinit(&frame);
                return -1;
            }
        }
        base += hor_stride * ver_stride;
        for (uint32_t row = 0; row < height / 2; row++) {
            if (fwrite(base + row * hor_stride, 1, width, output) != width) {
                mpp_frame_deinit(&frame);
                return -1;
            }
        }

        frames++;
        mpp_frame_deinit(&frame);
    }

    return frames;
}

static int decode_packet(MppApi *mpi, MppCtx ctx, MppBufferGroup group,
                         const uint8_t *data, size_t size, int64_t pts,
                         uint32_t width, uint32_t height, size_t buffer_size,
                         uint32_t hor_stride, uint32_t ver_stride,
                         FILE *output, MppBuffer *retained_buffer) {
    MppPacket packet = NULL;
    MppBuffer output_buffer = NULL;
    MppFrame output_frame = NULL;
    int returned_any = 0;
    int frames = -1;

    *retained_buffer = NULL;
    if (mpp_buffer_get(group, &output_buffer, buffer_size) != MPP_OK ||
        mpp_frame_init(&output_frame) != MPP_OK ||
        mpp_packet_init(&packet, (void *)data, size) != MPP_OK) {
        fprintf(stderr, "advanced packet allocation failed\n");
        goto done;
    }
    mpp_frame_set_width(output_frame, width);
    mpp_frame_set_height(output_frame, height);
    mpp_frame_set_hor_stride(output_frame, hor_stride);
    mpp_frame_set_ver_stride(output_frame, ver_stride);
    mpp_frame_set_fmt(output_frame, MPP_FMT_YUV420SP);
    mpp_frame_set_buffer(output_frame, output_buffer);
    if (mpp_meta_set_frame(mpp_packet_get_meta(packet), KEY_OUTPUT_FRAME,
                           output_frame) != MPP_OK) {
        fprintf(stderr, "advanced output frame metadata failed\n");
        goto done;
    }
    mpp_packet_set_pts(packet, pts);
    if (mpi->decode_put_packet(ctx, packet) != MPP_OK ||
        mpp_packet_get_length(packet) != 0) {
        fprintf(stderr, "decode_put_packet failed\n");
        goto done;
    }
    frames =
        drain_frames(mpi, ctx, output, output_frame, output_buffer,
                     hor_stride, buffer_size, &returned_any);

done:
    mpp_packet_deinit(&packet);
    if (output_frame) {
        if (returned_any)
            output_frame = NULL;
        else
            mpp_frame_deinit(&output_frame);
    }
    if (output_buffer) {
        if (frames >= 0)
            *retained_buffer = output_buffer;
        else
            mpp_buffer_put(output_buffer);
    }
    return frames;
}

static int run_packets(MppApi *mpi, MppCtx ctx, MppBufferGroup group,
                       const uint8_t *data, size_t size, uint32_t width,
                       uint32_t height, size_t buffer_size, uint32_t hor_stride,
                       uint32_t ver_stride, FILE *output, int *decoded_frames) {
    int is_ivf =
        size >= 32 && !memcmp(data, "DKIF", 4);
    size_t offset;
    uint64_t sequence = 1;
    MppBuffer *retained = NULL;
    size_t retained_count = 0;
    size_t retained_capacity = 0;
    int result = -1;

    *decoded_frames = 0;
    if (is_ivf) {
        size_t header_size = le16(data + 6);

        if (header_size < 32 || header_size > size) {
            fprintf(stderr, "invalid IVF header size\n");
            goto done;
        }
        offset = header_size;
    } else {
        offset = 0;
    }

    do {
        const uint8_t *packet_data;
        size_t packet_size;
        uint64_t pts = sequence;
        int frames;
        MppBuffer retained_buffer = NULL;

        if (is_ivf) {
            size_t next;

            if (size - offset < 12) {
                fprintf(stderr, "truncated IVF frame header\n");
                goto done;
            }
            packet_size = le32(data + offset);
            pts = le64(data + offset + 4);
            next = offset + 12;
            if (packet_size > size - next) {
                fprintf(stderr, "truncated IVF packet\n");
                goto done;
            }
            packet_data = data + next;
            offset = next + packet_size;
        } else {
            packet_data = data;
            packet_size = size;
            offset = size;
        }

        frames = decode_packet(mpi, ctx, group, packet_data, packet_size,
                               (int64_t)pts, width, height, buffer_size,
                               hor_stride, ver_stride, output,
                               &retained_buffer);
        if (frames < 0)
            goto done;
        if (retained_buffer) {
            if (retained_count == retained_capacity) {
                size_t capacity = retained_capacity ? retained_capacity * 2 : 16;
                MppBuffer *grown =
                    realloc(retained, capacity * sizeof(*retained));

                if (!grown) {
                    fprintf(stderr, "output buffer list allocation failed\n");
                    mpp_buffer_put(retained_buffer);
                    goto done;
                }
                retained = grown;
                retained_capacity = capacity;
            }
            retained[retained_count++] = retained_buffer;
        }
        *decoded_frames += frames;
        sequence++;
    } while (is_ivf && offset < size);

    {
        MppPacket packet = NULL;
        int tail;

        if (mpp_packet_init(&packet, NULL, 0) != MPP_OK ||
            mpp_packet_set_eos(packet) != MPP_OK ||
            mpi->decode_put_packet(ctx, packet) != MPP_OK) {
            fprintf(stderr, "EOS submit failed\n");
            mpp_packet_deinit(&packet);
            goto done;
        }
        mpp_packet_deinit(&packet);
        tail = drain_frames(mpi, ctx, output, NULL, NULL, 0, 0, NULL);
        if (tail < 0)
            goto done;
        *decoded_frames += tail;
    }
    result = 0;

done:
    for (size_t i = 0; i < retained_count; i++)
        mpp_buffer_put(retained[i]);
    free(retained);
    return result;
}

static int object_api_checks(void) {
    MppBufferGroup group = NULL;
    MppBuffer buffer = NULL;
    MppBuffer original_buffer = NULL;
    MppPacket packet = NULL;
    MppPacket packet_copy = NULL;
    MppFrame frame = NULL;
    MppBufferGroup external_group = NULL;
    MppBuffer external_buffer = NULL;
    MppBuffer original_external_buffer = NULL;
    MppBuffer imported_buffer = NULL;
    MppBufferInfo info = {0};
    MppBufferInfo external_info = {0};
    uint8_t source[16];
    uint8_t copied[16] = {0};
    uint8_t external[64] = {0};
    uint8_t packet_data[8] = {0, 1, 2, 3, 4, 5, 6, 7};
    MPP_RET ret;

    for (int i = 0; i < 16; i++)
        source[i] = (uint8_t)(0xa0 + i);

    ret = mpp_buffer_group_get_internal(&group, MPP_BUFFER_TYPE_DMA_HEAP);
    if (ret != MPP_OK || !group)
        return -1;
    ret = mpp_buffer_get(group, &buffer, 64);
    if (ret != MPP_OK || !buffer || mpp_buffer_get_size(buffer) != 64) {
        fprintf(stderr, "internal buffer allocation failed: ret %d size %zu\n",
                ret, buffer ? mpp_buffer_get_size(buffer) : 0);
        return -1;
    }
    original_buffer = buffer;
    if (mpp_buffer_info_get(buffer, &info) != MPP_OK || info.size != 64 ||
        info.ptr != mpp_buffer_get_ptr(buffer) ||
        mpp_buffer_get_fd(buffer) < 0 || info.fd != mpp_buffer_get_fd(buffer) ||
        info.type != MPP_BUFFER_TYPE_DMA_HEAP ||
        mpp_buffer_group_mode(group) != MPP_BUFFER_INTERNAL ||
        mpp_buffer_group_type(group) != MPP_BUFFER_TYPE_DMA_HEAP) {
        fprintf(stderr,
                "internal buffer info failed: type %d size %zu fd %d "
                "mode %d group_type %d\n",
                info.type, info.size, info.fd, mpp_buffer_group_mode(group),
                mpp_buffer_group_type(group));
        return -1;
    }
    if (mpp_buffer_sync_begin(buffer) != MPP_OK ||
        mpp_buffer_sync_end(buffer) != MPP_OK ||
        mpp_buffer_write(buffer, 7, source, sizeof(source)) != MPP_OK ||
        mpp_buffer_read(buffer, 7, copied, sizeof(copied)) != MPP_OK ||
        memcmp(source, copied, sizeof(source)) != 0) {
        fprintf(stderr, "internal buffer sync/read/write failed\n");
        return -1;
    }
    if (mpp_buffer_inc_ref(buffer) != MPP_OK ||
        mpp_buffer_put(buffer) != MPP_OK ||
        mpp_buffer_put(buffer) != MPP_OK) {
        fprintf(stderr, "internal buffer refcount failed\n");
        return -1;
    }
    if (mpp_buffer_group_unused(group) != 1 ||
        mpp_buffer_group_usage(group) != 64 ||
        mpp_buffer_get(group, &buffer, 64) != MPP_OK ||
        buffer != original_buffer ||
        mpp_buffer_put(buffer) != MPP_OK) {
        fprintf(stderr,
                "internal buffer recycle failed: unused %d usage %zu "
                "buffer %p original %p\n",
                mpp_buffer_group_unused(group), mpp_buffer_group_usage(group),
                buffer, original_buffer);
        return -1;
    }

    if (mpp_packet_init(&packet, packet_data, sizeof(packet_data)) != MPP_OK ||
        packet == NULL)
        return -1;
    mpp_packet_set_pts(packet, 1234);
    mpp_packet_set_pos(packet, packet_data + 2);
    mpp_packet_set_length(packet, 4);
    if (mpp_packet_get_pts(packet) != 1234 ||
        mpp_packet_get_pos(packet) != packet_data + 2 ||
        mpp_packet_get_length(packet) != 4 ||
        mpp_packet_get_size(packet) != sizeof(packet_data)) {
        return -1;
    }
    if (mpp_packet_copy_init(&packet_copy, packet) != MPP_OK ||
        mpp_packet_get_pts(packet_copy) != 1234 ||
        mpp_packet_get_length(packet_copy) != 4 ||
        memcmp(mpp_packet_get_data(packet_copy), packet_data + 2, 4) != 0) {
        return -1;
    }
    if (mpp_packet_deinit(&packet_copy) != MPP_OK)
        return -1;
    if (mpp_packet_deinit(&packet) != MPP_OK || packet != NULL)
        return -1;

    if (mpp_frame_init(&frame) != MPP_OK || !frame)
        return -1;
    mpp_frame_set_width(frame, 64);
    mpp_frame_set_height(frame, 48);
    mpp_frame_set_hor_stride(frame, 64);
    mpp_frame_set_ver_stride(frame, 48);
    mpp_frame_set_fmt(frame, MPP_FMT_YUV420SP);
    if (mpp_frame_get_width(frame) != 64 ||
        mpp_frame_get_height(frame) != 48 ||
        mpp_frame_get_fmt(frame) != MPP_FMT_YUV420SP) {
        return -1;
    }
    if (mpp_frame_deinit(&frame) != MPP_OK || frame != NULL)
        return -1;

    external_info.type = MPP_BUFFER_TYPE_NORMAL;
    external_info.size = sizeof(external);
    external_info.ptr = external;
    external_info.fd = -1;
    external_info.index = 23;
    if (mpp_buffer_group_get_external(&external_group,
                                      MPP_BUFFER_TYPE_NORMAL) != MPP_OK ||
        external_group == NULL) {
        fprintf(stderr, "external group creation failed\n");
        return -1;
    }
    if (mpp_buffer_commit(external_group, &external_info) != MPP_OK ||
        mpp_buffer_group_unused(external_group) != 1) {
        fprintf(stderr, "external commit failed: unused %d\n",
                mpp_buffer_group_unused(external_group));
        return -1;
    }
    if (mpp_buffer_get(external_group, &external_buffer,
                       sizeof(external)) != MPP_OK ||
        external_buffer == NULL) {
        fprintf(stderr, "external get failed\n");
        return -1;
    }
    original_external_buffer = external_buffer;
    if (mpp_buffer_get_ptr(external_buffer) != external ||
        mpp_buffer_get_size(external_buffer) != sizeof(external) ||
        mpp_buffer_get_index(external_buffer) != 23 ||
        mpp_buffer_info_get(external_buffer, &info) != MPP_OK ||
        info.type != MPP_BUFFER_TYPE_NORMAL || info.ptr != external ||
        info.size != sizeof(external) || info.fd != -1 ||
        info.index != 23) {
        fprintf(stderr,
                "external info failed: type %d size %zu fd %d index %d\n",
                info.type, info.size, info.fd, info.index);
        return -1;
    }
    if (mpp_buffer_write(external_buffer, 3, source, sizeof(source)) != MPP_OK ||
        memcmp(external + 3, source, sizeof(source)) != 0 ||
        mpp_buffer_put(external_buffer) != MPP_OK) {
        fprintf(stderr, "external write failed\n");
        return -1;
    }
    if (mpp_buffer_group_unused(external_group) != 1 ||
        mpp_buffer_group_usage(external_group) != sizeof(external) ||
        mpp_buffer_get(external_group, &external_buffer,
                       sizeof(external)) != MPP_OK ||
        external_buffer != original_external_buffer ||
        mpp_buffer_get_ptr(external_buffer) != external ||
        mpp_buffer_put(external_buffer) != MPP_OK) {
        fprintf(stderr,
                "external recycle failed: unused %d usage %zu buffer %p "
                "original %p\n",
                mpp_buffer_group_unused(external_group),
                mpp_buffer_group_usage(external_group), external_buffer,
                original_external_buffer);
        return -1;
    }
    if (mpp_buffer_group_put(external_group) != MPP_OK) {
        return -1;
    }

    external_info.index = 29;
    if (mpp_buffer_import(&imported_buffer, &external_info) != MPP_OK ||
        imported_buffer == NULL ||
        mpp_buffer_get_ptr(imported_buffer) != external ||
        mpp_buffer_get_fd(imported_buffer) != -1 ||
        mpp_buffer_get_index(imported_buffer) != 29 ||
        mpp_buffer_read(imported_buffer, 3, copied, sizeof(copied)) != MPP_OK ||
        memcmp(copied, source, sizeof(source)) != 0 ||
        mpp_buffer_put(imported_buffer) != MPP_OK) {
        return -1;
    }

    int external_fd = memfd_create("rk-mpp-external", MFD_CLOEXEC);
    if (external_fd < 0 || ftruncate(external_fd, sizeof(external)) != 0) {
        if (external_fd >= 0)
            close(external_fd);
        return -1;
    }
    external_info.ptr = NULL;
    external_info.type = MPP_BUFFER_TYPE_EXT_DMA;
    external_info.size = sizeof(external);
    external_info.fd = external_fd;
    external_info.index = 31;
    if (mpp_buffer_import(&imported_buffer, &external_info) != MPP_OK ||
        imported_buffer == NULL ||
        mpp_buffer_get_fd(imported_buffer) != external_fd ||
        mpp_buffer_get_ptr(imported_buffer) == NULL ||
        mpp_buffer_write(imported_buffer, 5, source, sizeof(source)) != MPP_OK ||
        mpp_buffer_read(imported_buffer, 5, copied, sizeof(copied)) != MPP_OK ||
        memcmp(copied, source, sizeof(source)) != 0 ||
        mpp_buffer_info_get(imported_buffer, &info) != MPP_OK ||
        info.type != MPP_BUFFER_TYPE_EXT_DMA ||
        info.fd != external_fd || info.index != 31 ||
        mpp_buffer_put(imported_buffer) != MPP_OK) {
        close(external_fd);
        return -1;
    }
    close(external_fd);

    if (mpp_buffer_group_put(group) != MPP_OK)
        return -1;
    return 0;
}

int main(int argc, char **argv) {
    MppCtx ctx = NULL;
    MppApi *mpi = NULL;
    MppBufferGroup output_group = NULL;
    FILE *input = NULL;
    FILE *output = NULL;
    uint8_t *data = NULL;
    long size;
    int frames;
    int coding;
    int expected_frames;
    int width;
    int height;
    uint32_t hor_stride;
    uint32_t ver_stride;
    size_t buffer_size;
    int rc = 1;

    if (argc != 7) {
        fprintf(stderr,
                "usage: %s <stream> <out.nv12> <coding> <frames> <width> <height>\n",
                argv[0]);
        return 2;
    }
    coding = atoi(argv[3]);
    expected_frames = atoi(argv[4]);
    width = atoi(argv[5]);
    height = atoi(argv[6]);
    if (coding <= 0 || expected_frames <= 0 || width <= 0 || height <= 0) {
        fprintf(stderr, "invalid coding, frame count or geometry\n");
        return 2;
    }
    if (object_api_checks()) {
        fprintf(stderr, "packet/frame/buffer object checks failed\n");
        return 1;
    }

    input = fopen(argv[1], "rb");
    output = fopen(argv[2], "wb");
    if (!input || !output) {
        perror("open");
        goto done;
    }
    if (fseek(input, 0, SEEK_END) || (size = ftell(input)) < 0 ||
        fseek(input, 0, SEEK_SET)) {
        perror("seek");
        goto done;
    }
    data = malloc((size_t)size);
    if (!data || fread(data, 1, (size_t)size, input) != (size_t)size) {
        perror("read");
        goto done;
    }

    if (mpp_create(&ctx, &mpi) != MPP_OK ||
        mpp_init(ctx, MPP_CTX_DEC, coding) != MPP_OK ||
        mpp_buffer_group_get_internal(&output_group,
                                      MPP_BUFFER_TYPE_DMA_HEAP) != MPP_OK) {
        fprintf(stderr, "decoder initialization failed\n");
        goto done;
    }
    buffer_size = output_layout(coding, (uint32_t)width, (uint32_t)height,
                                &hor_stride, &ver_stride);
    if (!buffer_size ||
        run_packets(mpi, ctx, output_group, data, (size_t)size,
                    (uint32_t)width, (uint32_t)height, buffer_size, hor_stride,
                    ver_stride, output, &frames)) {
        goto done;
    }

    if (frames != expected_frames) {
        fprintf(stderr, "decoded %d frames, expected %d\n", frames,
                expected_frames);
        goto done;
    }
    printf("PASS official packet/frame/buffer API decoded %d frames (coding %d)\n",
           frames, coding);
    rc = 0;

done:
    mpp_buffer_group_put(output_group);
    if (ctx && mpp_destroy(ctx) != MPP_OK)
        rc = 1;
    free(data);
    if (input)
        fclose(input);
    if (output)
        fclose(output);
    return rc;
}
