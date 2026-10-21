#![forbid(unsafe_code)]

#[test]
fn route_diagnostics() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
