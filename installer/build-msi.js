const path = require('path');
const fs = require('fs-extra');
const { execSync } = require('child_process');

const ARCH = process.env.ARCH || 'x64';
const VERSION = require('../package.json').version;
const IS_ARM64 = ARCH === 'arm64';
const IS_WAILS = (process.env.BUILD_TYPE || 'electron') === 'wails';

// Electron and Wails MSIs share the same install directory but must NOT
// share an UpgradeCode — otherwise Windows Installer treats them as the same
// product chain and silently rolls back whichever was installed first.
const ELECTRON_UPGRADE_CODE = 'c1d377b2-2c61-4c5e-8773-8e3c703b8b41'; // Electron build (also used by electron-forge MakerWix)
const UPGRADE_CODE = IS_WAILS
  ? 'd5f799e1-4b83-4a7f-b662-1a3c924d5e0f'   // Wails build — separate upgrade chain
  : ELECTRON_UPGRADE_CODE;

// Fixed component GUIDs. Electron and Wails are separate products, so their
// shared-path components (registration + shortcuts) MUST use different GUIDs —
// reusing one GUID across products violates the MSI component rules and caused
// the "dead desktop shortcut / Prizrak-Box.exe missing from the install folder"
// bug when both were installed. Electron keeps its original GUIDs so its own
// upgrade chain is undisturbed.
const COMPONENT_GUIDS = IS_WAILS
  ? {
      AppRegistrationGuid: 'b7e6d1a0-9c34-4f21-8a5e-1f2a3b4c5d60',
      DesktopShortcutGuid: 'b7e6d1a0-9c34-4f21-8a5e-1f2a3b4c5d61',
      StartMenuShortcutGuid: 'b7e6d1a0-9c34-4f21-8a5e-1f2a3b4c5d62',
    }
  : {
      AppRegistrationGuid: '12345678-1234-1234-1234-123456789abc',
      DesktopShortcutGuid: '23456789-1234-1234-1234-123456789abc',
      StartMenuShortcutGuid: '34567890-1234-1234-1234-123456789abc',
    };

// Clears the installing user's system proxy when it is left pointing at the
// px that the custom actions above just force-killed (uninstall, upgrade):
// neither the shell nor px gets to undo it then, and the user is left with a
// proxy on a dead port — no web access until the app is started again, or
// ever, after an uninstall. Only a proxy that can be ours is touched: plain
// "host:port" on loopback or the app's bindAddress, with nothing listening
// on it once our processes are gone (px may have moved off the configured
// port, so the port alone cannot tell). Same writes as px's own
// DisableProxy (src-go/pkg/sys/proxy/proxy_windows.go) plus the WinINet
// refresh. Passed to powershell -EncodedCommand: Base64 has none of the
// [ ] { } characters Windows Installer would format.
const RELEASE_USER_PROXY_PS = String.raw`
$ErrorActionPreference = 'SilentlyContinue'
$k = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
$s = Get-ItemProperty $k
if ($s.ProxyEnable -ne 1 -or $s.ProxyServer -notmatch '^\[?([^\]=;]+?)\]?:(\d+)$') { exit }
$h = $Matches[1]; $p = [int]$Matches[2]
$hosts = @('127.0.0.1', 'localhost', '::1')
$f = Join-Path $env:APPDATA 'prizrak-box\home.path'
$dir = if (Test-Path $f) { (Get-Content $f -Raw).Trim() } else { '' }
if (-not $dir) { $dir = Join-Path $env:USERPROFILE 'Prizrak-Box-V3' }
$c = Join-Path $dir 'px-electron.db\config.json'
if (Test-Path $c) { $b = ((Get-Content $c -Raw | ConvertFrom-Json).setting | ConvertFrom-Json).bindAddress; if ($b) { $hosts += $b } }
if ($hosts -notcontains $h) { exit }
$alive = $false
try { $t = New-Object Net.Sockets.TcpClient; $alive = $t.ConnectAsync($h, $p).Wait(1500) -and $t.Connected; $t.Close() } catch {}
if ($alive) { exit }
Set-ItemProperty $k ProxyEnable 0 -Type DWord
Set-ItemProperty $k ProxyServer ''
Add-Type -Namespace PB -Name WinInet -MemberDefinition '[DllImport("wininet.dll")] public static extern bool InternetSetOption(IntPtr h, int o, IntPtr b, int l);'
[void][PB.WinInet]::InternetSetOption([IntPtr]::Zero, 39, [IntPtr]::Zero, 0)
[void][PB.WinInet]::InternetSetOption([IntPtr]::Zero, 37, [IntPtr]::Zero, 0)
`;
const RELEASE_USER_PROXY_CMD = Buffer.from(RELEASE_USER_PROXY_PS.trim(), 'utf16le').toString('base64');

const PATHS = {
  root: path.resolve(__dirname, '..'),
  installer: path.resolve(__dirname, 'wix'),
  out: path.resolve(__dirname, '..', 'out'),
  appFiles: path.resolve(__dirname, '..', 'out', 'Prizrak-Box-win32-' + ARCH),
  msiOut: path.resolve(__dirname, '..', 'out', 'msi', ARCH)
};

async function harvestFiles() {
  console.log('🔍 Harvesting application files with heat.exe...');

  const heatWxsFile = path.join(PATHS.msiOut, 'HarvestedFiles.wxs');

  // Use heat.exe to harvest all files from the app directory
  const heatCmd = `heat.exe dir "${PATHS.appFiles}" -nologo -cg ApplicationFiles -gg -sfrag -srd -sreg -dr INSTALLDIR -var var.SourceDir -out "${heatWxsFile}"`;

  try {
    execSync(heatCmd, { stdio: 'inherit' });
    console.log(`✅ Files harvested to: ${path.basename(heatWxsFile)}\n`);
    return heatWxsFile;
  } catch (error) {
    console.error('❌ Failed to harvest files:', error.message);
    throw error;
  }
}

