Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!ifndef VERSION
 !error "VERSION is required"
!endif
!ifndef STAGE
 !error "STAGE is required"
!endif
!ifndef OUTPUT
 !error "OUTPUT is required"
!endif
!ifndef REMOVE_FILES
 !error "REMOVE_FILES is required"
!endif
Name "Articulate"
OutFile "${OUTPUT}"
InstallDir "$LOCALAPPDATA\Programs\Articulate"
InstallDirRegKey HKCU "Software\Obiente\Articulate" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "Articulate by Obiente"
Icon "..\assets\brand\articulate.ico"
UninstallIcon "..\assets\brand\articulate.ico"
VIProductVersion "${VERSION}.0"
VIAddVersionKey /LANG=1033 "ProductName" "Articulate"
VIAddVersionKey /LANG=1033 "CompanyName" "Obiente"
VIAddVersionKey /LANG=1033 "FileDescription" "Articulate setup"
VIAddVersionKey /LANG=1033 "FileVersion" "${VERSION}"
VIAddVersionKey /LANG=1033 "LegalCopyright" "Copyright 2026 Articulate contributors"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Your words, ready to use."
!define MUI_WELCOMEPAGE_TEXT "Install Articulate for local dictation and call transcripts.$\r$\n$\r$\nSpeech models are downloaded inside the app after installation. Your recordings are never uploaded for transcription.$\r$\n$\r$\nClose Articulate before continuing an update. Your saved sessions and vocabulary will be kept."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\Articulate.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Open Articulate"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function .onInit
 ${IfNot} ${RunningX64}
  MessageBox MB_OK|MB_ICONSTOP "Articulate requires 64-bit Windows."
  Abort
 ${EndIf}
 ${IfNot} ${AtLeastWin10}
  MessageBox MB_OK|MB_ICONSTOP "Articulate requires Windows 10 or newer."
  Abort
 ${EndIf}
 ; Microsoft documents these two registry locations for Evergreen WebView2.
 ; HKLM uses the 32-bit view on x64 Windows; HKCU is the per-user install.
 SetRegView 32
 ReadRegStr $0 HKLM "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
 SetRegView 64
 ReadRegStr $1 HKCU "SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
 SetRegView 32
 ${If} $0 == "0.0.0.0"
  StrCpy $0 ""
 ${EndIf}
 ${If} $1 == "0.0.0.0"
  StrCpy $1 ""
 ${EndIf}
 ${If} $0 == ""
 ${AndIf} $1 == ""
  IfSilent webview_missing_silent
  MessageBox MB_OKCANCEL|MB_ICONINFORMATION "Articulate needs Microsoft Edge WebView2 Runtime.$\r$\n$\r$\nChoose OK to open Microsoft's download page. Install the Evergreen Runtime, then run Articulate setup again." IDCANCEL webview_missing_silent
  ExecShell "open" "https://developer.microsoft.com/microsoft-edge/webview2/#download-section"
  webview_missing_silent:
  SetErrorLevel 1603
  Abort
 ${EndIf}
FunctionEnd

Section "Articulate" SEC_APP
 SetShellVarContext current
 SetOutPath "$INSTDIR"
 ClearErrors
 File /r "${STAGE}\*"
 ${If} ${Errors}
  MessageBox MB_OK|MB_ICONSTOP "Some files could not be installed. Close Articulate and run setup again."
  Abort
 ${EndIf}
 WriteUninstaller "$INSTDIR\Uninstall.exe"
 CreateDirectory "$SMPROGRAMS\Articulate"
 CreateShortcut "$SMPROGRAMS\Articulate\Articulate.lnk" "$INSTDIR\Articulate.exe"
 WriteRegStr HKCU "Software\Obiente\Articulate" "InstallDir" "$INSTDIR"
 WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "DisplayName" "Articulate"
 WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "DisplayVersion" "${VERSION}"
 WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "Publisher" "Obiente"
 WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "DisplayIcon" "$INSTDIR\Articulate.exe"
 WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "InstallLocation" "$INSTDIR"
 WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "URLInfoAbout" "https://github.com/Obiente/articulate"
 WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "UninstallString" '"$INSTDIR\Uninstall.exe"'
 WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
 WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "NoModify" 1
 WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate" "NoRepair" 1
SectionEnd

Section "Uninstall"
 SetShellVarContext current
 !include "${REMOVE_FILES}"
 Delete "$INSTDIR\Uninstall.exe"
 RMDir "$INSTDIR"
 Delete "$SMPROGRAMS\Articulate\Articulate.lnk"
 RMDir "$SMPROGRAMS\Articulate"
 DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\Articulate"
 DeleteRegKey HKCU "Software\Obiente\Articulate"
 ; Models, vocabulary and history remain in LOCALAPPDATA\TranscribeLocal.
SectionEnd
