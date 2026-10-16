#pragma once
// Independent experiment using Windows SDK interfaces, not SDK sample code.
#include "dxgi_support.h"
#include <d3d10.h>
#include <mfapi.h>
#include <mferror.h>
#include <mfidl.h>
#include <mftransform.h>
#include <codecapi.h>
#include <wmcodecdsp.h>
#include <functional>

class MediaRuntime {
public:
    MediaRuntime() {
        check(CoInitializeEx(nullptr, COINIT_MULTITHREADED), "CoInitializeEx");
        const auto hr = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        if (FAILED(hr)) { CoUninitialize(); check(hr, "MFStartup"); }
    }
    ~MediaRuntime() { MFShutdown(); CoUninitialize(); }
    MediaRuntime(const MediaRuntime&) = delete;
    MediaRuntime& operator=(const MediaRuntime&) = delete;
};

struct DecodedFrame {
    // The sample owns the decoder surface pool slot; retain until GPU rendering
    // completes, not merely until ProcessOutput or VideoProcessorBlt returns.
    ComPtr<IMFSample> sample;
    ComPtr<ID3D11Texture2D> texture;
    UINT subresource = 0;
    LONGLONG timestamp = 0;
};

class HardwareDecoder {
    ComPtr<IMFDXGIDeviceManager> manager_;
    ComPtr<IMFTransform> decoder_;
    ComPtr<ID3D11Device> device_;
    UINT width_ = 0, height_ = 0;
    LONGLONG duration_ = 0;

    void output_type() {
        for (DWORD index = 0;; ++index) {
            ComPtr<IMFMediaType> type;
            const auto hr = decoder_->GetOutputAvailableType(0, index, &type);
            if (hr == MF_E_NO_MORE_TYPES) break;
            check(hr, "GetOutputAvailableType");
            GUID format{};
            check(type->GetGUID(MF_MT_SUBTYPE, &format), "Output subtype");
            if (format != MFVideoFormat_NV12) continue;
            UINT width = 0, height = 0;
            check(MFGetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, &width, &height), "Output size");
            // Decoder allocation can be macroblock-aligned (1088 rows for
            // 1080 visible rows). Validate its aperture, never stretch padding.
            MFVideoArea aperture{};
            auto aperture_hr = type->GetBlob(MF_MT_MINIMUM_DISPLAY_APERTURE,
                reinterpret_cast<UINT8*>(&aperture), sizeof(aperture), nullptr);
            if (aperture_hr == MF_E_ATTRIBUTENOTFOUND)
                aperture_hr = type->GetBlob(MF_MT_GEOMETRIC_APERTURE,
                    reinterpret_cast<UINT8*>(&aperture), sizeof(aperture), nullptr);
            if (aperture_hr == MF_E_ATTRIBUTENOTFOUND) {
                aperture.Area.cx = static_cast<LONG>(width);
                aperture.Area.cy = static_cast<LONG>(height);
            } else check(aperture_hr, "Decoder display aperture");
            const auto matrix = MFGetAttributeUINT32(type.Get(), MF_MT_YUV_MATRIX, MFVideoTransferMatrix_Unknown);
            const auto range = MFGetAttributeUINT32(type.Get(), MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_Unknown);
            if ((matrix != MFVideoTransferMatrix_Unknown && matrix != MFVideoTransferMatrix_BT709) ||
                (range != MFNominalRange_Unknown && range != MFNominalRange_16_235))
                throw std::runtime_error("Unexpected color metadata; this probe requires SDR limited BT.709");
            if (width < width_ || height < height_ || width > width_ + 15 || height > height_ + 15 ||
                aperture.OffsetX.value || aperture.OffsetX.fract || aperture.OffsetY.value || aperture.OffsetY.fract ||
                aperture.Area.cx != static_cast<LONG>(width_) || aperture.Area.cy != static_cast<LONG>(height_))
                throw std::runtime_error("Decoder aperture/format changed: " + std::to_string(width) + "x" +
                    std::to_string(height) + ", visible " + std::to_string(aperture.Area.cx) + "x" +
                    std::to_string(aperture.Area.cy) + "; expected " + std::to_string(width_) + "x" + std::to_string(height_));
            check(decoder_->SetOutputType(0, type.Get(), 0), "Set NV12 output type");
            MFT_OUTPUT_STREAM_INFO info{};
            check(decoder_->GetOutputStreamInfo(0, &info), "GetOutputStreamInfo");
            if (!(info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES))
                throw std::runtime_error("Decoder did not provide GPU samples; CPU fallback refused");
            return;
        }
        throw std::runtime_error("Decoder has no NV12 output");
    }

