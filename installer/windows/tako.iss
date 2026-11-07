; tako — Windows インストーラー（Inno Setup 6）
;
; 設計メモ:
;   - per-user インストール（{localappdata}\Programs\tako）。管理者権限を要求しない。
;     テスターに「右クリック → 管理者として実行」を強いないことを優先した。
;   - 配置するのは tako-app.exe（GUI）と tako.exe（CLI）の 2 本。
;     この 2 本は必ず同一ディレクトリに置く。dispatch.rs の resolve_tako_binary() が
;     current_exe の兄弟として tako.exe を解決するため、離すと AI 経由の CLI 操作が壊れる。
;   - ユーザー PATH（HKCU\Environment）へ {app} を追加する。tako 自身は PATH を触らないので、
;     ペイン内で `tako` を叩く「AI フルコントロール」経路はこの追加に依存している。
;     HKLM は一切触らない。
;   - アプリ内更新（#2434）は tako がこのインストーラーを無言で起こす:
;       /SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS /SP- /LOG="..."
;       /TAKOWAITPID=<抜ける tako の pid> /TAKORELAUNCH="<起動し直す tako-app.exe>"
;     後ろ 2 つは tako 専用の指定で、[Code] の InitializeSetup / DeinitializeSetup が読む
;     （tako が抜けるのを待ってから上書きし、終わり際に tako を起動し直す）。
;     指定が無い起動（手動・CI の無人インストール）では何もしない。
;     正本は crates/tako-app/src/platform/installer_update.rs
;   - バージョンはこのファイルに埋め込まず、ISCC の /DAppVersion=... で注入する
;     （build-installer.ps1 が Cargo.toml から読んで渡す）。
;   - 出力ファイル名も /DAssetBaseName=... で注入する。命名規則の正は
;     crates/tako-core/src/platform/release_assets.rs（#594 / #595）。ここに名前を
;     書き下すと更新チェック側（update_checker）と食い違って「自 OS 向けアセットが
;     無いのに更新通知が出る / 掴めない」事故になる。
;
; ビルド（**build-installer.ps1 経由が正**。アセット名を命名規則から組んで渡してくれる）:
;   pwsh -File installer/windows/build-installer.ps1
;   ISCC を直に叩くなら名前も明示する:
;   ISCC.exe /DAppVersion=0.5.12 /DAssetBaseName=tako-v0.5.12-windows-x86_64 installer\windows\tako.iss

#define AppName "tako"
#define AppPublisher "tako project"
#define AppURL "https://github.com/takushio2525/tako"
#define AppExeName "tako-app.exe"
#define CliExeName "tako.exe"
#define IconName "tako.ico"

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif

; 配布アセット名（拡張子なし）。命名規則の正は
; crates/tako-core/src/platform/release_assets.rs で、PowerShell 側の写しである
; installer/windows/lib/release-assets.ps1 の Get-TakoAssetBaseName が組み立てて
; build-installer.ps1 が /DAssetBaseName=... で渡す。
; **フォールバックは意図的に持たない**。ここで名前を組み直すと命名規則の 3 つ目の実装に
; なり、同期テストの網にかからないまま「リリース側と `tako update` で食い違う」（#595）が
; 再発しうる。定義が無ければ黙って別名を出すより落ちる方が安全。
#ifndef AssetBaseName
  #error AssetBaseName undefined. Build via installer/windows/build-installer.ps1, or pass the name from Get-TakoAssetBaseName (installer/windows/lib/release-assets.ps1) e.g. /DAssetBaseName=tako-v0.7.0-windows-x86_64
#endif

; リポジトリ相対のパス。.iss からの相対で解決される
#ifndef RepoRoot
  #define RepoRoot "..\.."
#endif
#ifndef BinDir
  #define BinDir RepoRoot + "\target\release"
#endif
#ifndef OutDir
  #define OutDir RepoRoot + "\dist\windows"
#endif

