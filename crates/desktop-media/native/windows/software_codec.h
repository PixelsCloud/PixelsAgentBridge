#pragma once
#include "nvenc_encoder.h"
#include "mf_decoder.h"
#include "mf_encoder.h"
#include <memory>
#include <optional>

#include "../software_codec_ffi.h"

class CaptureEncoder {
    std::unique_ptr<Encoder> hardware_;
    std::unique_ptr<IntelHardwareEncoder> intel_;
    void* software_ = nullptr;
    ComPtr<ID3D11Device> device_;
    ComPtr<ID3D11DeviceContext> context_;
    ComPtr<ID3D11Texture2D> readback_;
    ComPtr<ID3D11Texture2D> padded_;
    UINT width_, height_, fps_;
    bool fail_encoder_ = false;
    double readback_ms_ = 0, codec_ms_ = 0;
    uint64_t frames_ = 0;
    uint32_t budget_ = 0;
    uint64_t readback_us_ = 0, conversion_us_ = 0, codec_us_ = 0;
    void initialize_software() {
        // Retire NVENC's retained surface/session before constructing fallback.
        hardware_.reset();
        intel_.reset();
        padded_.Reset();
        D3D11_TEXTURE2D_DESC description{};
        description.Width = width_; description.Height = height_;
        description.MipLevels = description.ArraySize = description.SampleDesc.Count = 1;
        description.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
        description.Usage = D3D11_USAGE_STAGING; description.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
        check(device_->CreateTexture2D(&description,nullptr,&readback_), "Software capture readback texture");
        device_->GetImmediateContext(&context_);
        software_ = pab_soft_encoder_new(width_,height_,static_cast<uint16_t>(fps_));
        if (!software_) throw std::runtime_error("Software encoder initialization failed");
        if (budget_ && !pab_soft_encoder_budget(software_, budget_))
            throw std::runtime_error("Software encoder budget initialization failed");
    }
public:
    CaptureEncoder(ID3D11Device* device, UINT width, UINT height, UINT fps, bool force_software, bool fail_encoder = false)
        : device_(device), width_(width), height_(height), fps_(fps), fail_encoder_(fail_encoder) {
        const PabVideoGeometry geometry(width, height);
        if (!force_software) {
            auto candidate = std::make_unique<Encoder>();
            try {
                candidate->initialize(device,geometry.coded_width,geometry.coded_height,fps);
                if (width != geometry.coded_width || height != geometry.coded_height) {
                    D3D11_TEXTURE2D_DESC description{};
                    description.Width = geometry.coded_width; description.Height = geometry.coded_height;
                    description.MipLevels = description.ArraySize = description.SampleDesc.Count = 1;
                    description.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
                    description.Usage = D3D11_USAGE_DEFAULT;
                    check(device->CreateTexture2D(&description,nullptr,&padded_), "Padded GPU capture texture");
                    device->GetImmediateContext(&context_);
                }
                hardware_ = std::move(candidate);
            }
            catch (const std::exception&) {} // Native RAII retires partial GPU setup before fallback.
        }
        if (!force_software && !hardware_) {
            try { intel_ = std::make_unique<IntelHardwareEncoder>(device,width,height,fps); }
            catch (const std::exception& error) {
                std::cerr << "media encoder_unavailable=intel_mf_gpu reason=" << error.what() << '\n';
            } // Unsupported/partial setup is retired before CPU fallback.
        }
        if (fail_encoder_ && !hardware_ && !intel_) throw std::runtime_error("Fault test requires a working hardware encoder");
        if (!hardware_ && !intel_) initialize_software();
    }
    ~CaptureEncoder() { if (software_) pab_soft_encoder_drop(software_); }
    CaptureEncoder(const CaptureEncoder&) = delete;
    bool software() const { return software_ != nullptr; }
    const char* name() const { return software_ ? "openh264_software" : intel_ ? "intel_mf_gpu" : "nvenc_gpu"; }
    uint64_t readback_us() const { return readback_us_; }
    uint64_t conversion_us() const { return conversion_us_; }
    uint64_t codec_us() const { return codec_us_; }
    bool budget(uint32_t value) {
        if (value && (value < 150000 || value > 16000000)) throw std::runtime_error("Encoder budget outside bounds");
        if (intel_) {
            // Keep the qualified quality mode and surface path. Report optional
            // rate control as unsupported rather than quietly restarting a live codec.
            return value == 0;
        }
        if (hardware_) {
            try { hardware_->budget(value); budget_ = value; return true; }
            catch (const std::exception&) {
                // Rate-control support alone must not introduce CPU readback
                // on an otherwise healthy direct GPU capture/encode path.
                std::cerr << "media encoder_budget=unsupported encoder=nvenc_hardware\n";
                return false;
            }
        }
        if (!pab_soft_encoder_budget(software_, value)) throw std::runtime_error("Software encoder budget update failed");
        budget_ = value;
        return true;
    }
    void report() const {
        if (frames_) std::cerr << "software capture frames=" << frames_
            << " mean_readback_ms=" << readback_ms_/frames_
            << " mean_conversion_encode_ms=" << codec_ms_/frames_ << '\n';
    }
    void encode(ID3D11Texture2D* texture, uint64_t index, std::vector<uint8_t>& packet) {
        readback_us_ = conversion_us_ = codec_us_ = 0;
        D3D11_TEXTURE2D_DESC description{}; texture->GetDesc(&description);
        if (description.Width != width_ || description.Height != height_ || description.Format != DXGI_FORMAT_B8G8R8A8_UNORM || description.ArraySize != 1 || description.SampleDesc.Count != 1)
            throw std::runtime_error("Capture texture changed");
        if (intel_) {
            try {
                if (fail_encoder_ && index == 2) throw std::runtime_error("Injected encoder failure");
                const auto started = Clock::now();
                packet = intel_->encode(texture,index);
                codec_us_ = std::chrono::duration_cast<std::chrono::microseconds>(Clock::now()-started).count();
                return;
            } catch (const std::exception& error) {
                initialize_software();
                std::cerr << "media encoder fallback=openh264_software readback_per_frame=1 reason=" << error.what() << '\n';
            }
        }
        if (hardware_) {
            try {
                if (fail_encoder_ && index == 2) throw std::runtime_error("Injected encoder failure");
                // Even surfaces still register directly. Odd surfaces require
                // GPU-only edge padding; no CPU readback or conversion.
                if (padded_) {
                    const D3D11_BOX image{0,0,0,width_,height_,1};
                    context_->CopySubresourceRegion(padded_.Get(),0,0,0,0,texture,0,&image);
                    if (width_ & 1) {
                        const D3D11_BOX edge{width_-1,0,0,width_,height_,1};
                        context_->CopySubresourceRegion(padded_.Get(),0,width_,0,0,texture,0,&edge);
                    }
                    if (height_ & 1) {
                        const D3D11_BOX edge{0,height_-1,0,width_,height_,1};
                        context_->CopySubresourceRegion(padded_.Get(),0,0,height_,0,texture,0,&edge);
                        if (width_ & 1) {
                            const D3D11_BOX corner{width_-1,height_-1,0,width_,height_,1};
                            context_->CopySubresourceRegion(padded_.Get(),0,width_,height_,0,texture,0,&corner);
                        }
                    }
                }
                const auto codec_start = Clock::now();
                hardware_->encode(padded_ ? padded_.Get() : texture,index,nullptr,&packet);
                codec_us_ = std::chrono::duration_cast<std::chrono::microseconds>(Clock::now()-codec_start).count();
                return;
            } catch (const std::exception&) {
                initialize_software();
                // Its first published output is a new SPS/PPS + IDR. No failed
                // packet is sent and no frame number or input generation resets.
                std::cerr << "media encoder fallback=openh264_software readback_per_frame=1\n";
            }
        }
        // This copy/readback exists only in the software fallback. The hardware
        // branch above registers a GPU surface with NVENC.
        const auto started = Clock::now();
        context_->CopyResource(readback_.Get(),texture);
        D3D11_MAPPED_SUBRESOURCE mapped{};
        check(context_->Map(readback_.Get(),0,D3D11_MAP_READ,0,&mapped), "Map software capture");
        const auto mapped_at = Clock::now();
        PabCodecBytes output{};
        const bool prepared = mapped.RowPitch >= width_*4 && pab_soft_prepare_bgra(software_,static_cast<const uint8_t*>(mapped.pData),
            size_t(mapped.RowPitch)*(height_-1)+size_t(width_)*4,mapped.RowPitch);
        const auto converted = Clock::now();
        context_->Unmap(readback_.Get(),0);
        const auto codec_start = Clock::now();
        const bool ok = prepared && pab_soft_encode_prepared(software_, &output);
        readback_us_ = std::chrono::duration_cast<std::chrono::microseconds>(mapped_at-started).count();
        conversion_us_ = std::chrono::duration_cast<std::chrono::microseconds>(converted-mapped_at).count();
        codec_us_ = std::chrono::duration_cast<std::chrono::microseconds>(Clock::now()-codec_start).count();
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
        const PabVideoGeometry geometry(width, height);
        width_ = geometry.coded_width; height_ = geometry.coded_height;
        if (!force_software) {
            try { hardware_ = std::make_unique<HardwareDecoder>(device,width_,height_,fps); }
            catch (const std::exception&) {}
        }
        if (!hardware_) {
            software_ = pab_soft_decoder_new(width_,height_);
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
