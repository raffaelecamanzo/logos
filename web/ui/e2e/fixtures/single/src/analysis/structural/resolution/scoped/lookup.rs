// Looks a name up in the fixture's one scope (fixture, S-616).
pub fn lookup(name: &str) -> String {
    match name {
        "" => String::new(),
        n if n.starts_with('_') => format!("private.{n}"),
        n => format!("crate.{n}"),
    }
}
