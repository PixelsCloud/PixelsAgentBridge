#pragma once
// Independent NVENC experiment shared by encode and decode/display probes.
#include "dxgi_support.h"
#include <nvEncodeAPI.h>
#include <fstream>

class Encoder {
    HMODULE library_ = nullptr;
    NV_ENCODE_API_FUNCTION_LIST api_{};
    void* session_ = nullptr;
    NV_ENC_OUTPUT_PTR bitstream_ = nullptr;
    NV_ENC_REGISTERED_PTR registered_ = nullptr;
    NV_ENC_INPUT_PTR mapped_ = nullptr;
    ComPtr<ID3D11Texture2D> retained_;
    UINT width_ = 0, height_ = 0;
    NV_ENC_CONFIG configuration_{};
    NV_ENC_INITIALIZE_PARAMS initialization_{};

    static void require(NVENCSTATUS status, const char* stage) {
        if (status != NV_ENC_SUCCESS)
            throw std::runtime_error(std::string(stage) + " NVENC status=" + std::to_string(status));
    }

    void release_input() {
        if (mapped_) {
            require(api_.nvEncUnmapInputResource(session_, mapped_), "UnmapInputResource");
            mapped_ = nullptr;
        }
        if (registered_) {
            require(api_.nvEncUnregisterResource(session_, registered_), "UnregisterResource");
            registered_ = nullptr;
        }
        retained_.Reset();
    }

    void stop() noexcept {
        // On any failure, destroy the session before releasing the capture lease
        // or texture: the driver may still own an accepted input resource.
        if (session_) {
            if (!mapped_ && !registered_ && bitstream_)
                api_.nvEncDestroyBitstreamBuffer(session_, bitstream_);
            api_.nvEncDestroyEncoder(session_);
            session_ = nullptr;
        }
        mapped_ = nullptr;
        registered_ = nullptr;
        bitstream_ = nullptr;
        retained_.Reset();
    }

public:
    Encoder() = default;
    Encoder(const Encoder&) = delete;
    Encoder& operator=(const Encoder&) = delete;
    ~Encoder() {
        stop();
        if (library_) FreeLibrary(library_);
    }

    void initialize(ID3D11Device* device, UINT width, UINT height, unsigned fps) {
        width_ = width;
        height_ = height;
        library_ = LoadLibraryExW(L"nvEncodeAPI64.dll", nullptr, LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!library_) throw std::runtime_error("NVIDIA encoder driver unavailable");
        using Create = NVENCSTATUS(NVENCAPI*)(NV_ENCODE_API_FUNCTION_LIST*);
        using Version = NVENCSTATUS(NVENCAPI*)(uint32_t*);
        const auto create = reinterpret_cast<Create>(GetProcAddress(library_, "NvEncodeAPICreateInstance"));
        const auto version = reinterpret_cast<Version>(GetProcAddress(library_, "NvEncodeAPIGetMaxSupportedVersion"));
        if (!create || !version) throw std::runtime_error("Missing NVENC entry points");
        uint32_t supported = 0;
        require(version(&supported), "GetMaxSupportedVersion");
        if (supported < ((NVENCAPI_MAJOR_VERSION << 4) | NVENCAPI_MINOR_VERSION))
            throw std::runtime_error("Driver does not support the pinned NVENC API version");
        api_.version = NV_ENCODE_API_FUNCTION_LIST_VER;
        require(create(&api_), "CreateInstance");
        NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS open{};
        open.version = NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER;
        open.apiVersion = NVENCAPI_VERSION;
        open.device = device;
        open.deviceType = NV_ENC_DEVICE_TYPE_DIRECTX;
        require(api_.nvEncOpenEncodeSessionEx(&open, &session_), "OpenEncodeSession");
        NV_ENC_PRESET_CONFIG preset{};
        preset.version = NV_ENC_PRESET_CONFIG_VER;
        preset.presetCfg.version = NV_ENC_CONFIG_VER;
        require(api_.nvEncGetEncodePresetConfigEx(session_, NV_ENC_CODEC_H264_GUID,
            NV_ENC_PRESET_P3_GUID, NV_ENC_TUNING_INFO_LOW_LATENCY, &preset), "GetPreset");
        auto& config = preset.presetCfg;
        config.profileGUID = NV_ENC_H264_PROFILE_HIGH_GUID;
        config.frameIntervalP = 1;
        config.gopLength = fps;
        config.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
        config.rcParams.averageBitRate = 6000000;
        config.rcParams.maxBitRate = 6000000;
        config.rcParams.enableLookahead = 0;
        config.rcParams.lookaheadDepth = 0;
        config.rcParams.zeroReorderDelay = 1;
        config.encodeCodecConfig.h264Config.idrPeriod = fps;
        config.encodeCodecConfig.h264Config.repeatSPSPPS = 1;
        // Explicit SDR metadata matching the RGB-input path. Synthetic decoded
        // corner checks independently verify matrix/range and channel ordering.
        auto& vui = config.encodeCodecConfig.h264Config.h264VUIParameters;
        vui.videoSignalTypePresentFlag = 1;
        vui.videoFormat = NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
        vui.videoFullRangeFlag = 0;
        vui.colourDescriptionPresentFlag = 1;
        vui.colourPrimaries = NV_ENC_VUI_COLOR_PRIMARIES_BT709;
        vui.transferCharacteristics = NV_ENC_VUI_TRANSFER_CHARACTERISTIC_BT709;
        vui.colourMatrix = NV_ENC_VUI_MATRIX_COEFFS_BT709;
        NV_ENC_INITIALIZE_PARAMS init{};
        init.version = NV_ENC_INITIALIZE_PARAMS_VER;
        init.encodeGUID = NV_ENC_CODEC_H264_GUID;
        init.presetGUID = NV_ENC_PRESET_P3_GUID;
        init.tuningInfo = NV_ENC_TUNING_INFO_LOW_LATENCY;
        init.encodeWidth = init.darWidth = width;
        init.encodeHeight = init.darHeight = height;
        init.frameRateNum = fps;
        init.frameRateDen = 1;
        init.enablePTD = 1;
        init.enableEncodeAsync = 0;
        init.encodeConfig = &config;
        require(api_.nvEncInitializeEncoder(session_, &init), "InitializeEncoder");
        configuration_ = config;
        initialization_ = init;
        initialization_.encodeConfig = &configuration_;
        NV_ENC_CREATE_BITSTREAM_BUFFER output{};
        output.version = NV_ENC_CREATE_BITSTREAM_BUFFER_VER;
        require(api_.nvEncCreateBitstreamBuffer(session_, &output), "CreateBitstreamBuffer");
        bitstream_ = output.bitstreamBuffer;
    }
    void budget(uint32_t bits_per_second) {
        if (bits_per_second && (bits_per_second < 150000 || bits_per_second > 16000000))
            throw std::runtime_error("Encoder budget outside bounds");
        auto candidate = configuration_;
        candidate.rcParams.averageBitRate = candidate.rcParams.maxBitRate = bits_per_second ? bits_per_second : 6000000;
        NV_ENC_RECONFIGURE_PARAMS update{};
        update.version = NV_ENC_RECONFIGURE_PARAMS_VER;
        update.reInitEncodeParams = initialization_;
        update.reInitEncodeParams.encodeConfig = &candidate;
        update.forceIDR = 1;
        require(api_.nvEncReconfigureEncoder(session_, &update), "Reconfigure encoder budget");
        configuration_ = candidate;
    }