; VersionInfoVersion は数値 x.y.z[.w] しか受け付けないので、タグ形式（v0.6.0 /
; 0.6.0-rc1）から先頭の "v" と "-" 以降を落とした数値部分を作る
#define NumericVersion AppVersion
#if Copy(NumericVersion, 1, 1) == "v"
  #define NumericVersion Copy(NumericVersion, 2)
#endif
#if Pos("-", NumericVersion) > 0
  #define NumericVersion Copy(NumericVersion, 1, Pos("-", NumericVersion) - 1)
#endif

[Setup]
; AppId は一度決めたら変えない（変えると別アプリ扱いになり上書き更新できなくなる）
AppId={{95CA86D9-AAFB-4ABC-8D9B-2C66F75CF739}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
VersionInfoVersion={#NumericVersion}
VersionInfoProductName={#AppName}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}/issues
AppUpdatesURL={#AppURL}/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
AllowNoIcons=yes
LicenseFile={#RepoRoot}\LICENSE
OutputDir={#OutDir}
OutputBaseFilename={#AssetBaseName}
SetupIconFile={#RepoRoot}\assets\icon\{#IconName}
; tako-app.exe にはまだアイコンリソースが埋まっていないので、同梱した .ico を使う
UninstallDisplayIcon={app}\{#IconName}
UninstallDisplayName={#AppName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ShowLanguageDialog=auto

; 管理者権限を要求しない。{group} / {autodesktop} もユーザー配下に解決される
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763

; PATH を書き換えるので WM_SETTINGCHANGE のブロードキャストを Inno に任せる
; （インストーラー・アンインストーラーの両方で飛ぶ）
ChangesEnvironment=yes

; 実行中の tako を Restart Manager で検出して閉じる（閉じないと上書きコピーに失敗する）。
; 無言のとき（アプリ内更新）は常に閉じる。Restart Manager による再起動はしない
; （アプリ内更新の起動し直しは [Code] の DeinitializeSetup が受け持つ。#2434）
CloseApplications=yes
CloseApplicationsFilter=*.exe
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[CustomMessages]
english.PathGroup=Command line:
english.AddToPathTask=Add tako to my PATH (recommended)
english.PathWriteFailed=Failed to update the PATH environment variable. You can add "%1" to your PATH manually.
japanese.PathGroup=コマンドライン:
japanese.AddToPathTask=tako を PATH に追加する（推奨）
japanese.PathWriteFailed=PATH 環境変数の更新に失敗しました。"%1" を手動で PATH へ追加してください。

[Tasks]
Name: "addtopath"; Description: "{cm:AddToPathTask}"; GroupDescription: "{cm:PathGroup}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
; GUI と CLI は同一ディレクトリに置くこと（冒頭の設計メモ参照）
Source: "{#BinDir}\{#AppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\{#CliExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\assets\icon\{#IconName}"; DestDir: "{app}"; Flags: ignoreversion
; ライセンス本文と第三者の告知（Issue #1845。macOS の build-app.sh・ポータブル zip と同じ 3 本）。
; GPL-3.0 第 4 条・Apache-2.0 第 4 条 (a)・MIT / BSD の表示義務は、バイナリの受け取り手へ
; 本文と著作権表示を渡すことを求める。LICENSE は .txt を付けてメモ帳で開けるようにする。
; 3 本が揃っていることは crates/tako-control/tests/license_bundle_watchdog.rs が検査する
Source: "{#RepoRoot}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#RepoRoot}\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\THIRD-PARTY-LICENSES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"; IconFilename: "{app}\{#IconName}"
Name: "{group}\{cm:UninstallProgram,{#AppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; IconFilename: "{app}\{#IconName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  EnvironmentKey = 'Environment';
  { アプリ内更新（#2434）: 抜ける tako を待つ上限。過ぎたら Restart Manager に閉じさせる }
  TakoExitWaitMs = 30000;
  SYNCHRONIZE = $00100000;
  WAIT_OBJECT_0 = 0;

var
  { アプリ内更新（#2434）: ファイルの上書きが最後まで済んだか }
  InstallSucceeded: Boolean;

function OpenProcess(dwDesiredAccess: DWORD; bInheritHandle: BOOL; dwProcessId: DWORD): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForSingleObject(hHandle: THandle; dwMilliseconds: DWORD): DWORD;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseHandle(hObject: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

{ /TAKOWAITPID=<pid>（アプリ内更新で抜ける tako）。指定が無ければ 0 }
function TakoWaitPid(): Integer;
begin
  Result := StrToIntDef(ExpandConstant('{param:TAKOWAITPID|0}'), 0);
end;

{ Pid のプロセスが TimeoutMs 以内に終わったか（もう居ない・開けない場合も True）。
  TimeoutMs = 0 なら「いま居ないか」だけを見る }
function ProcessExited(Pid: Integer; TimeoutMs: Integer): Boolean;
var
  H: THandle;
begin
  Result := True;
  if Pid <= 0 then
    Exit;
  H := OpenProcess(SYNCHRONIZE, False, Pid);
  if H = 0 then
    Exit;
  Result := WaitForSingleObject(H, TimeoutMs) = WAIT_OBJECT_0;
  CloseHandle(H);
end;

{ アプリ内更新（#2434）: tako が抜けてから上書きを始める。抜けなければ Restart Manager に任せる }
function InitializeSetup(): Boolean;
var
  Pid: Integer;
begin
  Result := True;
  Pid := TakoWaitPid();
  if Pid <= 0 then
    Exit;
  if ProcessExited(Pid, TakoExitWaitMs) then
    Log(Format('tako (pid %d) has exited; continuing the update', [Pid]))
  else
    Log(Format('tako (pid %d) is still running after %d ms; Restart Manager will close it', [Pid, TakoExitWaitMs]));
end;

{ アプリ内更新（#2434）: 終わり際に tako を起動し直す。成功なら入れた先の新しい tako-app.exe、
  失敗・中断ならロールバックで元に戻った /TAKORELAUNCH の exe を起こす。
  待っていた tako がまだ残っているなら 2 つ目は起こさない。
  （波括弧のコメントは入れ子にできないので、この中に波括弧の定数を書かない） }
procedure DeinitializeSetup();
var
  Target, Installed: string;
  ResultCode: Integer;
begin
  Target := ExpandConstant('{param:TAKORELAUNCH|}');
  if Target = '' then
    Exit;
  if not ProcessExited(TakoWaitPid(), 0) then
  begin
    Log('tako is still running; not relaunching it');
    Exit;
  end;
  if InstallSucceeded then
  begin
    Installed := ExpandConstant('{app}\{#AppExeName}');
    if FileExists(Installed) then
      Target := Installed;
  end;
  if not FileExists(Target) then
  begin
    Log('relaunch target not found: ' + Target);
    Exit;
  end;
  if Exec(Target, '', ExtractFileDir(Target), SW_SHOWNORMAL, ewNoWait, ResultCode) then
    Log('relaunched tako: ' + Target)
  else
    Log(Format('failed to relaunch tako: %s (%s)', [Target, SysErrorMessage(ResultCode)]));
end;

{ 比較用に正規化する: 前後の空白を落とし、末尾のバックスラッシュを落とし、小文字化し、
  レジストリに未展開のまま入りがちな環境変数を展開する。
  PATH の重複追加防止はこの正規化の精度で決まる }
function NormalizePathEntry(const S: string): string;
begin
  Result := Lowercase(Trim(S));
  StringChangeEx(Result, '%localappdata%', Lowercase(GetEnv('LOCALAPPDATA')), True);
  StringChangeEx(Result, '%userprofile%', Lowercase(GetEnv('USERPROFILE')), True);
  StringChangeEx(Result, '%appdata%', Lowercase(GetEnv('APPDATA')), True);
  while (Length(Result) > 1) and (Result[Length(Result)] = '\') do
    Delete(Result, Length(Result), 1);
end;

{ ';' 区切りを分解する。空要素は捨てる }
function SplitPath(const Value: string): TArrayOfString;
var
  Rest, Item: string;
  P, N: Integer;
begin
  SetArrayLength(Result, 0);
  N := 0;
  Rest := Value;
  while Rest <> '' do
  begin
    P := Pos(';', Rest);
    if P > 0 then
    begin
      Item := Copy(Rest, 1, P - 1);
      Rest := Copy(Rest, P + 1, Length(Rest) - P);
    end
    else
    begin
      Item := Rest;
      Rest := '';
    end;
    if Trim(Item) <> '' then
    begin
      N := N + 1;
      SetArrayLength(Result, N);
      Result[N - 1] := Item;
    end;
  end;
end;

function IndexOfPathEntry(const Parts: TArrayOfString; const Dir: string): Integer;
var
  I: Integer;
  Needle: string;
begin
  Result := -1;
  Needle := NormalizePathEntry(Dir);
  for I := 0 to GetArrayLength(Parts) - 1 do
    if NormalizePathEntry(Parts[I]) = Needle then
    begin
      Result := I;
      Exit;
    end;
end;

{ HKCU\Environment\Path の末尾へ Dir を足す。既に入っていれば何もしない。
  HKLM 側は絶対に触らない }
procedure EnvAddPath(const Dir: string);
var
  OldPath, NewPath: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', OldPath) then
    OldPath := '';

  if IndexOfPathEntry(SplitPath(OldPath), Dir) >= 0 then
    Exit;

  if Trim(OldPath) = '' then
    NewPath := Dir
  else if Copy(OldPath, Length(OldPath), 1) = ';' then
    NewPath := OldPath + Dir
  else
    NewPath := OldPath + ';' + Dir;

  { REG_EXPAND_SZ で書く。Windows がユーザー PATH に使う既定の型で、
    %USERPROFILE% のような既存エントリを壊さない。
    元が REG_SZ だった場合は型が REG_EXPAND_SZ へ変わる（アンインストールしても戻らない）。
    Inno の Pascal Script には値の型を読む API が無いため型の復元はできないが、
    REG_SZ → REG_EXPAND_SZ は %VAR% の展開が効くようになるだけで既存の解決結果は変わらない。
    rustup / VS Code など他のインストーラーも同じ選択をしている }
  if not RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', NewPath) then
  begin
    { 無人インストール（/SILENT・/VERYSILENT）でダイアログを出すと止まってしまうので、
      その場合はセットアップログに残すだけにする }
    Log('PATH の更新に失敗した: ' + Dir);
    if not WizardSilent then
      MsgBox(FmtMessage(CustomMessage('PathWriteFailed'), [Dir]), mbError, MB_OK);
  end;
end;

{ Dir に一致するエントリだけを取り除く。残りは元の文字列のまま並べ直す }
procedure EnvRemovePath(const Dir: string);
var
  OldPath, NewPath, Needle: string;
  Parts: TArrayOfString;
  I: Integer;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', OldPath) then
    Exit;

  Parts := SplitPath(OldPath);
  if IndexOfPathEntry(Parts, Dir) < 0 then
    Exit;

  Needle := NormalizePathEntry(Dir);
  NewPath := '';
  for I := 0 to GetArrayLength(Parts) - 1 do
    if NormalizePathEntry(Parts[I]) <> Needle then
    begin
      if NewPath <> '' then
        NewPath := NewPath + ';';
      NewPath := NewPath + Parts[I];
    end;

  if NewPath = '' then
    RegDeleteValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path')
  else
    RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvironmentKey, 'Path', NewPath);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    EnvAddPath(ExpandConstant('{app}'));
  { ssPostInstall に来た = ファイルの上書きとレジストリの書き込みが済んだ（#2434） }
  if CurStep = ssPostInstall then
    InstallSucceeded := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  { タスクを選んでいたかに関わらず消す。入っていなければ EnvRemovePath は何もしない }
  if CurUninstallStep = usPostUninstall then
    EnvRemovePath(ExpandConstant('{app}'));
end;
