use std::error::Error;
use tokio::net::TcpListener;
use chrono::Local;
use redis::Commands;

use retro_chat::{ AppState, handle_connection, load_tls_config };

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8082".to_string());
    let redis_url = std::env
        ::var("REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());

    let listener = TcpListener::bind(&bind_addr).await?;
    let pool = sqlx::postgres::PgPoolOptions
        ::new()
        .max_connections(5)
        .connect(&std::env::var("DATABASE_URL")?).await?;
    let redis_client = redis::Client::open(redis_url.as_str())?;

    let tls_acceptor = load_tls_config()?;

    sqlx::migrate!().run(&pool).await?;

    sqlx
        ::query(
            "INSERT INTO rooms (name, created_by) VALUES ('general', 'system') ON CONFLICT (name) DO NOTHING"
        )
        .execute(&pool).await?;

    let state = AppState::new(pool, redis_client);

    let tls_status = if tls_acceptor.is_some() { "TLS" } else { "TCP" };
    let line1 = "RETRO CHAT SERVER ACTIVE";
    let line2 = format!("{} Listening on: {}", tls_status, bind_addr);
    let line3 = "Press Ctrl+C to shutdown";
    let inner_width = line1.len().max(line2.len()).max(line3.len()) + 2;
    let bar: String = "═".repeat(inner_width);
    let pad = |s: &str| format!("║ {:^width$} ║", s, width = inner_width - 2);
    println!("╔{}╗", bar);
    println!("{}", pad(line1));
    println!("{}", pad(&line2));
    println!("{}", pad(line3));
    println!("╚{}╝", bar);

    loop {
        let (socket, addr) = listener.accept().await?;

        println!("┌─[{}] New connection", Local::now().format("%H:%M:%S"));
        println!("└─ Address: {}", addr);

        let state = state.clone();
        let redis_client = state.redis_client.clone();

        let ip = addr.ip();
        let conn_count: i32 = redis_client
            .get_connection()
            .unwrap()
            .incr(format!("conn_ratelimit:{}", ip), 1)
            .unwrap();
        if conn_count == 1 {
            let _: () = redis_client
                .get_connection()
                .unwrap()
                .expire(format!("conn_ratelimit:{}", ip), 60)
                .unwrap();
        }
        if conn_count > 20 {
            println!("  └─ Connection rate limited (IP: {})", ip);
            continue;
        }

        if let Some(ref acceptor) = tls_acceptor {
            match acceptor.accept(socket).await {
                Ok(tls_stream) => {
                    tokio::spawn(async move { handle_connection(tls_stream, addr, state).await });
                }
                Err(e) => {
                    println!("  └─ TLS handshake failed: {e}");
                }
            }
        } else {
            tokio::spawn(async move { handle_connection(socket, addr, state).await });
        }
    }
}
