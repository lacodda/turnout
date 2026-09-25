// Embeds the Windows executable icon and version metadata. The icon is the S
// tile of the lacodda line mark, exported to a multi-size .ico so Explorer
// picks the right resolution for each view.
fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("failed to embed the Windows resources");
    }

    // Windows gives a program's main thread 1 MiB of stack; Linux and macOS
    // give 8. clap's derive builds the arguments of every subcommand in one
    // function, and a debug build keeps a stack slot per argument there - in
    // v0.21.0 the debug binary overflowed before it had parsed a single one,
    // `--version` included, while the release build never came close - so it
    // is the tests and the developer that hit it, never with a word about
    // why. The same 8 MiB the other platforms give.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        const STACK: u32 = 8 * 1024 * 1024;
        match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => println!("cargo:rustc-link-arg-bins=/STACK:{STACK}"),
            _ => println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK}"),
        }
    }
}
