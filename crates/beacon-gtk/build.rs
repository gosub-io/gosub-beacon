/// This will be run prior to compiling the project and will compile the resources
fn main() {
    glib_build_tools::compile_resources(&["./resources"], "./resources/resources.gresource.xml", "gosub.gresource");
}