async function buildMSI() {
  console.log(`\n🔧 Building MSI installer for ${ARCH}...`);

  // Ensure output directory exists
  await fs.ensureDir(PATHS.msiOut);

  // Prepare WiX variables (use English as primary language)
  const wixVars = {
    ProductVersion: VERSION.replace(/-.*$/, '').replace(/^(\d+\.\d+\.\d+).*/, '$1'), // MSI requires numeric version only
    FullVersion: VERSION, // Keep full version string with alpha/beta suffix for display
    Platform: IS_ARM64 ? 'arm64' : 'x64',
    Win64: 'yes',
    ProgramFilesFolder: 'ProgramFiles64Folder',
    SourceDir: PATHS.appFiles,
    Language: '1033', // Primary language: English
    Culture: 'en-us',
    UpgradeCode: UPGRADE_CODE,
    // Electron UpgradeCode to supersede, consumed by the wails-only
    // <Upgrade>/cleanup logic in Product.wxs (harmless/unused for electron).
    ElectronUpgradeCode: ELECTRON_UPGRADE_CODE,
    ReleaseUserProxyCmd: RELEASE_USER_PROXY_CMD,
    ...COMPONENT_GUIDS,
  };

  // Gate the Electron-supersede + cleanup logic on a plain "is this the Wails
  // build?" flag. Defined ONLY for the Wails build so Product.wxs can use
  // <?ifdef Wails ?> — a bare existence check that avoids the WiX v3
  // preprocessor's quoted-string comparison pitfall (`$(var.X) = "wails"` does
  // not match, which previously dropped all the wails-only authoring).
  if (IS_WAILS) {
    wixVars.Wails = '1';
  }

  const wixDefines = Object.entries(wixVars)
    .map(([key, value]) => `-d${key}="${value}"`)
    .join(' ');

  try {
    // Step 1: Harvest files with heat.exe
    const harvestedWxs = await harvestFiles();

    // Step 2: Compile WiX sources (.wxs -> .wixobj)
    console.log('📦 Compiling WiX sources...');

    const wxsFiles = [
      path.join(PATHS.installer, 'Product.wxs'),
      path.join(PATHS.installer, 'UI.wxs'),
      path.join(PATHS.installer, 'Files.wxs'),
      harvestedWxs
    ];

    const wixobjFiles = [];

    for (const wxsFile of wxsFiles) {
      const wixobjFile = path.join(PATHS.msiOut, path.basename(wxsFile, '.wxs') + '.wixobj');
      wixobjFiles.push(wixobjFile);

      const candleCmd = `candle.exe -nologo -arch ${IS_ARM64 ? 'arm64' : 'x64'} ${wixDefines} -ext WixUIExtension -ext WixUtilExtension -out "${wixobjFile}" "${wxsFile}"`;

      console.log(`  Compiling ${path.basename(wxsFile)}...`);
      execSync(candleCmd, { stdio: 'inherit', cwd: PATHS.installer });
    }

    // Step 3: Link WiX objects to create single MSI (.wixobj -> .msi)
    console.log('🔗 Linking MSI package...');
    console.log('   Language: English');

    const msiFile = path.join(PATHS.msiOut, `windows-${ARCH}.msi`);
    const enLocFile = path.join(PATHS.installer, 'localization', 'en-us.wxl');

    // Build single English MSI (no language transforms)
    const lightCmd = `light.exe -nologo ${wixobjFiles.map(f => `"${f}"`).join(' ')} -ext WixUIExtension -ext WixUtilExtension -cultures:en-us -loc "${enLocFile}" -out "${msiFile}" -sval`;
    execSync(lightCmd, { stdio: 'inherit', cwd: PATHS.installer });

    console.log(`✅ MSI created: ${msiFile}`);
    console.log(`   Language: English`);
    console.log(`   Size: Optimized single-language build\n`);
    return msiFile;

  } catch (error) {
    console.error('❌ Failed to build MSI:', error.message);
    throw error;
  }
}

async function main() {
  console.log('🚀 Prizrak-Box MSI Installer Builder\n');
  console.log(`Architecture: ${ARCH}`);
  console.log(`Expected app path: ${PATHS.appFiles}`);

  // Check if app is built
  if (!fs.existsSync(PATHS.appFiles)) {
    console.error(`❌ Application files not found at: ${PATHS.appFiles}`);
    console.error('\n📁 Checking what exists in out/ directory:');

    try {
      const outDirs = fs.readdirSync(PATHS.out);
      outDirs.forEach(dir => {
        const fullPath = path.join(PATHS.out, dir);
        if (fs.statSync(fullPath).isDirectory()) {
          console.error(`   - ${dir}`);
        }
      });
    } catch (err) {
      console.error(`   Could not read out/ directory: ${err.message}`);
    }

    console.error('\n   Please run "npm run package" first!');
    process.exit(1);
  }

  console.log(`✓ Found application files\n`);

  try {
    // Build single English MSI
    await buildMSI();

    console.log('🎉 MSI installer built successfully!\n');
    console.log('ℹ️  The installer includes:');
    console.log('   - English UI');
    console.log('   - GPL3 license agreement');
    console.log('   - Feature selection (Main App + TUN Service)');
    console.log('   - Automatic process/service cleanup');
    console.log('   - TUN service installation\n');
  } catch (error) {
    console.error('❌ Build failed');
    process.exit(1);
  }
}

if (require.main === module) {
  main();
}

module.exports = { buildMSI };
