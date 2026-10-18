!ifndef PAB_SETUP_DIAGNOSTICS_INCLUDED
!define PAB_SETUP_DIAGNOSTICS_INCLUDED
!include "FileFunc.nsh"
!include "Win\WinError.nsh"

!ifndef PAB_SETUP_MUTEX_NAME
    !define PAB_SETUP_MUTEX_NAME "Global\PixelsAgentBridgeSetup"
!endif

Var SetupLog
Var SetupLogRoot
Var SetupPhase
Var SetupMessage
Var SetupMutex
Var SetupWin32Error
Var SetupCoordinatorMutex
Var SetupRecoveryAttempt
Var SetupRecoveryExit

LangString SetupLockFailed ${LANG_SIMPCHINESE} "无法初始化安装锁，安装未继续。Windows 错误码："
LangString SetupLockFailed ${LANG_ENGLISH} "Could not initialize the setup lock. Setup did not continue. Windows error:"
LangString SetupLogLocation ${LANG_SIMPCHINESE} "诊断日志："
LangString SetupLogLocation ${LANG_ENGLISH} "Diagnostic log:"
LangString SetupForcePrompt ${LANG_SIMPCHINESE} "另一个 Pixels 安装或卸载程序仍在运行。是否结束旧安装进程并强制升级？设备身份、密码和配置将保留。"
LangString SetupForcePrompt ${LANG_ENGLISH} "Another Pixels setup or uninstaller is running. End the previous setup processes and force the upgrade? Device identity, passwords and configuration will be retained."

!macro PabTrySetupRecovery Prefix RetryLabel Suffix
    !if "${Prefix}" == ""
    !ifdef PAB_SETUP_RECOVERY_FUNCTION
        ${If} $SetupRecoveryAttempt = 0
            ${GetParameters} $R0
            ClearErrors
            ${GetOptions} $R0 "/FORCE" $R1
            ${If} ${Errors}
                IfSilent no_recovery_${Suffix}
                MessageBox MB_ICONQUESTION|MB_YESNO "$(SetupForcePrompt)" IDNO no_recovery_${Suffix}
            ${EndIf}
            StrCpy $SetupRecoveryAttempt 1
            Call ${PAB_SETUP_RECOVERY_FUNCTION}
            ${If} $SetupRecoveryExit = 0
                Goto ${RetryLabel}
            ${EndIf}
        ${EndIf}
    no_recovery_${Suffix}:
    !endif
    !endif
!macroend

!macro PabDefineSetupDiagnostics Prefix Action
Function ${Prefix}PabSetupLog
    Push $0
    Push $1
    Push $2
    Push $3
    Push $4
    Push $5
    Push $6
    Push $7
    Push $8
    StrCpy $8 0
    ${If} ${Errors}
        StrCpy $8 1
    ${EndIf}
    StrCmp $SetupLog "" done
    ClearErrors
    FileOpen $7 "$SetupLog" a
    IfErrors done
    FileSeek $7 0 END $0
    ${If} $0 = 0
        FileWriteUTF16LE /BOM $7 "Pixels Agent Bridge setup diagnostics$\r$\n"
    ${EndIf}
    ${GetTime} "" "LS" $0 $1 $2 $3 $4 $5 $6
    FileWriteUTF16LE $7 "$2-$1-$0T$4:$5:$6Z [nsis] phase=$SetupPhase $SetupMessage$\r$\n"
    FileClose $7
done:
    ${If} $8 = 1
        SetErrors
    ${Else}
        ClearErrors
    ${EndIf}
    Pop $8
    Pop $7
    Pop $6
    Pop $5
    Pop $4
    Pop $3
    Pop $2
    Pop $1
    Pop $0
FunctionEnd

Function ${Prefix}PabInitSetupLog
    Push $0
    Push $1
    StrCpy $SetupLog ""
    StrCpy $SetupPhase "initializing"
    !ifdef PAB_SETUP_LOG_ROOT
        StrCpy $SetupLogRoot "${PAB_SETUP_LOG_ROOT}"
    !else
        StrCpy $SetupLogRoot "$APPDATA\PixelsAgentBridge\logs\setup"
    !endif
    ClearErrors
    CreateDirectory "$SetupLogRoot"
    IfErrors fallback
    GetTempFileName $SetupLog "$SetupLogRoot"
    IfErrors fallback ready
fallback:
    !ifdef PAB_SETUP_FALLBACK_LOG_ROOT
        StrCpy $SetupLogRoot "${PAB_SETUP_FALLBACK_LOG_ROOT}"
    !else
        StrCpy $SetupLogRoot "$TEMP\PixelsAgentBridge-InstallerLogs"
    !endif
    ClearErrors
    CreateDirectory "$SetupLogRoot"
    IfErrors unavailable
    GetTempFileName $SetupLog "$SetupLogRoot"
    IfErrors unavailable ready
unavailable:
    StrCpy $SetupLog ""
    DetailPrint "Could not create diagnostics in ProgramData or TEMP."
    Goto done
ready:
    ${GetFileName} "$SetupLog" $0
    StrCpy $1 "$SetupLogRoot\${Action}-$0.log"
    ClearErrors
    Rename "$SetupLog" "$1"
    ${IfNot} ${Errors}
        StrCpy $SetupLog $1
    ${EndIf}
    System::Call 'kernel32::GetCurrentProcessId()i.r0'
    StrCpy $SetupMessage "start action=${Action} version=${APP_VERSION} profile=${BUILD_PROFILE} pid=$0 exe=$EXEPATH"
    Call ${Prefix}PabSetupLog
    DetailPrint "$(SetupLogLocation) $SetupLog"
