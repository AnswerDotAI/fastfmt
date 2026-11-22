fn main() { std::process::exit(match fastfmt::run() { Ok(code) => code, Err(e) => { eprintln!("{e}"); 2 } }); }
