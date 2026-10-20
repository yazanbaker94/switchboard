; Remove only the URL route that still points to this exact installation.
; Keep the registration intact during upgrades and keep WhatsApp sessions.
!macro NSIS_HOOK_POSTUNINSTALL
  Push $0
  Push $1
  Push $2
  Push $3
  Push $R7

  ${If} $UpdateMode <> 1
    ClearErrors
    ReadRegStr $R7 HKCU "Software\Classes\CLSID\{FA77BC6A-2B23-41D2-A383-88CE46CCAF91}\LocalServer32" ""
    ${IfNot} ${Errors}
    ${AndIf} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" --toast-activated"
      DeleteRegKey HKCU "Software\Classes\CLSID\{FA77BC6A-2B23-41D2-A383-88CE46CCAF91}\LocalServer32"
      DeleteRegKey /ifempty HKCU "Software\Classes\CLSID\{FA77BC6A-2B23-41D2-A383-88CE46CCAF91}"
    ${EndIf}
    ClearErrors
    ReadRegStr $R7 HKCU "Software\Classes\switchboard-wa\shell\open\command" ""
    ${IfNot} ${Errors}
    ${AndIf} $R7 == "$\"$INSTDIR\${MAINBINARYNAME}.exe$\" $\"%1$\""
      DeleteRegKey HKCU "Software\Classes\switchboard-wa"
      DeleteRegKey HKCU "Software\Classes\AppUserModelId\com.switchboard.desktop"
    ${EndIf}

    !insertmacro IsShortcutTarget "$SMPROGRAMS\Switchboard Workspace.lnk" "$INSTDIR\${MAINBINARYNAME}.exe"
    Pop $R7
    ${If} $R7 = 1
      !insertmacro UnpinShortcut "$SMPROGRAMS\Switchboard Workspace.lnk"
      Delete "$SMPROGRAMS\Switchboard Workspace.lnk"
    ${EndIf}
  ${EndIf}

  Pop $R7
  Pop $3
  Pop $2
  Pop $1
  Pop $0
!macroend
