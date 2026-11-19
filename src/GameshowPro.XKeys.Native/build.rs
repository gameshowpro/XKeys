//! Embeds a Windows version resource (VERSIONINFO) in xkhid.dll, so the file's properties show who made it, what it is and which
//! release it belongs to. It does nothing on other targets.
//!
//! The build that knows the release version passes it in: GSP_FILE_VERSION (a.b.c.d, each part at most 65535) and
//! GSP_PRODUCT_VERSION (the package version, for example 2.2.5). A plain `cargo build` falls back to the crate's version.

use std::env;
use std::fs;
use std::path::PathBuf;

const COMPANY: &str = "Barjonas LLC";
const COPYRIGHT: &str = "Copyright (c) 2026 Barjonas LLC";
const PRODUCT: &str = "GameshowPro.XKeys";
const DESCRIPTION: &str = "X-keys HID reader with hardware-accurate report timestamps";
const DLL_NAME: &str = "xkhid.dll";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GSP_FILE_VERSION");
    println!("cargo:rerun-if-env-changed=GSP_PRODUCT_VERSION");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let pkg_version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by cargo");
    let file_version = match env::var("GSP_FILE_VERSION") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => format!("{pkg_version}.0"),
    };
    let product_version = match env::var("GSP_PRODUCT_VERSION") {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => format!("{pkg_version}-local"),
    };

    let parts: Vec<u16> = file_version
        .split('.')
        .map(|p| p.parse::<u16>())
        .collect::<Result<_, _>>()
        .unwrap_or_default();
    if parts.len() != 4 {
        panic!(
            "GSP_FILE_VERSION must be four numeric parts a.b.c.d, each at most 65535, but is '{file_version}'"
        );
    }
    let numeric = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
    if product_version.contains(['"', '\\']) {
        panic!("GSP_PRODUCT_VERSION must not contain quotes or backslashes: '{product_version}'");
    }

    let rc = format!(
        r#"#include <winver.h>

VS_VERSION_INFO VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK VS_FFI_FILEFLAGSMASK
FILEFLAGS 0
FILEOS VOS_NT_WINDOWS32
FILETYPE VFT_DLL
FILESUBTYPE VFT2_UNKNOWN
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904B0"
        BEGIN
            VALUE "CompanyName", "{COMPANY}"
            VALUE "FileDescription", "{DESCRIPTION}"
            VALUE "FileVersion", "{file_version}"
            VALUE "InternalName", "{DLL_NAME}"
            VALUE "LegalCopyright", "{COPYRIGHT}"
            VALUE "OriginalFilename", "{DLL_NAME}"
            VALUE "ProductName", "{PRODUCT}"
            VALUE "ProductVersion", "{product_version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0409, 1200
    END
END
"#
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let rc_path = out_dir.join("xkhid.rc");
    fs::write(&rc_path, rc).expect("write the version resource script");
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_required()
        .expect("compile the version resource (needs rc.exe from the Windows SDK)");
}
