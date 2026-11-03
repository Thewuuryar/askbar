//! docs/openapi/control.v1.yaml describes exactly the controller's routes.

#[test]
fn the_file_describes_exactly_the_controllers_routes() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/openapi/control.v1.yaml");
    let doc: serde_json::Value = serde_yaml_ng::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut in_file: Vec<(String, String)> = doc["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(p, ops)| ops.as_object().unwrap().keys().map(move |m| (m.to_uppercase(), p.clone())))
        .collect();
    let mut in_code: Vec<(String, String)> = askbar_controller::route_table().into_iter().map(|(m, p)| (m.to_string(), p)).collect();
    in_file.sort();
    in_code.sort();
    assert_eq!(in_file, in_code);
}
