!macro NSIS_HOOK_POSTINSTALL
  ClearErrors
  CreateDirectory "$COMMONPROGRAMDATA\GolfReport"
  IfErrors 0 +2
    Abort "Cannot create Golf Report runtime directory in ProgramData."
  DetailPrint "Golf Report runtime directory: $COMMONPROGRAMDATA\GolfReport"
  nsExec::ExecToStack '"$SYSDIR\icacls.exe" "$COMMONPROGRAMDATA\GolfReport" /grant:r *S-1-5-32-545:(OI)(CI)M'
  Pop $0
  Pop $1
  StrCmp $0 "0" +2
    DetailPrint "Golf Report runtime ACL failed: icacls exit=$0; output=$1"
  StrCmp $0 "0" +2
    Abort "Cannot grant standard users access to Golf Report runtime directory (icacls exit=$0)."
!macroend
