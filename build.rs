fn main() {
    // Embed the application icon into the Windows executable so it shows up in
    // Explorer, the taskbar and shortcuts. No-op on non-Windows targets.
    //
    // No application manifest is embedded here on purpose: GPUI's Windows
    // platform crate embeds one of its own, and two RT_MANIFEST resources of
    // the same id cannot be merged — the GNU linker kept both and the loader
    // ended up with the wrong activation context, killing the process at
    // 0xC0000139 STATUS_ENTRYPOINT_NOT_FOUND before `main`. GPUI's manifest
    // already declares Common-Controls 6.0 and PerMonitorV2, which is
    // everything the window needs. (assets/xenterm.exe.manifest is kept in the
    // tree only as a record of what ours declared.)
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/xenterm.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/xenterm.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows icon: {e}");
        }
    }
}
