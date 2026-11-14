#[cfg(windows)]
fn main() {
    // Служебные крейты (tools/video_probe) собираются в неинтерактивной
    // SSH-сессии, где процесс с requireAdministrator просто не стартует.
    // Для таких сборок манифест отключается переменной окружения.
    println!("cargo:rerun-if-env-changed=ALBUS_NO_ADMIN_MANIFEST");
    if std::env::var_os("ALBUS_NO_ADMIN_MANIFEST").is_some() {
        return;
    }

    let mut res = winres::WindowsResource::new();
    res.set_manifest(
        r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>
"#,
    );
    if let Err(e) = res.compile() {
        eprintln!("Не удалось скомпилировать манифест Windows: {}", e);
    }
}

#[cfg(not(windows))]
fn main() {}
