//! Integration test target. Its items must be excluded from analysis.

#[test]
fn it_works() {
    assert_eq!(targets::api::visible(), 0);
}
