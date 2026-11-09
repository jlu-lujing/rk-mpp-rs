#include <rk_mpi.h>

#include <stdio.h>

int main(void) {
    MppCtx ctx = NULL;
    MppApi *mpi = NULL;

    if (mpp_create(NULL, &mpi) != MPP_ERR_NULL_PTR ||
        mpp_create(&ctx, NULL) != MPP_ERR_NULL_PTR) {
        fprintf(stderr, "null mpp_create checks failed\n");
        return 1;
    }
    if (mpp_create(&ctx, &mpi) != MPP_OK || !ctx || !mpi ||
        mpi->size != sizeof(MppApi) || !mpi->decode ||
        !mpi->decode_put_packet || !mpi->decode_get_frame || !mpi->encode ||
        !mpi->encode_put_frame || !mpi->encode_get_packet || !mpi->poll ||
        !mpi->dequeue || !mpi->enqueue || !mpi->reset || !mpi->control) {
        fprintf(stderr, "mpp_create ABI check failed\n");
        return 1;
    }
    if (mpp_check_support_format(MPP_CTX_DEC, MPP_VIDEO_CodingAVC) != MPP_OK ||
        mpp_check_support_format(MPP_CTX_DEC, MPP_VIDEO_CodingHEVC) != MPP_OK ||
        mpp_check_support_format(MPP_CTX_DEC, MPP_VIDEO_CodingAV1) != MPP_OK ||
        mpp_check_support_format(MPP_CTX_ENC, MPP_VIDEO_CodingMJPEG) != MPP_OK ||
        mpp_check_support_format(MPP_CTX_DEC, MPP_VIDEO_CodingUnused) != MPP_NOK) {
        fprintf(stderr, "format support query failed\n");
        return 1;
    }
    if (mpp_init(ctx, MPP_CTX_DEC, MPP_VIDEO_CodingAVC) != MPP_OK ||
        mpp_init(ctx, MPP_CTX_ENC, MPP_VIDEO_CodingAVC) != MPP_OK ||
        mpp_init(ctx, MPP_CTX_DEC, MPP_VIDEO_CodingUnused) == MPP_OK) {
        fprintf(stderr, "mpp_init contract failed\n");
        return 1;
    }
    if (mpi->reset(ctx) != MPP_OK ||
        mpi->control(ctx, MPP_SET_OUTPUT_BLOCK, NULL) == MPP_OK) {
        fprintf(stderr, "api function table contract failed\n");
        return 1;
    }
    if (mpp_destroy(ctx) != MPP_OK ||
        mpp_destroy(NULL) != MPP_ERR_NULL_PTR) {
        fprintf(stderr, "mpp_destroy contract failed\n");
        return 1;
    }
    puts("PASS official rk_mpi.h lifecycle and MppApi layout");
    return 0;
}
