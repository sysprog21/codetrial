mod fixtures;

#[test]
fn accepts_up_to_the_backlog() {
    let mut stream = fixtures::stream();
    assert!(stream.accept());
}
