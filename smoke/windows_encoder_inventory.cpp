// Read-only Media Foundation hardware encoder discovery. No desktop capture,
// rendering, codec activation, device configuration changes or user input.
#include <windows.h>
#include <mfapi.h>
#include <mfidl.h>
#include <wrl/client.h>
#include <iostream>
#include <string>
using Microsoft::WRL::ComPtr;
static void quoted(const wchar_t* value) {
    std::cout << '"';
    for (const wchar_t* c=value;*c;++c) {
        if (*c=='"' || *c=='\\') std::cout << '\\' << char(*c);
        else if(*c>=32 && *c<127)std::cout<<char(*c);
        else { char escaped[7]{};sprintf_s(escaped,"\\u%04x",unsigned(*c));std::cout<<escaped; }
    }
    std::cout << '"';
}
int main() {
    const auto com=CoInitializeEx(nullptr,COINIT_MULTITHREADED);
    if(FAILED(com))return 1;
    if(FAILED(MFStartup(MF_VERSION,MFSTARTUP_FULL))){CoUninitialize();return 2;}
    MFT_REGISTER_TYPE_INFO input{MFMediaType_Video,MFVideoFormat_NV12};
    MFT_REGISTER_TYPE_INFO output{MFMediaType_Video,MFVideoFormat_H264};
    IMFActivate** items=nullptr;UINT32 count=0;
    const auto hr=MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER,MFT_ENUM_FLAG_HARDWARE|MFT_ENUM_FLAG_SORTANDFILTER,&input,&output,&items,&count);
    if(FAILED(hr)){MFShutdown();CoUninitialize();return 3;}
    std::cout<<"{\"operation\":\"hardware_h264_nv12_inventory\",\"desktop_access\":false,\"encoders\":[";
    for(UINT32 i=0;i<count;++i){
        if(i)std::cout<<',';
        wchar_t* name=nullptr;UINT32 length=0;
        const auto found=items[i]->GetAllocatedString(MFT_FRIENDLY_NAME_Attribute,&name,&length);
        std::cout<<"{\"name\":";quoted(SUCCEEDED(found)&&name?name:L"unknown");
        GUID clsid{};wchar_t id[64]{};
        if(SUCCEEDED(items[i]->GetGUID(MFT_TRANSFORM_CLSID_Attribute,&clsid)))StringFromGUID2(clsid,id,64);
        std::cout<<",\"clsid\":";quoted(id);std::cout<<'}';
        CoTaskMemFree(name);items[i]->Release();
    }
    std::cout<<"]}\n";
    CoTaskMemFree(items);MFShutdown();CoUninitialize();return 0;
}
