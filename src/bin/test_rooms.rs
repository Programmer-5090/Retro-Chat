use std::env;
use std::io::{ Read, Write };
use std::net::TcpStream;
use std::thread;
use std::time::Duration;

fn connect() -> TcpStream {
    TcpStream::connect("127.0.0.1:8082").unwrap()
}

fn recv_lines(stream: &mut TcpStream, timeout: Duration) -> Vec<String> {
    let start = std::time::Instant::now();
    let mut res = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if start.elapsed() > timeout {
            break;
        }
        stream.set_read_timeout(Some(Duration::from_millis(50))).ok();
        match stream.read(&mut buf) {
            Ok(0) => {
                break;
            }
            Err(_) => {
                continue;
            }
            Ok(n) => {
                let s = String::from_utf8_lossy(&buf[..n]);
                for line in s.lines() {
                    let t = line.trim().to_string();
                    if !t.is_empty() {
                        res.push(t);
                    }
                }
            }
        }
    }
    res
}

fn send(stream: &mut TcpStream, msg: &str) {
    writeln!(stream, "{}", msg).unwrap();
}

fn auth(stream: &mut TcpStream, username: &str, password: &str) {
    send(stream, username);
    thread::sleep(Duration::from_millis(300));
    let lines = recv_lines(stream, Duration::from_millis(1000));
    for l in &lines {
        println!("  <- {l}");
    }

    if lines.iter().any(|l| l.contains("Register")) {
        send(stream, &format!("/register {password}"));
    } else {
        send(stream, &format!("/login {password}"));
    }
    thread::sleep(Duration::from_millis(300));
    let lines = recv_lines(stream, Duration::from_millis(1500));
    for l in &lines {
        println!("  <- {l}");
    }
    assert!(
        lines.iter().any(|l| l.contains("Token:")),
        "Auth failed for {username}"
    );
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: test_rooms <setup|alice|bob|charlie>");
        return;
    }
    match args[1].as_str() {
        "alice" => {
            thread::sleep(Duration::from_secs(1));
            let mut s = connect();
            auth(&mut s, "alice", "pass1_alice");

            send(&mut s, "/rooms");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "/join games");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "/rooms");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "hello gamers!");
            loop {
                for l in recv_lines(&mut s, Duration::from_secs(1)) {
                    println!("  <- {l}");
                }
            }
        }
        "bob" => {
            thread::sleep(Duration::from_secs(3));
            let mut s = connect();
            auth(&mut s, "bob", "pass2_bob");

            send(&mut s, "/rooms");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "/join games");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "hey alice!");
            loop {
                for l in recv_lines(&mut s, Duration::from_secs(1)) {
                    println!("  <- {l}");
                }
            }
        }
        "setup" => {
            for (user, pass) in &[
                ("alice", "pass1_alice"),
                ("bob", "pass2_bob"),
            ] {
                let mut s = connect();
                send(&mut s, user);
                thread::sleep(Duration::from_millis(200));
                let lines = recv_lines(&mut s, Duration::from_millis(500));
                let need_register = lines.iter().any(|l| l.contains("Register"));
                if need_register {
                    send(&mut s, &format!("/register {pass}"));
                } else {
                    send(&mut s, &format!("/login {pass}"));
                }
                let resp = recv_lines(&mut s, Duration::from_millis(500));
                for l in resp {
                    println!("{user}: {l}");
                }
            }
            println!("Setup done.");
        }
        "charlie" => {
            thread::sleep(Duration::from_secs(2));
            let mut s = connect();
            auth(&mut s, "charlie", "charlie_pass");

            send(&mut s, "/rooms");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "/msg alice hello from charlie!");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "/rooms");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "/join __dm__alice_charlie");
            for l in recv_lines(&mut s, Duration::from_millis(500)) {
                println!("  <- {l}");
            }

            send(&mut s, "back at you from DM!");
            loop {
                for l in recv_lines(&mut s, Duration::from_secs(1)) {
                    println!("  <- {l}");
                }
            }
        }
        _ => {}
    }
}
