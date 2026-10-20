Unicode true

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"

!ifndef PAYLOAD_DIR
    !error "PAYLOAD_DIR is required"
!endif
!ifndef OUTPUT_FILE
    !error "OUTPUT_FILE is required"
!endif
!ifndef CONTROL_URL
    !error "CONTROL_URL is required"
!endif
!ifndef RELAY_URL
    !error "RELAY_URL is required"
!endif
!if "${CONTROL_URL}" == ""
    !if "${RELAY_URL}" != ""
        !error "Server URLs must be supplied together"
    !endif
    !define SERVER_ARGUMENTS ""
!else
    !if "${RELAY_URL}" == ""
        !error "Server URLs must be supplied together"
    !endif
    !define SERVER_ARGUMENTS '-ControlUrl "${CONTROL_URL}" -RelayUrl "${RELAY_URL}"'
!endif
!ifndef APP_ICON
    !error "APP_ICON is required"
!endif
!ifndef BUILD_PROFILE
    !error "BUILD_PROFILE is required"
!endif
!ifndef APP_VERSION
    !error "APP_VERSION is required"
!endif

Name "Pixels Agent Bridge"
OutFile "${OUTPUT_FILE}"
InstallDir "$PROGRAMFILES64\PixelsAgentBridge"
InstallDirRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "InstallLocation"
RequestExecutionLevel admin
ShowInstDetails show
ShowUninstDetails show
SetCompressor /SOLID lzma
SetCompressorDictSize 64
Icon "${APP_ICON}"
UninstallIcon "${APP_ICON}"
VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "Pixels Agent Bridge"
VIAddVersionKey /LANG=1033 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "CompanyName" "Pixels"
VIAddVersionKey /LANG=1033 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=1033 "FileDescription" "Pixels Agent Bridge ${BUILD_PROFILE} Installer"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Copyright (C) 2026 Pixels"

!define MUI_ABORTWARNING
!define MUI_CUSTOMFUNCTION_ABORT PabSetupCancelled
!define MUI_CUSTOMFUNCTION_UNABORT un.PabSetupCancelled
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

LangString InstallFailed ${LANG_SIMPCHINESE} "安装服务失败。请查看上方安装日志。退出代码："
LangString InstallFailed ${LANG_ENGLISH} "Service installation failed. Check the installation log above. Exit code:"
LangString SetupAlreadyRunning ${LANG_SIMPCHINESE} "另一个 Pixels Agent Bridge 安装或卸载程序仍在运行。请先完成或关闭它，然后重试。"
LangString SetupAlreadyRunning ${LANG_ENGLISH} "Another Pixels Agent Bridge setup or uninstaller is running. Close it before retrying."
LangString UninstallFailed ${LANG_SIMPCHINESE} "卸载服务失败。请查看上方卸载日志。退出代码："
LangString UninstallFailed ${LANG_ENGLISH} "Service removal failed. Check the uninstallation log above. Exit code:"

!define PAB_SETUP_RECOVERY_FUNCTION PabRunSetupRecovery
!include "${__FILEDIR__}\setup-diagnostics.nsh"

Function PabRunSetupRecovery
    InitPluginsDir
    SetOutPath "$PLUGINSDIR\recovery"
    File "${PAYLOAD_DIR}\installer-logging.ps1"
    File "${PAYLOAD_DIR}\installer-recovery.ps1"
    System::Call 'kernel32::GetCurrentProcessId()i.r0'
    StrCpy $SetupMessage "force_upgrade_recovery_started"
    Call PabSetupLog
    nsExec::ExecToLog /TIMEOUT=30000 '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\recovery\installer-recovery.ps1" -CallerPid $0 -InstallerLogPath "$SetupLog"'
    Pop $SetupRecoveryExit
    StrCpy $SetupMessage "force_upgrade_recovery_exit=$SetupRecoveryExit"
    Call PabSetupLog
FunctionEnd

Function .onInit
    !insertmacro PabInitializeSetup ""
    ${IfNot} ${RunningX64}
        StrCpy $SetupMessage "unsupported_architecture exit_code=3"
        Call PabSetupLog
        IfSilent unsupported_arch
        MessageBox MB_ICONSTOP "This package requires 64-bit Windows."
    unsupported_arch:
        SetErrorLevel 3
        Abort
    ${EndIf}
FunctionEnd

Function un.onInit
    !insertmacro PabInitializeSetup "un."
FunctionEnd

Function .onGUIEnd
    Call PabCloseSetupLog
FunctionEnd

Function un.onGUIEnd
    Call un.PabCloseSetupLog
FunctionEnd

Function .onInstFailed
    StrCpy $SetupMessage "installation_failed"
    Call PabSetupLog
FunctionEnd

Function un.onUninstFailed
    StrCpy $SetupMessage "uninstallation_failed"
    Call un.PabSetupLog
FunctionEnd

Function .onInstSuccess
    StrCpy $SetupPhase "completed"
    StrCpy $SetupMessage "installation_succeeded"
    Call PabSetupLog
FunctionEnd

Function un.onUninstSuccess
    StrCpy $SetupPhase "completed"
    StrCpy $SetupMessage "uninstallation_succeeded"
    Call un.PabSetupLog
FunctionEnd

