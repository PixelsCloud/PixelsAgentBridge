// Native-only diagnostic. Uses the production Capture and software codec;
// saves timing/byte counts, never screen pixels or encoded desktop packets.
#include "../../crates/desktop-media/native/software_codec_ffi.h"
#include <chrono>
#include <cstdio>

static double conversion_ms = 0, encoding_ms = 0;
static bool measured_prepare(void* encoder, const uint8_t* data, size_t size,
                             size_t stride) {
    const auto begin = std::chrono::steady_clock::now();
    const bool ok = pab_soft_prepare_bgra(encoder, data, size, stride);
    conversion_ms = std::chrono::duration<double, std::milli>(
        std::chrono::steady_clock::now() - begin).count();
    return ok;
}
static bool measured_encode(void* encoder, PabCodecBytes* output) {
    const auto begin = std::chrono::steady_clock::now();
    const bool ok = pab_soft_encode_prepared(encoder, output);
    encoding_ms = std::chrono::duration<double, std::milli>(
        std::chrono::steady_clock::now() - begin).count();
    return ok;
}
// Interpose only this experiment's callsite, without changing production code.
#define pab_soft_prepare_bgra measured_prepare
#define pab_soft_encode_prepared measured_encode
#include "../../crates/desktop-media/native/linux/capture.h"
#undef pab_soft_prepare_bgra
#undef pab_soft_encode_prepared

int main(int argc, char** argv) {
    pw_init(nullptr, nullptr);
    int status = 1;
    try {
        if (argc != 2 || (std::string(argv[1]) != "embedded" &&
                          std::string(argv[1]) != "separate"))
            throw std::runtime_error("usage: linux-capture-timing embedded|separate");
        using namespace pab_linux;
        const bool separate = std::string(argv[1]) == "separate";
        GError* error = nullptr;
        Connection bus(g_bus_get_sync(G_BUS_TYPE_SESSION, nullptr, &error));
        if (!bus) {
            if (error) g_error_free(error);
            throw std::runtime_error("No session bus");
        }
        const auto screens = displays(bus.get());
        if (screens.size() != 1 || screens[0].size.width != 1920 ||
            screens[0].size.height != 1080)
            throw std::runtime_error("Requires a generated single 1080p fixture");
        CursorState cursor;
        Capture source(bus.get(), screens[0], 15, separate ? &cursor : nullptr);
        source.start();
        Encoder encoder(pab_soft_encoder_new(1920, 1080, 15));
        if (!encoder) throw std::runtime_error("Cannot create codec");
        const auto start = Clock::now();
        auto next = start;
        for (unsigned i = 0; i < 120; ++i) {
            while (Clock::now() < next) {
                parent_alive(); pump();
                std::this_thread::sleep_for(std::chrono::milliseconds(1));
            }
            const auto requested = Clock::now();
            const auto frame = source.next(encoder.get());
            const auto finished = Clock::now();
            const auto ms = [](auto a, auto b) {
                return std::chrono::duration<double, std::milli>(b-a).count();
            };
            printf("{\"index\":%u,\"separate_cursor\":%s,\"sampled_ms\":%.6f,"
                   "\"requested_ms\":%.6f,\"finished_ms\":%.6f,\"next_ms\":%.6f,"
                   "\"codec_ms\":%.6f,\"conversion_ms\":%.6f,\"encoding_ms\":%.6f,\"bytes\":%zu}\n",
                   i, separate ? "true" : "false", ms(start, frame.sampled),
                   ms(start, requested), ms(start, finished), ms(requested, finished),
                   conversion_ms + encoding_ms, conversion_ms, encoding_ms, frame.bytes.size);
            next = std::max(next + std::chrono::microseconds(1000000 / 15), Clock::now());
        }
        status = 0;
    } catch (const std::exception& error) {
        fprintf(stderr, "capture timing: %s\n", error.what());
    }
    pw_deinit();
    return status;
}
