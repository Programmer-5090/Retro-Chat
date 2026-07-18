use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{ mpsc, RwLock };
use sqlx::{ Pool, Postgres };

pub struct AppState {
    pub rooms: RwLock<HashMap<String, Vec<(String, mpsc::UnboundedSender<String>)>>>,
    pub user_rooms: RwLock<HashMap<String, String>>,
    pub user_senders: RwLock<HashMap<String, mpsc::UnboundedSender<String>>>,
    pub pool: Pool<Postgres>,
    pub redis_client: redis::Client,
    presence_cache: RwLock<(Instant, Vec<String>)>,
}

impl AppState {
    pub fn new(pool: Pool<Postgres>, redis_client: redis::Client) -> Arc<Self> {
        Arc::new(Self {
            rooms: RwLock::new(HashMap::new()),
            user_rooms: RwLock::new(HashMap::new()),
            user_senders: RwLock::new(HashMap::new()),
            pool,
            redis_client,
            presence_cache: RwLock::new((Instant::now(), Vec::new())),
        })
    }

    /// Subscribes `username` to live delivery for `room` without removing
    /// them from any other room they're already subscribed to. A connected
    /// user can be subscribed to several rooms at once (their DMs, plus
    /// whatever channels they've joined)
    pub async fn subscribe_room(
        &self,
        username: &str,
        room: &str,
        tx: mpsc::UnboundedSender<String>
    ) {
        let mut rooms = self.rooms.write().await;
        let members = rooms.entry(room.to_string()).or_default();
        members.retain(|(name, _)| name != username);
        members.push((username.to_string(), tx));
    }

    /// Removes `username` from live delivery for `room` only — their other
    /// room subscriptions are untouched. An explicit /leave.
    /// switching which room you're viewing should not call this.
    pub async fn unsubscribe_room(&self, username: &str, room: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(members) = rooms.get_mut(room) {
            members.retain(|(name, _)| name != username);
        }
    }

    /// Sets which room `username` is currently viewing. This only affects
    /// where their plain-text messages get routed (`get_user_room`)
    pub async fn set_active_room(&self, username: &str, room: &str) {
        self.user_rooms.write().await.insert(username.to_string(), room.to_string());
    }

    /// Convenience used at initial connect / new-room creation. subscribes
    /// to `room` for live delivery and makes it the active room in one call.
    pub async fn join_room(&self, username: &str, room: &str, tx: mpsc::UnboundedSender<String>) {
        self.subscribe_room(username, room, tx).await;
        self.set_active_room(username, room).await;
    }

    pub async fn leave_all_rooms(&self, username: &str) {
        let mut rooms = self.rooms.write().await;
        for (_, members) in rooms.iter_mut() {
            members.retain(|(name, _)| name != username);
        }
    }

    pub async fn get_room_users(&self, room: &str) -> Vec<String> {
        let rooms = self.rooms.read().await;
        rooms
            .get(room)
            .map(|members|
                members
                    .iter()
                    .map(|(name, _)| name.clone())
                    .collect()
            )
            .unwrap_or_default()
    }

    pub async fn get_user_subscribed_rooms(&self, username: &str) -> Vec<String> {
        let rooms = self.rooms.read().await;
        rooms
            .iter()
            .filter(|(_, members)| members.iter().any(|(name, _)| name == username))
            .map(|(room, _)| room.clone())
            .collect()
    }

    pub async fn get_online_users(&self) -> Vec<String> {
        {
            let cache = self.presence_cache.read().await;
            if cache.0.elapsed().as_secs() < 2 {
                return cache.1.clone();
            }
        }
        let mut cache = self.presence_cache.write().await;
        if cache.0.elapsed().as_secs() < 2 {
            return cache.1.clone();
        }
        let mut conn = self.redis_client.get_connection().unwrap();
        let keys: Vec<String> = redis
            ::cmd("KEYS")
            .arg("presence:*")
            .query(&mut conn)
            .unwrap_or_default();
        let users: Vec<String> = keys
            .into_iter()
            .filter_map(|k| k.strip_prefix("presence:").map(|u| u.to_string()))
            .collect();
        *cache = (Instant::now(), users.clone());
        users
    }

    pub async fn send_to_room(&self, room: &str, msg: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(members) = rooms.get_mut(room) {
            members.retain(|(_, tx)| tx.send(msg.to_string()).is_ok());
        }
    }

    pub async fn get_user_room(&self, username: &str) -> String {
        self.user_rooms
            .read().await
            .get(username)
            .cloned()
            .unwrap_or_else(|| "general".to_string())
    }

    pub async fn get_or_create_db_room(&self, room: &str, created_by: &str) -> i32 {
        sqlx::query(
            "INSERT INTO rooms (name, created_by) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING"
        )
            .bind(room)
            .bind(created_by)
            .execute(&self.pool).await
            .ok();

        sqlx::query_scalar::<_, i32>("SELECT id FROM rooms WHERE name = $1")
            .bind(room)
            .fetch_one(&self.pool).await
            .unwrap_or(1)
    }

    pub async fn list_db_rooms(&self) -> Vec<String> {
        let all: Vec<String> = sqlx
            ::query_scalar::<_, String>("SELECT name FROM rooms ORDER BY name")
            .fetch_all(&self.pool).await
            .unwrap_or_default();
        all.into_iter()
            .filter(|n| !n.starts_with("__dm__"))
            .collect()
    }

    pub async fn register_sender(&self, username: &str, tx: mpsc::UnboundedSender<String>) {
        self.user_senders.write().await.insert(username.to_string(), tx);
    }

    pub async fn unregister_sender(&self, username: &str) {
        self.user_senders.write().await.remove(username);
    }

    pub async fn get_sender(&self, username: &str) -> Option<mpsc::UnboundedSender<String>> {
        self.user_senders.read().await.get(username).cloned()
    }

    pub async fn save_room_membership(&self, username: &str, room: &str) {
        let room_id = self.get_or_create_db_room(room, "system").await;
        sqlx::query(
            "INSERT INTO room_members (room_id, username) VALUES ($1, $2) ON CONFLICT (room_id, username) DO UPDATE SET last_joined_at = now()"
        )
            .bind(room_id)
            .bind(username)
            .execute(&self.pool).await
            .ok();
    }

    pub async fn remove_room_membership(&self, username: &str, room: &str) {
        let room_id: Option<i32> = sqlx
            ::query_scalar("SELECT id FROM rooms WHERE name = $1")
            .bind(room)
            .fetch_optional(&self.pool).await
            .ok()
            .flatten();
        if let Some(id) = room_id {
            sqlx::query("DELETE FROM room_members WHERE room_id = $1 AND username = $2")
                .bind(id)
                .bind(username)
                .execute(&self.pool).await
                .ok();
        }
    }

    pub async fn get_user_room_memberships(&self, username: &str) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT r.name FROM room_members rm JOIN rooms r ON rm.room_id = r.id WHERE rm.username = $1 ORDER BY rm.last_joined_at DESC"
        )
            .bind(username)
            .fetch_all(&self.pool).await
            .unwrap_or_default()
    }

    pub async fn get_last_room(&self, username: &str) -> Option<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT r.name FROM room_members rm JOIN rooms r ON rm.room_id = r.id WHERE rm.username = $1 ORDER BY rm.last_joined_at DESC LIMIT 1"
        )
            .bind(username)
            .fetch_optional(&self.pool).await
            .ok()
            .flatten()
    }
}