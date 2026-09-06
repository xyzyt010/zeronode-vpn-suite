; Inno Setup script — ZeroNode VPN Client ONLY (no server).
; Build (needs Inno Setup 6):  iscc tools\installer-client-only.iss
; Reads staged files from dist\windows-client-only\  (see packaging notes).
#define MyAppName "ZeroNode VPN"
#define MyAppVersion "0.3.1"
#define MyAppPublisher "ZeroNode"
#define MyAppExeName "vpn-client.exe"

[Setup]
AppId={{3B9E7C2A-1F4D-4A6E-9C55-7E1A2B8D0F31}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
OutputDir=..\dist\windows-client-only
OutputBaseFilename=ZeroNode-VPN-Client-Setup-x64
SetupIconFile=..\dist\windows-client-only\icon.ico
Compression=lzma
SolidCompression=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
VersionInfoCompany={#MyAppPublisher}
VersionInfoDescription=ZeroNode VPN Client Setup
VersionInfoProductName={#MyAppName}
VersionInfoProductVersion={#MyAppVersion}
VersionInfoVersion={#MyAppVersion}
UninstallDisplayIcon={app}\icon.ico

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\dist\windows-client-only\bin\vpn-client.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "..\dist\windows-client-only\bin\wireguard.exe"; DestDir: "{app}\bin"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\dist\windows-client-only\bin\wg.exe"; DestDir: "{app}\bin"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\dist\windows-client-only\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\windows-client-only\README-CLIENT.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{group}\ZeroNode VPN Client"; Filename: "{app}\bin\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"
Name: "{group}\Uninstall ZeroNode VPN"; Filename: "{uninstallexe}"
Name: "{autodesktop}\ZeroNode VPN Client"; Filename: "{app}\bin\{#MyAppExeName}"; Tasks: desktopicon; IconFilename: "{app}\icon.ico"

[Registry]
; Always run as admin: tunnels (Wintun/routes) fail without it.
Root: HKLM; Subkey: "SOFTWARE\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Layers"; ValueType: string; ValueName: "{app}\bin\{#MyAppExeName}"; ValueData: "RUNASADMIN"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\bin\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent shellexec
