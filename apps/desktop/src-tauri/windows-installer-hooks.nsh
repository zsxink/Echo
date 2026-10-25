; Echo Windows installer hooks (tauri.conf.json > bundle > windows > nsis >
; installerHooks). The main Tauri NSIS template does not include WinVer.nsh,
; so pull it in here; NSIS headers are guarded against re-inclusion.
!include "WinVer.nsh"

; PREINSTALL runs after the wizard pages and before any file is copied.
; WebView2 — and therefore Echo — only supports Windows 10+, so gate here
; with a bilingual message instead of letting the install fail vaguely.
; Under silent installs (/S) Abort simply exits with a non-zero code.
!macro NSIS_HOOK_PREINSTALL
  ${IfNot} ${AtLeastWin10}
    MessageBox MB_OK|MB_ICONSTOP "Echo 需要 Windows 10 或更高版本。$\r$\nEcho requires Windows 10 or later."
    Abort
  ${EndIf}
!macroend
