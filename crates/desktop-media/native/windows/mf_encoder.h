#pragma once
// Intel Media Foundation hardware encoding with GPU-only BT.709 conversion.
// One input/output at a time: no frame reordering, unbounded queues or readback.
#include "mf_decoder.h"
#include "media_wait.h"
#include <algorithm>
#include <thread>

class IntelHardwareEncoder {
    MediaRuntime runtime_;
    struct Activation {
        ComPtr<IMFActivate> value;
        ~Activation() { if (value) value->ShutdownObject(); }
    } activation_;
    ComPtr<IMFTransform> transform_;
    ComPtr<IMFMediaEventGenerator> events_;
    ComPtr<IMFDXGIDeviceManager> manager_;
    ComPtr<ID3D11Device> device_;
    ComPtr<ID3D11DeviceContext> context_;
    ComPtr<ID3D11VideoDevice> video_;
    ComPtr<ID3D11VideoContext> video_context_;
    ComPtr<ID3D11VideoProcessorEnumerator> enumerator_;
    ComPtr<ID3D11VideoProcessor> processor_;
    ComPtr<ID3D11Texture2D> nv12_;
    ComPtr<ID3D11VideoProcessorOutputView> output_view_;
    ComPtr<ID3D11Query> completed_;
    DWORD input_ = 0, output_ = 0;
    UINT input_credits_ = 0, output_credits_ = 0;
    UINT width_, height_, fps_;

