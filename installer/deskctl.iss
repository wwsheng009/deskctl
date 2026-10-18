[Setup]
AppId={{9C65F75E-D1BD-4C06-A391-44A65C51F2C2}}
AppName=Deskctl
AppVersion=0.1.0
DefaultDirName={autopf}\Deskctl
DefaultGroupName=Deskctl
OutputDir=..\dist
OutputBaseFilename=deskctl-windows-x64-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64
ArchitecturesInstallIn64BitMode=x64
PrivilegesRequired=lowest
UninstallDisplayIcon={app}\deskctl.exe

[Files]
Source: "..\target\release\deskctl.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\models\*"; DestDir: "{app}\models"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\skills\*"; DestDir: "{app}\skills"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\contracts\*"; DestDir: "{app}\contracts"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Deskctl"; Filename: "{app}\deskctl.exe"

[Run]
Filename: "{app}\deskctl.exe"; Parameters: "doctor"; Description: "Run Deskctl doctor"; Flags: postinstall nowait skipifsilent

