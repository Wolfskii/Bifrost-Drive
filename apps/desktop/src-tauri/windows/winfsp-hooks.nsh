; Explorer transfer menu (platforms/windows/shell). The class id and handler keys below must
; match platforms/windows/shell/src/registration.rs; a unit test there keeps them in step.
!define BIFROST_SHELL_CLSID "{7A3C1F52-5B0E-4D7B-9E34-2C1A6F8B90D1}"
!define BIFROST_SHELL_NAME "Bifrost Drive Explorer menu"
!define BIFROST_SHELL_DLL "$INSTDIR\resources\bifrost_windows_shell.dll"

!macro BifrostShellHandler subkey
  WriteRegStr HKLM "Software\Classes\${subkey}" "" "${BIFROST_SHELL_CLSID}"
!macroend

!macro BifrostShellHandlerRemove subkey
  DeleteRegKey HKLM "Software\Classes\${subkey}"
!macroend

!macro BifrostShellRegister
  SetRegView 64
  WriteRegStr HKLM "Software\Bifrost Drive" "ExePath" "$INSTDIR\${MAINBINARYNAME}.exe"
  WriteRegStr HKLM "Software\Classes\CLSID\${BIFROST_SHELL_CLSID}" "" "${BIFROST_SHELL_NAME}"
  WriteRegStr HKLM "Software\Classes\CLSID\${BIFROST_SHELL_CLSID}\InprocServer32" "" "${BIFROST_SHELL_DLL}"
  WriteRegStr HKLM "Software\Classes\CLSID\${BIFROST_SHELL_CLSID}\InprocServer32" "ThreadingModel" "Apartment"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Shell Extensions\Approved" "${BIFROST_SHELL_CLSID}" "${BIFROST_SHELL_NAME}"
  !insertmacro BifrostShellHandler "Directory\shellex\DragDropHandlers\BifrostDrive"
  !insertmacro BifrostShellHandler "Directory\Background\shellex\DragDropHandlers\BifrostDrive"
  !insertmacro BifrostShellHandler "Drive\shellex\DragDropHandlers\BifrostDrive"
  !insertmacro BifrostShellHandler "Directory\shellex\ContextMenuHandlers\BifrostDrive"
  !insertmacro BifrostShellHandler "Directory\Background\shellex\ContextMenuHandlers\BifrostDrive"
  !insertmacro BifrostShellHandler "Drive\shellex\ContextMenuHandlers\BifrostDrive"
  !insertmacro BifrostShellHandler "*\shellex\ContextMenuHandlers\BifrostDrive"
  SetRegView lastused
!macroend

!macro BifrostShellUnregister
  SetRegView 64
  !insertmacro BifrostShellHandlerRemove "Directory\shellex\DragDropHandlers\BifrostDrive"
  !insertmacro BifrostShellHandlerRemove "Directory\Background\shellex\DragDropHandlers\BifrostDrive"
  !insertmacro BifrostShellHandlerRemove "Drive\shellex\DragDropHandlers\BifrostDrive"
  !insertmacro BifrostShellHandlerRemove "Directory\shellex\ContextMenuHandlers\BifrostDrive"
  !insertmacro BifrostShellHandlerRemove "Directory\Background\shellex\ContextMenuHandlers\BifrostDrive"
  !insertmacro BifrostShellHandlerRemove "Drive\shellex\ContextMenuHandlers\BifrostDrive"
  !insertmacro BifrostShellHandlerRemove "*\shellex\ContextMenuHandlers\BifrostDrive"
  DeleteRegKey HKLM "Software\Classes\CLSID\${BIFROST_SHELL_CLSID}"
  DeleteRegValue HKLM "Software\Microsoft\Windows\CurrentVersion\Shell Extensions\Approved" "${BIFROST_SHELL_CLSID}"
  DeleteRegKey HKLM "Software\Bifrost Drive"
  SetRegView lastused
!macroend

; Tells Explorer that handlers changed (SHCNE_ASSOCCHANGED).
!macro BifrostShellNotify
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Explorer keeps an installed extension loaded and locked. A loaded DLL can be renamed but not
  ; overwritten, so an update moves the old one aside; Windows removes it after a restart.
  IfFileExists "${BIFROST_SHELL_DLL}" 0 bifrost_shell_not_installed
    Delete "${BIFROST_SHELL_DLL}.old"
    Rename "${BIFROST_SHELL_DLL}" "${BIFROST_SHELL_DLL}.old"
  bifrost_shell_not_installed:
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\WinFsp" "InstallDir"
  ${If} $0 == ""
    DetailPrint "Installing WinFsp filesystem runtime..."
    CopyFiles "$INSTDIR\resources\winfsp-2.1.25156.msi" "$TEMP\winfsp-2.1.25156.msi"
    ExecWait 'msiexec.exe /i "$TEMP\winfsp-2.1.25156.msi" /qn /norestart' $1
    Delete "$TEMP\winfsp-2.1.25156.msi"

    ${If} $1 == 0
      DetailPrint "WinFsp installed successfully."
    ${ElseIf} $1 == 3010
      DetailPrint "WinFsp installed successfully; Windows requested a restart."
      SetRebootFlag true
    ${Else}
      MessageBox MB_ICONSTOP "Bifrost Drive could not install its WinFsp filesystem runtime (error $1). Drive mounting will be unavailable."
      Abort
    ${EndIf}
  ${Else}
    DetailPrint "WinFsp is already installed."
  ${EndIf}

  Delete "$INSTDIR\resources\winfsp-2.1.25156.msi"

  DetailPrint "Registering the Explorer transfer menu..."
  !insertmacro BifrostShellRegister
  Delete /REBOOTOK "${BIFROST_SHELL_DLL}.old"
  !insertmacro BifrostShellNotify

  SetAutoClose true
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    DetailPrint "Stopping Bifrost Drive before integration cleanup..."
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --quit-for-uninstall' $0
    Sleep 1500
    nsExec::ExecToLog '"$SYSDIR\taskkill.exe" /IM "${MAINBINARYNAME}.exe" /T /F'
    DetailPrint "Removing Bifrost Drive mounts and Explorer integrations..."
    ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --cleanup-windows-integrations' $0
    ${If} $0 <> 0
      DetailPrint "Some integrations were already absent or could not be removed (error $0)."
    ${EndIf}
    DetailPrint "Removing the Explorer transfer menu..."
    !insertmacro BifrostShellUnregister
    !insertmacro BifrostShellNotify
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Explorer may still hold the extension; delete it after the next restart if so.
  Delete /REBOOTOK "${BIFROST_SHELL_DLL}"
  Delete /REBOOTOK "${BIFROST_SHELL_DLL}.old"
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    DetailPrint "Removing Bifrost Drive local data..."
    RmDir /r "$PROFILE\Bifrost Drive"
    RmDir /r /REBOOTOK "$PROFILE\Bifrost Drive"
  ${EndIf}
!macroend
