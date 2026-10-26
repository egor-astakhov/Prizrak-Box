use std::path::PathBuf;

fn core_version() -> String {
    // Same source as vite.config.ts readCoreVersion(): the Prizrak-Core
    // replace line in src-go/go.mod.
    let go_mod = std::fs::read_to_string("../src-go/go.mod").unwrap_or_default();
    for line in go_mod.lines() {
        if let Some(i) = line.find("legiz-ru/Prizrak-Core") {
            let rest = line[i + "legiz-ru/Prizrak-Core".len()..].trim();
            let v: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
            if v.starts_with('v') {
                return v;
            }
        }
    }
    String::new()
}

/// The release version: package.json (CI stamps it with build/sync-version.js,
/// exactly like the Electron and Wails builds), else the crate version.
fn app_version() -> String {
    let pkg = std::fs::read_to_string("../package.json").unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&pkg)
        .ok()
        .and_then(|v| v.get("version").and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_default())
}

fn numeric(v: &str) -> [u16; 4] {
    let core = v.trim_start_matches('v').split(['-', '+']).next().unwrap_or("0");
    let mut out = [0u16; 4];
    for (i, p) in core.split('.').take(4).enumerate() {
        out[i] = p.parse().unwrap_or(0);
    }
    out
}

fn main() {
    println!("cargo:rerun-if-changed=../src-go/go.mod");
    println!("cargo:rerun-if-changed=../package.json");
    println!("cargo:rerun-if-changed=../src/locales");
    let version = app_version();
    println!("cargo:rustc-env=PRIZRAK_CORE_VERSION={}", core_version());
    println!("cargo:rustc-env=PRIZRAK_APP_VERSION={version}");

    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config).expect("slint compile");

    if std::env::var("CARGO_CFG_WINDOWS").is_ok() {
        // Icon + manifest + a real FILEVERSION: the MSI's file costing needs a
        // versioned exe (see the Wails job in .github/workflows/release.yml).
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        let manifest = std::fs::canonicalize("windows/app.manifest").unwrap();
        let icon = std::fs::canonicalize("../build/appicon.ico").unwrap();
        let [a, b, c, d] = numeric(&version);
        let rc = format!(
            r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {a},{b},{c},{d}
PRODUCTVERSION {a},{b},{c},{d}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "legiz-ru"
      VALUE "FileDescription", "Prizrak-Box"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "Prizrak-Box"
      VALUE "OriginalFilename", "Prizrak-Box.exe"
      VALUE "ProductName", "Prizrak-Box"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "GPL-3.0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
            icon = icon.display().to_string().replace('\\', "/"),
            manifest = manifest.display().to_string().replace('\\', "/"),
        );
        let rc_path = out.join("app.rc");
        std::fs::write(&rc_path, rc).unwrap();
        println!("cargo:rerun-if-changed=windows/app.manifest");
        // Required, not optional: without a resource compiler (llvm-rc when
        // cross-compiling) the exe silently loses its FILEVERSION, and an MSI
        // upgrade over a versioned exe then leaves no Prizrak-Box.exe at all.
        embed_resource::compile(&rc_path, embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
