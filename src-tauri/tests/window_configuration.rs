#[test]
fn main_window_has_a_desktop_minimum_and_a_valid_initial_size() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
    let main = config["app"]["windows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|window| window["label"] == "main")
        .unwrap();
    assert_eq!(main["minWidth"], 1280);
    assert_eq!(main["minHeight"], 720);
    assert!(main["width"].as_u64().unwrap() >= main["minWidth"].as_u64().unwrap());
    assert!(main["height"].as_u64().unwrap() >= main["minHeight"].as_u64().unwrap());
    assert_eq!(main["resizable"], true);
}
