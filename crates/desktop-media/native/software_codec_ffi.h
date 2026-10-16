#pragma once
#include <cstddef>
#include <cstdint>

// Private Rust ABI: handles are uniquely owned; output borrows codec storage
// until the next call or drop. Never pass either pointer over process/network IPC.
struct PabCodecBytes { const uint8_t* data; size_t size; };
extern "C" {
void* pab_soft_encoder_new(uint32_t width, uint32_t height, uint16_t fps);
void pab_soft_encoder_drop(void* encoder);
bool pab_soft_encode(void* encoder, const uint8_t* data, size_t size, size_t stride, PabCodecBytes* output);
void* pab_soft_decoder_new(uint32_t width, uint32_t height);
void pab_soft_decoder_drop(void* decoder);
bool pab_soft_decode(void* decoder, const uint8_t* data, size_t size, PabCodecBytes* output);
}
