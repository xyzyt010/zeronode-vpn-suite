; Inno Setup Compiler Script for ZeroNode VPN Suite
#define MyAppName "ZeroNode VPN"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "ZeroNode"
#define MyAppExeName "vpn-client.exe"
#define MyServerExeName "vpn-server.exe"
#define MyServerGuiExeName "vpn-server-gui.exe"
#define MyCliExeName "vpnctl.exe"

[Setup]
AppId={{C6C26A8F-09D7-4B2E-A43A-BD5972BE10CD}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
AllowNoIcons=yes
OutputDir=..\target
OutputBaseFilename=ZeroNodeVPN_Setup_x64
SetupIconFile=..\dist\windows\icon.ico
Compression=lzma
SolidCompression=yes
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
VersionInfoCompany={#MyAppPublisher}
VersionInfoDescription=ZeroNode VPN Setup
VersionInfoProductName={#MyAppName}
VersionInfoProductVersion={#MyAppVersion}
VersionInfoVersion={#MyAppVersion}
UninstallDisplayIcon={app}\icon.ico

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Types]
Name: "custom"; Description: "Custom installation"; Flags: iscustom

[Components]
Name: "client"; Description: "Install VPN Client (to connect to other servers)"; Types: custom; Flags: fixed
Name: "server"; Description: "Install VPN Server (to host a VPN on this machine)"; Types: custom

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Components: client; Flags: unchecked

[Files]
Source: "..\dist\windows\bin\vpn-client.exe"; DestDir: "{app}\bin"; Components: client; Flags: ignoreversion
Source: "..\dist\windows\bin\vpnctl.exe"; DestDir: "{app}\bin"; Components: client; Flags: ignoreversion
Source: "..\dist\windows\bin\vpn-server.exe"; DestDir: "{app}\bin"; Components: server; Flags: ignoreversion
Source: "..\dist\windows\bin\vpn-server-gui.exe"; DestDir: "{app}\bin"; Components: server; Flags: ignoreversion
Source: "..\dist\windows\bin\wireguard.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "..\dist\windows\bin\wg.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "..\dist\windows\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\windows\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\ZeroNode VPN Client"; Filename: "{app}\bin\{#MyAppExeName}"; Components: client; IconFilename: "{app}\icon.ico"
Name: "{group}\ZeroNode VPN CLI"; Filename: "{app}\bin\{#MyCliExeName}"; Components: client; IconFilename: "{app}\icon.ico"
Name: "{group}\ZeroNode VPN Server"; Filename: "{app}\bin\{#MyServerGuiExeName}"; Components: server; IconFilename: "{app}\icon.ico"
Name: "{group}\ZeroNode VPN Server Dashboard"; Filename: "{app}\bin\{#MyServerGuiExeName}"; Components: server; IconFilename: "{app}\icon.ico"
Name: "{group}\Uninstall ZeroNode VPN"; Filename: "{uninstallexe}"
Name: "{autodesktop}\ZeroNode VPN Client"; Filename: "{app}\bin\{#MyAppExeName}"; Tasks: desktopicon; Components: client; IconFilename: "{app}\icon.ico"

[Run]
Filename: "{app}\bin\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; Components: client; Flags: nowait postinstall skipifsilent shellexec