done:
    ClearErrors
    Pop $1
    Pop $0
FunctionEnd

Function ${Prefix}PabCheckSetupLock
    StrCpy $SetupPhase "check_setup_lock"
    StrCpy $SetupRecoveryAttempt 0
coordinator_retry:
    System::Call 'kernel32::CreateMutexW(p0,i0,w"${PAB_SETUP_MUTEX_NAME}-v2")p.r0?e'
    Pop $1
    ${If} $0 = 0
        StrCpy $SetupWin32Error $1
        StrCpy $SetupMessage "coordinator_initialization_failed win32_error=$1 exit_code=2"
        Call ${Prefix}PabSetupLog
        IfSilent coordinator_failed
        MessageBox MB_ICONSTOP "$(SetupLockFailed) $SetupWin32Error$\r$\n$(SetupLogLocation) $SetupLog"
    coordinator_failed:
        SetErrorLevel 2
        Abort
    ${EndIf}
    ${If} $1 = ${ERROR_ALREADY_EXISTS}
        System::Call 'kernel32::CloseHandle(pr0)'
        !insertmacro PabTrySetupRecovery "${Prefix}" coordinator_retry coordinator_busy
        StrCpy $SetupMessage "setup_already_running coordinator exit_code=2"
        Call ${Prefix}PabSetupLog
        IfSilent coordinator_busy
        MessageBox MB_ICONSTOP "$(SetupAlreadyRunning)$\r$\n$(SetupLogLocation) $SetupLog"
    coordinator_busy:
        SetErrorLevel 2
        Abort
    ${EndIf}
    StrCpy $SetupCoordinatorMutex $0
legacy_retry:
    System::Call 'kernel32::CreateMutexW(p0,i0,w"${PAB_SETUP_MUTEX_NAME}")p.r0?e'
    Pop $1
    StrCpy $SetupMutex $0
    StrCpy $SetupWin32Error $1
    ${If} $0 = 0
        ${If} $SetupWin32Error != ${ERROR_INVALID_HANDLE}
            ; Access denied can mean an older SYSTEM installer is still active.
            ; Require verified takeover before falling back in that case.
            !insertmacro PabTrySetupRecovery "${Prefix}" legacy_retry legacy_init
            ${If} $SetupRecoveryAttempt != 1
            ${OrIf} $SetupRecoveryExit != 0
                StrCpy $SetupMessage "lock_initialization_failed win32_error=$SetupWin32Error exit_code=2"
                Call ${Prefix}PabSetupLog
                IfSilent legacy_failed
                MessageBox MB_ICONSTOP "$(SetupLockFailed) $SetupWin32Error$\r$\n$(SetupLogLocation) $SetupLog"
            legacy_failed:
                SetErrorLevel 2
                Abort
            ${EndIf}
        ${EndIf}
        ; An object of the wrong type cannot belong to a functioning old setup.
        ; v2 still excludes all new installers and uninstallers.
        StrCpy $SetupMessage "legacy_lock_unavailable win32_error=$SetupWin32Error coordinator_fallback"
        Call ${Prefix}PabSetupLog
        Return
    ${EndIf}
    ${If} $1 = ${ERROR_ALREADY_EXISTS}
        System::Call 'kernel32::CloseHandle(p $SetupMutex)'
        StrCpy $SetupMutex 0
        !insertmacro PabTrySetupRecovery "${Prefix}" legacy_retry legacy_busy
        StrCpy $SetupMessage "setup_already_running win32_error=${ERROR_ALREADY_EXISTS} exit_code=2"
        Call ${Prefix}PabSetupLog
        IfSilent already_running
        MessageBox MB_ICONSTOP "$(SetupAlreadyRunning)$\r$\n$(SetupLogLocation) $SetupLog"
    already_running:
        SetErrorLevel 2
        Abort
    ${EndIf}
    StrCpy $SetupMessage "setup_lock_acquired"
    Call ${Prefix}PabSetupLog
FunctionEnd

Function ${Prefix}PabSetupCancelled
    StrCpy $SetupMessage "user_cancelled"
    Call ${Prefix}PabSetupLog
FunctionEnd

Function ${Prefix}PabCloseSetupLog
    GetErrorLevel $0
    StrCpy $SetupMessage "window_closed exit_code=$0"
    Call ${Prefix}PabSetupLog
    ${If} $SetupMutex != 0
        System::Call 'kernel32::CloseHandle(p $SetupMutex)'
        StrCpy $SetupMutex 0
    ${EndIf}
    ${If} $SetupCoordinatorMutex != 0
        System::Call 'kernel32::CloseHandle(p $SetupCoordinatorMutex)'
        StrCpy $SetupCoordinatorMutex 0
    ${EndIf}
FunctionEnd
!macroend

!insertmacro PabDefineSetupDiagnostics "" "install"
!insertmacro PabDefineSetupDiagnostics "un." "uninstall"

!macro PabInitializeSetup Prefix
    SetRegView 64
    SetShellVarContext all
    Call ${Prefix}PabInitSetupLog
    Call ${Prefix}PabCheckSetupLock
!macroend
!endif
