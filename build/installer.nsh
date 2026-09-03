# Custom installer logic for WaSSH (electron-builder NSIS include).
# Automatically picked up from build/installer.nsh and injected into the generated installer.

!include nsDialogs.nsh

# Shortcuts are created by the custom page logic below, not by the default template.
!ifndef DO_NOT_CREATE_START_MENU_SHORTCUT
  !define DO_NOT_CREATE_START_MENU_SHORTCUT
!endif
!ifndef DO_NOT_CREATE_DESKTOP_SHORTCUT
  !define DO_NOT_CREATE_DESKTOP_SHORTCUT
!endif

# When an installation already exists, keep its install mode (user/all) instead of
# letting the user switch it during an update or reinstall.
!macro customInstallMode
  !ifndef BUILD_UNINSTALLER
    ${if} $hasPerUserInstallation == "1"
    ${orIf} $hasPerMachineInstallation == "1"
      ${if} $installMode == "all"
        StrCpy $isForceMachineInstall "1"
      ${else}
        StrCpy $isForceCurrentInstall "1"
      ${endIf}
    ${endIf}
  !endif
!macroend

# Ask which shortcuts to create. Defaults: desktop off, Start Menu on. Checkboxes
# are pre-checked for shortcuts that already exist (reinstall / manual upgrade).
!macro customPageAfterChangeDir
  Var createStartMenuShortcut
  Var createDesktopShortcut
  Var shortcutsDecided
  Var shortcutsPage
  Var shortcutsDesktopCheckbox
  Var shortcutsMenuCheckbox

  Function wasshShortcutsPageCreate
    !insertmacro MUI_HEADER_TEXT "Choose shortcuts" "Select the shortcuts to create"
    !insertmacro setLinkVars

    nsDialogs::Create 1018
    Pop $shortcutsPage

    ${NSD_CreateLabel} 0 0 100% 30u "Select where to place ${PRODUCT_NAME} shortcuts:"
    Pop $0
    ${NSD_CreateCheckBox} 0 40u 100% 14u "Add a desktop shortcut"
    Pop $shortcutsDesktopCheckbox
    ${NSD_CreateCheckBox} 0 60u 100% 14u "Add a Start Menu shortcut"
    Pop $shortcutsMenuCheckbox

    ${if} ${FileExists} "$newDesktopLink"
      SendMessage $shortcutsDesktopCheckbox ${BM_SETCHECK} ${BST_CHECKED} 0
    ${else}
      SendMessage $shortcutsDesktopCheckbox ${BM_SETCHECK} ${BST_UNCHECKED} 0
    ${endIf}

    SendMessage $shortcutsMenuCheckbox ${BM_SETCHECK} ${BST_CHECKED} 0

    nsDialogs::Show
  FunctionEnd

  Function wasshShortcutsPageLeave
    StrCpy $shortcutsDecided "1"
    ${NSD_GetState} $shortcutsDesktopCheckbox $createDesktopShortcut
    ${NSD_GetState} $shortcutsMenuCheckbox $createStartMenuShortcut
  FunctionEnd

  PageEx custom
    PageCallbacks wasshShortcutsPageCreate wasshShortcutsPageLeave
    Caption " "
  PageExEnd
!macroend

# Create exactly the shortcuts chosen on the page (defaults for unattended fresh
# installs). Existing installations are left untouched when this runs unattended.
!macro customInstall
  # Unattended installs never reach the shortcuts page: keep existing shortcuts for
  # updates, or apply the defaults (Start Menu only) on a fresh install.
  ${if} $shortcutsDecided != "1"
    !ifndef INSTALL_MODE_PER_ALL_USERS
      ${if} $hasPerUserInstallation != "0"
        Goto wasshShortcutsDone
      ${endIf}
      ${if} $hasPerMachineInstallation != "0"
        Goto wasshShortcutsDone
      ${endIf}
    !endif
    # Unattended fresh install: Start Menu shortcut only.
    StrCpy $createStartMenuShortcut "1"
    StrCpy $createDesktopShortcut "0"
  ${endIf}

  ${if} $createStartMenuShortcut != "0"
    !insertmacro createMenuDirectory
    CreateShortCut "$newStartMenuLink" "$appExe" "" "$appExe" 0 "" "" "${APP_DESCRIPTION}"
    ClearErrors
    WinShell::SetLnkAUMI "$newStartMenuLink" "${APP_ID}"
  ${endIf}

  ${if} $createDesktopShortcut != "0"
    CreateShortCut "$newDesktopLink" "$appExe" "" "$appExe" 0 "" "" "${APP_DESCRIPTION}"
    ClearErrors
    WinShell::SetLnkAUMI "$newDesktopLink" "${APP_ID}"
    System::Call 'Shell32::SHChangeNotify(i 0x8000000, i 0, i 0, i 0)'
  ${endIf}

  # "Run after install" should point at the app when no Start Menu shortcut exists.
  ${if} ${FileExists} "$newStartMenuLink"
    StrCpy $launchLink "$newStartMenuLink"
  ${else}
    StrCpy $launchLink "$appExe"
  ${endIf}

  wasshShortcutsDone:
!macroend
