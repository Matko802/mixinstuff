
fn main() {
    println!("cargo:rerun-if-changed=assets/icons");
    println!("cargo:rerun-if-changed=resources");
    glib_build_tools::compile_resources(&["resources", "assets/icons"], "resources/resources.gresource.xml", "musishark.gresource");
}
