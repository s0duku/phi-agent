use crate::container::job::JobHandle;

#[test]
fn random_handle_has_the_shared_encoding() {
    let handle = JobHandle::random().unwrap();

    assert_eq!(handle.0.len(), 9);
    assert!(JobHandle::is_valid(&handle.0));
}

#[test]
fn handle_validation_requires_two_lowercase_name_parts() {
    assert!(JobHandle::is_valid("mira-kest"));
    assert!(!JobHandle::is_valid("short"));
    assert!(!JobHandle::is_valid("0123-4567"));
    assert!(!JobHandle::is_valid("Mira-kest"));
    assert!(!JobHandle::is_valid("mira/kest"));
}
