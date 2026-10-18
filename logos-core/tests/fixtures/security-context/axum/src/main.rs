mod db;

use axum::routing::get;
use axum::Router;

fn app() -> Router {
    Router::new().route("/users/:id", get(show_user))
}

async fn show_user(id: String) -> String {
    let query = db::find_user(&id);
    query
}

fn main() {
    let _ = app();
}
