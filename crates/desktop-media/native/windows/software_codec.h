#pragma once
#include "nvenc_encoder.h"
#include "mf_decoder.h"
#include <memory>
#include <optional>

#include "../software_codec_ffi.h"

class CaptureEncoder {
    std::unique_ptr<Encoder> hardware_;
    void* software_ = nullptr;
    ComPtr<ID3D11Device> device_;
    ComPtr<ID3D11DeviceContext> context_;
    ComPtr<ID3D11Texture2D> readback_;
    UINT width_, height_, fps_;
    bool fail_encoder_ = false;
    double readback_ms_ = 0, codec_ms_ = 0;
    uint64_t frames_ = 0;
    void initialize_software() {
        // Retire NVENC's retained surface/session before constructing fallback.
        hardware_.reset();
        D3D11_TEXTURE2D_DESC description{};
        description.Width = width_; description.Height = height_;
        description.MipLevels = description.ArraySize = description.SampleDesc.Count = 1;
        description.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        description.Usage = D3D11_USAGE_STAGING; description.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
        check(device_->CreateTexture2D(&description,nullptr,&readback_), "Software capture readback texture");
        device_->GetImmediateContext(&context_);
        software_ = pab_soft_encoder_new(width_,height_,static_cast<uint16_t>(fps_));
        if (!software_) throw std::runtime_error("Software encoder initialization failed");
    }
public:
    CaptureEncoder(ID3D11Device* device, UINT width, UINT height, UINT fps, bool force_software, bool fail_encoder = false)
        : device_(device), width_(width), height_(height), fps_(fps), fail_encoder_(fail_encoder) {
        if (!force_software) {
            auto candidate = std::make_unique<Encoder>();
            try { candidate->initialize(device,width,height,fps); hardware_ = std::move(candidate); }
            catch (const std::exception&) {} // Native RAII retires partial GPU setup before fallback.
        }
        if (fail_encoder_ && !hardware_) throw std::runtime_error("Fault test requires a working hardware encoder");
        if (!hardware_) initialize_software();
    }
    ~CaptureEncoder() { if (software_) pab_soft_encoder_drop(software_); }
    CaptureEncoder(const CaptureEncoder&) = delete;
    bool software() const { return software_ != nullptr; }
    void report() const {
        if (frames_) std::cerr << "software capture frames=" << frames_
            << " mean_readback_ms=" << readback_ms_/frames_
            << " mean_conversion_encode_ms=" << codec_ms_/frames_ << '\n';
    }
    void encode(ID3D11Texture2D* texture, uint64_t index, std::vector<uint8_t>& packet) {
        if (hardware_) {
            try {
                if (fail_encoder_ && index == 2) throw std::runtime_error("Injected encoder failure");
                hardware_->encode(texture,index,nullptr,&packet); return;
            } catch (const std::exception&) {
                initialize_software();
                // Its first published output is a new SPS/PPS + IDR. No failed
                // packet is sent and no frame number or input generation resets.
                std::cerr << "media encoder fallback=openh264_software readback_per_frame=1\n";
            }
        }
        D3D11_TEXTURE2D_DESC description{}; texture->GetDesc(&description);
        if (description.Width != width_ || description.Height != height_ || description.Format != DXGI_FORMAT_B8G8R8A8_UNORM || description.ArraySize != 1 || description.SampleDesc.Count != 1)
            throw std::runtime_error("Software capture texture changed");
        // This copy/readback exists only in the software fallback. The hardware
        // branch above still directly registers the captured texture with NVENC.
        const auto started = Clock::now();
        context_->CopyResource(readback_.Get(),texture);
        D3D11_MAPPED_SUBRESOURCE mapped{};
        check(context_->Map(readback_.Get(),0,D3D11_MAP_READ,0,&mapped), "Map software capture");
        const auto mapped_at = Clock::now();
        PabCodecBytes output{};
        const bool ok = mapped.RowPitch >= width_*4 && pab_soft_encode(software_,static_cast<const uint8_t*>(mapped.pData),
            size_t(mapped.RowPitch)*(height_-1)+size_t(width_)*4,mapped.RowPitch,&output);
        context_->Unmap(readback_.Get(),0);
        readback_ms_ += std::chrono::duration<double,std::milli>(mapped_at-started).count();
        codec_ms_ += std::chrono::duration<double,std::milli>(Clock::now()-mapped_at).count();
        ++frames_;
        if (!ok || !output.data || !output.size) throw std::runtime_error("Software encode failed");
        packet.assign(output.data,output.data+output.size);
    }
};

class VideoDecoder {
    std::unique_ptr<HardwareDecoder> hardware_;
    void* software_ = nullptr;
    ComPtr<ID3D11Device> device_;
    std::optional<DecodedFrame> ready_;
    UINT width_, height_;
public:
    VideoDecoder(ID3D11Device* device, UINT width, UINT height, UINT fps, bool force_software)
        : device_(device), width_(width), height_(height) {
        if (!force_software) {
            try { hardware_ = std::make_unique<HardwareDecoder>(device,width,height,fps); }
            catch (const std::exception&) {}
        }
        if (!hardware_) {
            software_ = pab_soft_decoder_new(width,height);
            if (!software_) throw std::runtime_error("Software decoder initialization failed");
        }
    }
    ~VideoDecoder() { if (software_) pab_soft_decoder_drop(software_); }
    VideoDecoder(const VideoDecoder&) = delete;
    bool software() const { return software_ != nullptr; }
    bool try_submit(const std::vector<uint8_t>& packet, LONGLONG timestamp) {
        if (hardware_) return hardware_->try_submit(packet,timestamp);
        if (ready_) return false;
        PabCodecBytes output{};
        if (!pab_soft_decode(software_,packet.data(),packet.size(),&output)) throw std::runtime_error("Software decode failed");
        if (!output.size) return true;
        if (!output.data || output.size != size_t(width_)*height_*3/2) throw std::runtime_error("Software decoded image size mismatch");
        D3D11_TEXTURE2D_DESC description{};
        description.Width = width_; description.Height = height_;
        description.MipLevels = description.ArraySize = description.SampleDesc.Count = 1;
        description.Format = DXGI_FORMAT_NV12; description.Usage = D3D11_USAGE_DEFAULT;
        // VideoProcessorInputView accepts default-usage textures with no bind
        // flags. SHADER_RESOURCE alone is explicitly not a valid input binding.
        description.BindFlags = 0;
        D3D11_SUBRESOURCE_DATA initial{}; initial.pSysMem = output.data; initial.SysMemPitch = width_;
        DecodedFrame frame; frame.timestamp = timestamp;
        // Software output must be uploaded. Keep its own GPU texture alive for
        // rendering/static repaint; no shared decoder surface is overwritten.
        check(device_->CreateTexture2D(&description,&initial,&frame.texture), "Upload software NV12");
        ready_ = std::move(frame);
        return true;
    }
    void submit(const std::vector<uint8_t>& packet, LONGLONG timestamp) {
        if (!try_submit(packet,timestamp)) throw std::runtime_error("Decoder requires output before input");
    }
    void receive(const std::function<void(DecodedFrame&)>& consume) {
        if (hardware_) { hardware_->receive(consume); return; }
        if (ready_) { consume(*ready_); ready_.reset(); }
    }
    void drain(const std::function<void(DecodedFrame&)>& consume) {
        if (hardware_) hardware_->drain(consume); else receive(consume);
    }
};
