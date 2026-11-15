//! `bopogram-host --client`: a manual test client. Reads request lines from
//! stdin, sends each to the running host, prints the response.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::thread;
use std::time::Duration;

const ERROR_PIPE_BUSY: i32 = 231;

fn connect(name: &str) -> io::Result<File> {
    let mut attempts = 0;
    loop {
        match OpenOptions::new().read(true).write(true).open(name) {
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempts < 20 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

pub fn run(pipe_name: &str) -> io::Result<()> {
    let pipe = connect(pipe_name)?;
    let mut responses = BufReader::new(&pipe);
    for line in io::stdin().lock().lines() {
        let line = line?;
        // PowerShell puts a byte order mark in front of piped input.
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        (&pipe).write_all(format!("{line}\n").as_bytes())?;
        let mut response = String::new();
        if responses.read_line(&mut response)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "host closed the connection",
            ));
        }
        println!("{}", response.trim_end());
    }
    Ok(())
}
