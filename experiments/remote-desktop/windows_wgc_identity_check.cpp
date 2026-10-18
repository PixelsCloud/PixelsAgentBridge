// Opt-in read-only activation probe. No capture, input, service or display changes.
// Run --launch SESSION system|user|impersonate OUTPUT as SYSTEM. Each child uses
// the same selected interactive desktop; only its process/thread token differs.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <wtsapi32.h>
#include <userenv.h>
#include <sddl.h>
#include <winrt/Windows.Graphics.Capture.h>
#include <fstream>
#include <string>
#include <vector>
#include <stdexcept>

struct Handle {
    HANDLE value = nullptr;
    ~Handle() { if (value) CloseHandle(value); }
};
static void require(BOOL ok) {
    if (!ok) throw std::runtime_error("Windows API failed: " + std::to_string(GetLastError()));
}
static bool system_token(HANDLE token) {
    DWORD size = 0;
    GetTokenInformation(token, TokenUser, nullptr, 0, &size);
    std::vector<unsigned char> bytes(size);
    require(GetTokenInformation(token, TokenUser, bytes.data(), size, &size));
    return IsWellKnownSid(reinterpret_cast<TOKEN_USER*>(bytes.data())->User.Sid, WinLocalSystemSid);
}
int wmain(int argc, wchar_t** argv) {
    try {
        if (argc == 5 && std::wstring(argv[1]) == L"--launch") {
            Handle own; require(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY | TOKEN_DUPLICATE, &own.value));
            if (!system_token(own.value)) throw std::runtime_error("SYSTEM launcher required");
            const DWORD session = std::stoul(argv[2]);
            if (!session) throw std::runtime_error("Interactive session required");
            const std::wstring mode(argv[3]);
            if (mode != L"system" && mode != L"user" && mode != L"impersonate") throw std::runtime_error("Unknown mode");
            Handle user; require(WTSQueryUserToken(session, &user.value));
            Handle primary;
            require(DuplicateTokenEx(mode == L"user" ? user.value : own.value, TOKEN_ALL_ACCESS,
                nullptr, SecurityImpersonation, TokenPrimary, &primary.value));
            require(SetTokenInformation(primary.value, TokenSessionId, const_cast<DWORD*>(&session), sizeof(session)));
            wchar_t executable[32768]{};
            const auto length = GetModuleFileNameW(nullptr, executable, 32768);
            if (!length || length == 32768) throw std::runtime_error("Executable path unavailable");
            std::wstring output(argv[4]);
            if (output.find(L'"') != std::wstring::npos) throw std::runtime_error("Invalid output path");
            std::wstring command = L"\"" + std::wstring(executable) + L"\" --query " + mode + L" \"" + output + L"\"";
            STARTUPINFOW startup{}; startup.cb = sizeof(startup);
            wchar_t desktop[] = L"WinSta0\\Default"; startup.lpDesktop = desktop;
            PROCESS_INFORMATION created{};
            void* environment = nullptr; require(CreateEnvironmentBlock(&environment, primary.value, FALSE));
            const BOOL launched = CreateProcessAsUserW(primary.value, executable, command.data(), nullptr,
                nullptr, FALSE, CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW, environment, nullptr, &startup, &created);
            const DWORD launch_error = GetLastError(); DestroyEnvironmentBlock(environment);
            if (!launched) { SetLastError(launch_error); require(FALSE); }
            Handle process{created.hProcess}, thread{created.hThread};
            if (WaitForSingleObject(process.value, 15000) != WAIT_OBJECT_0) {
                TerminateProcess(process.value, 2); WaitForSingleObject(process.value, 5000);
                throw std::runtime_error("Owned activation probe timed out");
            }
            DWORD exit = 0; require(GetExitCodeProcess(process.value, &exit)); return static_cast<int>(exit);
        }
        if (argc != 4 || std::wstring(argv[1]) != L"--query") throw std::runtime_error("Invalid arguments");
        Handle own; require(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &own.value));
        DWORD session = 0; require(ProcessIdToSessionId(GetCurrentProcessId(), &session));
        const bool impersonate = std::wstring(argv[2]) == L"impersonate";
        Handle user;
        if (impersonate) { require(WTSQueryUserToken(session, &user.value)); require(ImpersonateLoggedOnUser(user.value)); }
        const char* stage = "runtime"; HRESULT result = S_OK; bool supported = false;
        bool initialized = false;
        try {
            winrt::init_apartment(winrt::apartment_type::multi_threaded); initialized = true;
            stage = "activation_factory";
            auto factory = winrt::get_activation_factory<winrt::Windows::Graphics::Capture::GraphicsCaptureSession,
                winrt::Windows::Graphics::Capture::IGraphicsCaptureSessionStatics>();
            stage = "support_query"; supported = factory.IsSupported();
            stage = "complete";
        } catch (...) { result = winrt::to_hresult(); }
        if (initialized) winrt::uninit_apartment();
        if (impersonate) require(RevertToSelf());
        std::ofstream output(argv[3]);
        output << "{\"session\":" << session << ",\"process_system\":" << (system_token(own.value) ? "true" : "false")
            << ",\"impersonated\":" << (impersonate ? "true" : "false") << ",\"stage\":\"" << stage
            << "\",\"hresult\":\"0x" << std::hex << static_cast<unsigned long>(result)
            << "\",\"supported\":" << (supported ? "true" : "false") << "}\n";
        if (!output) throw std::runtime_error("Output unavailable");
        return 0; // A failed activation is a recorded probe result.
    } catch (const std::exception& error) { fputs(error.what(), stderr); return 1; }
}
