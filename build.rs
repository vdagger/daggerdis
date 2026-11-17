fn main() {
    // Штамп сборки: по нему видно, у кого из собеседников запущена старая
    // версия. Скопированный на другой компьютер exe остаётся прежним, и по
    // «роботу» в наушниках этого не понять — а по штампу видно сразу.
    // Берём самое свежее время изменения в исходниках, а не «сейчас»:
    // иначе cargo не перезапускает скрипт, и в новом exe остаётся старый штамп.
    // Одинакового штампа у двух разных сборок быть не может, а у копии того же
    // exe — такой же, что и нужно.
    let stamp = newest_mtime(std::path::Path::new("src"), std::time::UNIX_EPOCH)
        .max(newest_mtime(
            std::path::Path::new("build.rs"),
            std::time::UNIX_EPOCH,
        ))
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=ALBUS_BUILD_STAMP={stamp}");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(windows)]
    windows_manifest();
}

/// Самое свежее время изменения файла или каталога (рекурсивно).
fn newest_mtime(path: &std::path::Path, acc: std::time::SystemTime) -> std::time::SystemTime {
    let Ok(meta) = std::fs::metadata(path) else {
        return acc;
    };
    let mut newest = acc;
    if let Ok(modified) = meta.modified() {
        newest = newest.max(modified);
    }
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                newest = newest_mtime(&entry.path(), newest);
            }
        }
    }
    newest
}

#[cfg(windows)]
fn windows_manifest() {
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
