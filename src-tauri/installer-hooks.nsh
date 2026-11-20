!macro NSIS_HOOK_POSTINSTALL
  ClearErrors
  CreateDirectory "$INSTDIR"
  IfErrors 0 +2
    Abort "Cannot access the application installation directory."
  ; Keep installed EXEs/resources read-only to Users. New config/runtime objects
  ; belong to their creator. Never grant Everyone or Modify over installed binaries.
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$INSTDIR" /grant:r "*S-1-5-32-545:(OI)(CI)(RX)" "*S-1-3-0:(OI)(CI)(IO)(F)"'
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
    DetailPrint "Application runtime ACL failed: icacls exit=$0; output=$1"
  StrCmp $0 "0" +2
    Abort "Cannot configure application installation permissions (icacls exit=$0)."
  ; WD/AD apply to this directory only: create config and runtime when needed.
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$INSTDIR" /grant "*S-1-5-32-545:(WD,AD)"'
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
    Abort "Cannot allow config/runtime creation in application directory (icacls exit=$0)."
!macroend
