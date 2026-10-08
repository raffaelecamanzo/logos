// Fixture project for the web UI browser tests (S-611).
//
// The nested `analysis` module puts two files under a path longer than the
// Files & Risk path budget, so the browser tests see an abbreviated path (S-616).
mod analysis {
    pub mod structural {
        pub mod resolution {
            pub mod scoped {
                pub mod binder;
                pub mod lookup;
            }
        }
    }
}

use analysis::structural::resolution::scoped::{binder, lookup};

fn main() {
    println!("{}", greeting("logos"));
    println!("{}", binder::bind(&lookup::lookup("logos")));
}

fn greeting(name: &str) -> String {
    format!("hello, {}", shout(name))
}

fn shout(name: &str) -> String {
    name.to_uppercase()
}
