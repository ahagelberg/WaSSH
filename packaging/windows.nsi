Unicode true
!include "MUI2.nsh"

Name "WaSSH"
OutFile "..\dist\WaSSH-${VERSION}-win-setup-x64.exe"
InstallDir "$LOCALAPPDATA\Programs\WaSSH Native"
InstallDirRegKey HKCU "Software\WaSSH Native" "InstallLocation"
RequestExecutionLevel user

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"
  File "..\target\release\wassh.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\WaSSH Native" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native" "DisplayName" "WaSSH Native"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native" "DisplayIcon" "$INSTDIR\wassh.exe"
  CreateShortcut "$SMPROGRAMS\WaSSH Native.lnk" "$INSTDIR\wassh.exe"
SectionEnd

Section "Uninstall"
  Delete "$SMPROGRAMS\WaSSH Native.lnk"
  Delete "$INSTDIR\wassh.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native"
  DeleteRegKey HKCU "Software\WaSSH Native"
SectionEnd