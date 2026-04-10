fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/app-icon.ico");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

    if cfg!(target_os = "windows") {
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/app-icon.ico");
        resource.set("FileVersion", &version);
        resource.set("ProductVersion", &version);
        resource
            .compile()
            .expect("compile Windows resources for ainput-desktop");
    }
}
