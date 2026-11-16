Unicode true
!include "MUI2.nsh"

Name "WaSSH"
OutFile "..\dist\WaSSH-${VERSION}-win-setup-x64.exe"
; One copy for every account on the machine, which is what Program Files and the
; machine-wide registry entries need: the installer asks for elevation first.
InstallDir "$PROGRAMFILES64\WaSSH"
InstallDirRegKey HKLM "Software\WaSSH" "InstallLocation"
RequestExecutionLevel admin

!define APP_NAME "WaSSH"
!define APP_EXE "wassh.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH"
!define PUBLISHER "Andreas Hagelberg"

!define MUI_ICON "..\res\WaSSH.ico"
!define MUI_UNICON "..\res\WaSSH.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "WaSSH" SecApp
  ; Earlier builds called the install "WaSSH Native": one copy in the user's profile up to
  ; 2.0.7, and one in Program Files from 2.0.8. Both are cleared, with their shortcuts,
  ; registry keys and uninstall entries, so the machine ends up with the one build below.
  SetShellVarContext current
  Delete "$SMPROGRAMS\WaSSH Native.lnk"
  Delete "$LOCALAPPDATA\Programs\WaSSH Native\wassh.exe"
  Delete "$LOCALAPPDATA\Programs\WaSSH Native\Uninstall.exe"
  RMDir "$LOCALAPPDATA\Programs\WaSSH Native"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native"
  DeleteRegKey HKCU "Software\WaSSH Native"
  SetShellVarContext all
  Delete "$SMPROGRAMS\WaSSH Native.lnk"
  Delete "$PROGRAMFILES64\WaSSH Native\wassh.exe"
  Delete "$PROGRAMFILES64\WaSSH Native\Uninstall.exe"
  RMDir "$PROGRAMFILES64\WaSSH Native"
  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\WaSSH Native"
  DeleteRegKey HKLM "Software\WaSSH Native"

  SetOutPath "$INSTDIR"
  File "..\target\release\${APP_EXE}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"

  WriteRegStr HKLM "Software\WaSSH" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKLM "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKLM "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTALL_KEY}" "NoRepair" 1
SectionEnd

Section "Start Menu shortcut" SecStartMenu
  SetShellVarContext all
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortcut "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk" "$INSTDIR\Uninstall.exe"
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  SetShellVarContext all
  CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecApp} "The program itself, in Program Files."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecStartMenu} "A ${APP_NAME} entry and an uninstaller entry in the Start Menu."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A shortcut to ${APP_NAME} on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  SetShellVarContext all
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\Uninstall ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  DeleteRegKey HKLM "${UNINSTALL_KEY}"
  DeleteRegKey HKLM "Software\WaSSH"
SectionEnd