fn main() {
    println!("cargo:rerun-if-changed=$CARGO_MANIFEST_DIR");
}
