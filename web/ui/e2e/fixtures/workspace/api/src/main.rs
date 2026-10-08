// Workspace fixture member `api` for the web UI browser tests (S-611).
fn main() {
    println!("{}", route("/orders"));
}

fn route(path: &str) -> String {
    format!("GET {path}")
}