    void property(const GUID& key, VARTYPE kind, ULONG value, bool required) {
        ComPtr<ICodecAPI> codec;
        check(transform_.As(&codec), "Encoder codec API");
        VARIANT setting{}; setting.vt = kind;
        if (kind == VT_BOOL) setting.boolVal = value ? VARIANT_TRUE : VARIANT_FALSE;
        else setting.ulVal = value;
        const auto hr = codec->SetValue(&key, &setting);
        if (required) check(hr, "Encoder required codec property");
    }
    void event_until(bool output) {
        const auto deadline = Clock::now() + std::chrono::seconds(2);
        unsigned consumed = 0;
        while (!(output ? output_credits_ : input_credits_)) {
            if (Clock::now() >= deadline || consumed > 256)
                throw std::runtime_error(output ? "Encoder output deadline" : "Encoder input deadline");
            ComPtr<IMFMediaEvent> event;
            const auto hr = events_->GetEvent(MF_EVENT_FLAG_NO_WAIT, &event);
            if (hr == MF_E_NO_EVENTS_AVAILABLE) {
                media_wait::sleep(std::chrono::milliseconds(1)); continue;
            }
            check(hr, "Encoder event");
            ++consumed;
            HRESULT status = S_OK; MediaEventType type = MEUnknown;
            check(event->GetStatus(&status), "Encoder event status"); check(status, "Encoder event failure");
            check(event->GetType(&type), "Encoder event type");
            if (type == METransformNeedInput) {
                if (MFGetAttributeUINT32(event.Get(), MF_EVENT_MFT_INPUT_STREAM_ID, input_) != input_)
                    throw std::runtime_error("Unexpected encoder input stream");
                if (++input_credits_ > 32) throw std::runtime_error("Unbounded encoder input requests");
            } else if (type == METransformHaveOutput) {
                if (++output_credits_ > 32) throw std::runtime_error("Unbounded encoder output requests");
            } else if (type == MEError) throw std::runtime_error("Encoder reported MEError");
        }
    }
public:
    IntelHardwareEncoder(ID3D11Device* device, UINT width, UINT height, UINT fps)
        : device_(device), width_(width), height_(height), fps_(fps) {
        ComPtr<IDXGIDevice> dxgi;
        ComPtr<IDXGIAdapter> adapter;
        DXGI_ADAPTER_DESC adapter_info{};
        check(device->QueryInterface(IID_PPV_ARGS(&dxgi)), "Intel encoder DXGI device");
        check(dxgi->GetAdapter(&adapter), "Intel encoder adapter");
        check(adapter->GetDesc(&adapter_info), "Intel encoder adapter description");
        if (adapter_info.VendorId != 0x8086 || !width || !height || width > 8192 || height > 8192 ||
            uint64_t(width)*height > 33554432 || (width & 1) || (height & 1) || !fps || fps > 60)
            throw std::runtime_error("Intel hardware encoder requires an even-sized Intel GPU surface");
        device->GetImmediateContext(&context_);
        ComPtr<ID3D10Multithread> protected_context;
        check(context_.As(&protected_context), "Protect encoder D3D context");
        protected_context->SetMultithreadProtected(TRUE);
        MFT_REGISTER_TYPE_INFO in{MFMediaType_Video, MFVideoFormat_NV12};
        MFT_REGISTER_TYPE_INFO out{MFMediaType_Video, MFVideoFormat_H264};
        IMFActivate** candidates = nullptr; UINT32 count = 0;
        check(MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            &in, &out, &candidates, &count), "Enumerate hardware encoder");
        for (UINT32 i = 0; i < count; ++i) {
            // The qualification device is explicitly Intel. Do not accidentally
            // accept a different vendor's transform on a multi-GPU test machine.
            wchar_t* name = nullptr; UINT32 length = 0;
            if (SUCCEEDED(candidates[i]->GetAllocatedString(MFT_FRIENDLY_NAME_Attribute, &name, &length)) &&
                name && std::wstring(name).find(L"Intel") != std::wstring::npos && !activation_.value)
                activation_.value = candidates[i];
            if (activation_.value.Get() == candidates[i]) {
                UINT64 luid = 0;
                if (SUCCEEDED(candidates[i]->GetUINT64(MFT_ENUM_ADAPTER_LUID, &luid))) {
                    const auto expected = (UINT64(UINT32(adapter_info.AdapterLuid.HighPart)) << 32) | adapter_info.AdapterLuid.LowPart;
                    if (luid != expected) activation_.value.Reset();
                }
            }
            CoTaskMemFree(name); candidates[i]->Release();
        }
        CoTaskMemFree(candidates);
        if (!activation_.value) throw std::runtime_error("No Intel hardware H264 transform");
        check(activation_.value->ActivateObject(IID_PPV_ARGS(&transform_)), "Activate Intel encoder");
        ComPtr<IMFAttributes> attributes;
        check(transform_->GetAttributes(&attributes), "Encoder attributes");
        if (!MFGetAttributeUINT32(attributes.Get(), MF_TRANSFORM_ASYNC, FALSE))
            throw std::runtime_error("Hardware encoder is not asynchronous");
        check(attributes->SetUINT32(MF_TRANSFORM_ASYNC_UNLOCK, TRUE), "Unlock async encoder");
        if (!MFGetAttributeUINT32(attributes.Get(), MF_SA_D3D11_AWARE, FALSE))
            throw std::runtime_error("Intel encoder does not support D3D11 surfaces");
        check(transform_.As(&events_), "Encoder event generator");
        const auto stream_ids = transform_->GetStreamIDs(1, &input_, 1, &output_);
        if (stream_ids != E_NOTIMPL) check(stream_ids, "Encoder stream IDs");
        UINT token = 0;
        check(MFCreateDXGIDeviceManager(&token, &manager_), "Encoder DXGI manager");
        check(manager_->ResetDevice(device, token), "Encoder reset D3D device");
        check(transform_->ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER,
            reinterpret_cast<ULONG_PTR>(manager_.Get())), "Encoder D3D manager");
        property(CODECAPI_AVLowLatencyMode, VT_BOOL, TRUE, true);
        property(CODECAPI_AVEncCommonRealTime, VT_BOOL, TRUE, false);
        property(CODECAPI_AVEncMPVDefaultBPictureCount, VT_UI4, 0, true);
        property(CODECAPI_AVEncCommonRateControlMode, VT_UI4, eAVEncCommonRateControlMode_Quality, true);
        property(CODECAPI_AVEncCommonQuality, VT_UI4, 90, true);
        auto media_type = [&](const GUID& subtype) {
            ComPtr<IMFMediaType> type;
            check(MFCreateMediaType(&type), "Encoder media type");
            check(type->SetGUID(MF_MT_MAJOR_TYPE, MFMediaType_Video), "Encoder media major type");
            check(type->SetGUID(MF_MT_SUBTYPE, subtype), "Encoder media subtype");
            check(MFSetAttributeSize(type.Get(), MF_MT_FRAME_SIZE, width, height), "Encoder dimensions");
            check(MFSetAttributeRatio(type.Get(), MF_MT_FRAME_RATE, fps, 1), "Encoder frame rate");
            check(MFSetAttributeRatio(type.Get(), MF_MT_PIXEL_ASPECT_RATIO, 1, 1), "Encoder pixel ratio");
            check(type->SetUINT32(MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive), "Encoder interlace");
            check(type->SetUINT32(MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709), "Encoder color matrix");
            check(type->SetUINT32(MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235), "Encoder color range");
            return type;
        };
        auto output = media_type(MFVideoFormat_H264);
        check(output->SetUINT32(MF_MT_AVG_BITRATE, 12000000), "Encoder nominal bitrate");
        check(output->SetUINT32(MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Base), "Encoder baseline profile");
        check(transform_->SetOutputType(output_, output.Get(), 0), "Encoder output type");
        auto input = media_type(MFVideoFormat_NV12);
        check(transform_->SetInputType(input_, input.Get(), 0), "Encoder input type");
        check(device_.As(&video_), "Encoder video device");
        check(context_.As(&video_context_), "Encoder video context");
        D3D11_VIDEO_PROCESSOR_CONTENT_DESC content{};
        content.InputFrameFormat = D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE;
        content.InputFrameRate = content.OutputFrameRate = {fps, 1};
        content.InputWidth = content.OutputWidth = width; content.InputHeight = content.OutputHeight = height;
        content.Usage = D3D11_VIDEO_USAGE_OPTIMAL_SPEED;
        check(video_->CreateVideoProcessorEnumerator(&content, &enumerator_), "Encoder video enumerator");
        check(video_->CreateVideoProcessor(enumerator_.Get(), 0, &processor_), "Encoder video processor");
        D3D11_TEXTURE2D_DESC texture{};
        texture.Width = width; texture.Height = height;
        texture.MipLevels = texture.ArraySize = texture.SampleDesc.Count = 1;
        texture.Format = DXGI_FORMAT_NV12; texture.Usage = D3D11_USAGE_DEFAULT;
        texture.BindFlags = D3D11_BIND_RENDER_TARGET;
        check(device->CreateTexture2D(&texture, nullptr, &nv12_), "Encoder NV12 texture");
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC view{};
        view.ViewDimension = D3D11_VPOV_DIMENSION_TEXTURE2D;
        check(video_->CreateVideoProcessorOutputView(nv12_.Get(), enumerator_.Get(), &view, &output_view_), "Encoder NV12 view");
        video_context_->VideoProcessorSetStreamFrameFormat(processor_.Get(), 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        video_context_->VideoProcessorSetStreamAutoProcessingMode(processor_.Get(), 0, FALSE);
        RECT area{0, 0, LONG(width), LONG(height)};
        video_context_->VideoProcessorSetStreamSourceRect(processor_.Get(), 0, TRUE, &area);
        video_context_->VideoProcessorSetStreamDestRect(processor_.Get(), 0, TRUE, &area);
        video_context_->VideoProcessorSetOutputTargetRect(processor_.Get(), TRUE, &area);
        D3D11_VIDEO_PROCESSOR_COLOR_SPACE rgb{}, yuv{};
        rgb.RGB_Range = 0; rgb.Nominal_Range = D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_0_255;
        yuv.YCbCr_Matrix = 1; yuv.Nominal_Range = D3D11_VIDEO_PROCESSOR_NOMINAL_RANGE_16_235;
        video_context_->VideoProcessorSetStreamColorSpace(processor_.Get(), 0, &rgb);
        video_context_->VideoProcessorSetOutputColorSpace(processor_.Get(), &yuv);
        D3D11_QUERY_DESC query{D3D11_QUERY_EVENT, 0};
        check(device->CreateQuery(&query, &completed_), "Encoder GPU completion query");
        check(transform_->ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0), "Begin hardware encode");
        check(transform_->ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0), "Start hardware encode");
    }
    ~IntelHardwareEncoder() {
        if (transform_) {
            transform_->ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            transform_->ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
        // Activation outlives the transform and shuts down even if setup throws.
    }
    std::vector<uint8_t> encode(ID3D11Texture2D* rgb, uint64_t index) {
        event_until(false);
        D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC view{};
        view.ViewDimension = D3D11_VPIV_DIMENSION_TEXTURE2D;
        ComPtr<ID3D11VideoProcessorInputView> input_view;
        check(video_->CreateVideoProcessorInputView(rgb, enumerator_.Get(), &view, &input_view), "Encoder RGB view");
        D3D11_VIDEO_PROCESSOR_STREAM stream{};
        stream.Enable = TRUE; stream.pInputSurface = input_view.Get();
        check(video_context_->VideoProcessorBlt(processor_.Get(), output_view_.Get(), 0, 1, &stream), "GPU BGRA to NV12");
        context_->End(completed_.Get()); context_->Flush();
        const auto deadline = Clock::now() + std::chrono::seconds(2);
        HRESULT hr;
        while ((hr = context_->GetData(completed_.Get(), nullptr, 0, 0)) == S_FALSE) {
            if (Clock::now() >= deadline) throw std::runtime_error("Encoder GPU conversion deadline");
            media_wait::sleep(std::chrono::milliseconds(1));
        }
        check(hr, "Encoder GPU conversion completion");
        ComPtr<IMFSample> sample;
        ComPtr<IMFMediaBuffer> buffer;
        check(MFCreateSample(&sample), "Encoder sample");
        check(MFCreateDXGISurfaceBuffer(__uuidof(ID3D11Texture2D), nv12_.Get(), 0, FALSE, &buffer), "Encoder GPU sample buffer");
        check(sample->AddBuffer(buffer.Get()), "Encoder sample buffer");
        check(sample->SetSampleTime(index * (10000000LL / fps_)), "Encoder timestamp");
        check(sample->SetSampleDuration(10000000LL / fps_), "Encoder duration");
        check(transform_->ProcessInput(input_, sample.Get(), 0), "Encode GPU input"); --input_credits_;
        for (unsigned attempt = 0; attempt < 4; ++attempt) {
            event_until(true);
            MFT_OUTPUT_STREAM_INFO info{};
            check(transform_->GetOutputStreamInfo(output_, &info), "Encoder output stream information");
            ComPtr<IMFSample> result;
            if (!(info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES)) {
                if (info.cbSize > 16 * 1024 * 1024) throw std::runtime_error("Encoder output allocation limit");
                check(MFCreateSample(&result), "Encoder output sample");
                ComPtr<IMFMediaBuffer> bytes;
                check(MFCreateAlignedMemoryBuffer((std::max)(info.cbSize, DWORD(1024 * 1024)), info.cbAlignment, &bytes), "Encoder output memory");
                check(result->AddBuffer(bytes.Get()), "Encoder output buffer");
            }
            MFT_OUTPUT_DATA_BUFFER output{}; output.dwStreamID = output_; output.pSample = result.Get(); DWORD status = 0;
            hr = transform_->ProcessOutput(0, 1, &output, &status); --output_credits_;
            ComPtr<IMFCollection> event_collection; event_collection.Attach(output.pEvents);
            if (!result) result.Attach(output.pSample);
            if (hr == MF_E_TRANSFORM_STREAM_CHANGE) {
                ComPtr<IMFMediaType> changed;
                check(transform_->GetOutputAvailableType(output_, 0, &changed), "Encoder updated output type");
                GUID subtype{}; UINT width = 0, height = 0;
                check(changed->GetGUID(MF_MT_SUBTYPE, &subtype), "Encoder updated subtype");
                check(MFGetAttributeSize(changed.Get(), MF_MT_FRAME_SIZE, &width, &height), "Encoder updated size");
                if (subtype != MFVideoFormat_H264 || width != width_ || height != height_)
                    throw std::runtime_error("Hardware encoder unexpectedly changed geometry/codec");
                check(transform_->SetOutputType(output_, changed.Get(), 0), "Accept encoder updated type");
                continue;
            }
            check(hr, "Encode GPU output");
            if (!result) throw std::runtime_error("Encoder returned no output");
            LONGLONG timestamp = -1;
            check(result->GetSampleTime(&timestamp), "Encoder output timestamp");
            if (timestamp != LONGLONG(index * (10000000LL / fps_))) throw std::runtime_error("Encoder reordered frames");
            ComPtr<IMFMediaBuffer> bytes;
            check(result->ConvertToContiguousBuffer(&bytes), "Encoder compressed bytes");
            BYTE* data = nullptr; DWORD size = 0;
            check(bytes->Lock(&data, nullptr, &size), "Encoder compressed lock");
            if (!size || size > 16 * 1024 * 1024) { bytes->Unlock(); throw std::runtime_error("Encoder packet bounds"); }
            std::vector<uint8_t> packet(data, data + size); check(bytes->Unlock(), "Encoder compressed unlock");
            return packet;
        }
        throw std::runtime_error("Encoder output format-change limit");
    }
};
