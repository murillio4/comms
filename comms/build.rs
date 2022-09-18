fn main() -> std::io::Result<()> {
    println!(
        "cargo:rerun-if-changed={}/resources",
        env!("CARGO_MANIFEST_DIR")
    );

    #[cfg(all(windows, not(debug_assertions)))]
    {
        winres::WindowsResource::new()
            // This path can be absolute, or relative to your crate root.
            .set_icon("resources/logo.ico")
            .compile()?;
    }
    Ok(())
}
