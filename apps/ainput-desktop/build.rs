fn main() {
    println!("cargo:rerun-if-changed=../../assets/app-icon.ico");

    if cfg!(target_os = "windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/app-icon.ico");
        resource
            .compile()
            .expect("compile Windows resources for ainput-desktop");
    }
}
