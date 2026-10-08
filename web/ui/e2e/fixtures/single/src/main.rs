// Fixture project for the web UI browser tests (S-611).
fn main() {
    println!("{}", greeting("logos"));
}

fn greeting(name: &str) -> String {
    format!("hello, {}", shout(name))
}

fn shout(name: &str) -> String {
    name.to_uppercase()
}
