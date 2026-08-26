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
  ${EndIf}
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    DetailPrint "Removing Bifrost Drive local data..."
    RmDir /r "$PROFILE\Bifrost Drive"
    RmDir /r /REBOOTOK "$PROFILE\Bifrost Drive"
  ${EndIf}
!macroend