    uint64_t encode(ID3D11Texture2D* texture, uint64_t index, std::ofstream* output, std::vector<uint8_t>* packet = nullptr) {
        try {
            D3D11_TEXTURE2D_DESC desc{};
            texture->GetDesc(&desc);
            if (desc.Width != width_ || desc.Height != height_ || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM)
                throw std::runtime_error("Texture changed; rebuild encoder generation");
            retained_ = texture;
            NV_ENC_REGISTER_RESOURCE resource{};
            resource.version = NV_ENC_REGISTER_RESOURCE_VER;
            resource.resourceType = NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX;
            resource.resourceToRegister = texture;
            resource.width = width_;
            resource.height = height_;
            resource.bufferFormat = NV_ENC_BUFFER_FORMAT_ARGB;
            resource.bufferUsage = NV_ENC_INPUT_IMAGE;
            require(api_.nvEncRegisterResource(session_, &resource), "RegisterCaptureTexture");
            registered_ = resource.registeredResource;
            NV_ENC_MAP_INPUT_RESOURCE map{};
            map.version = NV_ENC_MAP_INPUT_RESOURCE_VER;
            map.registeredResource = registered_;
            require(api_.nvEncMapInputResource(session_, &map), "MapInputResource");
            mapped_ = map.mappedResource;
            NV_ENC_PIC_PARAMS pic{};
            pic.version = NV_ENC_PIC_PARAMS_VER;
            pic.inputBuffer = mapped_;
            pic.bufferFmt = map.mappedBufferFmt;
            pic.inputWidth = width_;
            pic.inputHeight = height_;
            pic.outputBitstream = bitstream_;
            pic.pictureStruct = NV_ENC_PIC_STRUCT_FRAME;
            pic.frameIdx = static_cast<uint32_t>(index);
            pic.inputTimeStamp = index;
            if (!index) pic.encodePicFlags = NV_ENC_PIC_FLAG_FORCEIDR | NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
            // A single outstanding input. NEED_MORE_INPUT is an explicit failure
            // for this low-latency experiment, never silently release/reuse input.
            require(api_.nvEncEncodePicture(session_, &pic), "EncodePicture");
            NV_ENC_LOCK_BITSTREAM lock{};
            lock.version = NV_ENC_LOCK_BITSTREAM_VER;
            lock.outputBitstream = bitstream_;
            require(api_.nvEncLockBitstream(session_, &lock), "LockBitstream");
            const uint64_t bytes = lock.bitstreamSizeInBytes;
            if (packet) {
                const auto* begin = static_cast<const uint8_t*>(lock.bitstreamBufferPtr);
                packet->assign(begin, begin + lock.bitstreamSizeInBytes);
            }
            if (output) output->write(static_cast<const char*>(lock.bitstreamBufferPtr), lock.bitstreamSizeInBytes);
            const auto timestamp = lock.outputTimeStamp;
            require(api_.nvEncUnlockBitstream(session_, bitstream_), "UnlockBitstream");
            if (!bytes || timestamp != index) throw std::runtime_error("Missing or delayed encoded frame");
            if (output && !*output) throw std::runtime_error("Cannot write synthetic bitstream");
            release_input();
            return bytes;
        } catch (...) {
            stop();
            throw;
        }
    }
};
