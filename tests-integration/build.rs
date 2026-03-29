// build.rs - Required for nvim_oxi::test macro
// This sets up the nvim-oxi test environment
fn main() -> Result<(), nvim_oxi::tests::BuildError> {
    nvim_oxi::tests::build()
}
