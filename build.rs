fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=fzip.manifest");
    println!("cargo:rerun-if-changed=brand/fzip.ico");

    #[cfg(windows)]
    {
        let version = env!("CARGO_PKG_VERSION");
        let mut res = winresource::WindowsResource::new();
        // An unmanifested executable is treated as legacy by Windows and reads
        // as less trustworthy to reputation heuristics. The manifest also turns
        // on long-path and UTF-8 support, which this tool genuinely needs.
        res.set_manifest_file("fzip.manifest");
        // Explorer, the taskbar and the Open With dialog all read this.
        // The .ico carries 16 through 256 px so every one of them gets a
        // crisp size instead of a scaled-down 256.
        res.set_icon("brand/fzip.ico");
        res.set("ProductName", "Fzip");
        res.set("FileDescription", "Fzip - fast portable zip tool");
        res.set("CompanyName", "Tcoder LLC");
        res.set("LegalCopyright", "Copyright (c) 2026 Tcoder LLC. MIT licensed.");
        res.set("LegalTrademarks", "Fzip is a product of Tcoder LLC.");
        res.set("OriginalFilename", "fzip.exe");
        res.set("InternalName", "fzip");
        res.set("ProductVersion", version);
        res.set("FileVersion", version);
        // An analyst looking at an unsigned binary checks the version resource
        // first. Stating the origin here lets them verify the claim against a
        // live site and a public repository instead of guessing.
        res.set(
            "Comments",
            "Homepage https://fzip.org/ - source https://github.com/xmetaads/Fzip",
        );
        // A missing resource compiler must not break the build on other setups.
        if let Err(e) = res.compile() {
            println!("cargo:warning=version resource not embedded: {}", e);
        }
    }
}

