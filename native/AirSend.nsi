Unicode true
!include "MUI2.nsh"
!include "x64.nsh"

Name "AirSend ${VERSION}"
OutFile "${SETUP_PATH}"
InstallDir "$LOCALAPPDATA\Programs\AirSend WinUI"
RequestExecutionLevel user
SetCompressor /SOLID lzma
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "AirSend"
VIAddVersionKey "FileDescription" "AirSend Setup"
VIAddVersionKey "FileVersion" "${VERSION}"

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\AirSend.WinUI"
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "Spanish"

Function .onInit
  ${IfNot} ${RunningX64}
    Abort
  ${EndIf}
  SetShellVarContext current
FunctionEnd

Section "AirSend"
  ClearErrors
  SetOutPath "$INSTDIR"
  File /r "${APP_DIR}\*"
  IfErrors 0 +3
    SetErrorLevel 1
    Abort "Close AirSend from the system tray and try again."
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  CreateShortcut "$SMPROGRAMS\AirSend.lnk" "$INSTDIR\AirSend.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "AirSend"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\AirSend.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  !include "${UNINSTALL_FILES}"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\AirSend.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
SectionEnd
