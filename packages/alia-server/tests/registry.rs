use alia_server::registry::{is_plugin_source, owner_from_source};

#[test]
fn classifies_registry_sources() {
    let builtin = owner_from_source("builtin");
    assert_eq!(builtin.kind, "builtin");
    assert_eq!(builtin.name, "builtin");
    assert!(!is_plugin_source("builtin"));

    let component = owner_from_source("component:embedded");
    assert_eq!(component.kind, "component");
    assert_eq!(component.name, "embedded");
    assert!(!is_plugin_source("component:embedded"));

    let plugin = owner_from_source("zigbee-bridge");
    assert_eq!(plugin.kind, "plugin");
    assert_eq!(plugin.name, "zigbee-bridge");
    assert!(is_plugin_source("zigbee-bridge"));

    let plugin = owner_from_source("plugin:zigbee-bridge");
    assert_eq!(plugin.kind, "plugin");
    assert_eq!(plugin.name, "zigbee-bridge");
    assert!(is_plugin_source("plugin:zigbee-bridge"));
}
