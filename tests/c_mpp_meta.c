#include <rk_mpi.h>

#include <stdint.h>
#include <stdio.h>
#include <string.h>

/* Some deployed RK3576 headers predate the public duplicate declaration. */
MppMeta mpp_meta_dup(MppMeta meta);
MPP_RET mpp_meta_inc_ref(MppMeta meta);

#define CHECK(condition)                                                        \
    do {                                                                        \
        if (!(condition)) {                                                     \
            fprintf(stderr, "CHECK failed at %s:%d: %s\n", __FILE__, __LINE__, \
                    #condition);                                                \
            return 1;                                                           \
        }                                                                       \
    } while (0)

static int test_scalar_meta(MppMeta meta) {
    RK_S64 s64 = 0;
    RK_S32 value = 0;
    MppMeta duplicate;

    CHECK(mpp_meta_size(meta) == 0);
    CHECK(mpp_meta_set_s32(meta, KEY_TEMPORAL_ID, 37) == MPP_OK);
    CHECK(mpp_meta_size(meta) == 1);
    CHECK(mpp_meta_get_s32(meta, KEY_TEMPORAL_ID, &value) == MPP_OK);
    CHECK(value == 37);
    CHECK(mpp_meta_size(meta) == 0);
    CHECK(mpp_meta_get_s32(meta, KEY_TEMPORAL_ID, &value) == MPP_NOK);

    value = -1;
    CHECK(mpp_meta_get_s32_d(meta, KEY_TEMPORAL_ID, &value, 19) == MPP_NOK);
    CHECK(value == 19);
    CHECK(mpp_meta_get_s64(meta, KEY_TEMPORAL_ID, &s64) == MPP_NOK);

    CHECK(mpp_meta_set_s64(meta, KEY_ENC_SSE, -1234567890123LL) == MPP_OK);
    CHECK(mpp_meta_get_s64(meta, KEY_ENC_SSE, &s64) == MPP_OK);
    CHECK(s64 == -1234567890123LL);

    CHECK(mpp_meta_set_s32(meta, KEY_TEMPORAL_ID, 41) == MPP_OK);
    duplicate = mpp_meta_dup(meta);
    CHECK(duplicate != NULL);
    CHECK(mpp_meta_get_s32(meta, KEY_TEMPORAL_ID, &value) == MPP_OK);
    CHECK(value == 41);
    CHECK(mpp_meta_size(duplicate) == 1);
    CHECK(mpp_meta_get_s32(duplicate, KEY_TEMPORAL_ID, &value) == MPP_OK);
    CHECK(value == 41);
    CHECK(mpp_meta_put(duplicate) == MPP_OK);
    return 0;
}

static int test_pointer_meta(MppMeta meta) {
    static const char expected[] = "mpp-meta-payload";
    static const uint8_t expected_uuid[16] = {
        0x66, 0x72, 0x6d, 0x00, 0x63, 0x66, 0x67, 0x2d,
        0x75, 0x64, 0x73, 0x00, 0xa5, 0x5a, 0x19, 0x26,
    };
    static const char expected_set_0[] = "first";
    static const char expected_set_1[] = "second";
    MppEncUserData user_data = {
        .len = sizeof(expected),
        .pdata = NULL,
    };
    MppEncUserData *user_data_get = NULL;
    MppEncUserDataSet user_set;
    MppEncUserDataSet *user_set_get = NULL;
    MppEncUserDataFull full[2];
    char payload[sizeof(expected)];
    char set_payload_0[sizeof(expected_set_0)];
    char set_payload_1[sizeof(expected_set_1)];
    uint8_t uuid[16];
    void *pointer = NULL;
    int marker = 7;

    CHECK(mpp_meta_set_ptr(meta, KEY_ROI_DATA, &marker) == MPP_OK);
    CHECK(mpp_meta_get_ptr(meta, KEY_ROI_DATA, &pointer) == MPP_OK);
    CHECK(pointer == &marker);
    pointer = &marker;
    CHECK(mpp_meta_get_ptr_d(meta, KEY_ROI_DATA, &pointer, NULL) == MPP_NOK);
    CHECK(pointer == NULL);
    CHECK(mpp_meta_set_ptr(meta, KEY_USER_DATA, NULL) == MPP_OK);
    pointer = &marker;
    CHECK(mpp_meta_get_ptr(meta, KEY_USER_DATA, &pointer) == MPP_OK);
    CHECK(pointer == NULL);

    memcpy(payload, expected, sizeof(payload));
    user_data.pdata = payload;
    CHECK(mpp_meta_set_ptr(meta, KEY_USER_DATA, &user_data) == MPP_OK);
    memset(payload, 'x', sizeof(payload));
    CHECK(mpp_meta_get_ptr(meta, KEY_USER_DATA, (void **)&user_data_get) ==
          MPP_OK);
    CHECK(user_data_get != NULL);
    CHECK(user_data_get->len == sizeof(expected));
    CHECK(memcmp(user_data_get->pdata, expected, sizeof(expected)) == 0);

    memcpy(set_payload_0, expected_set_0, sizeof(set_payload_0));
    memcpy(set_payload_1, expected_set_1, sizeof(set_payload_1));
    memcpy(uuid, expected_uuid, sizeof(uuid));
    full[0].len = sizeof(expected_set_0);
    full[1].len = sizeof(expected_set_1);
    full[0].uuid = uuid;
    full[1].uuid = uuid;
    full[0].pdata = set_payload_0;
    full[1].pdata = set_payload_1;
    user_set.count = 2;
    user_set.datas = full;

    CHECK(mpp_meta_set_ptr(meta, KEY_USER_DATAS, &user_set) == MPP_OK);
    memset(set_payload_0, 'x', sizeof(set_payload_0));
    memset(set_payload_1, 'y', sizeof(set_payload_1));
    memset(uuid, 0, sizeof(uuid));
    CHECK(mpp_meta_get_ptr(meta, KEY_USER_DATAS, (void **)&user_set_get) ==
          MPP_OK);
    CHECK(user_set_get != NULL);
    CHECK(user_set_get->count == 2);
    CHECK(user_set_get->datas != NULL);
    CHECK(user_set_get->datas[0].len == sizeof(expected_set_0));
    CHECK(user_set_get->datas[1].len == sizeof(expected_set_1));
    CHECK(memcmp(user_set_get->datas[0].uuid, expected_uuid,
                 sizeof(expected_uuid)) == 0);
    CHECK(memcmp(user_set_get->datas[1].uuid, expected_uuid,
                 sizeof(expected_uuid)) == 0);
    CHECK(memcmp(user_set_get->datas[0].pdata, expected_set_0,
                 sizeof(expected_set_0)) == 0);
    CHECK(memcmp(user_set_get->datas[1].pdata, expected_set_1,
                 sizeof(expected_set_1)) == 0);
    return 0;
}

static int test_object_meta(MppMeta meta) {
    MppBufferGroup group = NULL;
    MppBuffer buffer = NULL;
    MppBuffer buffer_get = NULL;
    MppPacket packet = NULL;
    MppPacket packet_get = NULL;
    MppFrame frame = NULL;
    MppFrame frame_get = NULL;
    uint8_t bytes[4] = {0};

    CHECK(mpp_buffer_group_get_internal(&group, MPP_BUFFER_TYPE_DMA_HEAP) ==
          MPP_OK);
    CHECK(mpp_buffer_get(group, &buffer, 64) == MPP_OK);
    CHECK(mpp_packet_init(&packet, bytes, sizeof(bytes)) == MPP_OK);
    CHECK(mpp_frame_init(&frame) == MPP_OK);

    CHECK(mpp_meta_set_buffer(meta, KEY_MOTION_INFO, buffer) == MPP_OK);
    CHECK(mpp_meta_get_buffer(meta, KEY_MOTION_INFO, &buffer_get) == MPP_OK);
    CHECK(buffer_get == buffer);
    CHECK(mpp_meta_set_packet(meta, KEY_OUTPUT_PACKET, packet) == MPP_OK);
    CHECK(mpp_meta_get_packet(meta, KEY_OUTPUT_PACKET, &packet_get) == MPP_OK);
    CHECK(packet_get == packet);
    CHECK(mpp_meta_set_frame(meta, KEY_OUTPUT_FRAME, frame) == MPP_OK);
    CHECK(mpp_meta_get_frame(meta, KEY_OUTPUT_FRAME, &frame_get) == MPP_OK);
    CHECK(frame_get == frame);
    CHECK(mpp_meta_set_frame(meta, KEY_TEMPORAL_ID, frame) == MPP_NOK);

    buffer_get = buffer;
    CHECK(mpp_meta_get_buffer_d(meta, KEY_MOTION_INFO, &buffer_get, NULL) ==
          MPP_NOK);
    CHECK(buffer_get == NULL);

    CHECK(mpp_packet_deinit(&packet) == MPP_OK);
    CHECK(mpp_frame_deinit(&frame) == MPP_OK);
    CHECK(mpp_buffer_put(buffer) == MPP_OK);
    CHECK(mpp_buffer_group_put(group) == MPP_OK);
    return 0;
}

static int test_attached_meta(void) {
    MppFrame frame = NULL;
    MppPacket packet = NULL;
    MppMeta frame_meta;
    MppMeta packet_meta;
    MppMeta standalone = NULL;
    uint8_t bytes[8] = {0};
    RK_S32 value = 0;

    CHECK(mpp_frame_init(&frame) == MPP_OK);
    CHECK(mpp_frame_has_meta(frame) == 0);
    frame_meta = mpp_frame_get_meta(frame);
    CHECK(frame_meta != NULL);
    CHECK(mpp_frame_has_meta(frame) == 1);
    CHECK(mpp_frame_get_meta(frame) == frame_meta);
    CHECK(mpp_meta_set_s32(frame_meta, KEY_TEMPORAL_ID, 5) == MPP_OK);
    CHECK(mpp_meta_get_s32(frame_meta, KEY_TEMPORAL_ID, &value) == MPP_OK);
    CHECK(value == 5);
    CHECK(mpp_frame_deinit(&frame) == MPP_OK);

    CHECK(mpp_packet_init(&packet, bytes, sizeof(bytes)) == MPP_OK);
    CHECK(mpp_packet_has_meta(packet) == 0);
    packet_meta = mpp_packet_get_meta(packet);
    CHECK(packet_meta != NULL);
    CHECK(mpp_packet_has_meta(packet) == 1);
    CHECK(mpp_packet_get_meta(packet) == packet_meta);
    CHECK(mpp_packet_deinit(&packet) == MPP_OK);

    CHECK(mpp_frame_init(&frame) == MPP_OK);
    CHECK(mpp_meta_get(&standalone) == MPP_OK);
    CHECK(standalone != NULL);
    CHECK(mpp_meta_set_s32(standalone, KEY_TEMPORAL_ID, 9) == MPP_OK);
    mpp_frame_set_meta(frame, standalone);
    CHECK(mpp_frame_get_meta(frame) == standalone);
    CHECK(mpp_frame_deinit(&frame) == MPP_OK);
    return 0;
}

int main(void) {
    MppMeta meta = NULL;
    MppMeta shared = NULL;

    CHECK(mpp_meta_get(&meta) == MPP_OK);
    CHECK(meta != NULL);
    CHECK(test_scalar_meta(meta) == 0);
    CHECK(test_pointer_meta(meta) == 0);
    CHECK(test_object_meta(meta) == 0);
    CHECK(mpp_meta_size(meta) == 0);
    CHECK(mpp_meta_put(meta) == MPP_OK);
    CHECK(mpp_meta_get(&shared) == MPP_OK);
    CHECK(mpp_meta_inc_ref(shared) == MPP_OK);
    CHECK(mpp_meta_put(shared) == MPP_OK);
    CHECK(mpp_meta_put(shared) == MPP_OK);
    CHECK(test_attached_meta() == 0);
    puts("PASS official MppMeta scalar, pointer, object, duplicate and lifetime API");
    return 0;
}
