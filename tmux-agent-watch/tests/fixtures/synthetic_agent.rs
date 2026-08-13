use std::env;
use std::io::{self, Write};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    let mode = env::args().nth(1).unwrap_or_else(|| "static".to_owned());
    match mode.as_str() {
        "static" => thread::sleep(Duration::from_secs(30)),
        "changing" => {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut count = 0_u64;
            while Instant::now() < deadline {
                count += 1;
                println!("{count}");
                io::stdout().flush().expect("stdout should flush");
                thread::sleep(Duration::from_millis(20));
            }
        }
        "child" => {
            let status = Command::new("sleep")
                .arg("30")
                .status()
                .expect("child should start");
            std::process::exit(status.code().unwrap_or(1));
        }
        _ => panic!("unknown synthetic agent mode: {}", mode),
    }
}
