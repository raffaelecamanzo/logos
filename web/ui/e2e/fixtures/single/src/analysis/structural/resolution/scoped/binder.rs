// Binds a looked-up name to a scope label (fixture, S-616).
pub fn bind(name: &str) -> String {
    if name.is_empty() {
        return String::from("<anonymous>");
    }
    let mut label = String::new();
    for (i, part) in name.split('.').enumerate() {
        if i > 0 {
            label.push_str("::");
        }
        if part.len() > 8 {
            label.push_str(&part[..8]);
        } else {
            label.push_str(part);
        }
    }
    label
}
