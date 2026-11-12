Unicode true
!include "MUI2.nsh"

Name "WaSSH"
OutFile "..\dist\WaSSH-${VERSION}-win-setup-x64.exe"
; One copy for every account on the machine, which is what Program Files and the
; machine-wide registry entries need: the installer asks for elevation first.
InstallDir "$PROGRAMFILES64\WaSSH Native"
InstallDirRegKey HKLM "Software\WaSSH Native" "InstallLocation"
RequestExecutionLevel admin

!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  ; Up to 2.0.7 the app installed into the user's profile; clear that copy, its shortcut
  ; and its uninstall entry, so the machine is left with the one install below.
  SetShellVarContext current
  Delete "$SMPROGRAMS\WaSSH Native.lnk"
  Delete "$LOCALAPPDATA\Programs\WaSSH Native\wassh.exe"
  Delete "$LOCALAPPDATA\Programs\WaSSH Native\Uninstall.exe"
  RMDir "$LOCALAPPDATA\Programs\WaSSH Native"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native"
  DeleteRegKey HKCU "Software\WaSSH Native"
  ; Shortcuts in the all-users Start menu, beside the machine-wide registry entries.
  SetShellVarContext all
  SetOutPath "$INSTDIR"
  File "..\target\release\wassh.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKLM "Software\WaSSH Native" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native" "DisplayName" "WaSSH Native"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native" "DisplayIcon" "$INSTDIR\wassh.exe"
  CreateShortcut "$SMPROGRAMS\WaSSH Native.lnk" "$INSTDIR\wassh.exe"
SectionEnd

Section "Uninstall"
  SetShellVarContext all
  Delete "$SMPROGRAMS\WaSSH Native.lnk"
  Delete "$INSTDIR\wassh.exe"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native"
  DeleteRegKey HKLM "Software\WaSSH Native"
SectionEnd