public:
    HardwareDecoder(ID3D11Device* device, UINT width, UINT height, UINT fps)
        : device_(device), width_(width), height_(height), duration_(10000000LL / fps) {
        ComPtr<ID3D10Multithread> multithread;
        check(device->QueryInterface(IID_PPV_ARGS(&multithread)), "D3D multithread protection");
        multithread->SetMultithreadProtected(TRUE);
        UINT token = 0;
        check(MFCreateDXGIDeviceManager(&token, &manager_), "MFCreateDXGIDeviceManager");
        check(manager_->ResetDevice(device, token), "ResetDevice");
        check(CoCreateInstance(CLSID_CMSH264DecoderMFT, nullptr, CLSCTX_INPROC_SERVER,
            IID_PPV_ARGS(&decoder_)), "Create system H264 decoder");
        ComPtr<IMFAttributes> attributes;
        check(decoder_->GetAttributes(&attributes), "Get decoder attributes");
        UINT32 aware = 0;
        check(attributes->GetUINT32(MF_SA_D3D11_AWARE, &aware), "D3D11 aware");
        if (!aware) throw std::runtime_error("Decoder is not D3D11 aware");
        check(attributes->SetUINT32(CODECAPI_AVLowLatencyMode, TRUE), "Low latency decode");
        check(attributes->SetUINT32(CODECAPI_AVDecVideoAcceleration_H264, TRUE), "Enable DXVA");
        check(attributes->SetUINT32(CODECAPI_AVDecNumWorkerThreads, 2), "Decoder worker threads");
        check(decoder_->ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER,
            reinterpret_cast<ULONG_PTR>(manager_.Get())), "Set decoder D3D manager");
        ComPtr<IMFMediaType> input;
        check(MFCreateMediaType(&input), "Create decoder input type");
        check(input->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video), "Input major type");
        check(input->SetGUID(MF_MT_SUBTYPE, MFVideoFormat_H264), "Input subtype");
        check(MFSetAttributeSize(input.Get(), MF_MT_FRAME_SIZE, width, height), "Input dimensions");
        check(MFSetAttributeRatio(input.Get(), MF_MT_FRAME_RATE, fps, 1), "Input FPS");
        check(MFSetAttributeRatio(input.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1), "Input PAR");
        check(input->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive), "Input progressive");
        check(decoder_->SetInputType(0, input.Get(), 0), "Set decoder input type");
        output_type();
        check(decoder_->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0), "Begin decode");
        check(decoder_->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0), "Start decode");
    }
    ~HardwareDecoder() {
        if (decoder_) {
            decoder_->ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            decoder_->ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            decoder_->ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, 0);
        }
    }
    void submit(const std::vector<uint8_t>& packet, LONGLONG timestamp) {
        if (packet.empty() || packet.size() > 16 * 1024 * 1024)
            throw std::runtime_error("Invalid compressed frame size");
        ComPtr<IMFSample> sample;
        ComPtr<IMFMediaBuffer> buffer;
        check(MFCreateSample(&sample), "Create compressed sample");
        check(MFCreateMemoryBuffer(static_cast<DWORD>(packet.size()), &buffer), "Create compressed buffer");
        BYTE* data = nullptr;
        check(buffer->Lock(&data, nullptr, nullptr), "Lock compressed buffer");
        memcpy(data, packet.data(), packet.size()); // compressed bytes, not pixels
        check(buffer->Unlock(), "Unlock compressed buffer");
        check(buffer->SetCurrentLength(static_cast<DWORD>(packet.size())), "Compressed buffer length");
        check(sample->AddBuffer(buffer.Get()), "Add compressed buffer");
        check(sample->SetSampleTime(timestamp), "Set sample time");
        check(sample->SetSampleDuration(duration_), "Set sample duration");
        check(decoder_->ProcessInput(0, sample.Get(), 0), "Decode ProcessInput");
    }
    void receive(const std::function<void(DecodedFrame&)>& consume) {
        // Bound even a faulty transform's format-change/output loop.
        for (unsigned attempt = 0; attempt < 32; ++attempt) {
            MFT_OUTPUT_DATA_BUFFER output{};
            DWORD status = 0;
            const auto hr = decoder_->ProcessOutput(0, 1, &output, &status);
            ComPtr<IMFCollection> events;
            events.Attach(output.pEvents);
            DecodedFrame frame;
            frame.sample.Attach(output.pSample);
            if (hr == MF_E_TRANSFORM_STREAM_CHANGE) { output_type(); continue; }
            if (hr == MF_E_TRANSFORM_NEED_MORE_INPUT) return;
            check(hr, "Decode ProcessOutput");
            if (!frame.sample) throw std::runtime_error("Decoder returned no sample");
            check(frame.sample->GetSampleTime(&frame.timestamp), "Decoded timestamp");
            ComPtr<IMFMediaBuffer> buffer;
            ComPtr<IMFDXGIBuffer> gpu;
            check(frame.sample->GetBufferByIndex(0, &buffer), "Get decoded buffer");
            check(buffer.As(&gpu), "Require GPU decoded buffer (no software fallback)");
            check(gpu->GetResource(IID_PPV_ARGS(&frame.texture)), "Get decoder texture");
            check(gpu->GetSubresourceIndex(&frame.subresource), "Get decoder texture slice");
            D3D11_TEXTURE2D_DESC desc{};
            frame.texture->GetDesc(&desc);
            ComPtr<ID3D11Device> owner;
            frame.texture->GetDevice(&owner);
            if (owner.Get() != device_.Get() || desc.Format != DXGI_FORMAT_NV12 ||
                !(desc.BindFlags & D3D11_BIND_DECODER) || desc.CPUAccessFlags ||
                desc.Usage != D3D11_USAGE_DEFAULT || desc.Width < width_ || desc.Height < height_ ||
                desc.MipLevels != 1 || frame.subresource >= desc.ArraySize)
                throw std::runtime_error("Unexpected decoder surface/device; implicit copies refused");
            consume(frame);
        }
        throw std::runtime_error("Decoder output loop exceeded bound");
    }
    void drain(const std::function<void(DecodedFrame&)>& consume) {
        check(decoder_->ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0), "Decode end of stream");
        check(decoder_->ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0), "Decode drain");
        receive(consume);
    }
};
