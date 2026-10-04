#[cfg(windows)]
mod windows;
fn main() {
    #[cfg(windows)]
    {
        let result = windows::main_capture();
        // Exactly one bounded JSON response. Diagnostics belong on stderr.
        match serde_json::to_string(&result) {
            Ok(json) => println!("{json}"),
            Err(_) => std::process::exit(2),
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("The fixed-function capture broker runs only on Windows.");
        std::process::exit(2);
    }
}
