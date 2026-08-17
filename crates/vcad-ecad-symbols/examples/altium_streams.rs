//! Dump the CFB storages and the head of each `Data` stream from an Altium
//! file, for diagnosing record layouts:
//! `cargo run -p vcad-ecad-symbols --example altium_streams -- file.PcbDoc [Storage]`
use std::io::{Cursor, Read};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("path");
    let want = args.next();
    let bytes = std::fs::read(&path).expect("read");
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes)).expect("cfb");
    let names: Vec<String> = cf
        .read_storage("/")
        .unwrap()
        .filter(|e| e.is_storage())
        .map(|e| e.name().to_string())
        .collect();
    if want.is_none() {
        println!("storages: {}", names.join(", "));
        return;
    }
    let want = want.unwrap();
    for n in names.iter().filter(|n| n.contains(&want)) {
        let mut buf = Vec::new();
        match cf.open_stream(format!("/{n}/Data")) {
            Ok(mut s) => {
                s.read_to_end(&mut buf).unwrap();
            }
            Err(e) => {
                println!("{n}: no Data stream ({e})");
                continue;
            }
        }
        println!("== {n}: {} bytes", buf.len());
        let n: usize = std::env::var("HEAD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(160);
        let head = &buf[..buf.len().min(n)];
        if std::env::var("HEAD").is_err() {
            println!(
                "hex: {}",
                head.iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        println!(
            "txt: {}",
            head.iter()
                .map(|&b| if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '.'
                })
                .collect::<String>()
        );
    }
}
