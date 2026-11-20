; RocketZip installer - Inno Setup 6 script
; Build with build_windows.bat, or after `cargo build --release`:
;   ISCC installer\rocketzip.iss

#define AppName "RocketZip"
; The GitHub workflow passes /DAppVersion=<version from Cargo.toml>.
#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif
#ifndef AppPublisher
  #define AppPublisher "RocketZip"
#endif
#ifndef AppURL
  #define AppURL "https://github.com/OWNER/rocketzip"
#endif
#define AppExe "RocketZip.exe"
#define CliExe "rocketzip-cli.exe"

[Setup]
AppId={{D94B3FC2-1ACB-4C77-B82F-54F64B0453BA}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
AppCopyright=Copyright (c) 2026 The RocketZip authors
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
LicenseFile=..\LICENSE
DefaultDirName={autopf}\RocketZip
DefaultGroupName=RocketZip
DisableProgramGroupPage=yes
OutputDir=Output
OutputBaseFilename=RocketZip-Setup-{#AppVersion}
SetupIconFile=..\rocketzip\assets\rocketzip.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName} {#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
; Admin install for all users by default; users may choose a per-user install.
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
ChangesAssociations=yes
ChangesEnvironment=yes
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "contextmenu"; Description: "Add RocketZip to the right-click menu (Add to archive, Extract here)"; GroupDescription: "Windows Explorer:"
Name: "assoc_zfx"; Description: "Open .zfx archives with RocketZip"; GroupDescription: "File types:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "addtopath"; Description: "Add the rocketzip-cli command to PATH"; GroupDescription: "Command line:"; Flags: unchecked

[Files]
; Two self-contained programs - no runtime or DLLs needed.
Source: "..\target\release\RocketZip.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\rocketzip-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\PRIVACY.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\RocketZip"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\RocketZip"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon
; Send to > RocketZip archive: handles many selected files in one archive
Name: "{usersendto}\RocketZip archive"; Filename: "{app}\{#AppExe}"; Parameters: "--add"; IconFilename: "{app}\{#AppExe}"; Tasks: contextmenu

[Registry]
; ---- .zfx: RocketZip's own format
Root: HKA; Subkey: "Software\Classes\.zfx"; ValueType: string; ValueName: ""; ValueData: "RocketZip.zfx"; Flags: uninsdeletevalue; Tasks: assoc_zfx
Root: HKA; Subkey: "Software\Classes\.zfx\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.zfx"; ValueData: ""; Flags: uninsdeletevalue; Tasks: assoc_zfx
Root: HKA; Subkey: "Software\Classes\RocketZip.zfx"; ValueType: string; ValueName: ""; ValueData: "RocketZip Ultra archive"; Flags: uninsdeletekey; Tasks: assoc_zfx
Root: HKA; Subkey: "Software\Classes\RocketZip.zfx\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: assoc_zfx
Root: HKA; Subkey: "Software\Classes\RocketZip.zfx\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assoc_zfx

; ---- other archive types: offered under 'Open with' (Windows lets the user pick the default)
Root: HKA; Subkey: "Software\Classes\RocketZip.archive"; ValueType: string; ValueName: ""; ValueData: "Archive (RocketZip)"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\RocketZip.archive\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\RocketZip.archive\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "RocketZip"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\.zip\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".zip"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.7z\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".7z"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.rar\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".rar"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.tar\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tar"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.gz\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".gz"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.tgz\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tgz"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.bz2\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".bz2"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.tbz2\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tbz2"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.xz\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".xz"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.txz\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".txz"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.zst\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".zst"; ValueData: ""
Root: HKA; Subkey: "Software\Classes\.tzst\OpenWithProgids"; ValueType: string; ValueName: "RocketZip.archive"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".tzst"; ValueData: ""

; ---- right-click: Add to archive (any file, any folder)
Root: HKA; Subkey: "Software\Classes\*\shell\RocketZip.add"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Add to RocketZip archive..."; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\RocketZip.add"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\*\shell\RocketZip.add\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --add ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\RocketZip.add"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Add to RocketZip archive..."; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\RocketZip.add"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\Directory\shell\RocketZip.add\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --add ""%1"""; Tasks: contextmenu

; ---- right-click on archives: Extract here / Extract to <name>\
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zfx\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zfx\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zfx\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zfx\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zfx\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zfx\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zip\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.7z\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.7z\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.7z\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.7z\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.7z\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.7z\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.rar\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tar\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tar\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tar\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tar\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tar\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tar\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.gz\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.gz\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.gz\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.gz\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.gz\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.gz\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tgz\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tgz\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tgz\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tgz\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tgz\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tgz\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.bz2\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.bz2\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.bz2\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.bz2\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.bz2\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.bz2\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tbz2\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tbz2\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tbz2\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tbz2\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tbz2\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tbz2\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.xz\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.xz\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.xz\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.xz\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.xz\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.xz\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.txz\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.txz\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.txz\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.txz\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.txz\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.txz\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zst\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zst\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zst\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zst\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zst\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.zst\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tzst\shell\RocketZip.extracthere"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract here (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tzst\shell\RocketZip.extracthere"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tzst\shell\RocketZip.extracthere\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-here ""%1"""; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tzst\shell\RocketZip.extractto"; ValueType: string; ValueName: "MUIVerb"; ValueData: "Extract to folder (RocketZip)"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tzst\shell\RocketZip.extractto"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\{#AppExe},0"; Tasks: contextmenu
Root: HKA; Subkey: "Software\Classes\SystemFileAssociations\.tzst\shell\RocketZip.extractto\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" --extract-to ""%1"""; Tasks: contextmenu

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,RocketZip}"; Flags: nowait postinstall skipifsilent

[Code]
// ---- add/remove the install folder on PATH (task "addtopath") -------------
function EnvRoot: Integer;
begin
  if IsAdminInstallMode then Result := HKEY_LOCAL_MACHINE else Result := HKEY_CURRENT_USER;
end;

function EnvKey: String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

procedure AddToPath(Dir: String);
var
  P: String;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', P) then P := '';
  if Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(P) + ';') > 0 then exit;
  if (P <> '') and (P[Length(P)] <> ';') then P := P + ';';
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', P + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  P: String;
  I: Integer;
begin
  if not RegQueryStringValue(EnvRoot, EnvKey, 'Path', P) then exit;
  P := ';' + P + ';';
  I := Pos(';' + Uppercase(Dir) + ';', Uppercase(P));
  if I = 0 then exit;
  Delete(P, I, Length(Dir) + 1);
  P := Copy(P, 2, Length(P) - 2);
  RegWriteExpandStringValue(EnvRoot, EnvKey, 'Path', P);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
