// Workspace fixture member `web` for the web UI browser tests (S-611).
fn main() {
    println!("{}", render("orders"));
}

fn render(page: &str) -> String {
    format!("<h1>{page}</h1>")
}
