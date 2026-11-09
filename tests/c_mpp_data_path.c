#define _GNU_SOURCE

#include <rk_mpi.h>

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

static int drain_frames(MppApi *mpi, MppCtx ctx, FILE *output) {
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

static int object_api_checks(void) {
    MppBufferGroup group = NULL;
    MppBuffer buffer = NULL;
    MppPacket packet = NULL;
    MppPacket packet_copy = NULL;
    MppFrame frame = NULL;
    MppBufferGroup external_group = NULL;
    MppBuffer external_buffer = NULL;
    MppBuffer imported_buffer = NULL;
    MppBufferInfo info;
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
    if (ret != MPP_OK || !buffer || mpp_buffer_get_size(buffer) != 64)
        return -1;
    if (mpp_buffer_info_get(buffer, &info) != MPP_OK || info.size != 64 ||
        info.ptr != mpp_buffer_get_ptr(buffer) ||
        mpp_buffer_get_fd(buffer) != -1 ||
        mpp_buffer_group_mode(group) != MPP_BUFFER_INTERNAL ||
        mpp_buffer_group_type(group) != MPP_BUFFER_TYPE_DMA_HEAP) {
        return -1;
    }
    if (mpp_buffer_write(buffer, 7, source, sizeof(source)) != MPP_OK ||
        mpp_buffer_read(buffer, 7, copied, sizeof(copied)) != MPP_OK ||
        memcmp(source, copied, sizeof(source)) != 0) {
        return -1;
    }
    if (mpp_buffer_inc_ref(buffer) != MPP_OK ||
        mpp_buffer_put(buffer) != MPP_OK ||
        mpp_buffer_put(buffer) != MPP_OK) {
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
        external_group == NULL ||
        mpp_buffer_commit(external_group, &external_info) != MPP_OK ||
        mpp_buffer_group_unused(external_group) != 1 ||
        mpp_buffer_get(external_group, &external_buffer,
                       sizeof(external)) != MPP_OK ||
        external_buffer == NULL ||
        mpp_buffer_get_ptr(external_buffer) != external ||
        mpp_buffer_get_size(external_buffer) != sizeof(external) ||
        mpp_buffer_get_index(external_buffer) != 23 ||
        mpp_buffer_info_get(external_buffer, &info) != MPP_OK ||
        info.type != MPP_BUFFER_TYPE_NORMAL || info.ptr != external ||
        info.size != sizeof(external) || info.fd != -1 ||
        info.index != 23 ||
        mpp_buffer_write(external_buffer, 3, source, sizeof(source)) != MPP_OK ||
        memcmp(external + 3, source, sizeof(source)) != 0 ||
        mpp_buffer_put(external_buffer) != MPP_OK ||
        mpp_buffer_group_unused(external_group) != 0 ||
        mpp_buffer_group_put(external_group) != MPP_OK) {
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
    MppPacket packet = NULL;
    FILE *input = NULL;
    FILE *output = NULL;
    uint8_t *data = NULL;
    long size;
    int frames;
    int coding;
    int expected_frames;
    int rc = 1;

    if (argc != 5) {
        fprintf(stderr, "usage: %s <stream> <out.nv12> <coding> <frames>\n",
                argv[0]);
        return 2;
    }
    coding = atoi(argv[3]);
    expected_frames = atoi(argv[4]);
    if (coding <= 0 || expected_frames <= 0) {
        fprintf(stderr, "invalid coding or frame count\n");
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
        mpp_packet_init(&packet, data, (size_t)size) != MPP_OK) {
        fprintf(stderr, "decoder initialization failed\n");
        goto done;
    }
    mpp_packet_set_pts(packet, 1);
    if (mpi->decode_put_packet(ctx, packet) != MPP_OK ||
        mpp_packet_get_length(packet) != 0) {
        fprintf(stderr, "decode_put_packet failed\n");
        goto done;
    }
    frames = drain_frames(mpi, ctx, output);
    if (frames < 0)
        goto done;

    mpp_packet_deinit(&packet);
    if (mpp_packet_init(&packet, NULL, 0) != MPP_OK ||
        mpp_packet_set_eos(packet) != MPP_OK ||
        mpi->decode_put_packet(ctx, packet) != MPP_OK) {
        fprintf(stderr, "EOS submit failed\n");
        goto done;
    }
    int tail = drain_frames(mpi, ctx, output);
    if (tail < 0)
        goto done;
    frames += tail;

    if (frames != expected_frames) {
        fprintf(stderr, "decoded %d frames, expected %d\n", frames,
                expected_frames);
        goto done;
    }
    printf("PASS official packet/frame/buffer API decoded %d frames (coding %d)\n",
           frames, coding);
    rc = 0;

done:
    mpp_packet_deinit(&packet);
    if (ctx && mpp_destroy(ctx) != MPP_OK)
        rc = 1;
    free(data);
    if (input)
        fclose(input);
    if (output)
        fclose(output);
    return rc;
}
