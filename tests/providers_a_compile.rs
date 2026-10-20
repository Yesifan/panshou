use pansou::providers::a_registry;
#[test]
fn registry_is_unique() {
    let mut names = std::collections::HashSet::new();
    for provider in a_registry::providers() {
        assert!(names.insert(provider.meta().name));
    }
}
