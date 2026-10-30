; Inno Setup script of the Atomik Rack installer. Built by scripts/release/package-windows.sh,
; which passes the values below with iscc /D...:
;   AppVersion         the display version (0.1.0, 0.1.0-rc.1, 0.1.0-nightly.20261024+abc1234)
;   AppVersionNumeric  four numeric parts for the file version resource (0.1.0.0)
;   SourceDir          the folder with the five programs (signed when signing is configured)
;   RepoDir            the repository root (LICENSE, README.md, CHANGELOG.md)
;   OutputDir          where the installer is written
;   OutputName         the installer file name without ".exe"

#ifndef AppVersion
  #error AppVersion is not defined (iscc /DAppVersion=...)
#endif
#ifndef AppVersionNumeric
  #error AppVersionNumeric is not defined (iscc /DAppVersionNumeric=...)
#endif
#ifndef SourceDir
  #error SourceDir is not defined (iscc /DSourceDir=...)
#endif
#ifndef RepoDir
  #error RepoDir is not defined (iscc /DRepoDir=...)
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
#ifndef OutputName
  #define OutputName "atomik-rack-setup"
#endif

[Setup]
; Never change the AppId: it is how a newer installer finds and upgrades an older install.
AppId={{4EA50DA5-13E0-4797-A4BE-41074BCB5BBE}
AppName=Atomik Rack
AppVersion={#AppVersion}
AppVerName=Atomik Rack {#AppVersion}
AppPublisher=Atomik Factory
AppPublisherURL=https://github.com/atomikfactory/atomik-rack
AppSupportURL=https://github.com/atomikfactory/atomik-rack/issues
AppUpdatesURL=https://github.com/atomikfactory/atomik-rack/releases
VersionInfoVersion={#AppVersionNumeric}
VersionInfoProductName=Atomik Rack
VersionInfoProductVersion={#AppVersionNumeric}
VersionInfoDescription=Atomik Rack installer
VersionInfoCompany=Atomik Factory
; Per machine (Program Files) by default; the first page offers an install for the current user only.
DefaultDirName={autopf}\Atomik Rack
DisableProgramGroupPage=yes
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog
; 64-bit Windows 10 or later only.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile={#RepoDir}\LICENSE
; The logo of the installer and uninstaller programs themselves (atomik.exe carries it too).
SetupIconFile={#RepoDir}\assets\icon\atomik-rack.ico
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; atomik.exe has the logo embedded (crates/app/build.rs): the Apps list shows it.
UninstallDisplayIcon={app}\atomik.exe
UninstallDisplayName=Atomik Rack
ChangesAssociations=yes
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "associate"; Description: "Open .atomik project files with Atomik Rack"; GroupDescription: "File associations:"

[Files]
Source: "{#SourceDir}\atomik.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\atomik-render.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\atomik-play.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\atomik-plugin-scan.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\atomik-plugin-host.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoDir}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Atomik Rack"; Filename: "{app}\atomik.exe"; IconFilename: "{app}\atomik.exe"
Name: "{autodesktop}\Atomik Rack"; Filename: "{app}\atomik.exe"; IconFilename: "{app}\atomik.exe"; Tasks: desktopicon

[Registry]
; HKA is HKLM for a per-machine install and HKCU for a per-user one.
Root: HKA; Subkey: "Software\Classes\.atomik"; ValueType: string; ValueName: ""; ValueData: "AtomikRack.Project"; Flags: uninsdeletevalue; Tasks: associate
Root: HKA; Subkey: "Software\Classes\AtomikRack.Project"; ValueType: string; ValueName: ""; ValueData: "Atomik Rack project"; Flags: uninsdeletekey; Tasks: associate
Root: HKA; Subkey: "Software\Classes\AtomikRack.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\atomik.exe,0"; Tasks: associate
Root: HKA; Subkey: "Software\Classes\AtomikRack.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\atomik.exe"" ""%1"""; Tasks: associate

[Run]
Filename: "{app}\atomik.exe"; Description: "{cm:LaunchProgram,Atomik Rack}"; Flags: nowait postinstall skipifsilent
