fn main() {
    println!("cargo:rerun-if-env-changed=ARTICULATE_RESOURCE");
    if let Ok(resource) = std::env::var("ARTICULATE_RESOURCE") {
        println!("cargo:rerun-if-changed={resource}");
        assert!(
            std::path::Path::new(&resource).is_file(),
            "The Windows resource file is missing"
        );
        println!("cargo:rustc-link-arg={resource}");
    }
}
