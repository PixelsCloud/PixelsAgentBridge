#pragma once
#include <cstddef>
#include <cstdint>
#include <stdexcept>

// Screen/config dimensions remain visible pixels. H.264 4:2:0 pads only the
// right/bottom odd edge; all presenters crop before rotation/input mapping.
struct PabVideoGeometry {
    uint32_t width, height;
    uint32_t coded_width, coded_height;
    explicit PabVideoGeometry(uint32_t w, uint32_t h) : width(w), height(h) {
        if (!w || !h || w > 8192 || h > 8192)
            throw std::runtime_error("Video dimensions outside bounds");
        coded_width = (w + 1) & ~uint32_t(1);
        coded_height = (h + 1) & ~uint32_t(1);
        if (uint64_t(coded_width) * coded_height > 33554432)
            throw std::runtime_error("Padded video dimensions outside bounds");
    }
    size_t nv12_bytes() const { return size_t(coded_width) * coded_height * 3 / 2; }
};

// Private Rust ABI: handles are uniquely owned; output borrows codec storage
// until the next call or drop. Never pass either pointer over process/network IPC.
struct PabCodecBytes { const uint8_t* data; size_t size; };
extern "C" {
void* pab_soft_encoder_new(uint32_t width, uint32_t height, uint16_t fps);
void pab_soft_encoder_drop(void* encoder);
bool pab_soft_encoder_budget(void* encoder, uint32_t bits_per_second);
bool pab_soft_encode(void* encoder, const uint8_t* data, size_t size, size_t stride, PabCodecBytes* output);
// The prepare call owns the converted planes; it retains no capture mapping.
// Prepared data is consumed once. The same handle remains single-thread owned.
bool pab_soft_prepare_bgra(void* encoder, const uint8_t* data, size_t size, size_t stride);
bool pab_soft_encode_prepared(void* encoder, PabCodecBytes* output);
bool pab_soft_encode_nv12(void* encoder, const uint8_t* y, size_t y_size, size_t y_stride,
    const uint8_t* uv, size_t uv_size, size_t uv_stride, PabCodecBytes* output);
void* pab_soft_decoder_new(uint32_t width, uint32_t height);
void pab_soft_decoder_drop(void* decoder);
bool pab_soft_decode(void* decoder, const uint8_t* data, size_t size, PabCodecBytes* output);
}
