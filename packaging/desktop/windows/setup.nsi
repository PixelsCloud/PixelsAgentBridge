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
!ifndef DEPLOYMENT_ID
    !error "DEPLOYMENT_ID is required"
!endif
!ifndef CONTROL_URL
    !error "CONTROL_URL is required"
!endif
!ifndef RELAY_URL
    !error "RELAY_URL is required"
!endif
!ifndef APP_ICON
    !error "APP_ICON is required"
!endif
!ifndef BUILD_PROFILE
    !error "BUILD_PROFILE is required"
!endif

Name "Pixels Agent Bridge"
OutFile "${OUTPUT_FILE}"
InstallDir "$PROGRAMFILES64\PixelsAgentBridge"
InstallDirRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "InstallLocation"
RequestExecutionLevel admin
ShowInstDetails show
ShowUninstDetails show
SetCompressor /SOLID lzma
Icon "${APP_ICON}"
UninstallIcon "${APP_ICON}"
VIProductVersion "0.1.0.0"
VIAddVersionKey /LANG=1033 "ProductName" "Pixels Agent Bridge"
VIAddVersionKey /LANG=1033 "ProductVersion" "0.1.0"
VIAddVersionKey /LANG=1033 "CompanyName" "Pixels"
VIAddVersionKey /LANG=1033 "FileVersion" "0.1.0"
VIAddVersionKey /LANG=1033 "FileDescription" "Pixels Agent Bridge ${BUILD_PROFILE} Installer"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Copyright (C) 2026 Pixels"

!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

LangString SasPolicyPrompt ${LANG_SIMPCHINESE} "远程 Ctrl+Alt+Delete 需要允许 Windows 服务发送安全注意序列。安装程序将把 SoftwareSASGeneration 设为 1。继续吗？"
LangString SasPolicyPrompt ${LANG_ENGLISH} "Remote Ctrl+Alt+Delete requires Windows services to send the secure attention sequence. Setup will set SoftwareSASGeneration to 1. Continue?"
LangString InstallFailed ${LANG_SIMPCHINESE} "安装服务失败。请查看上方安装日志。退出代码："
LangString InstallFailed ${LANG_ENGLISH} "Service installation failed. Check the installation log above. Exit code:"
LangString UninstallFailed ${LANG_SIMPCHINESE} "卸载服务失败。请查看上方卸载日志。退出代码："
LangString UninstallFailed ${LANG_ENGLISH} "Service removal failed. Check the uninstallation log above. Exit code:"

Function .onInit
    ${IfNot} ${RunningX64}
        MessageBox MB_ICONSTOP "This package requires 64-bit Windows."
        Abort
    ${EndIf}
    SetRegView 64
    SetShellVarContext all
FunctionEnd

Function un.onInit
    SetRegView 64
    SetShellVarContext all
FunctionEnd

Section "Install"
    ReadRegDWORD $0 HKLM "Software\Microsoft\Windows\CurrentVersion\Policies\System" "SoftwareSASGeneration"
    ${If} $0 != 1
    ${AndIf} $0 != 3
        MessageBox MB_ICONQUESTION|MB_YESNO "$(SasPolicyPrompt)" IDYES allow_service_sas
        Abort
    allow_service_sas:
        WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Policies\System" "SoftwareSASGeneration" 1
    ${EndIf}

    InitPluginsDir
    SetOutPath "$PLUGINSDIR\payload"
    File "${PAYLOAD_DIR}\pab-desktop.exe"
    File "${PAYLOAD_DIR}\pab-executor.exe"
    File "${PAYLOAD_DIR}\pab-mcp.exe"
    File "${PAYLOAD_DIR}\install.ps1"
    File "${PAYLOAD_DIR}\run-app.ps1"
    File "${PAYLOAD_DIR}\launch-app.ps1"
    File "${PAYLOAD_DIR}\run-session-supervisor.ps1"
    File "${PAYLOAD_DIR}\uninstall.ps1"
    File "${PAYLOAD_DIR}\INSTALL-WINDOWS.txt"

    nsExec::ExecToLog '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\payload\install.ps1" -DeploymentId "${DEPLOYMENT_ID}" -ControlUrl "${CONTROL_URL}" -RelayUrl "${RELAY_URL}" -InstallRoot "$INSTDIR"'
    Pop $0
    ${If} $0 != 0
        MessageBox MB_ICONSTOP "$(InstallFailed) $0"
        Abort
    ${EndIf}

    SetOutPath "$INSTDIR"
    File "${PAYLOAD_DIR}\INSTALL-WINDOWS.txt"
    WriteUninstaller "$INSTDIR\Uninstall.exe"
    CreateDirectory "$SMPROGRAMS\Pixels Agent Bridge"
    CreateShortCut "$SMPROGRAMS\Pixels Agent Bridge\Pixels Agent Bridge.lnk" "$WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe" '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "$INSTDIR\run-app.ps1"' "$INSTDIR\pab-desktop.exe"
    CreateShortCut "$SMPROGRAMS\Pixels Agent Bridge\Uninstall.lnk" "$INSTDIR\Uninstall.exe"

    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "DisplayName" "Pixels Agent Bridge"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "DisplayVersion" "0.1.0 ${BUILD_PROFILE}"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "Publisher" "Pixels"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "InstallLocation" "$INSTDIR"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "DisplayIcon" "$INSTDIR\pab-desktop.exe"
    WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "UninstallString" '"$INSTDIR\Uninstall.exe"'
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "NoModify" 1
    WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge" "NoRepair" 1

    nsExec::ExecToLog '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\launch-app.ps1" -ShortcutPath "$SMPROGRAMS\Pixels Agent Bridge\Pixels Agent Bridge.lnk"'
    Pop $0
    ${If} $0 != 0
        DetailPrint "Could not launch the desktop window automatically. Open Pixels Agent Bridge from the Start menu."
    ${EndIf}
SectionEnd

Section "Uninstall"
    nsExec::ExecToLog '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$INSTDIR\uninstall.ps1" -InstallRoot "$INSTDIR"'
    Pop $0
    ${If} $0 != 0
        MessageBox MB_ICONSTOP "$(UninstallFailed) $0"
        Abort
    ${EndIf}

    Delete "$INSTDIR\INSTALL-WINDOWS.txt"
    Delete "$INSTDIR\launch-app.ps1"
    Delete "$INSTDIR\Uninstall.exe"
    RMDir "$INSTDIR"
    Delete "$SMPROGRAMS\Pixels Agent Bridge\Pixels Agent Bridge.lnk"
    Delete "$SMPROGRAMS\Pixels Agent Bridge\Uninstall.lnk"
    RMDir "$SMPROGRAMS\Pixels Agent Bridge"
    DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\PixelsAgentBridge"
SectionEnd
