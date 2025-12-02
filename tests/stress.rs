use pea2pea::{Config, Node};
use std::time::{Duration, Instant};
use tokio::{net::TcpStream, task::JoinSet, time::sleep};

#[ignore]
#[tokio::test(flavor = "multi_thread")]
async fn persistent_connection_swarm() {
    // ensure your OS limits are high enough!
    // this involves the ulimit, SYN flood protection, etc.
    // 100k should be perfectly doable with additional fine-tuning
    const TARGET_CONNS: usize = 10_000;

    // create the server, including some extra margins
    let config = Config {
        name: Some("server".into()),
        max_connections: (TARGET_CONNS + 1000) as u16,
        max_connections_per_ip: (TARGET_CONNS + 1000) as u16,
        max_connecting: (TARGET_CONNS + 1000) as u16,
        ..Default::default()
    };
    let server = Node::new(config);
    let server_addr = server.toggle_listener().await.unwrap().unwrap();

    println!("⚡ Spawning {} persistent clients...", TARGET_CONNS);

    // these clients do NOT give up; they merely wait and retry
    let mut client_set = JoinSet::new();
    for _ in 0..TARGET_CONNS {
        client_set.spawn(async move {
            loop {
                match TcpStream::connect(server_addr).await {
                    Ok(stream) => {
                        // keep the socket active and count towards the limit
                        let _ = stream;
                        std::future::pending::<()>().await;
                    }
                    Err(_) => {
                        // simulates "backpressure" being handled by the client
                        let jitter = rand::random::<u64>() % 200;
                        sleep(Duration::from_millis(500 + jitter)).await;
                    }
                }
            }
        });
    }

    // monitor progress
    println!("🔥 Swarm released. Waiting for saturation...");
    let start = Instant::now();
    let mut last_log = Instant::now();
    let mut last_count = 0;

    loop {
        let count = server.num_connected();

        // log progress every second
        if last_log.elapsed() >= Duration::from_secs(1) {
            let rate = count.saturating_sub(last_count);
            println!(
                "   Status: {count}/{TARGET_CONNS} connections ({rate:+5}/sec) | Elapsed: {:.0}s",
                start.elapsed().as_secs()
            );
            last_log = Instant::now();
            last_count = count;
        }

        // success condition
        if count >= TARGET_CONNS {
            println!(
                "\n✅ SUCCESS: Reached {} connections in {:.2?}",
                count,
                start.elapsed()
            );
            break;
        }

        // timeout (failure) condition
        if start.elapsed() > Duration::from_secs(60) {
            panic!("\n❌ FAILURE: Stalled at {count} connections.");
        }

        sleep(Duration::from_millis(500)).await;
    }

    // cleanup
    client_set.abort_all();
    server.shut_down().await;
}
