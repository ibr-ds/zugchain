use std::{sync::Arc, time::Duration};

use tokio::{io::AsyncWriteExt, net::TcpStream};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut args = std::env::args();
    let host = args.nth(1).unwrap_or_else(|| "mcoms".to_string());
    let ips = match host.as_str() {
        "mcoms" => ["192.168.1.11", "192.168.1.12", "192.168.1.13", "192.168.1.14"],
        "rpis" => ["192.168.1.21", "192.168.1.22", "192.168.1.23", "192.168.1.24"],
        "localhost" => ["127.0.0.1", "127.0.0.1", "127.0.0.1", "127.0.0.1"],
        _ => unreachable!(),
    };

    let barrier = Arc::new(tokio::sync::Barrier::new(ips.len()));

    let mut handles = Vec::new();
    for ip in ips.iter().copied() {
        let barrier = barrier.clone();
        let h = tokio::spawn(async move {
            let mut i = 0;
            let mut stream = loop {
                match TcpStream::connect((ip, 9999)).await {
                    Ok(stream) => {
                        break stream;
                    }
                    Err(e) => {
                        eprintln!("failed {:?}: {}", ip, e);
                        i += 1;
                        if i > 10 {
                            panic!("exceeded: {}", e)
                        }
                    }
                };
                tokio::time::sleep(Duration::from_millis(500)).await;
            };
            barrier.wait().await;
            stream.write_all(b"go").await.unwrap();
        });
        handles.push(h);
    }

    for h in handles {
        h.await.unwrap();
    }
}
