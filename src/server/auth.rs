use tokio::io::{ AsyncRead, AsyncWrite, BufReader, AsyncBufReadExt };
use redis::Commands;
use argon2::{
    password_hash::{ SaltString, PasswordHash, PasswordHasher, PasswordVerifier, rand_core },
    Argon2,
};
use rand::Rng;

use crate::AppState;

use super::send_notice;

pub(super) fn validate_username(username: &str) -> bool {
    let len = username.len();
    len >= 3 && len <= 32 && username.chars().all(|c| c.is_alphanumeric() || c == '_')
}

pub(super) async fn check_banned(state: &AppState, username: &str) -> bool {
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM bans WHERE username = $1")
        .bind(username)
        .fetch_optional(&state.pool).await
        .unwrap()
        .is_some()
}

pub(super) async fn authenticate_user(
    reader: &mut BufReader<impl AsyncRead + Unpin>,
    writer: &mut (impl AsyncWrite + Unpin),
    state: &AppState,
    redis_conn: &mut redis::Connection,
    username: &str
) -> bool {
    let mut authenticated = false;
    let mut line = String::new();

    while !authenticated {
        line.clear();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return false;
        }
        let input = line.trim();

        if let Some(password) = input.strip_prefix("/register ") {
            if password.len() < 8 {
                send_notice(writer, "Password must be at least 8 characters.").await;
                continue;
            }
            let is_first: bool =
                sqlx
                    ::query_scalar::<_, i64>("SELECT COUNT(*) FROM users")
                    .fetch_one(&state.pool).await
                    .unwrap_or(0) == 0;
            let role = if is_first { "admin" } else { "user" };
            let salt = SaltString::generate(&mut rand_core::OsRng);
            let hash = Argon2::default()
                .hash_password(password.as_bytes(), &salt)
                .unwrap()
                .to_string();
            match
                sqlx
                    ::query("INSERT INTO users (username, password_hash, role) VALUES ($1, $2, $3)")
                    .bind(username)
                    .bind(&hash)
                    .bind(role)
                    .execute(&state.pool).await
            {
                Ok(_) => {
                    let token: String = rand
                        ::rng()
                        .sample_iter(&rand::distr::Alphanumeric)
                        .take(32)
                        .map(char::from)
                        .collect();
                    let _: () = redis_conn
                        .set_ex(format!("session:{}", token), username, 86400)
                        .unwrap();
                    send_notice(
                        writer,
                        &format!("Registered and logged in. Token: {}", token)
                    ).await;
                    authenticated = true;
                }
                Err(_) => send_notice(writer, "Username already taken.").await,
            }
        } else if let Some(password) = input.strip_prefix("/login ") {
            if password.is_empty() {
                send_notice(writer, "Password cannot be empty.").await;
                continue;
            }
            let attempts_key = format!("login_attempts:{}", username);
            let attempts: i32 = redis_conn.get(&attempts_key).unwrap_or(0);
            if attempts >= 3 {
                let ttl: i64 = redis_conn.ttl(&attempts_key).unwrap_or(60);
                send_notice(
                    writer,
                    &format!("Too many failed attempts. Try again in {} seconds.", ttl)
                ).await;
                continue;
            }
            match
                sqlx
                    ::query_scalar::<_, String>(
                        "SELECT password_hash FROM users WHERE username = $1"
                    )
                    .bind(username)
                    .fetch_optional(&state.pool).await
            {
                Ok(Some(stored_hash)) => {
                    let parsed = PasswordHash::new(&stored_hash).unwrap();
                    if Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok() {
                        let _: Result<(), _> = redis_conn.del(&attempts_key);
                        let token: String = rand
                            ::rng()
                            .sample_iter(&rand::distr::Alphanumeric)
                            .take(32)
                            .map(char::from)
                            .collect();
                        let _: () = redis_conn
                            .set_ex(format!("session:{}", token), username, 86400)
                            .unwrap();
                        send_notice(writer, &format!("Logged in. Token: {}", token)).await;
                        authenticated = true;
                    } else {
                        let count: i32 = redis_conn.incr(&attempts_key, 1).unwrap();
                        if count == 1 {
                            let _: () = redis_conn.expire(&attempts_key, 60).unwrap();
                        }
                        send_notice(
                            writer,
                            &format!("Wrong password. ({}/3 attempts)", count)
                        ).await;
                    }
                }
                Ok(None) => send_notice(writer, "User not found. Use /register first.").await,
                Err(_) => {
                    send_notice(writer, "Database error.").await;
                    return false;
                }
            }
        } else {
            send_notice(
                writer,
                "Authenticate first: /register <password> or /login <password>"
            ).await;
        }
    }
    true
}
