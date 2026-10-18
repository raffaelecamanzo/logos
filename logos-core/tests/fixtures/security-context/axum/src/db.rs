pub fn find_user(id: &str) -> String {
    let query = format!("SELECT * FROM users WHERE id = '{id}'");
    query
}

pub fn purge(table: &str) -> String {
    let query = format!("DELETE FROM {table}");
    query
}
