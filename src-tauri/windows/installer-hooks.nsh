!macro NSIS_HOOK_POSTINSTALL
  SetRegView 64
  WriteRegStr HKLM "Software\Classes\CLSID\{D73E9C6C-648A-41F1-8EF8-D4B479212BE4}" "" "CameraTuner Virtual Camera Media Source"
  WriteRegStr HKLM "Software\Classes\CLSID\{D73E9C6C-648A-41F1-8EF8-D4B479212BE4}\InProcServer32" "" "$INSTDIR\binaries\camera-tuner-media-source.dll"
  WriteRegStr HKLM "Software\Classes\CLSID\{D73E9C6C-648A-41F1-8EF8-D4B479212BE4}\InProcServer32" "ThreadingModel" "Both"

  CreateDirectory "$COMMONAPPDATA\CameraTuner"
  nsExec::ExecToLog '"$SYSDIR\icacls.exe" "$COMMONAPPDATA\CameraTuner" /grant *S-1-5-32-545:(OI)(CI)M /C'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$INSTDIR\binaries\camera-tuner-virtual-camera-x86_64-pc-windows-msvc.exe" remove'
  SetRegView 64
  DeleteRegKey HKLM "Software\Classes\CLSID\{D73E9C6C-648A-41F1-8EF8-D4B479212BE4}"
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  Delete "$COMMONAPPDATA\CameraTuner\frame-v1.bin"
  Delete "$COMMONAPPDATA\CameraTuner\frame-v2.bin"
  Delete "$COMMONAPPDATA\CameraTuner\frame-v3.bin"
  RMDir "$COMMONAPPDATA\CameraTuner"
!macroend