Section "Install"
    StrCpy $SetupPhase "extract_payload"
    StrCpy $SetupMessage "extracting_package"
    Call PabSetupLog
    InitPluginsDir
    SetOutPath "$PLUGINSDIR\payload"
    File "${PAYLOAD_DIR}\pab-desktop.exe"
    File "${PAYLOAD_DIR}\pab-executor.exe"
    File "${PAYLOAD_DIR}\pab-mcp.exe"
    File "${PAYLOAD_DIR}\pab-document-worker.exe"
    File "${PAYLOAD_DIR}\browser-runtime.zip"
    File "${PAYLOAD_DIR}\document-runtime.zip"
    File "${PAYLOAD_DIR}\install.ps1"
    File "${PAYLOAD_DIR}\run-app.ps1"
    File "${PAYLOAD_DIR}\launch-app.ps1"
    File "${PAYLOAD_DIR}\run-session-supervisor.ps1"
    File "${PAYLOAD_DIR}\uninstall.ps1"
    File "${PAYLOAD_DIR}\installer-logging.ps1"
    File "${PAYLOAD_DIR}\INSTALL-WINDOWS.txt"
    File "${PAYLOAD_DIR}\THIRD-PARTY-NOTICES.txt"

    StrCpy $SetupPhase "install_service"
    StrCpy $SetupMessage "starting_powershell log=$SetupLog.powershell.log"
    Call PabSetupLog
    nsExec::ExecToLog '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\payload\install.ps1" ${SERVER_ARGUMENTS} -InstallRoot "$INSTDIR" -InstallerLogPath "$SetupLog"'
    Pop $0
    StrCpy $SetupMessage "powershell_exit=$0"
    Call PabSetupLog
    ${If} $0 != 0
        DetailPrint "$(InstallFailed) $0"
        IfSilent install_failed_silent
        MessageBox MB_ICONSTOP "$(InstallFailed) $0$\r$\n$(SetupLogLocation) $SetupLog"
    install_failed_silent:
        SetErrorLevel 1
        Abort
    ${EndIf}

    StrCpy $SetupPhase "create_shortcuts_and_uninstaller"
    StrCpy $SetupMessage "registering_installation"
    Call PabSetupLog
    SetOutPath "$INSTDIR"
    File "${PAYLOAD_DIR}\INSTALL-WINDOWS.txt"
    WriteUninstaller "$INSTDIR\Uninstall.exe"
    CreateDirectory "$SMPROGRAMS\Pixels Agent Bridge"
    CreateShortCut "$SMPROGRAMS\Pixels Agent Bridge\Pixels Agent Bridge.lnk" "$WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\run-app.ps1"' "$INSTDIR\pab-desktop.exe"
    CreateShortCut "$SMPROGRAMS\Pixels Agent Bridge\Uninstall.lnk" "$INSTDIR\Uninstall.exe"

    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "DisplayName" "Pixels Agent Bridge"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "DisplayVersion" "${APP_VERSION} ${BUILD_PROFILE}"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "Publisher" "Pixels"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "InstallLocation" "$INSTDIR"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "DisplayIcon" "$INSTDIR\pab-desktop.exe"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "NoModify" 1
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "NoRepair" 1

    StrCpy $SetupPhase "launch_desktop"
    nsExec::ExecToLog '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\launch-app.ps1" -ShortcutPath "$SMPROGRAMS\Pixels Agent Bridge\Pixels Agent Bridge.lnk"'
    Pop $0
    StrCpy $SetupMessage "desktop_launch_exit=$0"
    Call PabSetupLog
    ${If} $0 != 0
        DetailPrint "Could not launch the desktop window automatically. Open Pixels Agent Bridge from the Start menu."
    ${EndIf}
SectionEnd

Section "Uninstall"
    StrCpy $SetupPhase "remove_service"
    StrCpy $SetupMessage "starting_powershell log=$SetupLog.powershell.log"
    Call un.PabSetupLog
    nsExec::ExecToLog '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\uninstall.ps1" -InstallRoot "$INSTDIR" -InstallerLogPath "$SetupLog"'
    Pop $0
    StrCpy $SetupMessage "powershell_exit=$0"
    Call un.PabSetupLog
    ${If} $0 != 0
        DetailPrint "$(UninstallFailed) $0"
        IfSilent uninstall_failed_silent
        MessageBox MB_ICONSTOP "$(UninstallFailed) $0$\r$\n$(SetupLogLocation) $SetupLog"
    uninstall_failed_silent:
        SetErrorLevel 1
        Abort
    ${EndIf}

    StrCpy $SetupPhase "remove_shortcuts_and_uninstaller"
    StrCpy $SetupMessage "removing_installation_metadata"
    Call un.PabSetupLog
    Delete "$INSTDIR\INSTALL-WINDOWS.txt"
    Delete "$INSTDIR\launch-app.ps1"
    Delete "$INSTDIR\Uninstall.exe"
    RMDir "$INSTDIR"
    Delete "$SMPROGRAMS\Pixels Agent Bridge\Pixels Agent Bridge.lnk"
    Delete "$SMPROGRAMS\Pixels Agent Bridge\Uninstall.lnk"
    RMDir "$SMPROGRAMS\Pixels Agent Bridge"
    DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge"
SectionEnd
