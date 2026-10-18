#pragma once
#include <optional>
#include <userenv.h>
#include <wtsapi32.h>

// WGC cannot query support from a SYSTEM process, including when that thread
// impersonates the desktop user. Keep input/recovery and Winlogon capture under
// the existing SYSTEM helper; run only Default capture/cursor in its WTS user.
// The child inherits the original media pipes directly: no frame copy or proxy
// protocol. A private kill-on-close job bounds it to this worker's lifetime.
namespace user_capture {
struct Handle {
    HANDLE value = nullptr;
    ~Handle() { if (value && value != INVALID_HANDLE_VALUE) CloseHandle(value); }
    Handle() = default;
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
};
inline void require(BOOL success, const char* stage) {
    if (!success) throw std::runtime_error(std::string(stage) + " error=" + std::to_string(GetLastError()));
}
struct Environment {
    void* value = nullptr;
    ~Environment() { if (value) DestroyEnvironmentBlock(value); }
};
struct Attributes {
    std::vector<uint8_t> bytes;
    LPPROC_THREAD_ATTRIBUTE_LIST list = nullptr;
    Attributes() {
        SIZE_T size = 0;
        InitializeProcThreadAttributeList(nullptr, 1, 0, &size);
        if (!size) throw std::runtime_error("Capture handle list unavailable");
        bytes.resize(size);
        auto* candidate = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(bytes.data());
        require(InitializeProcThreadAttributeList(candidate, 1, 0, &size), "Capture handle list");
        list = candidate;
    }
    ~Attributes() { if (list) DeleteProcThreadAttributeList(list); }
};
struct Child {
    PROCESS_INFORMATION info{};
    ~Child() {
        if (info.hProcess) {
            if (WaitForSingleObject(info.hProcess, 0) == WAIT_TIMEOUT) {
                TerminateProcess(info.hProcess, 1);
                WaitForSingleObject(info.hProcess, INFINITE);
            }
            CloseHandle(info.hProcess);
        }
        if (info.hThread) CloseHandle(info.hThread);
    }
};
inline std::optional<int> run(int argc, const char* const* argv) {
    if (argc < 2) return {};
    const std::string command(argv[1]);
    if (command != "capture" && command != "cursor") return {};
    Handle process_token;
    require(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &process_token.value), "Capture process token");
    DWORD size = 0;
    GetTokenInformation(process_token.value, TokenUser, nullptr, 0, &size);
    if (!size) throw std::runtime_error("Capture identity unavailable");
    std::vector<uint8_t> identity(size);
    require(GetTokenInformation(process_token.value, TokenUser, identity.data(), size, &size), "Capture identity");
    if (!IsWellKnownSid(reinterpret_cast<TOKEN_USER*>(identity.data())->User.Sid, WinLocalSystemSid)) return {};
    // The system worker attached to Winlogon must retain its secure-desktop
    // access. Its DXGI capture path does not use WGC.
    if (desktop_name(GetThreadDesktop(GetCurrentThreadId())) != L"Default") return {};
    DWORD session = 0;
    require(ProcessIdToSessionId(GetCurrentProcessId(), &session), "Capture session");
    if (!session) throw std::runtime_error("User capture requires an interactive session");
    Handle user;
    require(WTSQueryUserToken(session, &user.value), "Capture user token");
    Environment environment;
    require(CreateEnvironmentBlock(&environment.value, user.value, FALSE), "Capture user environment");

    Handle pipes[3]; HANDLE inherited[3]{};
    const DWORD identifiers[] = {STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE};
    for (size_t i = 0; i < 3; ++i) {
        require(DuplicateHandle(GetCurrentProcess(), GetStdHandle(identifiers[i]), GetCurrentProcess(),
            &pipes[i].value, 0, TRUE, DUPLICATE_SAME_ACCESS), "Capture inherited pipe");
        inherited[i] = pipes[i].value;
    }
    Attributes attributes;
    require(UpdateProcThreadAttribute(attributes.list, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        inherited, sizeof(inherited), nullptr, nullptr), "Capture inherited handle list");
    Handle job;
    job.value = CreateJobObjectW(nullptr, nullptr);
    require(job.value != nullptr, "Capture lifetime job");
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    require(SetInformationJobObject(job.value, JobObjectExtendedLimitInformation, &limits, sizeof(limits)),
        "Capture lifetime limit");
    wchar_t executable[32768]{};
    const auto length = GetModuleFileNameW(nullptr, executable, 32768);
    if (!length || length == 32768) throw std::runtime_error("Capture executable unavailable");
    std::wstring arguments(GetCommandLineW());
    wchar_t desktop[] = L"WinSta0\\Default";
    STARTUPINFOEXW startup{}; startup.StartupInfo.cb = sizeof(startup);
    startup.StartupInfo.lpDesktop = desktop;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = inherited[0];
    startup.StartupInfo.hStdOutput = inherited[1];
    startup.StartupInfo.hStdError = inherited[2];
    startup.lpAttributeList = attributes.list;
    Child child;
    require(CreateProcessAsUserW(user.value, executable, arguments.data(), nullptr, nullptr, TRUE,
        CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
        environment.value, nullptr, &startup.StartupInfo, &child.info), "Launch user capture");
    require(AssignProcessToJobObject(job.value, child.info.hProcess), "Bind user capture lifetime");
    require(ResumeThread(child.info.hThread) != DWORD(-1), "Start user capture");
    std::cerr << "media capture_identity=session_user pid=" << child.info.dwProcessId << '\n';
    require(WaitForSingleObject(child.info.hProcess, INFINITE) == WAIT_OBJECT_0, "Wait for user capture");
    DWORD exit = 1;
    require(GetExitCodeProcess(child.info.hProcess, &exit), "User capture result");
    return static_cast<int>(exit);
}
}
